//! Firmware subjects: the content-addressed object store, and the
//! repository state an image is built from.

use std::{
    fs::{self, File},
    io,
    path::Path,
};

use crate::Result;
use oer_hil_run_bundle_format::archived::{
    DEFLATE, compressed, read_archived, stored_identity, stored_path,
};
use oer_hil_run_bundle_format::run::RepositoryProvenance;
use oer_os_durable::atomic_write;
use oer_os_process::lock::{FileLock, Mode};

#[derive(Clone, Debug)]
pub struct ArchivedFile {
    pub size_bytes: u64,
    pub sha256: String,
}

/// Where a file to archive comes from.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    /// A file a build wrote.
    Built(&'a Path),
    /// A file another bundle archived, read through
    /// [`oer_hil_run_bundle_format::archived::read_archived`].
    Archived(&'a Path),
}

/// Archive `source` at `destination` in a bundle as a hard link to its
/// object in the content-addressed store below `target_directory` (a copy
/// when linking fails), compressed when `destination` is a
/// [`oer_hil_run_bundle_format::archived::COMPRESSED`] file; returns the size
/// and digest of its uncompressed bytes.
pub fn archive_content_addressed(
    source: Source<'_>,
    destination: &Path,
    target_directory: &Path,
) -> Result<ArchivedFile> {
    let path = match source {
        Source::Built(path) => path.to_owned(),
        Source::Archived(path) => stored_path(path),
    };
    if !fs::symlink_metadata(&path)?.file_type().is_file() {
        return Err(format!(
            "firmware artifact is not a regular file: {}",
            path.display()
        )
        .into());
    }
    let source_compressed = matches!(source, Source::Archived(path) if compressed(path));
    let destination_compressed = compressed(destination);
    let destination = stored_path(destination);
    if destination.try_exists()? {
        return Err(format!(
            "firmware artifact is already archived: {}",
            destination.display()
        )
        .into());
    }
    let oer_hil_run_bundle_format::archived::Identity { size_bytes, sha256 } =
        stored_identity(&path, source_compressed)?;
    // Collection deletes objects only while no archive uses the store.
    let _store = object_store_lock(target_directory, Mode::Shared)?;
    let mut object = target_directory
        .join("objects/sha256")
        .join(&sha256[..2])
        .join(&sha256);
    if destination_compressed {
        object.set_extension(DEFLATE);
    }
    if !object.try_exists()? {
        let bytes = match source {
            Source::Built(path) => fs::read(path)?,
            Source::Archived(path) => read_archived(path)?,
        };
        let bytes = if destination_compressed {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            io::Write::write_all(&mut encoder, &bytes)?;
            encoder.finish()?
        } else {
            bytes
        };
        atomic_write(&object, &bytes)?;
    }
    let require = |path: &Path| -> Result<()> {
        let actual = stored_identity(path, destination_compressed)?;
        if !fs::symlink_metadata(path)?.file_type().is_file()
            || actual.size_bytes != size_bytes
            || actual.sha256 != sha256
        {
            return Err(format!(
                "content-addressed firmware artifact has the wrong identity: {}",
                path.display()
            )
            .into());
        }
        Ok(())
    };
    require(&object)?;
    make_read_only(&object)?;
    let destination_parent = destination.parent().ok_or_else(|| {
        format!(
            "firmware archive path has no parent: {}",
            destination.display()
        )
    })?;
    fs::create_dir_all(destination_parent)?;
    if fs::hard_link(&object, &destination).is_err() {
        copy_regular_file(&object, &destination)?;
    }
    require(&destination)?;
    make_read_only(&destination)?;
    File::open(destination_parent)?.sync_all()?;
    Ok(ArchivedFile { size_bytes, sha256 })
}

/// The lock that orders archiving into a checkout's object store before its
/// collection: archives hold it shared, collection exclusively.
fn object_store_lock(target_directory: &Path, mode: Mode) -> Result<FileLock> {
    FileLock::acquire(&target_directory.join("objects/lock"), mode)
}

/// What [`collect_objects`] deleted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CollectedObjects {
    pub objects: usize,
    pub bytes: u64,
}

/// Delete the objects of the store below `target_directory` that no file
/// links to any more, and temporary files an interrupted archive left.
///
/// Every archived subject is a hard link to its object, or a copy when
/// linking failed, so an object with a single link serves no run: it only
/// saves a later identical archive a copy. Collection holds the store's lock
/// exclusively, so no archive reuses an object while it is deleted.
pub fn collect_objects(target_directory: &Path) -> Result<CollectedObjects> {
    use std::os::unix::fs::MetadataExt as _;
    let root = target_directory.join("objects/sha256");
    let mut collected = CollectedObjects::default();
    if !root.is_dir() {
        return Ok(collected);
    }
    let _store = object_store_lock(target_directory, Mode::Exclusive)?;
    for prefix in fs::read_dir(&root)? {
        let prefix = prefix?;
        if !prefix.file_type()?.is_dir() {
            continue;
        }
        for entry in fs::read_dir(prefix.path())? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let temporary = entry
                .file_name()
                .to_string_lossy()
                .starts_with(".firmware-artifact.tmp-");
            if metadata.is_file() && (metadata.nlink() == 1 || temporary) {
                fs::remove_file(entry.path())?;
                collected.objects += usize::from(!temporary);
                collected.bytes += metadata.len();
            }
        }
    }
    Ok(collected)
}

fn make_read_only(path: &Path) -> Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path) -> Result<()> {
    oer_os_durable::atomic_write(destination, &fs::read(source)?)
}

/// The repository state of the tree at `root`: its commit, and whether it
/// has changes the commit does not hold (tracked or untracked); a tree
/// outside git has no commit and is dirty.
pub fn repository(root: &Path) -> Result<RepositoryProvenance> {
    let Ok(commit) = oer_os_process::git::text(root, ["rev-parse", "HEAD"]) else {
        return Ok(RepositoryProvenance {
            commit: String::new(),
            dirty: true,
        });
    };
    let status = oer_os_process::git::text(
        root,
        ["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    Ok(RepositoryProvenance {
        commit,
        dirty: !status.is_empty(),
    })
}

#[cfg(test)]
mod tests;

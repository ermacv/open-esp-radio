//! Firmware subjects and source materials: the content-addressed object
//! store and the capture of a build's sources.

use std::{
    env,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::Result;
use oer_tool_base_durable::{atomic_write, sha256_file};
use oer_tool_base_process::lock::{FileLock, Mode};
use oer_tool_data_hil_run_bundle_format::archived::{
    DEFLATE, compressed, read_archived, stored_identity, stored_path,
};
use oer_tool_data_hil_run_bundle_format::build::*;
use oer_tool_service_hil_source_snapshot::FrozenSources;

/// Query the tools the image builder runs. Call it when the build runs,
/// so the record names the tools that produced the image.
pub fn capture_environment() -> BuildEnvironment {
    BuildEnvironment {
        tools: oer_tool_base_toolchain::versions()
            .into_iter()
            .map(|tool| BuildTool {
                name: tool.tool.name().to_owned(),
                program: tool.program,
                version: tool.version,
            })
            .collect(),
        inherited_rustflags: env::var("RUSTFLAGS").ok(),
        inherited_encoded_rustflags: env::var("CARGO_ENCODED_RUSTFLAGS").ok(),
        cargo_incremental: String::from("0"),
        source_date_epoch: env::var("SOURCE_DATE_EPOCH").ok(),
    }
}

/// A fixed environment in which every recorded tool reports a version,
/// independent of the tools installed on the test host.
#[cfg(any(test, feature = "test-support"))]
pub fn synthetic_environment() -> BuildEnvironment {
    BuildEnvironment {
        tools: oer_tool_base_toolchain::Tool::ALL
            .into_iter()
            .map(|tool| BuildTool {
                name: tool.name().to_owned(),
                program: tool.name().to_owned(),
                version: Some(format!("{} synthetic-test-version", tool.name())),
            })
            .collect(),
        inherited_rustflags: None,
        inherited_encoded_rustflags: None,
        cargo_incremental: String::from("0"),
        source_date_epoch: None,
    }
}

#[derive(Clone, Debug)]
pub struct ArchivedFile {
    pub size_bytes: u64,
    pub sha256: String,
}

struct GitSourceState {
    commit: String,
    status: String,
    tracked_diff: Vec<u8>,
    untracked_files: Vec<SourceFileIdentity>,
    workspace_sha256: String,
}

/// Where a file to archive comes from.
#[derive(Clone, Copy, Debug)]
pub enum Source<'a> {
    /// A file a build wrote.
    Built(&'a Path),
    /// A file another bundle archived, read through
    /// [`oer_tool_data_hil_run_bundle_format::archived::read_archived`].
    Archived(&'a Path),
}

/// Archive `source` at `destination` in a bundle as a hard link to its
/// object in the content-addressed store below `target_directory` (a copy
/// when linking fails), compressed when `destination` is a
/// [`oer_tool_data_hil_run_bundle_format::archived::COMPRESSED`] file; returns the size
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
    let oer_tool_data_hil_run_bundle_format::archived::Identity { size_bytes, sha256 } =
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
    oer_tool_base_durable::atomic_write(destination, &fs::read(source)?)
}

pub fn capture_sources(root: &Path, run_directory: &Path) -> Result<Vec<SourceMaterial>> {
    let mut materials = vec![capture_source_material(
        "repository",
        root,
        run_directory,
        Path::new("source/repository.patch"),
    )?];
    for (name, path) in external_source_override_paths()? {
        let patch_path = PathBuf::from("source/overrides").join(format!("{name}.patch"));
        materials.push(capture_source_material(
            &name,
            &path,
            run_directory,
            &patch_path,
        )?);
    }
    Ok(materials)
}

pub fn capture_source_material(
    name: &str,
    root: &Path,
    run_directory: &Path,
    patch_path: &Path,
) -> Result<SourceMaterial> {
    let checkout_path = fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
    let state = match capture_git_source_state(root)? {
        Some(state) => state,
        None => {
            return Ok(SourceMaterial {
                name: name.to_owned(),
                checkout_path,
                remote: None,
                commit: String::new(),
                dirty: true,
                workspace_sha256: oer_tool_base_durable::sha256_bytes(&[]),
                rebuild_status: SourceRebuildStatus::Incomplete,
                tracked_patch_path: None,
                tracked_patch_size_bytes: None,
                tracked_patch_sha256: None,
                untracked_files: Vec::new(),
                limitations: vec![SourceLimitation::RepositoryStateNotCaptured],
            });
        }
    };
    let (tracked_patch_path, tracked_patch_size_bytes, tracked_patch_sha256) =
        if state.tracked_diff.is_empty() {
            (None, None, None)
        } else {
            atomic_write(&run_directory.join(patch_path), &state.tracked_diff)?;
            (
                Some(patch_path.to_owned()),
                Some(u64::try_from(state.tracked_diff.len())?),
                Some(oer_tool_base_durable::sha256_bytes(&state.tracked_diff)),
            )
        };
    let mut limitations = Vec::new();
    if !state.untracked_files.is_empty() {
        limitations.push(SourceLimitation::UntrackedContentNotArchived);
    }
    if !state.status.is_empty() && state.tracked_diff.is_empty() && state.untracked_files.is_empty()
    {
        limitations.push(SourceLimitation::RepositoryStateNotCaptured);
    }
    let remote = oer_tool_base_process::git::text(root, ["remote", "get-url", "origin"])
        .ok()
        .filter(|remote| !remote.is_empty())
        .map(sanitize_git_remote);
    if remote.is_none() {
        limitations.push(SourceLimitation::SourceRemoteUnavailable);
    }
    let rebuild_status = if !limitations.is_empty() {
        SourceRebuildStatus::Incomplete
    } else if state.status.is_empty() {
        SourceRebuildStatus::CleanCommit
    } else {
        SourceRebuildStatus::TrackedPatch
    };
    Ok(SourceMaterial {
        name: name.to_owned(),
        checkout_path,
        remote,
        commit: state.commit,
        dirty: !state.status.is_empty(),
        workspace_sha256: state.workspace_sha256,
        rebuild_status,
        tracked_patch_path,
        tracked_patch_size_bytes,
        tracked_patch_sha256,
        untracked_files: state.untracked_files,
        limitations,
    })
}

fn capture_git_source_state(root: &Path) -> Result<Option<GitSourceState>> {
    let commit = match oer_tool_base_process::git::text(root, ["rev-parse", "HEAD"]) {
        Ok(commit) => commit,
        Err(_) => return Ok(None),
    };
    let status = oer_tool_base_process::git::text(
        root,
        ["status", "--porcelain=v1", "--untracked-files=normal"],
    )?;
    let tracked_diff =
        oer_tool_base_process::git::output(root, ["diff", "--binary", "HEAD", "--"])?;
    let untracked = oer_tool_base_process::git::output(
        root,
        ["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let mut digest = Sha256::new();
    digest.update(status.as_bytes());
    digest.update(&tracked_diff);
    let mut untracked_files = Vec::new();
    for path in untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        digest.update(path);
        let path = String::from_utf8(path.to_vec())?;
        let absolute = root.join(&path);
        if !fs::symlink_metadata(&absolute)?.file_type().is_file() {
            return Err(format!(
                "untracked HIL source identity is not a regular file: {}",
                absolute.display()
            )
            .into());
        }
        let contents = fs::read(&absolute)?;
        digest.update(&contents);
        untracked_files.push(SourceFileIdentity {
            path: PathBuf::from(path),
            size_bytes: u64::try_from(contents.len())?,
            sha256: oer_tool_base_durable::sha256_bytes(&contents),
        });
    }
    untracked_files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(Some(GitSourceState {
        commit,
        status,
        tracked_diff,
        untracked_files,
        workspace_sha256: format!("{:x}", digest.finalize()),
    }))
}

fn external_source_override_paths() -> Result<Vec<(String, PathBuf)>> {
    let current_directory = env::current_dir()?;
    let mut overrides = Vec::new();
    for (name, variable) in [("esp-hal", "ESP_HAL_ROOT")] {
        let Some(path) = env::var_os(variable).map(PathBuf::from) else {
            continue;
        };
        let path = if path.is_absolute() {
            path
        } else {
            current_directory.join(path)
        };
        overrides.push((name.to_owned(), fs::canonicalize(&path).unwrap_or(path)));
    }
    Ok(overrides)
}

fn sanitize_git_remote(remote: String) -> String {
    let Some((scheme, remainder)) = remote.split_once("://") else {
        return remote;
    };
    let authority_end = remainder.find('/').unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let sanitized_authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    format!(
        "{scheme}://{sanitized_authority}{}",
        &remainder[authority_end..]
    )
}

pub fn build_id(subjects: &[BuildSubject]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"open-esp-radio-hil-build-v1\0");
    for subject in subjects {
        digest.update(format!("{:?}", subject.role).as_bytes());
        digest.update([0]);
        digest.update(subject.path.as_os_str().as_encoded_bytes());
        digest.update([0]);
        digest.update(subject.sha256.as_bytes());
        digest.update(subject.size_bytes.to_le_bytes());
    }
    format!("{:x}", digest.finalize())
}

/// The material of the repository file `path`, named `name`.
pub fn build_file_material(root: &Path, name: &str, path: &Path) -> Result<BuildFileMaterial> {
    let absolute = root.join(path);
    let metadata = fs::symlink_metadata(&absolute)?;
    if !metadata.file_type().is_file() {
        return Err(format!(
            "build material is not a regular file: {}",
            absolute.display()
        )
        .into());
    }
    Ok(BuildFileMaterial {
        name: name.to_owned(),
        path: path.to_owned(),
        archive_path: None,
        size_bytes: metadata.len(),
        sha256: sha256_file(&absolute)?,
    })
}

pub fn archived_file_material(
    name: &str,
    path: &Path,
    archive_path: PathBuf,
    archived: &ArchivedFile,
) -> BuildFileMaterial {
    BuildFileMaterial {
        name: name.to_owned(),
        path: path.to_owned(),
        archive_path: Some(archive_path),
        size_bytes: archived.size_bytes,
        sha256: archived.sha256.clone(),
    }
}

/// The build file materials a source snapshot archives into a run: its
/// record and its manifest, by material name and file name.
pub const SNAPSHOT_MATERIALS: [(&str, &str); 2] = [
    ("source-snapshot-metadata", "snapshot.json"),
    ("source-snapshot-manifest", "manifest.json"),
];

/// Archive the snapshot in `directory` (its record and manifest) into the
/// bundle at `output`, content-addressed in the object store below `target`,
/// and open the archived copy, its files read from the source objects in
/// `objects`, in a free build workspace of `build_slots`.
pub fn archive_snapshot(
    directory: &Path,
    objects: &Path,
    output: &Path,
    target: &Path,
    build_slots: &Path,
) -> Result<(FrozenSources, Vec<SourceMaterial>, Vec<BuildFileMaterial>)> {
    let destination = output.join("source/snapshot");
    let mut files: Vec<BuildFileMaterial> = Vec::new();
    // The files themselves are objects in `objects`, which the manifest
    // names by digest; only the manifest and its record are archived.
    for (name, filename) in SNAPSHOT_MATERIALS {
        let relative = PathBuf::from("source/snapshot").join(filename);
        let archived = archive_content_addressed(
            Source::Built(&directory.join(filename)),
            &output.join(&relative),
            target,
        )?;
        files.push(archived_file_material(
            name,
            Path::new(filename),
            relative,
            &archived,
        ));
    }
    // A persistent host build slot keeps unchanged sources, so Cargo
    // rebuilds only the packages whose sources changed on each run.
    let frozen = FrozenSources::open_in_free_workspace(&destination, objects, build_slots)?;
    let sources = frozen
        .sources()
        .iter()
        .map(|source| {
            Ok(SourceMaterial {
                name: source.name.clone(),
                checkout_path: PathBuf::from(&source.name),
                remote: None,
                commit: source.commit.clone(),
                dirty: source.dirty,
                workspace_sha256: oer_tool_service_hil_source_snapshot::identity(source)?,
                rebuild_status: SourceRebuildStatus::SourceSnapshot,
                tracked_patch_path: None,
                tracked_patch_size_bytes: None,
                tracked_patch_sha256: None,
                untracked_files: Vec::new(),
                limitations: Vec::new(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((frozen, sources, files))
}

#[cfg(test)]
mod tests;

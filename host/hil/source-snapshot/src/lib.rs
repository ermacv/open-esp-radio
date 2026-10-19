//! Explicit, content-addressed source input capture, independent of firmware builds.
//!
//! Tracked source files are included automatically; agent guidance (`AGENTS.md`
//! and `.agents/`) is not a build input and is excluded. Every nonignored
//! untracked source file must be named explicitly before any content is
//! archived, or, with `--include-untracked`, lie inside a path package of the firmware
//! workspaces, the packages an image build reads, or inside the HIL host
//! packages and scenarios, which the run reads. The manifest lists every
//! untracked file it archived and why. This is a source
//! snapshot, not a hermetic build or a readiness decision.
//!
//! A snapshot directory holds its manifest and record; each file's bytes are
//! an object named by their SHA-256 in an objects directory shared by every
//! snapshot and run (the run store's `sources`), so an unchanged file is
//! stored once. Image builds read a snapshot through [`FrozenSources`]; run
//! evidence records it and verification re-derives each source's
//! [`identity`].

pub mod checkout;

use oer_os_durable::{Result, atomic_json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

pub use oer_hil_schema::snapshot::SourceInput;
use oer_hil_schema::snapshot::{
    FileInput, MANIFEST_SCHEMA, Manifest, SNAPSHOT_SCHEMA, UntrackedInput, UntrackedReason,
    is_agent_guidance, object,
};

/// Repository directories of the HIL host packages and scenarios, whose
/// untracked files `--include-untracked` also archives.
const HIL_HOST_INPUTS: [&str; 3] = ["tools/hil", "host", "portable/hil"];

/// Repository-relative directories of the path packages a HIL image of some
/// chip builds: its HIL agent and, for a staged boot, its platform's
/// bootstrap, with every repository package they reach.
pub fn image_packages(root: &Path) -> Result<Vec<PathBuf>> {
    let mut directories = BTreeSet::new();
    for profile in oer_image_profile::Profile::all(root)? {
        if !root.join(profile.hil_agent_directory()).is_dir() {
            continue;
        }
        let agent = profile.hil_agent_package();
        let mut roots = vec![agent.as_str()];
        let bootstrap = profile.bootstrap_package();
        if profile.staged.is_some() {
            roots.push(&bootstrap);
        }
        directories.extend(checkout::package_closure(
            root,
            &roots,
            checkout::Edges::All,
        )?);
    }
    Ok(directories.into_iter().collect())
}

#[derive(Deserialize, Serialize)]
pub struct Snapshot {
    schema: u16,
    snapshot_id: String,
    directory: PathBuf,
    files: usize,
}

impl Snapshot {
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The snapshot's identity: the SHA-256 of its manifest.
    pub fn id(&self) -> &str {
        &self.snapshot_id
    }

    /// The snapshot captured earlier into `directory`, whose manifest and
    /// every file in `objects` are verified before they are used.
    pub fn load(directory: &Path, objects: &Path) -> Result<Self> {
        let directory = directory.canonicalize()?;
        let snapshot: Self = serde_json::from_slice(&fs::read(directory.join("snapshot.json"))?)?;
        if snapshot.directory.canonicalize()? != directory {
            return Err(format!(
                "{} holds the record of another snapshot directory",
                directory.display()
            )
            .into());
        }
        FrozenSources::open(&directory, objects)?;
        Ok(snapshot)
    }
}

/// A source's identity: the SHA-256 of its manifest entry.
pub fn identity(source: &SourceInput) -> Result<String> {
    Ok(digest(&serde_json::to_vec(source)?))
}

/// Owns a verified private checkout. No build falls back to the live repository.
pub struct FrozenSources {
    checkout: Checkout,
    snapshot: Snapshot,
    manifest: Manifest,
}

/// Where a snapshot is materialized.
enum Checkout {
    /// A fresh temporary directory, for readers of the sources.
    Temporary(tempfile::TempDir),
    /// A fixed build workspace held under an exclusive lock. A stable path
    /// keeps Cargo's unit identities stable in the shared compile cache, and
    /// unchanged files keep their modification times, so Cargo rebuilds only
    /// the packages a snapshot actually changes.
    Workspace {
        path: PathBuf,
        _lock: oer_os_process::lock::FileLock,
    },
}

impl Checkout {
    fn path(&self) -> &Path {
        match self {
            Self::Temporary(directory) => directory.path(),
            Self::Workspace { path, .. } => path,
        }
    }
}

/// Persistent build workspaces of the host: one per source tree that can
/// build at the same time; later builds wait for a slot. They all compile in
/// the image pipeline's one shared compile cache.
const WORKSPACE_SLOTS: usize = 3;

impl FrozenSources {
    /// Materialize the snapshot in `directory`, its files read from
    /// `objects`, into a fresh temporary checkout.
    pub fn open(directory: &Path, objects: &Path) -> Result<Self> {
        let checkout = Checkout::Temporary(
            tempfile::Builder::new()
                .prefix("oer-source-build-")
                .tempdir()?,
        );
        Self::materialize(directory, objects, checkout)
    }

    /// Materialize into `workspace` once no other build holds it. The
    /// snapshot is verified in a staging directory; the workspace then keeps
    /// every file whose bytes and mode are unchanged, receives the rest and
    /// loses anything the snapshot does not contain.
    pub fn open_in_workspace(directory: &Path, objects: &Path, workspace: &Path) -> Result<Self> {
        let lock = oer_os_process::lock::FileLock::acquire(
            &Self::workspace_lock(workspace)?,
            oer_os_process::lock::Mode::Exclusive,
        )?;
        Self::open_locked(directory, objects, workspace, lock)
    }

    /// Materialize into the first free slot of the build workspaces
    /// `<base>-<n>`, waiting while every slot is busy. The slots persist, so
    /// an unchanged source file keeps its bytes and modification time and
    /// Cargo rebuilds only the packages whose sources changed. A cold build in
    /// a temporary directory takes longer than waiting for a slot.
    pub fn open_in_free_workspace(directory: &Path, objects: &Path, base: &Path) -> Result<Self> {
        let name = base
            .file_name()
            .ok_or("source build workspace has no name")?
            .to_string_lossy()
            .into_owned();
        let workspaces = (0..WORKSPACE_SLOTS)
            .map(|slot| base.with_file_name(format!("{name}-{slot}")))
            .collect::<Vec<_>>();
        let locks = workspaces
            .iter()
            .map(|workspace| Self::workspace_lock(workspace))
            .collect::<Result<Vec<_>>>()?;
        let lock = oer_os_process::lock::FileLock::wait_any(
            &locks,
            oer_os_process::lock::Mode::Exclusive,
            &format!(
                "waiting for one of the {WORKSPACE_SLOTS} source build slots at {}",
                base.display()
            ),
        )?;
        let slot = locks
            .iter()
            .position(|path| path == lock.path())
            .ok_or("a source build slot lock outside the slots")?;
        Self::open_locked(directory, objects, &workspaces[slot], lock)
    }

    /// The lock file of the build workspace `workspace`.
    fn workspace_lock(workspace: &Path) -> Result<PathBuf> {
        workspace
            .parent()
            .ok_or("source build workspace has no parent")?;
        Ok(workspace.with_extension("lock"))
    }

    fn open_locked(
        directory: &Path,
        objects: &Path,
        workspace: &Path,
        lock: oer_os_process::lock::FileLock,
    ) -> Result<Self> {
        let staging = workspace.with_extension("staging");
        if fs::symlink_metadata(&staging).is_ok() {
            fs::remove_dir_all(&staging)?;
        }
        fs::create_dir(&staging)?;
        let (snapshot, manifest) = materialize::materialize(directory, objects, &staging)?;
        if !fs::symlink_metadata(workspace).is_ok_and(|metadata| metadata.is_dir()) {
            if fs::symlink_metadata(workspace).is_ok() {
                fs::remove_file(workspace)?;
            }
            fs::create_dir(workspace)?;
        }
        synchronize(&staging, workspace)?;
        fs::remove_dir_all(&staging)?;
        let sources = Self {
            checkout: Checkout::Workspace {
                path: workspace.to_owned(),
                _lock: lock,
            },
            snapshot,
            manifest,
        };
        sources.verify_unchanged()?;
        Ok(sources)
    }

    fn materialize(directory: &Path, objects: &Path, checkout: Checkout) -> Result<Self> {
        let (snapshot, manifest) = materialize::materialize(directory, objects, checkout.path())?;
        Ok(Self {
            checkout,
            snapshot,
            manifest,
        })
    }

    pub fn repository(&self) -> PathBuf {
        self.checkout.path().join("repository")
    }

    /// The snapshot this checkout materializes.
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    /// Where the checkout of `role` (`repository`, `esp-hal`, `embassy` or
    /// `xarxa`) lies, if the snapshot holds that source.
    pub fn source_root(&self, role: &str) -> Option<PathBuf> {
        self.manifest
            .sources
            .iter()
            .any(|source| source.name == role)
            .then(|| self.checkout.path().join(role))
    }

    pub fn sources(&self) -> &[SourceInput] {
        &self.manifest.sources
    }

    pub fn verify_unchanged(&self) -> Result<()> {
        for source in &self.manifest.sources {
            for file in &source.files {
                let path = self.checkout.path().join(&source.name).join(&file.path);
                let metadata = fs::symlink_metadata(&path)?;
                if !metadata.is_file()
                    || metadata.len() != file.size_bytes
                    || oer_os_durable::sha256_file(&path)? != file.sha256
                {
                    return Err(format!(
                        "frozen build input changed: {}:{}",
                        source.name,
                        file.path.display()
                    )
                    .into());
                }
            }
        }
        Ok(())
    }
}

/// Make `destination` hold exactly the regular files of `source`, leaving
/// files with equal bytes and mode untouched.
fn synchronize(source: &Path, destination: &Path) -> Result<()> {
    let mut entries = fs::read_dir(destination)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let expected = source.join(entry.file_name());
        let kind = entry.file_type()?;
        let keep = match fs::symlink_metadata(&expected) {
            Ok(metadata) if metadata.is_dir() => kind.is_dir(),
            Ok(metadata) if metadata.is_file() => kind.is_file(),
            _ => false,
        };
        if !keep {
            if kind.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
    }
    let mut entries = fs::read_dir(source)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let target = destination.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if !target.is_dir() {
                fs::create_dir(&target)?;
            }
            synchronize(&entry.path(), &target)?;
        } else if !kind.is_file() {
            return Err("materialized snapshot holds a non-regular entry".into());
        } else if !same_file(&entry.path(), &target)? {
            // Replace atomically so a partly written file is never kept.
            let temporary =
                destination.join(format!(".{}.sync", entry.file_name().to_string_lossy()));
            fs::copy(entry.path(), &temporary)?;
            fs::rename(&temporary, &target)?;
        }
    }
    Ok(())
}

fn same_file(left: &Path, right: &Path) -> Result<bool> {
    let (left_metadata, Ok(right_metadata)) =
        (fs::symlink_metadata(left)?, fs::symlink_metadata(right))
    else {
        return Ok(false);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if left_metadata.permissions().mode() != right_metadata.permissions().mode() {
            return Ok(false);
        }
    }
    Ok(right_metadata.is_file()
        && left_metadata.len() == right_metadata.len()
        && fs::read(left)? == fs::read(right)?)
}

struct Selection {
    name: String,
    root: PathBuf,
    commit: String,
    dirty: bool,
    tracked: BTreeSet<PathBuf>,
    untracked: BTreeSet<PathBuf>,
}

/// Capture the sources at `root` into `store`, with the untracked files
/// `include` names and, when `include_untracked`, every untracked file inside
/// an image package.
pub fn capture(
    root: &Path,
    include: &[String],
    include_untracked: bool,
    store: &Path,
    objects: &Path,
) -> Result<Snapshot> {
    let overrides = Dependency::ALL
        .into_iter()
        .filter_map(|dependency| {
            std::env::var_os(dependency.root_env()).map(|path| (dependency, PathBuf::from(path)))
        })
        .collect::<Vec<_>>();
    capture_with_overrides(root, include, include_untracked, &overrides, store, objects)
}

/// [`capture`] with the local dependency checkouts named by `overrides`
/// instead of the environment's.
pub fn capture_with_overrides(
    root: &Path,
    include: &[String],
    include_untracked: bool,
    overrides: &[(Dependency, PathBuf)],
    store: &Path,
    objects: &Path,
) -> Result<Snapshot> {
    let scopes = if include_untracked {
        untracked_scopes(image_packages(root)?)
    } else {
        Vec::new()
    };
    let mut roots = vec![("repository".to_owned(), root.canonicalize()?)];
    for (dependency, path) in overrides {
        roots.push((dependency.id().into(), path.canonicalize()?));
    }
    capture_roots(&roots, include, &scopes, store, objects)
}

/// The repository directories whose untracked files `--include-untracked`
/// archives, each with the reason it records: the image `packages`, then the
/// HIL host inputs.
fn untracked_scopes(packages: Vec<PathBuf>) -> Vec<(PathBuf, UntrackedReason)> {
    packages
        .into_iter()
        .map(|package| (package, UntrackedReason::ImagePackage))
        .chain(
            HIL_HOST_INPUTS
                .into_iter()
                .map(|input| (PathBuf::from(input), UntrackedReason::HilHost)),
        )
        .collect()
}

/// The `--source-include` arguments that add each of `unresolved` (as
/// `role:path`) to a run, ready to paste.
fn source_include_arguments(unresolved: &[(&str, &Path)]) -> String {
    unresolved
        .iter()
        .map(|(role, path)| match *role {
            "repository" => format!("--source-include {}", path.display()),
            role => format!("--source-include {role}:{}", path.display()),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn contained(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!(
            "snapshot input must be a contained relative file: {}",
            path.display()
        )
        .into());
    }
    Ok(())
}

/// The Git state a snapshot archives: the checkout's index view
/// ([`checkout::IndexSnapshot`]), which names a tracked file the
/// worktree deleted, keeps a symlink as one and skips no component, so a
/// rebuild from the archive is the checkout's source state, excluding agent
/// guidance. The repository model instead lists existing files without build
/// output.
fn select(name: &str, root: &Path) -> Result<Selection> {
    let index = checkout::IndexSnapshot::read(root)
        .map_err(|error| format!("snapshot source {name}: {error}"))?;
    let paths = |paths: BTreeSet<String>| paths.into_iter().map(PathBuf::from).collect();
    let mut tracked: BTreeSet<PathBuf> = paths(index.tracked);
    let mut untracked: BTreeSet<PathBuf> = paths(index.untracked);
    tracked.retain(|path| !is_agent_guidance(path));
    untracked.retain(|path| !is_agent_guidance(path));
    Ok(Selection {
        name: name.into(),
        root: root.into(),
        commit: index.commit,
        dirty: index.dirty,
        tracked,
        untracked,
    })
}

/// A source that changed while it was read; the capture is taken again.
#[derive(Debug)]
struct ChangedDuringCapture(String);

impl std::fmt::Display for ChangedDuringCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "source {} changed during snapshot capture", self.0)
    }
}

impl std::error::Error for ChangedDuringCapture {}

/// Captures taken before a source that keeps changing fails the snapshot.
const CAPTURE_ATTEMPTS: u32 = 5;
/// The pause before the next capture, for an edit in progress to finish.
const CAPTURE_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(2);

/// `scopes` are repository-relative directories whose untracked files are
/// archived without being named, each with the reason recorded for them. A
/// source edited while it is read is captured again, so a snapshot is always
/// one consistent state of every source.
fn capture_roots(
    roots: &[(String, PathBuf)],
    include: &[String],
    scopes: &[(PathBuf, UntrackedReason)],
    output: &Path,
    objects: &Path,
) -> Result<Snapshot> {
    until_unchanged(CAPTURE_ATTEMPTS, CAPTURE_RETRY_PAUSE, || {
        capture_roots_once(roots, include, scopes, output, objects)
    })
}

/// Run `capture` again after a pause while it fails only because a source
/// changed under it, at most `attempts` times; any other failure, or the
/// last change, is returned.
fn until_unchanged<T>(
    attempts: u32,
    pause: std::time::Duration,
    mut capture: impl FnMut() -> Result<T>,
) -> Result<T> {
    let mut attempt = 1;
    loop {
        match capture() {
            Err(error) if attempt < attempts && error.is::<ChangedDuringCapture>() => {
                eprintln!("hil: {error}; capturing again");
                attempt += 1;
                oer_os_process::sleep(pause)?;
            }
            result => return result,
        }
    }
}

fn capture_roots_once(
    roots: &[(String, PathBuf)],
    include: &[String],
    scopes: &[(PathBuf, UntrackedReason)],
    output: &Path,
    objects: &Path,
) -> Result<Snapshot> {
    let selections = roots
        .iter()
        .map(|(name, root)| select(name, root))
        .collect::<Result<Vec<_>>>()?;
    let mut accepted = BTreeMap::<String, BTreeMap<PathBuf, UntrackedReason>>::new();
    for value in include {
        let (role, file) = value.split_once(':').unwrap_or(("repository", value));
        let path = PathBuf::from(file);
        contained(&path)?;
        let source = selections
            .iter()
            .find(|s| s.name == role)
            .ok_or_else(|| format!("unknown snapshot source role {role}"))?;
        if !source.untracked.contains(&path) {
            return Err(format!("--source-include is not an untracked file: {role}:{file}").into());
        }
        if accepted
            .entry(role.into())
            .or_default()
            .insert(path, UntrackedReason::SourceInclude)
            .is_some()
        {
            return Err(format!("duplicate --source-include: {role}:{file}").into());
        }
    }
    if let Some(repository) = selections.iter().find(|s| s.name == "repository") {
        let accepted = accepted.entry(repository.name.clone()).or_default();
        for path in &repository.untracked {
            if let Some((_, reason)) = scopes.iter().find(|(scope, _)| path.starts_with(scope)) {
                accepted.entry(path.clone()).or_insert(*reason);
            }
        }
    }
    let unresolved = selections
        .iter()
        .flat_map(|s| {
            s.untracked
                .iter()
                .filter(|p| !accepted.get(&s.name).is_some_and(|v| v.contains_key(*p)))
                .map(|p| (s.name.as_str(), p.as_path()))
        })
        .collect::<Vec<_>>();
    if !unresolved.is_empty() {
        return Err(format!(
            "source snapshot blocked by untracked files, which a build could read:\n{}\n\
             Commit, remove or ignore each one, or add them all to this run with\n  {}\n\
             `--include-untracked` adds those inside the packages an image builds, the HIL \
             host packages and hil/scenarios.",
            unresolved
                .iter()
                .map(|(role, path)| format!("{role}:{}", path.display()))
                .collect::<Vec<_>>()
                .join("\n"),
            source_include_arguments(&unresolved)
        )
        .into());
    }
    // No source bytes are archived until selection is complete for every root.
    fs::create_dir_all(output)?;
    let staging = tempfile::Builder::new()
        .prefix(".capture-")
        .tempdir_in(output)?;
    let mut sources = Vec::new();
    for selection in &selections {
        sources.push(read_source(selection, |file, bytes| {
            store_object(objects, &file.sha256, bytes)
        })?);
    }
    // Re-read bytes and membership, not just Git diff/stat caching. The archive
    // itself is authoritative; a changed capture is rejected, never relabelled.
    for (selection, captured) in selections.iter().zip(&sources) {
        let after = select(&selection.name, &selection.root)?;
        if after.tracked != selection.tracked
            || after.untracked != selection.untracked
            || read_source(&after, |_, _| Ok(()))? != *captured
        {
            return Err(ChangedDuringCapture(selection.name.clone()).into());
        }
    }
    for source in &mut sources {
        source.untracked = accepted
            .remove(&source.name)
            .unwrap_or_default()
            .into_iter()
            .map(|(path, by)| UntrackedInput { path, by })
            .collect();
    }
    let manifest = Manifest {
        schema: MANIFEST_SCHEMA,
        sources,
    };
    let snapshot_id = digest(&serde_json::to_vec(&manifest)?);
    atomic_json(&staging.path().join("manifest.json"), &manifest)?;
    let snapshot = Snapshot {
        schema: SNAPSHOT_SCHEMA,
        // A capture of an earlier record schema names the same manifest
        // digest and keeps its own directory; each schema has its own.
        directory: capture_directory(output).join(&snapshot_id),
        snapshot_id,
        files: manifest.sources.iter().map(|s| s.files.len()).sum(),
    };
    atomic_json(&staging.path().join("snapshot.json"), &snapshot)?;
    if snapshot.directory.exists() {
        if fs::read(snapshot.directory.join("manifest.json"))?
            != fs::read(staging.path().join("manifest.json"))?
            || fs::read(snapshot.directory.join("snapshot.json"))?
                != fs::read(staging.path().join("snapshot.json"))?
        {
            return Err("existing source snapshot has conflicting or corrupted content".into());
        }
        // A capture taken again is in use again.
        mark_used(&snapshot.directory)?;
    } else {
        let parent = capture_directory(output);
        fs::create_dir_all(&parent)?;
        fs::rename(staging.path(), &snapshot.directory)?;
        fs::File::open(&parent)?.sync_all()?;
    }
    Ok(snapshot)
}

fn read_source(
    selection: &Selection,
    mut consume: impl FnMut(&FileInput, &[u8]) -> Result<()>,
) -> Result<SourceInput> {
    let mut files = Vec::new();
    for relative in selection.tracked.union(&selection.untracked) {
        let mut path = selection.root.clone();
        let mut deleted = false;
        for component in relative.components() {
            path.push(component);
            match fs::symlink_metadata(&path) {
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound
                        && selection.tracked.contains(relative) =>
                {
                    deleted = true;
                    break;
                }
                Err(error) => return Err(error.into()),
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "snapshot requires review of symlink source: {}:{}",
                        selection.name,
                        relative.display()
                    )
                    .into());
                }
                Ok(_) => {}
            }
        }
        if deleted {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() {
            return Err(format!(
                "snapshot input is not a regular file: {}:{}",
                selection.name,
                relative.display()
            )
            .into());
        }
        let bytes = fs::read(&path)?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt as _;
            metadata.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        let file = FileInput {
            path: relative.clone(),
            size_bytes: bytes.len() as u64,
            sha256: digest(&bytes),
            mode: if executable { 0o755 } else { 0o644 },
        };
        consume(&file, &bytes)?;
        files.push(file);
    }
    Ok(SourceInput {
        name: selection.name.clone(),
        commit: selection.commit.clone(),
        dirty: selection.dirty,
        files,
        untracked: Vec::new(),
    })
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Record that the capture in `directory` was used just now: collection keeps
/// a capture for a while after its record was last written or used, so a
/// build or run from `--source-snapshot` calls this before it reads it.
pub fn mark_used(directory: &Path) -> Result<()> {
    fs::File::options()
        .append(true)
        .open(directory.join("snapshot.json"))?
        .set_modified(std::time::SystemTime::now())?;
    Ok(())
}

/// Where the captures of this build's record schema lie in the capture store
/// `store`: one directory per schema, so a capture never meets a directory
/// an earlier schema wrote for the same manifest.
pub fn capture_directory(store: &Path) -> PathBuf {
    store.join(format!("schema-{SNAPSHOT_SCHEMA}"))
}

/// Store `bytes`, whose SHA-256 is `sha256`, as an object in `objects`. An
/// object already there is kept when its bytes hash to its name, and its
/// modification time becomes now, so collection spares an object a capture
/// is about to name; one that does not is replaced.
fn store_object(objects: &Path, sha256: &str, bytes: &[u8]) -> Result<()> {
    let path = object(objects, sha256);
    if oer_os_durable::sha256_file(&path).is_ok_and(|stored| stored == sha256) {
        fs::File::options()
            .append(true)
            .open(&path)?
            .set_modified(std::time::SystemTime::now())?;
        return Ok(());
    }
    fs::create_dir_all(path.parent().ok_or("source object has no directory")?)?;
    // Concurrent captures of one digest write the same bytes; each renames
    // its own temporary file into place.
    oer_os_durable::atomic_write(&path, bytes)?;
    Ok(())
}

// Snapshot fixtures also serve the runner's cross-package tests.
#[cfg(any(test, feature = "test-support"))]
mod tests;
#[cfg(any(test, feature = "test-support"))]
pub use tests::{test_snapshot, test_snapshot_into};

mod materialize;
#[cfg(test)]
use materialize::materialize;
use oer_hil_schema::dependency::Dependency;

#[cfg(any(test, feature = "test-support"))]
pub fn test_capture(root: &Path) -> Snapshot {
    capture_roots(
        &[("repository".into(), root.to_owned())],
        &[],
        &[],
        &root.join("target/snapshots"),
        &test_objects(root),
    )
    .unwrap()
}

/// The objects directory of [`test_capture`]'s snapshots of `root`.
#[cfg(any(test, feature = "test-support"))]
pub fn test_objects(root: &Path) -> PathBuf {
    root.join("target/snapshots/sources")
}

/// The objects directory a test snapshot's files are in: `sources` beside
/// the snapshot's directory.
#[cfg(any(test, feature = "test-support"))]
pub fn test_objects_of(snapshot: &Snapshot) -> PathBuf {
    snapshot
        .directory()
        .parent()
        .and_then(Path::parent)
        .expect("a snapshot directory lies in a capture store")
        .join("sources")
}

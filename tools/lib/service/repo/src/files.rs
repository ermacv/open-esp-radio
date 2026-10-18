//! The repository's file set: what every tool that reads the tree as a
//! whole reads.
//!
//! A Git checkout's files are its tracked files and its untracked files Git
//! does not ignore (`git ls-files --cached --others --exclude-standard`);
//! another tree's are every file below its root. Build output, private
//! inputs and VCS state (any `target`, `_oracles` or `.git` component) never
//! belong to it, and only existing files count: a tracked file deleted in
//! the worktree is gone. Paths are repository-relative, `/`-separated and
//! normalized.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::Result;

/// Path components that never belong to the repository's file set.
pub const SKIPPED: [&str; 3] = ["target", "_oracles", ".git"];

/// The repository: its root and its files.
#[derive(Clone, Debug)]
pub struct Repo {
    root: PathBuf,
    files: BTreeSet<String>,
    /// Files Git tracks; every file for a tree that is not a checkout.
    tracked: BTreeSet<String>,
    directories: BTreeSet<String>,
}

/// Whether `path` lies below a skipped component.
pub fn skipped(path: &str) -> bool {
    path.split('/')
        .any(|component| SKIPPED.contains(&component))
}

impl Repo {
    /// The Git checkout at `root` when it is the top of one (holds `.git`),
    /// otherwise every file below `root`.
    pub fn load(root: &Path) -> Result<Self> {
        if root.join(".git").exists() {
            Self::from_git(root)
        } else {
            Self::from_dir(root)
        }
    }

    /// The tracked and untracked-but-not-ignored files of the Git checkout
    /// at `root`. A file that resolves outside the checkout is an error.
    pub fn from_git(root: &Path) -> Result<Self> {
        let list = |arguments: &[&str]| -> Result<Vec<String>> {
            let output = oer_process::git::output(root, arguments)
                .map_err(|error| format!("git ls-files failed: {error}"))?;
            let listing = String::from_utf8(output)
                .map_err(|_| "git ls-files printed a non-UTF-8 path".to_owned())?;
            Ok(listing
                .split('\0')
                .filter(|path| !path.is_empty() && !skipped(path))
                .map(str::to_owned)
                .collect())
        };
        let tracked = list(&["ls-files", "-z", "--cached"])?;
        let untracked = list(&["ls-files", "-z", "--others", "--exclude-standard"])?;
        let canonical = root
            .canonicalize()
            .map_err(|error| format!("cannot resolve {}: {error}", root.display()))?;
        let mut files = Vec::new();
        for path in tracked.iter().chain(&untracked) {
            if path.starts_with('/') || path.split('/').any(|part| part == "..") {
                return Err(format!("git listed a path outside the checkout: {path}"));
            }
            let absolute = root.join(path);
            if !absolute.is_file() {
                continue;
            }
            let link = fs::symlink_metadata(&absolute)
                .map_err(|error| format!("cannot read {path}: {error}"))?
                .file_type()
                .is_symlink();
            if link
                && !absolute
                    .canonicalize()
                    .is_ok_and(|target| target.starts_with(&canonical))
            {
                return Err(format!("{path} resolves outside the checkout"));
            }
            files.push(path.clone());
        }
        let mut repo = Self::new(root, files);
        repo.tracked = tracked
            .into_iter()
            .filter(|path| repo.files.contains(path))
            .collect();
        Ok(repo)
    }

    /// Every file below `root`, for trees that are not Git checkouts; every
    /// file counts as tracked.
    pub fn from_dir(root: &Path) -> Result<Self> {
        let mut files = vec![];
        let mut pending = vec![PathBuf::new()];
        while let Some(relative) = pending.pop() {
            let directory = root.join(&relative);
            let entries = fs::read_dir(&directory)
                .map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let path = relative.join(name);
                let display = path.to_string_lossy().replace('\\', "/");
                if skipped(&display) {
                    continue;
                }
                let kind = entry.file_type().map_err(|error| error.to_string())?;
                if kind.is_dir() {
                    pending.push(path);
                } else if kind.is_file() {
                    files.push(display);
                }
            }
        }
        let mut repo = Self::new(root, files);
        repo.tracked = repo.files.clone();
        Ok(repo)
    }

    fn new(root: &Path, files: impl IntoIterator<Item = String>) -> Self {
        let files: BTreeSet<String> = files.into_iter().collect();
        let mut directories = BTreeSet::new();
        for file in &files {
            let mut path = file.as_str();
            while let Some((parent, _)) = path.rsplit_once('/') {
                if !directories.insert(parent.to_owned()) {
                    break;
                }
                path = parent;
            }
        }
        Self {
            root: root.to_owned(),
            files,
            tracked: BTreeSet::new(),
            directories,
        }
    }

    /// The repository root on disk.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every file, ascending.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(String::as_str)
    }

    /// Every file Git tracks, ascending.
    pub fn tracked(&self) -> impl Iterator<Item = &str> {
        self.tracked.iter().map(String::as_str)
    }

    /// Whether `path` is a file Git tracks.
    pub fn is_tracked(&self, path: &str) -> bool {
        self.tracked.contains(path)
    }

    /// Whether `path` is a file of the repository.
    pub fn is_file(&self, path: &str) -> bool {
        self.files.contains(path)
    }

    /// Whether `path` names a file or a directory holding files.
    pub fn exists(&self, path: &str) -> bool {
        let path = path.trim_end_matches('/');
        path.is_empty() || self.files.contains(path) || self.directories.contains(path)
    }

    /// The files directly in `directory` (`""` for the root).
    pub fn children<'a>(&'a self, directory: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let prefix = if directory.is_empty() {
            String::new()
        } else {
            format!("{directory}/")
        };
        let length = prefix.len();
        self.files
            .range(prefix.clone()..)
            .take_while(move |file| file.starts_with(&prefix))
            .map(String::as_str)
            .filter(move |file| !file[length..].contains('/'))
    }

    /// The files at any depth below `directory`.
    pub fn below<'a>(&'a self, directory: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let prefix = format!("{}/", directory.trim_end_matches('/'));
        self.files
            .range(prefix.clone()..)
            .take_while(move |file| file.starts_with(&prefix))
            .map(String::as_str)
    }

    /// The contents of the repository file `path`.
    pub fn read(&self, path: &str) -> Result<String> {
        fs::read_to_string(self.root.join(path))
            .map_err(|error| format!("cannot read {path}: {error}"))
    }

    /// The absolute path of the repository file `path`.
    pub fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

/// The directory part of `path` (`""` at the root).
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// Whether `path` is `directory` or lies below it (`""` holds everything).
pub fn within(path: &str, directory: &str) -> bool {
    directory.is_empty()
        || path == directory
        || path
            .strip_prefix(directory)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// `relative` resolved against the repository directory `base`, normalized;
/// `None` when it is absolute or leaves the repository.
pub fn join(base: &str, relative: &str) -> Option<String> {
    if relative.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::tree;

    #[test]
    fn joins_resolve_parents_and_refuse_to_leave_the_root() {
        assert_eq!(join("a/b", "../c/./d.rs").as_deref(), Some("a/c/d.rs"));
        assert_eq!(join("", "x.rs").as_deref(), Some("x.rs"));
        assert_eq!(join("a", "../../x"), None);
        assert_eq!(join("a", "/etc/passwd"), None);
        assert!(within("a/b/c", "a/b") && within("a/b", "a/b") && within("x", ""));
        assert!(!within("a/bc", "a/b"));
    }

    #[test]
    fn directories_exist_only_when_they_hold_files() {
        let dir = tree(&[
            ("a/b/c.rs", ""),
            ("target/debug/x.rs", ""),
            ("_oracles/y", ""),
        ]);
        fs::create_dir_all(dir.path().join("empty")).unwrap();
        let repo = Repo::from_dir(dir.path()).unwrap();
        assert!(repo.exists("a") && repo.exists("a/b") && repo.exists("a/b/c.rs"));
        assert!(!repo.exists("empty"));
        assert!(!repo.exists("target/debug/x.rs") && !repo.exists("_oracles/y"));
        assert_eq!(repo.children("a/b").collect::<Vec<_>>(), ["a/b/c.rs"]);
        assert_eq!(repo.children("a").count(), 0);
        assert!(repo.is_tracked("a/b/c.rs"));
    }

    #[test]
    fn a_checkout_lists_tracked_and_unignored_files_only() {
        let dir = tree(&[
            (".gitignore", "/ignored/\n"),
            ("kept.rs", ""),
            ("new.rs", ""),
            ("ignored/x.rs", ""),
            ("gone.rs", ""),
        ]);
        let git = |arguments: &[&str]| {
            oer_process::run(oer_process::git::command(dir.path()).args(arguments)).unwrap();
        };
        git(&["init", "--quiet"]);
        git(&["add", "kept.rs", "gone.rs", ".gitignore"]);
        fs::remove_file(dir.path().join("gone.rs")).unwrap();
        let repo = Repo::load(dir.path()).unwrap();
        assert_eq!(
            repo.files().collect::<Vec<_>>(),
            [".gitignore", "kept.rs", "new.rs"]
        );
        assert!(repo.is_tracked("kept.rs") && !repo.is_tracked("new.rs"));
    }
}

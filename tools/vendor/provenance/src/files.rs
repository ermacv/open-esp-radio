//! The repository files the registry scans: a Git checkout's tracked and
//! untracked-but-not-ignored files, or every file below a directory that is
//! not a checkout.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use crate::Result;

/// Components never scanned.
const SKIPPED: [&str; 2] = ["target", ".git"];

fn skipped(path: &str) -> bool {
    path.split('/')
        .any(|component| SKIPPED.contains(&component))
}

/// The files of one tree, repository-relative.
pub struct Files {
    root: PathBuf,
    files: BTreeSet<String>,
}

impl Files {
    /// The Git checkout at `root` when it holds `.git`, otherwise every file
    /// below `root`.
    pub fn load(root: &Path) -> Result<Self> {
        if root.join(".git").exists() {
            Self::from_git(root)
        } else {
            Self::from_dir(root)
        }
    }

    fn from_git(root: &Path) -> Result<Self> {
        let list = |arguments: &[&str]| -> Result<Vec<String>> {
            let output = oer_os_process::git::output(root, arguments)
                .map_err(|error| format!("git ls-files failed: {error}"))?;
            let listing = String::from_utf8(output)
                .map_err(|_| "git ls-files printed a non-UTF-8 path".to_owned())?;
            Ok(listing
                .split('\0')
                .filter(|path| !path.is_empty() && !skipped(path))
                .map(str::to_owned)
                .collect())
        };
        let mut files = list(&["ls-files", "-z", "--cached"])?;
        files.extend(list(&["ls-files", "-z", "--others", "--exclude-standard"])?);
        let files = files
            .into_iter()
            .filter(|path| root.join(path).is_file())
            .collect();
        Ok(Self {
            root: root.to_owned(),
            files,
        })
    }

    /// Every file below `root`.
    pub fn from_dir(root: &Path) -> Result<Self> {
        let mut files = BTreeSet::new();
        let mut pending = vec![PathBuf::new()];
        while let Some(relative) = pending.pop() {
            let directory = root.join(&relative);
            let entries = fs::read_dir(&directory)
                .map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                let path = relative.join(entry.file_name());
                let name = path.to_string_lossy().replace('\\', "/");
                if skipped(&name) {
                    continue;
                }
                if entry
                    .file_type()
                    .map_err(|error| error.to_string())?
                    .is_dir()
                {
                    pending.push(path);
                } else {
                    files.insert(name);
                }
            }
        }
        Ok(Self {
            root: root.to_owned(),
            files,
        })
    }

    /// Every file.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(String::as_str)
    }

    /// The files below `directory`.
    pub fn below<'a>(&'a self, directory: &'a str) -> impl Iterator<Item = &'a str> {
        let prefix = format!("{}/", directory.trim_end_matches('/'));
        self.files().filter(move |file| file.starts_with(&prefix))
    }

    /// The text of `file`.
    pub fn read(&self, file: &str) -> Result<String> {
        fs::read_to_string(self.root.join(file))
            .map_err(|error| format!("cannot read {file}: {error}").into())
    }
}

//! The index view of a Git checkout: its state as Git records it, which a
//! source archive must reproduce.
//!
//! [`crate::Repo`] answers which files the tree holds now: existing files,
//! without build output, private inputs or VCS state. An [`IndexSnapshot`]
//! answers what the checkout's state is: every path of the index, a tracked
//! file deleted from the worktree included; every untracked file Git does
//! not ignore; a symlink as itself; and no component skipped. Paths are
//! repository-relative and `/`-separated; one that is absolute or climbs out
//! of the checkout is an error.

use std::{collections::BTreeSet, path::Path};

use crate::Result;

/// A checkout's commit, whether its worktree differs from it, its index
/// paths and its untracked, unignored files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexSnapshot {
    /// The commit `HEAD` names.
    pub commit: String,
    /// Whether `git status` reports any change, untracked files included.
    pub dirty: bool,
    /// Every path of the index, whether or not the worktree still holds it.
    pub tracked: BTreeSet<String>,
    /// Untracked files Git does not ignore.
    pub untracked: BTreeSet<String>,
}

impl IndexSnapshot {
    /// The index view of the checkout whose top level is `root`; a
    /// directory inside a checkout is refused.
    pub fn read(root: &Path) -> Result<Self> {
        let git = |arguments: &[&str]| -> Result<Vec<u8>> {
            oer_process::git::output(root, arguments)
                .map_err(|error| format!("Git query failed: {error}"))
        };
        let text = |arguments: &[&str]| -> Result<String> {
            String::from_utf8(git(arguments)?)
                .map(|text| text.trim().to_owned())
                .map_err(|_| format!("git {} printed non-UTF-8 text", arguments.join(" ")))
        };
        let top = text(&["rev-parse", "--show-toplevel"])?;
        let resolve = |path: &Path| {
            path.canonicalize()
                .map_err(|error| format!("cannot resolve {}: {error}", path.display()))
        };
        if resolve(Path::new(&top))? != resolve(root)? {
            return Err(format!(
                "{} is not the top level of its Git checkout",
                root.display()
            ));
        }
        Ok(Self {
            commit: text(&["rev-parse", "HEAD"])?,
            dirty: !git(&["status", "--porcelain=v1", "--untracked-files=normal"])?.is_empty(),
            tracked: paths(&git(&["ls-files", "--cached", "-z"])?)?,
            untracked: paths(&git(&["ls-files", "--others", "--exclude-standard", "-z"])?)?,
        })
    }
}

fn paths(listing: &[u8]) -> Result<BTreeSet<String>> {
    listing
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            let path = std::str::from_utf8(path)
                .map_err(|_| "git ls-files printed a non-UTF-8 path".to_owned())?;
            if path.starts_with('/') || path.split('/').any(|part| matches!(part, "" | "." | ".."))
            {
                return Err(format!("git listed a path outside the checkout: {path}"));
            }
            Ok(path.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(root: &Path, arguments: &[&str]) {
        let configured = [
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ];
        oer_process::git::output(root, configured.iter().chain(arguments)).unwrap();
    }

    fn write(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn the_index_keeps_deleted_files_symlinks_and_every_component() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-q"]);
        write(root, ".gitignore", "ignored.txt\n");
        write(root, "src/lib.rs", "a");
        write(root, "target/kept.rs", "b");
        write(root, "deleted.rs", "c");
        std::os::unix::fs::symlink("src/lib.rs", root.join("link.rs")).unwrap();
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "base"]);
        let clean = IndexSnapshot::read(root).unwrap();
        assert!(!clean.dirty);
        assert_eq!(
            clean.commit,
            oer_process::git::text(root, ["rev-parse", "HEAD"]).unwrap()
        );
        assert!(clean.untracked.is_empty());

        std::fs::remove_file(root.join("deleted.rs")).unwrap();
        write(root, "new.rs", "d");
        write(root, "_oracles/private.rs", "e");
        write(root, "ignored.txt", "f");
        let snapshot = IndexSnapshot::read(root).unwrap();
        assert!(snapshot.dirty);
        assert_eq!(
            snapshot.tracked,
            [
                ".gitignore",
                "deleted.rs",
                "link.rs",
                "src/lib.rs",
                "target/kept.rs"
            ]
            .map(String::from)
            .into()
        );
        assert_eq!(
            snapshot.untracked,
            ["_oracles/private.rs", "new.rs"].map(String::from).into()
        );
    }

    #[test]
    fn a_directory_inside_a_checkout_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        git(root, &["init", "-q"]);
        write(root, "src/lib.rs", "a");
        git(root, &["add", "."]);
        git(root, &["commit", "-qm", "base"]);
        let error = IndexSnapshot::read(&root.join("src")).unwrap_err();
        assert!(error.contains("not the top level"), "{error}");
    }
}

//! The one way repository tools run Git: `git -C <directory> <arguments>`,
//! supervised as an owned process, with Git's own error output in the error.

use std::{ffi::OsStr, path::Path, process::Command};

use crate::Result;

/// `git -C directory`, for a caller that adds its arguments, environment or
/// standard input itself and runs it through this crate.
pub fn command(directory: &Path) -> Command {
    let mut command = crate::command("git");
    command.arg("-C").arg(directory);
    command
}

/// Standard output of `git -C directory arguments…`; a failure carries Git's
/// standard error.
pub fn output<I, S>(directory: &Path, arguments: I) -> Result<Vec<u8>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command(directory);
    command.args(arguments);
    crate::capture(&mut command)
        .map(|output| output.stdout)
        .map_err(|error| format!("in {}: {error}", directory.display()).into())
}

/// [`output`] as UTF-8 text without surrounding white space.
pub fn text<I, S>(directory: &Path, arguments: I) -> Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Ok(String::from_utf8(output(directory, arguments)?)?
        .trim()
        .to_owned())
}

/// The non-empty lines of [`output`].
pub fn lines<I, S>(directory: &Path, arguments: I) -> Result<Vec<String>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Ok(String::from_utf8(output(directory, arguments)?)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Run `git -C directory arguments…` with its output on the terminal (or the
/// log [`crate::log_output_to`] chose), failing unless it succeeds.
pub fn run<I, S>(directory: &Path, arguments: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = command(directory);
    command.args(arguments);
    crate::run(&mut command)
}

/// The commit `revision` names in the repository at `directory`.
pub fn commit(directory: &Path, revision: &str) -> Result<String> {
    text(
        directory,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{revision}^{{commit}}"),
        ],
    )
    .map_err(|_| format!("{revision} names no commit").into())
}

/// A detached worktree of a repository at one revision: the one way the
/// tools check another revision out beside a checkout (image comparisons,
/// A/B arms, bisection steps).
#[derive(Debug)]
pub struct Worktree {
    repository: std::path::PathBuf,
    path: std::path::PathBuf,
}

impl Worktree {
    /// Check `revision` of the repository at `repository` out, detached, at
    /// `path`: in the worktree already there, discarding its changes and
    /// untracked files, or in a new one.
    pub fn detached(repository: &Path, path: &Path, revision: &str) -> Result<Self> {
        let worktree = Self {
            repository: repository.to_owned(),
            path: path.to_owned(),
        };
        worktree.checkout(revision)?;
        Ok(worktree)
    }

    /// Move the worktree to `revision`, discarding its changes and untracked
    /// files.
    pub fn checkout(&self, revision: &str) -> Result<()> {
        if self.path.join(".git").exists() {
            run(
                &self.path,
                ["checkout", "--quiet", "--detach", "--force", revision],
            )?;
            run(&self.path, ["clean", "-fdq"])
        } else {
            // A worktree whose directory is gone stays registered until
            // pruned, and its path cannot be added again before.
            run(&self.repository, ["worktree", "prune"])?;
            let mut add = command(&self.repository);
            add.args(["worktree", "add", "--detach", "--force"])
                .arg(&self.path)
                .arg(revision);
            crate::run(&mut add)
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the worktree and its directory.
    pub fn remove(self) -> Result<()> {
        let mut remove = command(&self.repository);
        remove
            .args(["worktree", "remove", "--force"])
            .arg(&self.path);
        crate::run(&mut remove)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> (tempfile::TempDir, String, String) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let git = |arguments: &[&str]| {
            run(
                root,
                [
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                ]
                .iter()
                .chain(arguments),
            )
            .unwrap()
        };
        git(&["init", "--quiet"]);
        std::fs::write(root.join("file"), "one").unwrap();
        git(&["add", "file"]);
        git(&["commit", "--quiet", "-m", "one"]);
        let first = commit(root, "HEAD").unwrap();
        std::fs::write(root.join("file"), "two").unwrap();
        git(&["commit", "--quiet", "-am", "two"]);
        let second = commit(root, "HEAD").unwrap();
        (directory, first, second)
    }

    #[test]
    fn a_worktree_moves_between_revisions_and_drops_its_changes() {
        let (repository, first, second) = repository();
        let path = repository.path().join("worktree");
        let worktree = Worktree::detached(repository.path(), &path, &first).unwrap();
        assert_eq!(std::fs::read_to_string(path.join("file")).unwrap(), "one");
        std::fs::write(path.join("untracked"), "x").unwrap();
        std::fs::write(path.join("file"), "changed").unwrap();
        worktree.checkout(&second).unwrap();
        assert_eq!(std::fs::read_to_string(path.join("file")).unwrap(), "two");
        assert!(!path.join("untracked").exists());
        // A second worktree at the same path reuses the checkout.
        let again = Worktree::detached(repository.path(), &path, &first).unwrap();
        assert_eq!(std::fs::read_to_string(path.join("file")).unwrap(), "one");
        again.remove().unwrap();
        assert!(!path.exists());
        assert!(commit(repository.path(), "no-such-revision").is_err());
    }
}

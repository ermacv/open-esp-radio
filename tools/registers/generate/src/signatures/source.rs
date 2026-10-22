//! Do not label dirty or unpinned header bytes as facts from a pinned tree.
use crate::Result;
use oer_os_process::git;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

fn repository(path: &Path) -> Result<PathBuf> {
    path.ancestors()
        .find(|p| p.join(".git").exists())
        .map(Path::to_owned)
        .ok_or_else(|| format!("{} is not in a Git source tree", path.display()).into())
}

fn verify_repository(
    root: &Path,
    repo: &Path,
    revision: &str,
    checked: &mut BTreeSet<PathBuf>,
) -> Result<()> {
    if !checked.insert(repo.to_owned()) {
        return Ok(());
    }
    let actual = git::text(repo, ["rev-parse", "HEAD"])?;
    if repo == root {
        if actual != revision {
            return Err("ESP-IDF revision changed".into());
        }
    } else {
        let parent = repository(repo.parent().ok_or("submodule without parent")?)?;
        if !parent.starts_with(root) {
            return Err("header repository is outside pinned ESP-IDF".into());
        }
        verify_repository(root, &parent, revision, checked)?;
        let relative = repo.strip_prefix(&parent)?.to_string_lossy();
        let entry = git::text(&parent, ["ls-tree", "HEAD", "--", &relative])?;
        let expected = format!("160000 commit {actual}\t");
        if !entry.starts_with(&expected) {
            return Err(format!(
                "header submodule {} differs from its pinned gitlink",
                relative
            )
            .into());
        }
    }
    Ok(())
}

pub(super) fn verify(idf: &Path, headers: &BTreeMap<String, String>, revision: &str) -> Result<()> {
    let root = fs::canonicalize(idf)?;
    let mut checked = BTreeSet::new();
    for (header, expected) in headers {
        let path = fs::canonicalize(root.join(header))?;
        if !path.starts_with(&root) {
            return Err("header escapes pinned ESP-IDF".into());
        }
        let repo = repository(&path)?;
        verify_repository(&root, &repo, revision, &mut checked)?;
        let relative = path.strip_prefix(&repo)?.to_string_lossy();
        let committed = git::output(&repo, ["show", &format!("HEAD:{relative}")])?;
        if oer_os_durable::sha256_bytes(&committed) != *expected
            || oer_os_durable::sha256_file(&path)? != *expected
        {
            return Err(format!("{header} differs from its pinned committed bytes").into());
        }
    }
    Ok(())
}

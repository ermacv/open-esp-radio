//! Cargo workspaces discovered from the manifests, as Cargo finds them.
//!
//! A workspace is a manifest with `[workspace]`, or a package that no
//! workspace above it claims or excludes into a root of its own. Tooling
//! that iterates every workspace (CI formatting, locks) takes this list
//! instead of keeping its own.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    files::{parent, within},
    manifest::{Manifests, Package},
};

/// Where a package belongs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Membership<'a> {
    /// A member of the workspace whose directory this is.
    Member(&'a str),
    /// Its own workspace root, explicit or implicit.
    Root,
    /// A workspace above it, at this directory, neither lists nor excludes it.
    Unclaimed(&'a str),
}

/// Whether the workspace at `workspace` excludes `directory`: below an
/// `exclude` entry and not itself an explicit member, as Cargo decides (a
/// package below a member's directory is not that member).
fn excludes(manifests: &Manifests, workspace: &str, directory: &str) -> bool {
    let below = |root: &String| within(directory, root);
    manifests
        .workspaces
        .iter()
        .filter(|declared| declared.directory == workspace)
        .any(|declared| {
            declared.exclude.iter().any(below)
                && !declared.members.iter().any(|member| member == directory)
        })
}

/// The directory of the nearest workspace declaration above `directory`
/// that does not exclude it, as Cargo searches for a package's root.
fn enclosing<'a>(manifests: &'a Manifests, directory: &str) -> Option<&'a str> {
    let mut current = directory;
    while !current.is_empty() {
        current = parent(current);
        if let Some(workspace) = manifests.workspace_at(current)
            && !excludes(manifests, current, directory)
        {
            return Some(&workspace.directory);
        }
    }
    None
}

/// Members of every workspace: listed members and, transitively, their
/// path dependencies inside the workspace directory.
fn members(manifests: &Manifests) -> BTreeMap<&str, BTreeSet<String>> {
    let mut all = BTreeMap::new();
    for workspace in &manifests.workspaces {
        let mut set: BTreeSet<String> = workspace.members.iter().cloned().collect();
        let mut pending: Vec<String> = set.iter().cloned().collect();
        while let Some(member) = pending.pop() {
            let Some(package) = manifests.package_at(&member) else {
                continue;
            };
            for dependency in &package.dependencies {
                let path = dependency.path.as_ref().or_else(|| {
                    // An inherited declaration names its path in the
                    // workspace's own table.
                    dependency
                        .inherited
                        .then(|| workspace.dependencies.get(&dependency.key))
                        .flatten()
                        .and_then(|declared| declared.path.as_ref())
                });
                if let Some(path) = path
                    && within(path, &workspace.directory)
                    && path != &workspace.directory
                    && !workspace
                        .exclude
                        .iter()
                        .any(|excluded| path.starts_with(excluded.as_str()))
                    && set.insert(path.clone())
                {
                    pending.push(path.clone());
                }
            }
        }
        all.insert(workspace.directory.as_str(), set);
    }
    all
}

/// Every package's membership, by manifest path.
pub fn memberships(manifests: &Manifests) -> BTreeMap<&str, Membership<'_>> {
    let members = members(manifests);
    manifests
        .packages
        .iter()
        .map(|package| {
            (
                package.manifest.as_str(),
                membership(manifests, &members, package),
            )
        })
        .collect()
}

fn membership<'a>(
    manifests: &'a Manifests,
    members: &BTreeMap<&str, BTreeSet<String>>,
    package: &'a Package,
) -> Membership<'a> {
    if manifests.workspace_at(&package.directory).is_some() {
        return Membership::Root;
    }
    let root = match &package.workspace {
        Some(root) => Some(root.as_str()),
        None => enclosing(manifests, &package.directory),
    };
    let Some(root) = root else {
        return Membership::Root;
    };
    if members
        .get(root)
        .is_some_and(|set| set.contains(&package.directory))
    {
        Membership::Member(root)
    } else {
        Membership::Unclaimed(root)
    }
}

/// Manifest paths of every workspace root, ascending.
pub fn discover(manifests: &Manifests) -> Vec<String> {
    let mut roots: BTreeSet<String> = manifests
        .workspaces
        .iter()
        .map(|workspace| workspace.manifest.clone())
        .collect();
    for (manifest, membership) in memberships(manifests) {
        if membership == Membership::Root {
            roots.insert(manifest.to_owned());
        }
    }
    roots.into_iter().collect()
}

/// The workspace root manifest of every claimed package, keyed by the
/// package's manifest path.
pub fn owners(manifests: &Manifests) -> BTreeMap<String, String> {
    let mut owners = BTreeMap::new();
    for (manifest, membership) in memberships(manifests) {
        let owner = match membership {
            Membership::Root => manifest.to_owned(),
            Membership::Member(root) => manifests.workspace_at(root).map_or_else(
                || format!("{root}/Cargo.toml"),
                |workspace| workspace.manifest.clone(),
            ),
            Membership::Unclaimed(_) => continue,
        };
        owners.insert(manifest.to_owned(), owner);
    }
    owners
}

/// The lock file beside the workspace root manifest `manifest`.
pub fn lock_of(manifest: &str) -> String {
    format!("{}Cargo.lock", manifest.trim_end_matches("Cargo.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{files::Repo, testing::tree};

    pub(crate) const FILES: &[(&str, &str)] = &[
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\", \"firmware/shared\", \"firmware/app/nested\"]\nexclude = [\"island\", \"firmware\"]\n",
        ),
        ("Cargo.lock", ""),
        (
            "crates/a/Cargo.toml",
            "[package]\nname = \"a\"\n[dependencies]\nb = { path = \"../../shared/b\" }\n",
        ),
        ("shared/b/Cargo.toml", "[package]\nname = \"b\"\n"),
        ("island/Cargo.toml", "[package]\nname = \"island\"\n"),
        ("island/Cargo.lock", ""),
        (
            "firmware/Cargo.toml",
            "[workspace]\nmembers = [\"app\"]\nexclude = [\"shared\", \"app/nested\"]\n",
        ),
        (
            "firmware/shared/Cargo.toml",
            "[package]\nname = \"shared\"\n",
        ),
        ("firmware/Cargo.lock", ""),
        ("firmware/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        // Below a member's directory but excluded: the root's, as for Cargo.
        (
            "firmware/app/nested/Cargo.toml",
            "[package]\nname = \"nested\"\n",
        ),
    ];

    #[test]
    fn explicit_and_implicit_workspace_roots_are_discovered() {
        let dir = tree(FILES);
        let repo = Repo::from_dir(dir.path()).unwrap();
        let manifests = Manifests::load(&repo).unwrap();
        assert_eq!(
            discover(&manifests),
            ["Cargo.toml", "firmware/Cargo.toml", "island/Cargo.toml"]
        );
        let owners = owners(&manifests);
        assert_eq!(owners["crates/a/Cargo.toml"], "Cargo.toml");
        assert_eq!(owners["shared/b/Cargo.toml"], "Cargo.toml");
        assert_eq!(owners["firmware/app/Cargo.toml"], "firmware/Cargo.toml");
        assert_eq!(owners["island/Cargo.toml"], "island/Cargo.toml");
        assert_eq!(owners["firmware/shared/Cargo.toml"], "Cargo.toml");
        assert_eq!(owners["firmware/app/nested/Cargo.toml"], "Cargo.toml");
        assert_eq!(lock_of("firmware/Cargo.toml"), "firmware/Cargo.lock");
        assert_eq!(lock_of("Cargo.toml"), "Cargo.lock");
    }
}

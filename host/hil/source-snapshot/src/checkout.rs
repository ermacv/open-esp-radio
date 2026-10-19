//! What a snapshot reads of the checkout: Git's index view of it, and the
//! repository packages a set of root packages reaches, as Cargo resolves
//! the workspace.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use oer_os_durable::Result;
use serde::Deserialize;

/// The checkout's index view.
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
            oer_os_process::git::output(root, arguments)
                .map_err(|error| format!("Git query failed: {error}").into())
        };
        let text = |arguments: &[&str]| -> Result<String> {
            String::from_utf8(git(arguments)?)
                .map(|text| text.trim().to_owned())
                .map_err(|_| format!("git {} printed non-UTF-8 text", arguments.join(" ")).into())
        };
        let top = PathBuf::from(text(&["rev-parse", "--show-toplevel"])?);
        if top.canonicalize()? != root.canonicalize()? {
            return Err(format!(
                "{} is not the top level of its Git checkout",
                root.display()
            )
            .into());
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
                return Err(format!("git listed a path outside the checkout: {path}").into());
            }
            Ok(path.to_owned())
        })
        .collect()
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
    resolve: Resolve,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
    /// `None` for a package of the repository.
    source: Option<String>,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    pkg: String,
    dep_kinds: Vec<DependencyKind>,
}

#[derive(Deserialize)]
struct DependencyKind {
    /// `None` for a normal dependency.
    kind: Option<String>,
}

/// Which dependency edges a closure follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Edges {
    /// Normal and build dependencies: what a build of the roots compiles.
    Build,
    /// Every dependency, development ones included.
    All,
}

/// The repository-relative directories of the repository packages `roots`
/// (package names) reach through `edges`, with every feature enabled, the
/// roots included.
pub fn package_closure(root: &Path, roots: &[&str], edges: Edges) -> Result<BTreeSet<PathBuf>> {
    let output = oer_os_process::capture(
        oer_os_process::command(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .current_dir(root)
            .args([
                "metadata",
                "--format-version",
                "1",
                "--locked",
                "--all-features",
            ]),
    )?;
    closure(&serde_json::from_slice(&output.stdout)?, root, roots, edges)
}

fn closure(
    metadata: &Metadata,
    root: &Path,
    roots: &[&str],
    edges: Edges,
) -> Result<BTreeSet<PathBuf>> {
    let packages: BTreeMap<&str, &Package> = metadata
        .packages
        .iter()
        .map(|package| (package.id.as_str(), package))
        .collect();
    let nodes: BTreeMap<&str, &Node> = metadata
        .resolve
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut pending = Vec::new();
    for name in roots {
        let package = metadata
            .packages
            .iter()
            .find(|package| package.name == *name && package.source.is_none())
            .ok_or_else(|| format!("the workspace has no package {name}"))?;
        pending.push(package.id.as_str());
    }
    let mut seen = BTreeSet::new();
    let mut directories = BTreeSet::new();
    let root = root.canonicalize()?;
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(package) = packages.get(id) else {
            continue;
        };
        if package.source.is_some() {
            continue;
        }
        let directory = package
            .manifest_path
            .parent()
            .ok_or("a manifest without a directory")?
            .strip_prefix(&root)
            .map_err(|_| format!("{} lies outside the repository", package.name))?;
        directories.insert(directory.to_owned());
        let Some(node) = nodes.get(id) else { continue };
        for dependency in &node.deps {
            let followed = dependency
                .dep_kinds
                .iter()
                .any(|kind| edges == Edges::All || kind.kind.as_deref() != Some("dev"));
            if followed {
                pending.push(dependency.pkg.as_str());
            }
        }
    }
    Ok(directories)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(root: &str) -> Metadata {
        let package = |id: &str, directory: &str, source: Option<&str>| Package {
            id: id.into(),
            name: id.into(),
            manifest_path: PathBuf::from(format!("{root}/{directory}/Cargo.toml")),
            source: source.map(Into::into),
        };
        let dependency = |pkg: &str, kind: Option<&str>| Dependency {
            pkg: pkg.into(),
            dep_kinds: vec![DependencyKind {
                kind: kind.map(Into::into),
            }],
        };
        Metadata {
            packages: vec![
                package("agent", "portable/hil/agent", None),
                package("hal", "crates/hal", None),
                package("tests", "crates/tests", None),
                package(
                    "serde",
                    "registry/serde",
                    Some("registry+https://crates.io"),
                ),
            ],
            resolve: Resolve {
                nodes: vec![
                    Node {
                        id: "agent".into(),
                        deps: vec![
                            dependency("hal", None),
                            dependency("tests", Some("dev")),
                            dependency("serde", Some("build")),
                        ],
                    },
                    Node {
                        id: "hal".into(),
                        deps: vec![],
                    },
                    Node {
                        id: "tests".into(),
                        deps: vec![],
                    },
                ],
            },
        }
    }

    #[test]
    fn a_closure_follows_its_edges_through_repository_packages_only() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let metadata = metadata(root_path.to_str().unwrap());
        let build = closure(&metadata, &root_path, &["agent"], Edges::Build).unwrap();
        assert_eq!(
            build,
            BTreeSet::from(["crates/hal".into(), "portable/hil/agent".into()])
        );
        let all = closure(&metadata, &root_path, &["agent"], Edges::All).unwrap();
        assert!(all.contains(Path::new("crates/tests")));
        assert!(closure(&metadata, &root_path, &["missing"], Edges::All).is_err());
    }

    #[test]
    fn a_listed_path_outside_the_checkout_is_refused() {
        assert!(paths(b"a/b\0c\0").is_ok());
        assert!(paths(b"../x\0").is_err());
        assert!(paths(b"/abs\0").is_err());
    }
}

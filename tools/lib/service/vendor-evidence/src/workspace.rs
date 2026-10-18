//! The repository's packages and their path closures, as `cargo metadata`
//! resolves the one workspace.
//!
//! A package's class is its folder (the layers of `tools/README.md`); no
//! manifest carries metadata for it. The evidence policy reads two classes
//! from it: report packages ([`Package::is_report`]) and host stands
//! ([`Workspace::host_stands`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Result;

/// The report service: it renders findings and decides no verdict.
pub const REPORT_SERVICE: &str = "tools/lib/service/vendor-scenario-report";
/// The command-line layer, which only renders and dispatches.
pub const CLI_LAYER: &str = "tools/cli/";
/// The folder of the service packages; a host stand is one of them.
pub const SERVICE_LAYER: &str = "tools/lib/service/";
/// The suffix of a host stand's directory: `<chip>-<engine>-vendor-host`.
pub const HOST_STAND_SUFFIX: &str = "-vendor-host";

/// One path package of the workspace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Package {
    pub name: String,
    /// Its directory, relative to the repository root.
    pub directory: String,
    /// Its manifest, relative to the repository root.
    pub manifest: String,
}

impl Package {
    /// Whether the package only renders findings, so no shard may record
    /// one of its files: the command-line layer and the report service.
    pub fn is_report(&self) -> bool {
        self.directory.starts_with(CLI_LAYER) || self.directory == REPORT_SERVICE
    }
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<MetadataPackage>,
    resolve: Resolve,
}

#[derive(Deserialize)]
struct MetadataPackage {
    id: String,
    name: String,
    source: Option<String>,
    manifest_path: PathBuf,
}

#[derive(Deserialize)]
struct Resolve {
    nodes: Vec<Node>,
}

#[derive(Deserialize)]
struct Node {
    id: String,
    deps: Vec<NodeDep>,
}

#[derive(Deserialize)]
struct NodeDep {
    pkg: String,
    dep_kinds: Vec<DepKind>,
}

#[derive(Deserialize)]
struct DepKind {
    kind: Option<String>,
}

/// The workspace as one `cargo metadata` resolution read it.
pub struct Workspace {
    packages: BTreeMap<String, Package>,
    /// Package id to name, for path packages.
    names: BTreeMap<String, String>,
    /// Package id to the ids its normal and build dependencies resolve to.
    build_edges: BTreeMap<String, Vec<String>>,
}

impl Workspace {
    /// Resolve the workspace of the checkout `root`, every feature on, for
    /// the target `platform` when given.
    ///
    /// # Errors
    ///
    /// `cargo metadata` fails or prints no metadata of this shape.
    pub fn load(root: &Path, platform: Option<&str>) -> Result<Self> {
        let mut command = oer_tool_base_process::command(
            std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
        );
        command.current_dir(root).args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--all-features",
        ]);
        if let Some(platform) = platform {
            command.args(["--filter-platform", platform]);
        }
        let output = oer_tool_base_process::capture(&mut command)?;
        Self::from_metadata(root, &output.stdout)
    }

    fn from_metadata(root: &Path, json: &[u8]) -> Result<Self> {
        let metadata: Metadata = serde_json::from_slice(json)?;
        let root = root.canonicalize()?;
        let mut packages = BTreeMap::new();
        let mut names = BTreeMap::new();
        for package in metadata.packages.into_iter().filter(|p| p.source.is_none()) {
            let manifest = package
                .manifest_path
                .strip_prefix(&root)
                .map_err(|_| format!("{} lies outside the repository", package.name))?
                .to_owned();
            let directory = manifest.parent().ok_or("a manifest without a directory")?;
            names.insert(package.id, package.name.clone());
            packages.insert(
                package.name.clone(),
                Package {
                    name: package.name,
                    directory: directory.to_string_lossy().into_owned(),
                    manifest: manifest.to_string_lossy().into_owned(),
                },
            );
        }
        let build_edges = metadata
            .resolve
            .nodes
            .into_iter()
            .map(|node| {
                let deps = node
                    .deps
                    .into_iter()
                    .filter(|dep| {
                        dep.dep_kinds
                            .iter()
                            .any(|kind| kind.kind.as_deref() != Some("dev"))
                    })
                    .map(|dep| dep.pkg)
                    .collect();
                (node.id, deps)
            })
            .collect();
        Ok(Self {
            packages,
            names,
            build_edges,
        })
    }

    /// Every path package.
    pub fn packages(&self) -> impl Iterator<Item = &Package> {
        self.packages.values()
    }

    /// The path package named `name`.
    pub fn package(&self, name: &str) -> Option<&Package> {
        self.packages.get(name)
    }

    /// The path package whose manifest is `manifest` (relative to the root).
    pub fn owner(&self, manifest: &str) -> Option<&Package> {
        self.packages().find(|package| package.manifest == manifest)
    }

    /// The path packages a build of `package` compiles (normal and build
    /// dependencies), `package` included.
    pub fn closure(&self, package: &Package) -> Vec<&Package> {
        let Some(start) = self
            .names
            .iter()
            .find(|(_, name)| **name == package.name)
            .map(|(id, _)| id.as_str())
        else {
            return vec![];
        };
        let mut seen = BTreeSet::new();
        let mut pending = vec![start];
        let mut closure = vec![];
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(name) = self.names.get(id) else {
                continue;
            };
            if let Some(package) = self.packages.get(name) {
                closure.push(package);
            }
            pending.extend(
                self.build_edges
                    .get(id)
                    .into_iter()
                    .flatten()
                    .map(String::as_str),
            );
        }
        closure
    }

    /// The host stands of `chip`, by shard name, with their manifests: the
    /// services `tools/lib/service/<chip>-<engine>-vendor-host`, whose shard
    /// is `<engine>-host`.
    pub fn host_stands(&self, chip: &str) -> BTreeMap<String, String> {
        let prefix = format!("{SERVICE_LAYER}{chip}-");
        self.packages()
            .filter_map(|package| {
                let engine = package
                    .directory
                    .strip_prefix(&prefix)?
                    .strip_suffix(HOST_STAND_SUFFIX)?;
                (!engine.contains('/'))
                    .then(|| (format!("{engine}-host"), package.manifest.clone()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(directory: &str) -> Package {
        Package {
            name: String::new(),
            directory: directory.into(),
            manifest: format!("{directory}/Cargo.toml"),
        }
    }

    #[test]
    fn the_folder_decides_a_report_package() {
        assert!(package("tools/cli/verification").is_report());
        assert!(package(REPORT_SERVICE).is_report());
        assert!(!package("tools/lib/service/vendor-scenario-engine").is_report());
        assert!(!package("crates/esp32s31/hal").is_report());
    }

    #[test]
    fn this_checkout_resolves_its_stands_and_closures() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let workspace = Workspace::load(&root, None).unwrap();
        let stands = workspace.host_stands("esp32s31");
        assert_eq!(
            stands.get("ieee802154-host").map(String::as_str),
            Some("tools/lib/service/esp32s31-ieee802154-vendor-host/Cargo.toml")
        );
        let evidence = workspace
            .package("oer-tool-service-vendor-evidence")
            .unwrap();
        let closure: Vec<_> = workspace
            .closure(evidence)
            .into_iter()
            .map(|package| package.directory.as_str())
            .collect();
        assert!(closure.contains(&"tools/lib/service/vendor-evidence"));
        assert!(closure.contains(&"tools/lib/data/vendor-evidence-shard"));
        assert!(
            !closure
                .iter()
                .any(|directory| directory.starts_with(CLI_LAYER))
        );
    }
}

//! The repository model: what every tool that reasons about the tree as a
//! whole reads, read once and the same way.
//!
//! - [`files`]: the file inventory (`git ls-files` of a checkout, without
//!   build output and private inputs);
//! - [`index`]: the index view of a checkout (its commit, whether it is
//!   dirty, every index path, deleted files and symlinks included, and its
//!   untracked files, nothing skipped), which a source archive reproduces;
//! - [`manifest`]: every Cargo manifest as text, without Cargo: packages,
//!   targets, features, dependencies and workspace declarations;
//! - [`workspaces`]: the workspaces Cargo finds and the one each package
//!   belongs to;
//! - [`chips`]: every chip profile;
//! - [`classification`]: every key of `[package.metadata.open-radio]`, typed;
//! - [`policy`]: which package may depend on which, by layer, platform and
//!   role;
//! - [`closure`]: the path packages a package reaches, by dependency kind,
//!   target and features, and the packages that reach a set.
//!
//! [`Model`] ties them together; [`Model::owner`] names the package a
//! repository path belongs to across every workspace.

pub mod chips;
pub mod classification;
pub mod closure;
pub mod files;
pub mod index;
pub mod lock;
pub mod manifest;
pub mod policy;
pub mod workspaces;

#[cfg(test)]
mod testing;

use std::collections::BTreeMap;

pub use chips::Chips;
pub use classification::{
    Classification, Evidence, Hil, HostApp, HostBoundary, HostLayer, Layer, Platform, Scope,
};
pub use files::Repo;
pub use manifest::{Dependency, Kind, Manifests, Package, Workspace};

pub type Result<T> = std::result::Result<T, String>;

/// The repository's packages, workspaces, chips and classifications.
#[derive(Clone, Debug)]
pub struct Model {
    pub manifests: Manifests,
    pub chips: Chips,
    /// The workspace root manifest of every claimed package, by its manifest.
    owners: BTreeMap<String, String>,
    /// Root manifests of every workspace, ascending.
    workspaces: Vec<String>,
    /// Every package's classification, by its manifest.
    classes: BTreeMap<String, std::result::Result<Classification, String>>,
}

impl Model {
    /// The model of `repo`.
    pub fn load(repo: &Repo) -> Result<Self> {
        let mut manifests = Manifests::load(repo)?;
        let owners = workspaces::owners(&manifests);
        let workspaces = workspaces::discover(&manifests);
        let declared: BTreeMap<String, BTreeMap<String, Dependency>> = manifests
            .workspaces
            .iter()
            .map(|workspace| (workspace.manifest.clone(), workspace.dependencies.clone()))
            .collect();
        for package in &mut manifests.packages {
            let Some(inherited) = owners
                .get(&package.manifest)
                .and_then(|owner| declared.get(owner))
            else {
                continue;
            };
            for dependency in package.dependencies.iter_mut().filter(|d| d.inherited) {
                if let Some(declaration) = inherited.get(&dependency.key) {
                    manifest::inherit(dependency, declaration);
                }
            }
        }
        let classes = manifests
            .packages
            .iter()
            .map(|package| (package.manifest.clone(), classification::of(package)))
            .collect();
        Ok(Self {
            manifests,
            chips: Chips::load(repo)?,
            owners,
            workspaces,
            classes,
        })
    }

    /// Every package, in manifest order.
    pub fn packages(&self) -> &[Package] {
        &self.manifests.packages
    }

    /// The package whose directory is `directory`.
    pub fn package_at(&self, directory: &str) -> Option<&Package> {
        self.manifests.package_at(directory)
    }

    /// The one package named `name` in any workspace.
    pub fn package(&self, name: &str) -> Result<&Package> {
        let mut found = self.packages().iter().filter(|p| p.name == name);
        let package = found
            .next()
            .ok_or_else(|| format!("no package {name} in the repository"))?;
        if let Some(other) = found.next() {
            return Err(format!(
                "package name {name} is ambiguous: {} and {}",
                package.manifest, other.manifest
            ));
        }
        Ok(package)
    }

    /// The package a repository path belongs to: the innermost package
    /// directory holding it, in any workspace.
    pub fn owner(&self, path: &str) -> Option<&Package> {
        self.packages()
            .iter()
            .filter(|package| {
                package.directory.is_empty() || path.starts_with(&format!("{}/", package.directory))
            })
            .max_by_key(|package| package.directory.len())
    }

    /// The root manifest of the workspace `package` belongs to; `None` for
    /// a package no workspace claims.
    pub fn workspace_of(&self, package: &Package) -> Option<&str> {
        self.owners.get(&package.manifest).map(String::as_str)
    }

    /// Root manifests of every workspace, ascending.
    pub fn workspaces(&self) -> &[String] {
        &self.workspaces
    }

    /// The packages of the workspace whose root manifest is `workspace`.
    pub fn members<'a>(&'a self, workspace: &'a str) -> impl Iterator<Item = &'a Package> + 'a {
        self.packages()
            .iter()
            .filter(move |package| self.workspace_of(package) == Some(workspace))
    }

    /// The classification of `package`.
    pub fn classification(&self, package: &Package) -> Result<&Classification> {
        match self.classes.get(&package.manifest) {
            Some(Ok(class)) => Ok(class),
            Some(Err(error)) => Err(error.clone()),
            None => Err(format!(
                "{} is not a package of the model",
                package.manifest
            )),
        }
    }

    /// The package a path dependency names.
    pub fn target(&self, dependency: &Dependency) -> Option<&Package> {
        self.package_at(dependency.path.as_deref()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::tree;

    #[test]
    fn owners_cross_workspaces_and_inherited_paths_resolve() {
        let dir = tree(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/a\"]\nexclude = [\"firmware\"]\n",
            ),
            ("crates/a/Cargo.toml", "[package]\nname = \"oer-a\"\n"),
            ("crates/a/src/lib.rs", ""),
            (
                "firmware/Cargo.toml",
                "[workspace]\nmembers = [\"app\"]\n[workspace.dependencies]\noer-a = { path = \"../crates/a\", features = [\"x\"] }\n",
            ),
            (
                "firmware/app/Cargo.toml",
                "[package]\nname = \"oer-app\"\n[dependencies]\noer-a = { workspace = true, features = [\"y\"] }\n",
            ),
            ("firmware/app/src/main.rs", ""),
            (
                "firmware/app/nested/Cargo.toml",
                "[package]\nname = \"oer-nested\"\nworkspace = \"..\"\n",
            ),
        ]);
        let model = Model::load(&Repo::from_dir(dir.path()).unwrap()).unwrap();
        assert_eq!(model.owner("crates/a/src/lib.rs").unwrap().name, "oer-a");
        assert_eq!(
            model.owner("firmware/app/src/main.rs").unwrap().name,
            "oer-app"
        );
        assert_eq!(
            model.owner("firmware/app/nested/x.rs").unwrap().name,
            "oer-nested"
        );
        assert!(model.owner("docs/x.md").is_none());
        let app = model.package("oer-app").unwrap();
        assert_eq!(model.workspace_of(app), Some("firmware/Cargo.toml"));
        let a = &app.dependencies[0];
        assert_eq!(a.path.as_deref(), Some("crates/a"));
        assert_eq!(a.features, ["y", "x"]);
        assert_eq!(model.target(a).unwrap().name, "oer-a");
        assert_eq!(model.workspaces(), ["Cargo.toml", "firmware/Cargo.toml"]);
        assert_eq!(
            model
                .members("Cargo.toml")
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["oer-a"]
        );
        let error = model.classification(app).unwrap_err();
        assert!(
            error.contains("lacks [package.metadata.open-radio]"),
            "{error}"
        );
    }

    #[test]
    fn this_checkout_has_one_owner_for_every_path() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let model = Model::load(&Repo::load(&root).unwrap()).unwrap();
        let mut owners = vec![
            ("tools/repo/src/lib.rs".to_owned(), "oer-repo".to_owned()),
            (
                "tools/blobray/cli/src/main.rs".to_owned(),
                "blobray-cli".to_owned(),
            ),
        ];
        // Every chip's HIL agent, in its own workspace.
        for chip in model.chips.profiles() {
            owners.push((
                format!("hil/targets/{}/agent/src/main.rs", chip.id),
                chip.hil_agent_package(),
            ));
        }
        for (path, owner) in &owners {
            assert_eq!(
                model.owner(path).map(|p| p.name.as_str()),
                Some(owner.as_str()),
                "{path}"
            );
        }
        for package in model.packages() {
            assert!(
                model.workspace_of(package).is_some(),
                "{}",
                package.manifest
            );
            model.classification(package).unwrap();
        }
    }
}

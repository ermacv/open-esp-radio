//! The checkout of this repository a tool acts on.
//!
//! Every repository tool is built from a checkout: its packages are path
//! dependencies of each other, so this crate's own directory lies in the same
//! checkout as the binary that links it. [`Checkout::discover`] is that
//! checkout, refusing to act on another one the tool was started in.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

use crate::Result;

/// This package's directory, relative to the repository root: the marker of
/// a checkout of this repository.
const PACKAGE: &str = "tools/os/process";

/// A checkout of this repository.
#[derive(Clone, Debug)]
pub struct Checkout {
    /// The checkout's top directory, canonical.
    pub root: PathBuf,
}

impl Checkout {
    /// The checkout at `root`, which must hold the repository manifest.
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().canonicalize()?;
        if !root.join("Cargo.toml").is_file() {
            return Err(format!("repository manifest missing: {}", root.display()).into());
        }
        Ok(Self { root })
    }

    /// The checkout this binary was built from. A target directory shared by
    /// two checkouts can leave one checkout's binary in the other; running it
    /// from inside a different checkout is refused instead of silently acting
    /// on the tree it was built from. `tool` names the tool in that error.
    pub fn discover(tool: &str) -> Result<Self> {
        let built = built_root();
        if let Some(invoked) = std::env::current_dir()
            .ok()
            .and_then(|directory| checkout_of(&directory))
            && invoked != built
        {
            return Err(format!(
                "this {tool} was built from {} but runs in {}; rebuild it there or pass --root",
                built.display(),
                invoked.display()
            )
            .into());
        }
        Self::new(built)
    }

    /// `program` started in the checkout's top directory.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let mut command = crate::command(program);
        command.current_dir(&self.root);
        command
    }
}

/// The top directory of the checkout this binary was built from:
/// canonical while it exists, else as the compiler saw it.
pub fn built_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    root.canonicalize().unwrap_or(root)
}

/// The top directory of the checkout of this repository containing
/// `directory`: the nearest ancestor that holds this package.
pub fn checkout_of(directory: &Path) -> Option<PathBuf> {
    directory
        .ancestors()
        .find(|candidate| candidate.join(PACKAGE).join("Cargo.toml").is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_marker_is_this_package() {
        assert!(env!("CARGO_MANIFEST_DIR").ends_with(PACKAGE));
    }

    #[test]
    fn the_built_checkout_holds_the_repository_manifest() {
        let checkout = Checkout::new(built_root()).unwrap();
        assert!(checkout.root.join(PACKAGE).join("Cargo.toml").is_file());
        assert_eq!(
            checkout_of(&checkout.root.join("tools/os/process/src")),
            Some(checkout.root)
        );
    }

    #[test]
    fn a_directory_outside_any_checkout_has_none() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(checkout_of(directory.path()), None);
        assert!(Checkout::new(directory.path()).is_err());
    }

    #[test]
    fn a_nested_checkout_is_found_from_its_subdirectories() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("work");
        std::fs::create_dir_all(nested.join(PACKAGE)).unwrap();
        std::fs::write(nested.join(PACKAGE).join("Cargo.toml"), "").unwrap();
        std::fs::write(nested.join("Cargo.toml"), "").unwrap();
        std::fs::create_dir_all(nested.join("hil/host")).unwrap();
        let found = checkout_of(&nested.join("hil/host")).unwrap();
        assert_eq!(found, nested.canonicalize().unwrap());
        assert_eq!(Checkout::new(&found).unwrap().root, found);
    }
}

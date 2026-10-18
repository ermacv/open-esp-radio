//! Who owns a stand lease: any agent that uses the stand, by a free-form
//! name.
//!
//! Every lease is charged to an owner, so an agent must use one name every
//! time. Each checkout registers its owner once (`cargo stand owner set
//! NAME`) in `owners.json` of the stand's state directory; nothing is
//! derived from directory names. [`OWNER_ENV`] overrides it for one command.
#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use oer_process::lock::{FileLock, Mode};
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Names the owner of this command's requests, over the checkout's.
pub const OWNER_ENV: &str = "OER_STAND_OWNER";
/// Owner of the explicitly admitted child operation.
pub const OWNER_KEY: &str = "stand.owner";

/// An agent that uses the stand: a name of lower-case letters, digits,
/// `-`, `_` and `.`, starting with a letter or digit.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct Owner(String);

impl Owner {
    pub fn new(name: &str) -> std::result::Result<Self, NotAnOwner> {
        let valid = (1..=64).contains(&name.len())
            && name
                .bytes()
                .next()
                .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
            && name.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'-' | b'_' | b'.')
            });
        if valid {
            Ok(Self(name.to_owned()))
        } else {
            Err(NotAnOwner(name.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Owner {
    type Error = NotAnOwner;

    fn try_from(name: String) -> std::result::Result<Self, NotAnOwner> {
        Self::new(&name)
    }
}

impl From<Owner> for String {
    fn from(owner: Owner) -> Self {
        owner.0
    }
}

impl std::fmt::Display for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A name that is not an owner's.
#[derive(Debug)]
pub struct NotAnOwner(pub String);

impl std::fmt::Display for NotAnOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is not an owner name: lower-case letters, digits, `-`, `_` and `.`, at most \
             64, starting with a letter or digit",
            self.0
        )
    }
}

impl std::error::Error for NotAnOwner {}

/// No owner is registered for the checkout a lease was requested from.
#[derive(Debug)]
pub struct NoOwner(pub PathBuf);

impl std::fmt::Display for NoOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no stand owner is registered for {}; register it once with \
             `cargo stand owner set NAME`, or set {OWNER_ENV}",
            self.0.display()
        )
    }
}

impl std::error::Error for NoOwner {}

#[derive(Default, Deserialize, Serialize)]
struct Registry {
    schema: u32,
    /// Checkout root → its owner.
    checkouts: BTreeMap<PathBuf, Owner>,
}

/// The registry of every checkout's owner.
#[derive(Clone, Debug)]
pub struct Owners {
    path: PathBuf,
}

impl Owners {
    /// The registry in `path`: a test's own.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The stand's registry, `owners.json` in its state directory.
    pub fn open() -> Result<Self> {
        Ok(Self::at(
            oer_stand_file::paths::arbiter()?.join("owners.json"),
        ))
    }

    fn registry(&self) -> Result<Registry> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
            Err(error) => Err(error.into()),
        }
    }

    /// Register `owner` as the owner of the checkout at `checkout`.
    pub fn set(&self, checkout: &Path, owner: Owner) -> Result<()> {
        let checkout = checkout.canonicalize()?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let _lock = FileLock::acquire(&self.path.with_extension("lock"), Mode::Exclusive)?;
        let mut registry = self.registry()?;
        registry.schema = 1;
        registry.checkouts.insert(checkout, owner);
        oer_durable::atomic_write(&self.path, &serde_json::to_vec_pretty(&registry)?)
    }

    /// The registered checkouts that still exist.
    pub fn checkouts(&self) -> Result<Vec<PathBuf>> {
        Ok(self
            .registry()?
            .checkouts
            .into_keys()
            .filter(|checkout| checkout.is_dir())
            .collect())
    }

    /// The owner registered for the checkout containing `directory`, the
    /// innermost one when checkouts nest.
    pub fn of(&self, directory: &Path) -> Result<Option<Owner>> {
        let directory = directory.canonicalize()?;
        Ok(self
            .registry()?
            .checkouts
            .into_iter()
            .filter(|(checkout, _)| directory.starts_with(checkout))
            .max_by_key(|(checkout, _)| checkout.components().count())
            .map(|(_, owner)| owner))
    }
}

/// The owner of a command run for the checkout at `root`: `explicit` (an
/// `--owner`), else [`OWNER_ENV`] (an enclosing lease's), else the owner
/// registered for the checkout, else that of the main checkout of a
/// worktree that registered none. Nothing is derived from directory names.
pub fn resolve(explicit: Option<&str>, root: &Path) -> Result<Owner> {
    let context = oer_process::Context::current()?;
    if let Some(owner) = explicit
        .map(str::to_owned)
        .or_else(|| context.get(OWNER_KEY).map(str::to_owned))
        .or_else(|| {
            std::env::var(OWNER_ENV)
                .ok()
                .filter(|owner| !owner.trim().is_empty())
        })
    {
        return Ok(Owner::new(owner.trim())?);
    }
    let owners = Owners::open()?;
    if let Some(owner) = owners.of(root)? {
        return Ok(owner);
    }
    // A worktree added from a registered checkout acts for that checkout's
    // owner until it registers one of its own.
    if let Some(main) = main_checkout(root)
        && main != root
        && let Some(owner) = owners.of(&main)?
    {
        return Ok(owner);
    }
    Err(NoOwner(root.to_owned()).into())
}

/// The main checkout of the worktree at `root`: the directory holding the
/// repository's common `.git`.
fn main_checkout(root: &Path) -> Option<PathBuf> {
    let common = oer_process::git::text(
        root,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    checkout_of_common_dir(Path::new(&common))
}

/// The checkout whose `.git` directory is `common`.
fn checkout_of_common_dir(common: &Path) -> Option<PathBuf> {
    (common.file_name()? == ".git").then(|| common.parent().map(Path::to_owned))?
}

/// The owner registered for the checkout this process runs in.
pub fn of_current_checkout() -> Result<Owner> {
    let directory = std::env::current_dir()?;
    Ok(Owners::open()?.of(&directory)?.ok_or(NoOwner(directory))?)
}

/// The owner [`OWNER_ENV`] names, else the checkout's registered one.
pub fn from_environment() -> Result<Owner> {
    if let Some(owner) = oer_process::Context::current()?.get(OWNER_KEY) {
        return Ok(Owner::new(owner)?);
    }
    match std::env::var(OWNER_ENV)
        .ok()
        .filter(|owner| !owner.trim().is_empty())
    {
        Some(owner) => Ok(Owner::new(owner.trim())?),
        None => of_current_checkout(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_owner_is_any_well_formed_name() {
        for name in ["wifi", "802154", "chip-b", "my-new-agent", "a.b_c"] {
            assert_eq!(Owner::new(name).unwrap().as_str(), name);
        }
        for name in ["", "Wifi", "-x", "a b", "a/b"] {
            assert!(Owner::new(name).is_err(), "{name}");
        }
        assert!(serde_json::from_str::<Owner>("\"Bad Name\"").is_err());
    }

    #[test]
    fn a_checkout_names_its_owner_for_every_directory_inside_it() {
        let directory = tempfile::tempdir().unwrap();
        let owners = Owners::at(directory.path().join("arbiter/owners.json"));
        let checkout = directory.path().join("checkout");
        fs::create_dir_all(checkout.join("hil/host")).unwrap();
        assert_eq!(owners.of(&checkout).unwrap(), None);
        owners.set(&checkout, Owner::new("wifi").unwrap()).unwrap();
        assert_eq!(
            owners.of(&checkout.join("hil/host")).unwrap(),
            Some(Owner::new("wifi").unwrap())
        );
        assert_eq!(owners.of(directory.path()).unwrap(), None);
    }
}

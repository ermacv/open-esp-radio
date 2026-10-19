//! Where the stand keeps its state, after the XDG base directories
//! (`oer-os-durable`'s `xdg`): every checkout of the user shares them.
//!
//! - the stand file: [`crate::path`];
//! - the arbiter's state (queue, holders, history, board journal, jobs):
//!   `open-esp-radio/arbiter` in `$XDG_CACHE_HOME` (else `~/.cache`), or
//!   `$OER_STAND_ARBITER_DIR`, which tests and a nested private arbiter use;
//! - the lock files, the arbiter's final exclusion layer:
//!   `open-esp-radio/leases` in the XDG cache directory. They do not move
//!   with `$OER_STAND_ARBITER_DIR`, so every process on the host excludes
//!   every other one from a board, whichever arbiter ordered it.

use std::{ffi::OsString, path::PathBuf};

use oer_os_durable::xdg::{Base, overridable_in, path_in};

pub use crate::STAND_FILE_ENV;

/// Overrides the arbiter's state directory, for tests and a nested arbiter.
pub const ARBITER_ENV: &str = "OER_STAND_ARBITER_DIR";

fn environment(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// The user's stand file.
pub fn stand_file() -> crate::Result<PathBuf> {
    crate::path()
}

/// The arbiter's state directory.
pub fn arbiter() -> crate::Result<PathBuf> {
    arbiter_in(environment)
}

/// The directory of the lock files.
pub fn locks() -> crate::Result<PathBuf> {
    locks_in(environment)
}

fn arbiter_in(variable: impl Fn(&str) -> Option<OsString>) -> crate::Result<PathBuf> {
    Ok(overridable_in(
        ARBITER_ENV,
        Base::Cache,
        "arbiter",
        variable,
    )?)
}

fn locks_in(variable: impl Fn(&str) -> Option<OsString>) -> crate::Result<PathBuf> {
    Ok(path_in(Base::Cache, "leases", variable)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn environment(name: &str) -> Option<OsString> {
        match name {
            "XDG_CACHE_HOME" => Some("/cache".into()),
            "HOME" => Some("/home/u".into()),
            _ => None,
        }
    }

    #[test]
    fn the_stand_state_lies_below_the_xdg_cache() {
        assert_eq!(
            arbiter_in(environment).unwrap(),
            Path::new("/cache/open-esp-radio/arbiter")
        );
        assert_eq!(
            locks_in(environment).unwrap(),
            Path::new("/cache/open-esp-radio/leases")
        );
    }

    #[test]
    fn a_private_arbiter_keeps_the_locks() {
        let private = |name: &str| {
            (name == ARBITER_ENV)
                .then(|| OsString::from("/tmp/private"))
                .or_else(|| environment(name))
        };
        assert_eq!(arbiter_in(private).unwrap(), Path::new("/tmp/private"));
        assert_eq!(locks_in(private).unwrap(), locks_in(environment).unwrap());
    }
}

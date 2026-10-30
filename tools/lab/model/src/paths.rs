//! Where the stand keeps its state, after the XDG base directories
//! (`oer-os-durable`'s `xdg`): every checkout of the user shares them.
//!
//! - the stand file: [`crate::path`];
//! - the arbiter's state (queue, holders, history, board journal):
//!   `open-esp-radio/arbiter` in `$XDG_CACHE_HOME` (else `~/.cache`), or
//!   `$OER_STAND_ARBITER_DIR`, which tests and a nested private arbiter use;
//! - the LAB service's socket: `open-esp-radio/lab.sock` in
//!   `$XDG_RUNTIME_DIR` (else the cache base), or `$OER_LAB_SOCKET`.

use std::{ffi::OsString, path::PathBuf};

use oer_os_durable::xdg::{Base, overridable_in};

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

/// The LAB service's state directory: its restrictions on the stand, its
/// balances and records, in `$XDG_STATE_HOME`.
pub fn arbiter() -> crate::Result<PathBuf> {
    arbiter_in(environment)
}

/// Overrides the LAB service's socket, for a private service.
pub const SOCKET_ENV: &str = "OER_LAB_SOCKET";

/// The LAB service's socket.
pub fn socket() -> crate::Result<PathBuf> {
    socket_in(environment)
}

fn socket_in(variable: impl Fn(&str) -> Option<OsString>) -> crate::Result<PathBuf> {
    overridable_in(SOCKET_ENV, Base::Runtime, "lab.sock", variable)
}

fn arbiter_in(variable: impl Fn(&str) -> Option<OsString>) -> crate::Result<PathBuf> {
    Ok(overridable_in(
        ARBITER_ENV,
        Base::State,
        "arbiter",
        variable,
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn environment(name: &str) -> Option<OsString> {
        match name {
            "XDG_CACHE_HOME" => Some("/cache".into()),
            "XDG_STATE_HOME" => Some("/state".into()),
            "HOME" => Some("/home/u".into()),
            _ => None,
        }
    }

    #[test]
    fn the_stand_state_lies_below_the_xdg_state_home() {
        assert_eq!(
            arbiter_in(environment).unwrap(),
            Path::new("/state/open-esp-radio/arbiter")
        );
    }

    #[test]
    fn the_service_socket_lies_in_the_runtime_directory_unless_moved() {
        let runtime = |name: &str| {
            (name == "XDG_RUNTIME_DIR")
                .then(|| OsString::from("/run/user/1"))
                .or_else(|| environment(name))
        };
        assert_eq!(
            socket_in(runtime).unwrap(),
            Path::new("/run/user/1/open-esp-radio/lab.sock")
        );
        let private = |name: &str| {
            (name == SOCKET_ENV)
                .then(|| OsString::from("/tmp/lab.sock"))
                .or_else(|| runtime(name))
        };
        assert_eq!(socket_in(private).unwrap(), Path::new("/tmp/lab.sock"));
    }
}

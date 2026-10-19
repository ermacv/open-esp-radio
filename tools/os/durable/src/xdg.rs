//! The user's state directories of this repository's tools, after the XDG
//! base directory specification: every cache, data store, lock directory
//! and configuration the tools keep outside a checkout lies below
//! `open-esp-radio/` of one of them.
//!
//! A variable that is unset or empty counts as unset, as the specification
//! requires: `$XDG_CACHE_HOME`, else `$HOME/.cache`; `$XDG_DATA_HOME`, else
//! `$HOME/.local/share`; `$XDG_CONFIG_HOME`, else `$HOME/.config`. A tool's
//! own override variable (such as `OER_STAND_ARBITER_DIR`) names the directory
//! itself and is read through [`overridable`].

use std::{ffi::OsString, path::PathBuf};

use crate::Result;

/// The directory below each base that holds this repository's state.
const APPLICATION: &str = "open-esp-radio";

/// An XDG base directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Base {
    /// Rebuildable files: build caches, fetched artifacts, locks.
    Cache,
    /// Files a user keeps: run stores, installed tools.
    Data,
    /// The user's configuration files.
    Config,
    /// Per-session runtime files: device locks. `$XDG_RUNTIME_DIR`, else
    /// the cache base.
    Runtime,
}

impl Base {
    fn variable(self) -> &'static str {
        match self {
            Self::Cache => "XDG_CACHE_HOME",
            Self::Data => "XDG_DATA_HOME",
            Self::Config => "XDG_CONFIG_HOME",
            Self::Runtime => "XDG_RUNTIME_DIR",
        }
    }

    fn below_home(self) -> &'static str {
        match self {
            Self::Cache => ".cache",
            Self::Data => ".local/share",
            Self::Config => ".config",
            Self::Runtime => ".cache",
        }
    }
}

/// Overrides the host-wide ESP-IDF cache directory.
pub const ESP_IDF_CACHE_ENV: &str = "OER_IDF_CACHE";

/// The host-wide cache of ESP-IDF trees and tools (OpenOCD included).
pub fn esp_idf_cache() -> Result<PathBuf> {
    overridable(ESP_IDF_CACHE_ENV, Base::Cache, "esp-idf")
}

/// `<base>/open-esp-radio/<relative>` from the process environment.
pub fn path(base: Base, relative: &str) -> Result<PathBuf> {
    path_in(base, relative, |name| std::env::var_os(name))
}

/// `$override_variable` when it is set and non-empty, else [`path`].
pub fn overridable(override_variable: &str, base: Base, relative: &str) -> Result<PathBuf> {
    overridable_in(override_variable, base, relative, |name| {
        std::env::var_os(name)
    })
}

/// [`path`] with `variable` reading the environment.
pub fn path_in(
    base: Base,
    relative: &str,
    variable: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf> {
    let set = |name: &str| variable(name).filter(|value| !value.is_empty());
    let base_directory = match set(base.variable()) {
        Some(directory) => PathBuf::from(directory),
        None if base == Base::Runtime => return path_in(Base::Cache, relative, variable),
        None => PathBuf::from(set("HOME").ok_or_else(|| {
            format!(
                "neither {} nor HOME is set to locate {APPLICATION}/{relative}",
                base.variable()
            )
        })?)
        .join(base.below_home()),
    };
    Ok(base_directory.join(APPLICATION).join(relative))
}

/// [`overridable`] with `variable` reading the environment.
pub fn overridable_in(
    override_variable: &str,
    base: Base,
    relative: &str,
    variable: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf> {
    match variable(override_variable).filter(|value| !value.is_empty()) {
        Some(directory) => Ok(PathBuf::from(directory)),
        None => path_in(base, relative, variable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into())
        }
    }

    #[test]
    fn a_set_base_variable_wins_over_home() {
        let env = environment(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "/cache")]);
        assert_eq!(
            path_in(Base::Cache, "arbiter", &env).unwrap(),
            PathBuf::from("/cache/open-esp-radio/arbiter")
        );
    }

    #[test]
    fn an_empty_base_variable_counts_as_unset() {
        let env = environment(&[
            ("HOME", "/home/u"),
            ("XDG_CACHE_HOME", ""),
            ("XDG_DATA_HOME", ""),
            ("XDG_CONFIG_HOME", ""),
        ]);
        assert_eq!(
            path_in(Base::Cache, "build", &env).unwrap(),
            PathBuf::from("/home/u/.cache/open-esp-radio/build")
        );
        assert_eq!(
            path_in(Base::Data, "hil", &env).unwrap(),
            PathBuf::from("/home/u/.local/share/open-esp-radio/hil")
        );
        assert_eq!(
            path_in(Base::Config, "stand.toml", &env).unwrap(),
            PathBuf::from("/home/u/.config/open-esp-radio/stand.toml")
        );
    }

    #[test]
    fn without_home_or_the_variable_there_is_no_directory() {
        assert!(path_in(Base::Data, "hil", environment(&[("HOME", "")])).is_err());
    }

    #[test]
    fn a_non_empty_override_names_the_directory_itself() {
        let env = environment(&[("HOME", "/home/u"), ("OER_X", "/elsewhere")]);
        assert_eq!(
            overridable_in("OER_X", Base::Cache, "x", &env).unwrap(),
            PathBuf::from("/elsewhere")
        );
        let env = environment(&[("HOME", "/home/u"), ("OER_X", "")]);
        assert_eq!(
            overridable_in("OER_X", Base::Cache, "x", &env).unwrap(),
            PathBuf::from("/home/u/.cache/open-esp-radio/x")
        );
    }
}

//! The supported chips: every `platform/<id>/chip.toml`, read through
//! `oer-chip-profile`, the one parser of the profile.

use oer_chip_profile::Profile;

/// The chip profile model, for the readers of [`Chips`].
pub use oer_chip_profile as profile;

use crate::{Result, files::Repo};

/// Every supported chip's profile, sorted by id.
#[derive(Clone, Debug, Default)]
pub struct Chips(Vec<Profile>);

impl Chips {
    pub fn load(repo: &Repo) -> Result<Self> {
        if !repo.exists("platform") {
            return Ok(Self::default());
        }
        Profile::all(repo.root())
            .map(Self)
            .map_err(|error| format!("chip profiles: {error}"))
    }

    /// Every chip profile below the checkout `root`, without reading the
    /// rest of the repository.
    pub fn at(root: &std::path::Path) -> Result<Self> {
        if !root.join("platform").is_dir() {
            return Ok(Self::default());
        }
        Profile::all(root)
            .map(Self)
            .map_err(|error| format!("chip profiles: {error}"))
    }

    pub fn profiles(&self) -> &[Profile] {
        &self.0
    }

    /// Every chip id, ascending.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|profile| profile.id.as_str())
    }

    /// Every family some chip belongs to.
    pub fn families(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|profile| profile.family.as_str())
    }

    /// The family of the chip `id`.
    pub fn family_of(&self, id: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|profile| profile.id == id)
            .map(|profile| profile.family.as_str())
    }

    /// The profile of the chip `id`.
    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.0.iter().find(|profile| profile.id == id)
    }
}

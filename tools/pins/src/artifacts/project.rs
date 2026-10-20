//! The chips with a vendor verification project and its fixed layout.
//!
//! The supported chips are those with a profile
//! ([`oer_chip_profile::Profile`]); a chip has a vendor verification project
//! when its pins `chips/<chip>/vendor/artifacts.toml` also exist. Its verification
//! files live at fixed paths below that directory, so no task keeps its own
//! list of chips.
use crate::artifacts::Result;
use std::path::Path;

/// Directory of every chip, relative to the root.
const CHIPS: &str = "chips";
/// Directory of the vendor packages, relative to the root.
const VENDOR: &str = "tools/vendor";

/// Every supported chip, sorted: the chips with a profile.
pub fn supported(root: &Path) -> Result<Vec<String>> {
    oer_chip_profile::supported(root)
}

/// Every supported chip with a vendor verification project, sorted: those
/// whose pins exist.
pub fn projects(root: &Path) -> Result<Vec<String>> {
    Ok(supported(root)?
        .into_iter()
        .filter(|chip| Project::new(root, chip).is_ok())
        .collect())
}

/// The package and binary that run a chip's typed vendor scenarios.
#[derive(Debug, Eq, PartialEq)]
pub struct Scenarios {
    /// Entry package in `tools/vendor/scenarios-<chip>-cli`.
    pub entry: String,
    /// Its explicitly named binary.
    pub binary: String,
}

/// The vendor verification project of one supported chip.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Project(String);

impl Project {
    /// `name`, when it is supported and has a verification project;
    /// otherwise an error that lists the supported chips or names the
    /// missing project.
    pub fn new(root: &Path, name: &str) -> Result<Self> {
        crate::manifest_path(root, name)?;
        Ok(Self(oer_chip_profile::Profile::load(root, name)?.id))
    }

    pub fn name(&self) -> &str {
        &self.0
    }

    /// The single pin of the chip's vendor artifacts.
    pub fn artifacts(&self) -> String {
        crate::manifest(&self.0)
    }

    /// The chip's ROM as its profile's `[rom]` names it: the pinned ROM
    /// ELF's artifact id and the reviewed summaries of the ROM functions its
    /// images call, relative to the root; `None` for a chip without one.
    pub fn rom(&self, root: &Path) -> Result<Option<oer_chip_profile::Rom>> {
        Ok(oer_chip_profile::Profile::load(root, &self.0)?.rom)
    }

    /// Directory of the chip's typed vendor scenario library.
    pub fn scenarios_directory(&self) -> String {
        format!("{VENDOR}/scenarios-{}", self.0)
    }

    /// Directory of the scenario library's decision modules.
    pub fn scenario_decisions(&self) -> String {
        format!("{}/src/decisions", self.scenarios_directory())
    }

    /// The chip's registry of reviewed vendor-function fingerprints.
    pub fn provenance_registry(&self) -> String {
        format!("{CHIPS}/{}/vendor/facts/provenance.toml", self.0)
    }

    /// The package of the chip's typed vendor scenarios.
    pub fn scenarios(&self) -> Scenarios {
        Scenarios {
            entry: format!("oer-tool-vendor-scenarios-{}-cli", self.0),
            binary: format!("oer-tool-vendor-scenarios-{}", self.0),
        }
    }
}

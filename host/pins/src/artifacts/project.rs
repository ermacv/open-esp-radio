//! The chips with a vendor verification project and its fixed layout.
//!
//! The supported chips are those with a profile
//! ([`oer_chip_profile::Profile`]); a chip has a vendor verification project
//! when its pins `knowledge/<chip>/vendor/artifacts.toml` also exist. Its verification
//! files live at fixed paths below that directory, so no task keeps its own
//! list of chips.
use crate::artifacts::Result;
use std::path::Path;

/// Directory of every chip's knowledge, relative to the root.
const KNOWLEDGE: &str = "knowledge";
/// Directory of the vendor packages, relative to the root.
const VENDOR: &str = "host/vendor";

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

/// Directory of `chip`'s vendor scenario evidence shards, relative to the
/// root: derived data, which `cargo vendor-check evidence` writes for the
/// checkout from its sources and pins, and nothing tracks. Its one location
/// for every reader.
pub fn evidence_shards(chip: &str) -> String {
    format!("target/verification/{chip}/evidence")
}

/// A chip's typed vendor scenarios: the verdict library that decides their
/// verdicts, and the entry package whose binary, of the same name, runs
/// them through the shared scenario command line (a report package the
/// verdict library never depends on).
#[derive(Debug, Eq, PartialEq)]
pub struct Scenarios {
    /// `host/vendor/scenarios-<chip>`.
    pub library: String,
    /// `tools/vendor-scenarios-<chip>`.
    pub entry: String,
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

    /// Directory of the chip's vendor scenario evidence shards
    /// ([`evidence_shards`]).
    pub fn evidence_shards(&self) -> String {
        evidence_shards(&self.0)
    }

    /// The chip's ROM as its profile's `[rom]` names it: the pinned ROM
    /// ELF's artifact id and the reviewed summaries of the ROM functions its
    /// images call, relative to the root; `None` for a chip without one.
    pub fn rom(&self, root: &Path) -> Result<Option<oer_chip_profile::Rom>> {
        Ok(oer_chip_profile::Profile::load(root, &self.0)?.rom)
    }

    /// The chip's reviewed acceptances of vendor roots its index retired or
    /// whose comparisons it narrowed, which a comparison with an earlier
    /// index accepts as intended rather than lost evidence.
    pub fn accepted_losses(&self) -> String {
        format!("{KNOWLEDGE}/{}/vendor/accepted-losses.toml", self.0)
    }

    /// The chip's reviewed coverage decisions.
    pub fn coverage(&self) -> String {
        format!("{KNOWLEDGE}/{}/vendor/coverage.toml", self.0)
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
        format!("{KNOWLEDGE}/{}/vendor/facts/provenance.toml", self.0)
    }

    /// The package of the chip's typed vendor scenarios.
    pub fn scenarios(&self) -> Scenarios {
        Scenarios {
            library: format!("oer-vendor-scenarios-{}", self.0),
            entry: format!("oer-tool-vendor-scenarios-{}", self.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn tracked_chips_are_supported_and_others_are_rejected_with_the_list() {
        let supported = supported(&root()).unwrap();
        assert_eq!(supported, oer_chip_profile::supported(&root()).unwrap());
        let error = Project::new(&root(), "chip-x").unwrap_err().to_string();
        assert!(error.contains("unsupported chip `chip-x`"), "{error}");
        assert!(error.contains(&supported.join(", ")), "{error}");
    }

    #[test]
    fn the_projects_are_the_chips_with_pins() {
        let projects = projects(&root()).unwrap();
        assert!(projects.contains(&"esp32s31".to_owned()), "{projects:?}");
        for chip in &projects {
            assert!(root().join(crate::manifest(chip)).is_file());
        }
    }

    #[test]
    fn paths_follow_the_chip_directory() {
        for name in supported(&root()).unwrap() {
            let Ok(chip) = Project::new(&root(), &name) else {
                continue;
            };
            // Every path of a project lies in this checkout.
            for path in [
                chip.artifacts(),
                chip.accepted_losses(),
                chip.coverage(),
                format!("{}/Cargo.toml", chip.scenarios_directory()),
                chip.scenario_decisions(),
            ] {
                assert!(root().join(&path).exists(), "{path}");
            }
            assert_eq!(
                chip.artifacts(),
                format!("knowledge/{name}/vendor/artifacts.toml")
            );
            assert_eq!(
                chip.evidence_shards(),
                format!("target/verification/{name}/evidence")
            );
            assert_eq!(
                chip.accepted_losses(),
                format!("knowledge/{name}/vendor/accepted-losses.toml")
            );
            assert_eq!(
                chip.scenarios_directory(),
                format!("host/vendor/scenarios-{name}")
            );
            assert_eq!(
                chip.scenarios(),
                Scenarios {
                    library: format!("oer-vendor-scenarios-{name}"),
                    entry: format!("oer-tool-vendor-scenarios-{name}"),
                }
            );
        }
    }
}

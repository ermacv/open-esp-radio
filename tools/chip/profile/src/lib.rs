//! The chips this repository supports, from their tracked layered facts
//! (`oer-espressif-facts`), and the stack policy of their image families
//! ([`policy`]).
//!
//! Every supported chip has `chips/<id>/platform/chip.toml`, its chip layer:
//! it extends the ISA of its harts and its family, and its module and board
//! extend it. A [`Profile`] is the resolved facts of the chip's board and the
//! image registry its chip layer holds (`[hil]`, `[packages]`, `[[probe]]`).
//! Everything else follows the id by convention
//! (`firmware/probes/<id>-<name>`, `chips/<id>/registers`), and a capability exists
//! where its directory does.
//! Host tools resolve a chip through [`Profile::load`] instead of matching
//! chip names, so an unknown chip is refused with the supported ones.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use oer_espressif_facts::{Facts, Source};
use serde::{Deserialize, Serialize};

/// The root of the chip packages, whose `<id>/platform` directories hold
/// each chip's platform data.
const CHIPS: &str = "chips";

/// The platform data directory of chip `id` (`chips/<id>/platform`),
/// relative to the root: its layers, stack policy, partitions and ROM
/// function summaries.
pub fn platform_dir(id: &str) -> PathBuf {
    PathBuf::from(oer_espressif_facts::platform_dir(id))
}

pub mod policy;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// One supported chip: its resolved facts and its image registry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Profile {
    /// The facts of the chip, its module and its board.
    pub facts: Facts,
    /// What the chip's HIL agent may not link and the HIL image recipes;
    /// `None` when the chip names neither.
    pub hil: Option<Hil>,
    /// The chip's packages as the repository policies classify them.
    pub packages: Packages,
    /// The Rust probe images of the chip's vendor comparison, in build
    /// order (`firmware/probes/<id>-*`).
    pub probe: Vec<Probe>,
}

/// The image registry of a chip layer: the keys
/// [`oer_espressif_facts::REGISTRY`] names.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registry {
    #[serde(default)]
    hil: Option<Hil>,
    #[serde(default)]
    packages: Packages,
    #[serde(default)]
    probe: Vec<Probe>,
}

/// The `[hil]` table: policy of the chip's HIL agent graph.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Hil {
    /// Packages neither the root lock nor the agent's workspace may pull in.
    #[serde(default)]
    pub forbidden_packages: Vec<String>,
    /// The vendor firmware projects the PHY calibration cross-check builds,
    /// relative to the repository root; `None` for a chip without one.
    #[serde(default)]
    pub vendor_calibration: Option<PathBuf>,
    /// Explicit image-class recipes, relative to the repository root.
    #[serde(default)]
    pub images: std::collections::BTreeMap<String, HilImage>,
}

/// One HIL image class's target package and policies.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct HilImage {
    pub package: String,
    pub manifest: PathBuf,
    pub stack_policy: PathBuf,
}

/// The `[packages]` table: the chip's packages by the role the repository
/// policies give them. Every list names root-workspace packages.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Packages {
    /// Generated register bindings: they state no crate-root unsafe policy
    /// and keep their own lint policy.
    #[serde(default)]
    pub generated: Vec<String>,
    /// The chip's closed radio PAC.
    #[serde(default)]
    pub closed_pac: Vec<String>,
    /// The only production consumers of the closed PACs.
    #[serde(default)]
    pub pac_consumers: Vec<String>,
    /// The chip's hardware backends the portable facade profiles must not
    /// reach.
    #[serde(default)]
    pub backends: Vec<String>,
    /// The chip's Wi-Fi packages.
    #[serde(default)]
    pub wifi: Vec<String>,
    /// The chip's Bluetooth packages.
    #[serde(default)]
    pub bluetooth: Vec<String>,
}

/// One Rust probe image of the vendor comparison: the artifact role it
/// serves and its package in `firmware/probes/<id>-*`. Every probe
/// compiles in the host's one firmware compile cache.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Probe {
    pub role: String,
    pub package: String,
}

/// The repository's files, every read recorded in the caller's inputs.
struct Tree<'a> {
    root: &'a Path,
    inputs: &'a mut oer_os_durable::inputs::Inputs,
}

impl Source for Tree<'_> {
    fn read(&mut self, path: &str) -> oer_espressif_facts::Result<String> {
        let path = self.root.join(path);
        self.inputs
            .read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()).into())
    }

    fn files(&mut self, directory: &str) -> oer_espressif_facts::Result<Vec<String>> {
        let directory = self.root.join(directory);
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!("cannot list {}: {error}", directory.display()).into());
            }
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
            if entry.path().is_file() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(names)
    }
}

impl Profile {
    /// The profile of `id`, or an error naming the supported chips.
    pub fn load(root: &Path, id: &str) -> Result<Self> {
        Self::load_with_inputs(root, id, &mut oer_os_durable::inputs::Inputs::default())
    }

    /// Load the profile and record every layer it reads in the caller's
    /// computation.
    pub fn load_with_inputs(
        root: &Path,
        id: &str,
        inputs: &mut oer_os_durable::inputs::Inputs,
    ) -> Result<Self> {
        if !root.join(oer_espressif_facts::chip_file(id)).is_file() {
            return Err(format!(
                "unsupported chip `{id}`; supported: {}",
                supported(root)?.join(", ")
            )
            .into());
        }
        Self::from_source(&mut Tree { root, inputs }, id)
    }

    /// The profile of `id` from the layer files this build compiled in
    /// ([`oer_espressif_facts::tracked`]).
    pub fn tracked(id: &str) -> Result<Self> {
        Self::from_source(&mut oer_espressif_facts::tracked::Tracked, id)
    }

    fn from_source(source: &mut dyn Source, id: &str) -> Result<Self> {
        let layers = oer_espressif_facts::load(source, id)?;
        let top = layers.top().to_owned();
        let mut table = layers.table;
        let mut registry = toml::Table::new();
        for key in oer_espressif_facts::REGISTRY {
            if let Some(value) = table.remove(*key) {
                registry.insert((*key).to_owned(), value);
            }
        }
        let registry: Registry = registry
            .try_into()
            .map_err(|error| format!("{top}: {error}"))?;
        Ok(Self {
            facts: Facts::from_table(id, &top, table)?,
            hil: registry.hil,
            packages: registry.packages,
            probe: registry.probe,
        })
    }

    /// Every supported chip, sorted by id.
    pub fn all(root: &Path) -> Result<Vec<Self>> {
        supported(root)?
            .iter()
            .map(|id| Self::load(root, id))
            .collect()
    }

    /// `<root>/chips/<id>/<directory>`, such as `chips/<id>/vendor`.
    pub fn directory(&self, root: &Path, directory: &str) -> PathBuf {
        root.join(CHIPS).join(&self.facts.id).join(directory)
    }

    /// The bootstrap package the staged boot builds into every image beside
    /// its runtime.
    pub fn bootstrap_package(&self) -> String {
        format!("oer-firmware-bootstrap-{}", self.facts.id)
    }

    /// The explicit recipe of `class`; an unsupported class has no recipe.
    pub fn hil_image(&self, class: &str) -> Option<&HilImage> {
        self.hil.as_ref()?.images.get(class)
    }

    /// The chip's own stack policy (`chips/<id>/platform/stack.toml`), relative
    /// to the root: the move limit and reserves of every image of the chip,
    /// which the HIL policy extends.
    pub fn stack_policy(&self) -> PathBuf {
        platform_dir(&self.facts.id).join("stack.toml")
    }

    /// The input sections the chip's staged boot zeroes, comma separated,
    /// as the image linker reads them; empty for a chip without a staged
    /// boot.
    pub fn zeroed_inputs(&self) -> String {
        self.facts
            .staged
            .as_ref()
            .map(|staged| staged.zeroed_inputs.join(","))
            .unwrap_or_default()
    }
}

/// The Rust target of `id`'s firmware, from its facts.
pub fn rust_target(root: &Path, id: &str) -> Result<String> {
    Ok(Profile::load(root, id)?.facts.rust_target)
}

/// The family of every supported chip, keyed by chip id.
pub fn families(root: &Path) -> Result<BTreeMap<String, String>> {
    Ok(Profile::all(root)?
        .into_iter()
        .map(|profile| (profile.facts.id, profile.facts.family))
        .collect())
}

/// Ids of the chips with a chip layer, sorted.
pub fn supported(root: &Path) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(root.join(CHIPS))? {
        let entry = entry?;
        let id = entry.file_name().to_string_lossy().into_owned();
        if root.join(oer_espressif_facts::chip_file(&id)).is_file() {
            ids.push(id);
        }
    }
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repository of the tracked layer files and partition tables.
    fn repository() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for (path, text) in oer_espressif_facts::tracked::FILES.iter().copied().chain([
            (
                "chips/esp32c5/platform/partitions/applications.csv",
                include_str!("../../../../chips/esp32c5/platform/partitions/applications.csv"),
            ),
            (
                "chips/esp32s31/platform/partitions/applications.csv",
                include_str!("../../../../chips/esp32s31/platform/partitions/applications.csv"),
            ),
        ]) {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        directory
    }

    #[test]
    fn the_tracked_profiles_load() {
        let repository = repository();
        let root = repository.path();
        let chips = Profile::all(root).unwrap();
        assert!(!chips.is_empty());
        assert_eq!(families(root).unwrap().len(), chips.len());
        for chip in &chips {
            let facts = &chip.facts;
            assert_eq!(*chip, Profile::tracked(&facts.id).unwrap(), "{}", facts.id);
            assert!(
                oer_espressif_facts::valid_identifier(&facts.family),
                "{}",
                facts.id
            );
            if let Some(flash) = &facts.flash {
                assert!(flash.bootloader < flash.partition_table, "{}", facts.id);
                assert!(flash.partition_table < flash.application, "{}", facts.id);
                // The partition table the platform defines exists.
                assert!(root.join(&flash.partitions).is_file(), "{}", facts.id);
            }
            assert_eq!(
                chip.directory(root, "registers"),
                root.join("chips").join(&facts.id).join("registers")
            );
            if let Some(memory) = &facts.memory {
                assert!(memory.task_stack_region().is_some(), "{}", facts.id);
            }
            if let Some(placement) = &facts.placement {
                let memory = facts.memory.as_ref().unwrap();
                for rule in &placement.range {
                    assert!(memory.region(&rule.region).is_some(), "{}", facts.id);
                }
            }
        }
    }

    #[test]
    fn a_load_records_every_layer_it_reads() {
        let repository = repository();
        let root = repository.path();
        let mut inputs = oer_os_durable::inputs::Inputs::tracked();
        Profile::load_with_inputs(root, "esp32c5", &mut inputs).unwrap();
        let read: Vec<PathBuf> = inputs.into_digests().into_keys().collect();
        let mut expected: Vec<PathBuf> = [
            "chips/espressif/platform/isa/rv32imac.toml",
            "chips/espressif/platform/family.toml",
            "chips/esp32c5/platform/chip.toml",
            "chips/esp32c5/platform/modules/esp32-c5-wroom-1-n16r8.toml",
            "chips/esp32c5/platform/boards/esp32-c5-devkit.toml",
        ]
        .iter()
        .map(|path| root.join(path))
        .collect();
        expected.sort();
        assert_eq!(read, expected);
    }

    #[test]
    fn each_hart_has_its_own_guard_and_usable_capacity() {
        for (chip, guards) in [
            ("esp32s31", BTreeMap::from([(0, 4096), (1, 1024)])),
            ("esp32c5", BTreeMap::from([(0, 4096)])),
        ] {
            let contract = Profile::tracked(chip).unwrap().facts.interrupts.unwrap();
            assert_eq!(contract.irq_stack_guard_bytes, guards);
            for (hart, guard) in guards {
                assert_eq!(
                    contract.irq_stack_usable_bytes(hart),
                    contract.irq_stack_bytes - guard
                );
            }
        }
    }

    #[test]
    fn a_guard_contract_rejects_missing_extra_and_invalid_harts_or_sizes() {
        let repository = repository();
        let path = repository.path().join("chips/esp32s31/platform/chip.toml");
        let source = std::fs::read_to_string(&path).unwrap();
        for guards in [
            "1024", // The old common value is no longer a contract.
            "{}",
            "{ \"0\" = 4096 }",
            "{ \"0\" = 4096, \"1\" = 1024, \"2\" = 1024 }",
            "{ \"0\" = 0, \"1\" = 1024 }",
            "{ \"0\" = 4095, \"1\" = 1024 }",
            "{ \"0\" = 32768, \"1\" = 1024 }",
        ] {
            std::fs::write(
                &path,
                source.replace("{ \"0\" = 4096, \"1\" = 1024 }", guards),
            )
            .unwrap();
            assert!(
                Profile::load(repository.path(), "esp32s31").is_err(),
                "{guards}"
            );
        }
        std::fs::write(&path, source.replace("harts = [0, 1]", "harts = [0, 0]")).unwrap();
        assert!(Profile::load(repository.path(), "esp32s31").is_err());
    }

    #[test]
    fn a_registry_key_outside_the_registry_is_refused() {
        let repository = repository();
        let path = repository.path().join("chips/esp32c5/platform/chip.toml");
        let source = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            source.replace("[hil]\n", "[hil]\nimage-names = []\n"),
        )
        .unwrap();
        let error = Profile::load(repository.path(), "esp32c5")
            .unwrap_err()
            .to_string();
        assert!(error.contains("image-names"), "{error}");
    }

    /// A chip layer without parents, module or board.
    fn standalone(id: &str, family: &str) -> String {
        format!(
            "schema = 2\nid = \"{id}\"\nfamily = \"{family}\"\nisa = \"rv32imac\"\n\
             rust-target = \"riscv32imac-unknown-none-elf\"\nboot = \"staged\"\n\
             espflash-chip = \"x\"\nrevisions = []\n[properties]\nwifi-bands = []\n\
             bluetooth = []\nieee802154 = false\ncores = 1\n"
        )
    }

    #[test]
    fn an_unknown_chip_names_the_supported_ones() {
        let root = tempfile::tempdir().unwrap();
        let platform = root.path().join(platform_dir("esp32x9"));
        std::fs::create_dir_all(&platform).unwrap();
        std::fs::write(platform.join("chip.toml"), standalone("esp32x9", "f")).unwrap();
        std::fs::create_dir_all(root.path().join(platform_dir("no-profile"))).unwrap();
        let error = Profile::load(root.path(), "esp32s2")
            .unwrap_err()
            .to_string();
        assert_eq!(error, "unsupported chip `esp32s2`; supported: esp32x9");
        assert_eq!(supported(root.path()).unwrap(), ["esp32x9"]);
        assert_eq!(
            Profile::load(root.path(), "esp32x9").unwrap().facts.id,
            "esp32x9"
        );
    }

    #[test]
    fn a_profile_must_name_its_own_chip() {
        let root = tempfile::tempdir().unwrap();
        let platform = root.path().join(platform_dir("esp32x9"));
        std::fs::create_dir_all(&platform).unwrap();
        std::fs::write(platform.join("chip.toml"), standalone("other", "f")).unwrap();
        assert!(Profile::load(root.path(), "esp32x9").is_err());
    }

    #[test]
    fn a_profile_must_name_a_valid_family() {
        let root = tempfile::tempdir().unwrap();
        let platform = root.path().join(platform_dir("esp32x9"));
        std::fs::create_dir_all(&platform).unwrap();
        std::fs::write(
            platform.join("chip.toml"),
            standalone("esp32x9", "espressif"),
        )
        .unwrap();
        assert_eq!(
            Profile::load(root.path(), "esp32x9").unwrap().facts.family,
            "espressif"
        );
        std::fs::write(
            platform.join("chip.toml"),
            standalone("esp32x9", "Espressif"),
        )
        .unwrap();
        let error = Profile::load(root.path(), "esp32x9")
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid family"), "{error}");
    }
}

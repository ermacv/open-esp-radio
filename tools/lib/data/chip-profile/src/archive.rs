//! The chips this repository supports, from their tracked profiles.
//!
//! Every supported chip has `platform/<id>/chip.toml`, holding only what
//! differs between chips and cannot be derived from the id: the chip family,
//! the Rust target, the boot flow, the chip name `espflash` uses, the silicon revisions and
//! the chip's properties (radio bands, Bluetooth modes, cores).
//! Everything else follows the id by convention (`verification/<id>`,
//! `registers/<id>`, `hil/targets/<id>`,
//! `target/hil/<id>`), and a capability exists where its directory does.
//! Host tools resolve a chip through [`Profile::load`] instead of matching
//! chip names, so an unknown chip is refused with the supported ones.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// Format of `chip.toml`.
pub const SCHEMA: u32 = 1;
/// Directory of the platforms, relative to the repository root.
const PLATFORM: &str = "platform";
/// Directory of every chip's HIL agent workspace, relative to the root.
const HIL_TARGETS: &str = "hil/targets";
const PROFILE: &str = "chip.toml";

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How an image starts an application.
///
/// Every chip's own images boot [`Boot::Staged`] (a profile names no other);
/// [`Boot::EspIdfBootloader`] describes only an image the firmware catalog
/// builds with ESP-IDF (a vendor or peer image), whose own bootloader loads
/// its application.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Boot {
    /// The ESP-IDF second-stage bootloader loads the platform's bootstrap,
    /// which stages the runtime into PSRAM.
    Staged,
    /// The ESP-IDF second-stage bootloader loads an ESP-IDF application.
    EspIdfBootloader,
}

/// One supported chip.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Profile {
    schema: u32,
    /// The chip id: lowercase, as in paths, package names and evidence.
    pub id: String,
    /// The family whose shared code the chip uses: a `family` package
    /// declaring this id builds for, and may be used by, every chip of it.
    pub family: String,
    /// The Rust target triple of its firmware.
    pub rust_target: String,
    /// How the chip's images start: [`Boot::Staged`].
    pub boot: Boot,
    /// The `--chip` value of `espflash`.
    pub espflash_chip: String,
    /// Silicon revisions the repository's models and pins describe.
    pub revisions: Vec<String>,
    /// Where the chip's images lie in flash: the image pipeline encodes
    /// for it, and every flash writes there. `None` for a chip that has no
    /// images yet.
    #[serde(default)]
    pub flash: Option<FlashMap>,
    /// What the chip has.
    pub properties: Properties,
    /// The address map images are checked against; `None` for a chip whose
    /// images are not checked by address.
    #[serde(default)]
    pub memory: Option<Memory>,
    /// The interrupt contract the interrupt-stack check reads.
    #[serde(default)]
    pub interrupts: Option<InterruptContract>,
    /// The ROM the stack analyses read; `None` for a chip without a pinned
    /// ROM ELF.
    #[serde(default)]
    pub rom: Option<Rom>,
    /// The placement contract the placement check reads.
    #[serde(default)]
    pub placement: Option<Placement>,
    /// What the chip's HIL agent may not link; `None` when nothing is
    /// forbidden.
    #[serde(default)]
    pub hil: Option<Hil>,
    /// The chip's packages as the repository policies classify them.
    #[serde(default)]
    pub packages: Packages,
    /// What the repository gate builds and audits for the chip.
    #[serde(default)]
    pub gate: Gate,
    /// The Rust probe images of the chip's vendor comparison, in build
    /// order (`verification/<id>/probes`).
    #[serde(default)]
    pub probe: Vec<Probe>,
    /// The staged boot's host contract; a chip with images names it.
    #[serde(default)]
    pub staged: Option<StagedBoot>,
    /// Where the host reads why the chip last reset; `None` for a chip whose
    /// stand cannot prove a power loss.
    #[serde(default)]
    pub reset_cause: Option<ResetCause>,
}

/// The `[reset-cause]` table: the field of the chip's platform publication
/// (`registers/<id>/published/platform.bindings.toml`) that holds why the
/// core last left reset, which the stand reads through JTAG after a power
/// cycle, and the code of a power-on reset.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ResetCause {
    /// `PERIPHERAL.REGISTER.FIELD`, the SVD names.
    pub field: String,
    pub power_on: u32,
}

/// A contiguous address range.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub origin: u32,
    pub length: u32,
}

impl Region {
    /// First address past the region.
    pub const fn end(self) -> u32 {
        self.origin + self.length
    }

    /// Whether `start..end` is a (possibly empty) range inside this region.
    pub const fn contains_range(self, start: u64, end: u64) -> bool {
        start >= self.origin as u64 && end >= start && end <= self.end() as u64
    }
}

/// The `[memory]` table: the address map host tools check images against.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Memory {
    pub sram: Region,
    pub flash_xip: Region,
    /// The cached external RAM aperture, on a chip that maps one apart
    /// from flash.
    #[serde(default)]
    pub psram: Option<Region>,
    /// Where a staged boot's runtime is linked.
    #[serde(default)]
    pub runtime_psram: Option<Region>,
    /// The region the runtime's task stacks lie in, by [`Memory::region`]
    /// name.
    pub task_stacks: String,
}

impl Memory {
    /// The region a placement rule names: `sram`, `flash-xip`, `psram` or
    /// `runtime-psram`; `None` for one the chip does not map.
    pub fn region(&self, name: &str) -> Option<Region> {
        match name {
            "sram" => Some(self.sram),
            "flash-xip" => Some(self.flash_xip),
            "psram" => self.psram,
            "runtime-psram" => self.runtime_psram,
            _ => None,
        }
    }

    /// The region the task stacks lie in.
    pub fn task_stack_region(&self) -> Option<Region> {
        self.region(&self.task_stacks)
    }

    /// The cached regions: code there cannot run while the cache is off.
    pub fn cached(&self) -> Vec<Region> {
        std::iter::once(self.flash_xip).chain(self.psram).collect()
    }
}

/// A writer of interrupt levels or routes and the functions it may run in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct LevelWriter {
    pub writer: String,
    pub contexts: Vec<String>,
}

/// The `[interrupts.ipc]` table: the inter-processor call whose dispatch
/// calls a posted function.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Ipc {
    /// The only writer of the posted callback.
    pub post: String,
    /// Its argument that is every function the dispatch runs.
    pub post_handler_argument: usize,
    /// The functions that call the posted callback.
    pub dispatch: Vec<String>,
}

/// The `[interrupts]` table: where an image's interrupt table, vectors and
/// dispatcher are, the levels every hart takes and the stack they run on.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct InterruptContract {
    pub table_symbol: String,
    pub table_entry_bytes: u32,
    /// Fields of a table entry as `[offset, bytes]`.
    pub table_source: [u32; 2],
    pub table_level: [u32; 2],
    pub table_core: [u32; 2],
    pub table_handler: u32,
    pub vector_table_symbol: String,
    pub exception_entry_symbol: String,
    pub source_table_symbol: String,
    pub harts: Vec<u32>,
    pub always_levels: Vec<u32>,
    /// The inter-processor call of a chip with more than one hart; `None`
    /// on a single hart.
    #[serde(default)]
    pub ipc: Option<Ipc>,
    pub level_writers: Vec<LevelWriter>,
    /// Each hart's dedicated SRAM interrupt stack.
    pub irq_stack_bytes: u32,
    pub irq_stack_guard_bytes: u32,
    pub irq_stack_margin_percent: u32,
}

impl InterruptContract {
    /// The bytes an interrupt stack may use: below them lies the guard.
    pub fn irq_stack_usable_bytes(&self) -> u32 {
        self.irq_stack_bytes - self.irq_stack_guard_bytes
    }
}

/// The `[staged]` table: the staged boot's host contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct StagedBoot {
    /// The input sections the runtime's linker script places in regions the
    /// boot zeroes: `name` or `name.*`.
    pub zeroed_inputs: Vec<String>,
    pub stage_two: StageTwo,
}

/// The stage-two image header the image pipeline packs: little-endian
/// words at these byte offsets.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct StageTwo {
    pub magic: u32,
    pub abi_version: u32,
    pub header_bytes: u32,
    pub magic_offset: u32,
    pub abi_version_offset: u32,
    pub header_size_offset: u32,
    /// The payload CRC-32, computed with this field read as zero.
    pub crc_offset: u32,
}

/// The `[rom]` table: the chip's mask ROM as the binary analyses read it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Rom {
    /// The id of the pinned ROM ELF in `verification/<id>/artifacts.toml`
    /// (`oer-vendor-pins`) the stack analyses run with.
    pub elf: String,
    /// The reviewed ROM function summaries of the stack analyses, relative
    /// to the repository root; `None` for a chip with none reviewed yet.
    #[serde(default)]
    pub summaries: Option<PathBuf>,
    /// ROM code no shipped image may call into.
    #[serde(default)]
    pub forbidden: Vec<RomRange>,
}

/// A named ROM address range, `start..end`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RomRange {
    pub name: String,
    pub start: u32,
    pub end: u32,
}

/// The `[placement]` table: where an image's linked symbols must lie in the
/// `[memory]` map. Regions are named as [`Memory::region`] names them.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Placement {
    /// The contract's name in the placement report.
    pub name: String,
    /// The flattened image: it spans these symbols from the origin of a
    /// region, and the flat file holds exactly them.
    #[serde(default)]
    pub image: Option<FlatImage>,
    /// The entry point and the `[start, end]` symbols of the text it lies in.
    #[serde(default)]
    pub entry: Option<Entry>,
    /// Symbol ranges that lie in a region, some of an exact size.
    #[serde(default)]
    pub range: Vec<RangeRule>,
    /// Symbols whose `bytes` from their address lie in a region.
    #[serde(default)]
    pub symbol: Vec<SymbolRule>,
    /// Entries whose first instruction swaps `sp` with `mscratch` before
    /// anything touches the interrupted stack.
    #[serde(default)]
    pub stack_swap: Vec<Symbols>,
}

/// The flattened image's span.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FlatImage {
    pub start: String,
    pub end: String,
    /// The region whose origin the image starts at.
    pub origin: String,
}

/// The entry point and its text.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Entry {
    pub symbol: String,
    pub text: [String; 2],
}

/// `[start, end]` symbols that lie in `region`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct RangeRule {
    pub range: [String; 2],
    pub region: String,
    /// The range's exact size, where the contract fixes one.
    #[serde(default)]
    pub bytes: Option<u64>,
}

/// Symbols whose `bytes` from their address lie in `region`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct SymbolRule {
    /// A symbol, or with `numbers` a family as [`Symbols`] names it.
    pub name: String,
    #[serde(default)]
    pub numbers: Option<[u32; 2]>,
    pub bytes: u64,
    pub region: String,
}

impl SymbolRule {
    /// The symbols the rule holds.
    pub fn symbols(&self) -> Symbols {
        Symbols {
            name: self.name.clone(),
            numbers: self.numbers,
        }
    }
}

/// One symbol, or a numbered family: `name` with `{n}` replaced by each of
/// `numbers` (inclusive).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Symbols {
    pub name: String,
    #[serde(default)]
    pub numbers: Option<[u32; 2]>,
}

impl Symbols {
    /// Every symbol named.
    pub fn names(&self) -> Vec<String> {
        match self.numbers {
            None => vec![self.name.clone()],
            Some([first, last]) => (first..=last)
                .map(|number| self.name.replace("{n}", &number.to_string()))
                .collect(),
        }
    }
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
    /// Production libraries whose unsafe code is audited (`crates/UNSAFE.md`).
    #[serde(default)]
    pub audited_unsafe: Vec<String>,
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

/// The `[gate]` table: what the repository gate builds and audits for the
/// chip, beyond what every chip gets.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Gate {
    /// Packages linted again with their `validation-probes` feature.
    #[serde(default)]
    pub validation_probes: Vec<String>,
    /// Chip-target doctests.
    #[serde(default)]
    pub doctests: Vec<Doctest>,
    /// Firmware package directories whose library tests run on the host.
    #[serde(default)]
    pub host_tests: Vec<PathBuf>,
    /// Firmware workspaces built in release for the chip.
    #[serde(default)]
    pub release_builds: Vec<PathBuf>,
    /// Root-workspace packages built for the chip target.
    #[serde(default)]
    pub target_builds: Vec<String>,
    /// Whether Clippy checks the chip's HIL agent for each of its feature
    /// profiles.
    #[serde(default)]
    pub agent_clippy: bool,
    /// Whether the register tool's shared-word check runs against the
    /// esp-hal and PAC sources the chip's HIL agent workspace resolves.
    #[serde(default)]
    pub shared_words: bool,
    /// Whether every package that builds esp-hal for the chip must enable
    /// `static-interrupts`, leaving the routes to the image's table.
    #[serde(default)]
    pub static_interrupts: bool,
    /// The production PHY library the PHY audit builds.
    #[serde(default)]
    pub phy: Option<PhyLibrary>,
    /// The facade's chip feature profiles.
    #[serde(default)]
    pub facade: Vec<FacadeProfile>,
    /// The network boundary profiles of the chip's packages.
    #[serde(default)]
    pub network: Vec<NetworkProfile>,
}

/// A chip-target doctest: a package with its features.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Doctest {
    pub package: String,
    pub features: String,
}

/// The production PHY library and the chip's packages its build may
/// compile beside the shared ones the audit reviews.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct PhyLibrary {
    pub package: String,
    pub packages: Vec<String>,
}

/// One facade feature profile: the packages it must and must not bring;
/// `forbidden` names groups (`wifi`, `bluetooth`, `ieee802154`, `backends`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FacadeProfile {
    pub features: String,
    pub required: Vec<String>,
    pub forbidden: Vec<String>,
    /// Whether the profile is a Bluetooth consumer that must reach no Wi-Fi
    /// package.
    #[serde(default)]
    pub bluetooth_only: bool,
}

/// One network boundary profile: a manifest, its boundary and the Cargo
/// feature flags it resolves with.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct NetworkProfile {
    pub boundary: String,
    pub manifest: PathBuf,
    #[serde(default)]
    pub features: Vec<String>,
}

/// One Rust probe image of the vendor comparison: the artifact role it
/// serves and its package in `verification/<id>/probes`. Every probe
/// compiles in the host's one firmware compile cache.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Probe {
    pub role: String,
    pub package: String,
}

/// A Wi-Fi band the chip's radio serves.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum WifiBand {
    #[serde(rename = "2g4")]
    Band2g4,
    #[serde(rename = "5g")]
    Band5g,
}

/// A Bluetooth mode the chip's controller serves.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BluetoothMode {
    Le,
    BrEdr,
}

/// The `[properties]` table of a chip profile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Properties {
    pub wifi_bands: Vec<WifiBand>,
    pub bluetooth: Vec<BluetoothMode>,
    pub ieee802154: bool,
    pub cores: u8,
}

/// The chip's flash map (`[flash]` of its `chip.toml`): where the
/// second-stage bootloader, the partition table, the application and the
/// OTA selection lie, the partition table the platform defines, and how the
/// bootloader and the application are encoded.
///
/// The partition table is the source of the partitions it holds; the image
/// pipeline checks that `application` and `otadata` are the offsets of its
/// first application partition and its OTA data partition.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FlashMap {
    pub bootloader: u32,
    pub partition_table: u32,
    /// The partition an image's application is written to.
    pub application: u32,
    /// The OTA data partition, which selects the application slot.
    pub otadata: u32,
    /// The partition table images boot with, as a CSV relative to the
    /// repository root.
    pub partitions: PathBuf,
    /// The flash settings in the application image's header, which the
    /// bootloader switches to before it loads the application.
    pub application_encoding: FlashSettings,
    /// The mode the ROM reads the second-stage bootloader in; the
    /// bootloader's own header keeps the application's frequency and size.
    pub bootloader_mode: FlashMode,
    /// How a written image starts.
    pub start: Start,
}

/// Flash settings of an ESP image header.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct FlashSettings {
    pub mode: FlashMode,
    /// The SPI clock, in MHz.
    pub frequency_mhz: u32,
    /// The flash chip's size, in MiB.
    pub size_mib: u32,
}

/// An SPI flash read mode of an ESP image header.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlashMode {
    Qio,
    Qout,
    Dio,
    Dout,
}

/// How the stand starts an image it wrote into a board's flash.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Start {
    /// The writer's hard reset after the last segment starts the image.
    Reset,
    /// The writer leaves the ROM in its download mode; a power-on reset of
    /// the board's hub port starts the image, or an RTS reset on a board
    /// that does not reset by power: on such a chip an RTS reset out of
    /// download mode starts the image with its USB Serial/JTAG console
    /// silent, and the writer's own reset can leave it in download mode.
    PowerOn,
}

impl Profile {
    /// The profile of `id`, or an error naming the supported chips.
    pub fn load(root: &Path, id: &str) -> Result<Self> {
        let path = profile_path(root, id);
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "unsupported chip `{id}`; supported: {}",
                    supported(root)?.join(", ")
                )
                .into());
            }
            Err(error) => return Err(error.into()),
        };
        let profile: Self =
            toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        if profile.schema != SCHEMA {
            return Err(format!(
                "{} has schema {}; this tool reads schema {SCHEMA}",
                path.display(),
                profile.schema
            )
            .into());
        }
        if profile.id != id {
            return Err(format!("{} names chip `{}`", path.display(), profile.id).into());
        }
        if profile.boot != Boot::Staged {
            return Err(format!(
                "{} names boot `{:?}`; a chip's images boot staged, the ESP-IDF bootloader \
                 boots only the firmware catalog's images",
                path.display(),
                profile.boot
            )
            .into());
        }
        if !valid_identifier(&profile.family) {
            return Err(format!(
                "{} names invalid family `{}`",
                path.display(),
                profile.family
            )
            .into());
        }
        Ok(profile)
    }

    /// Every supported chip, sorted by id.
    pub fn all(root: &Path) -> Result<Vec<Self>> {
        supported(root)?
            .iter()
            .map(|id| Self::load(root, id))
            .collect()
    }

    /// `<root>/<directory>/<id>`, such as `verification/<id>`.
    pub fn directory(&self, root: &Path, directory: &str) -> PathBuf {
        root.join(directory).join(&self.id)
    }

    /// The Cargo workspace of the chip's platform, which holds the
    /// bootstrap of a staged boot.
    pub fn platform_workspace(&self, root: &Path) -> PathBuf {
        self.directory(root, PLATFORM)
    }

    /// The bootstrap package the staged boot builds into every image beside
    /// its runtime.
    pub fn bootstrap_package(&self) -> String {
        format!("oer-{}-platform-bootstrap", self.id)
    }

    /// The Cargo workspace of the chip's HIL agent firmware
    /// (`hil/targets/<id>`, by convention).
    pub fn hil_agent_workspace(&self, root: &Path) -> PathBuf {
        self.directory(root, HIL_TARGETS)
    }

    /// The package of the chip's HIL agent firmware in that workspace.
    pub fn hil_agent_package(&self) -> String {
        format!("oer-{}-hil-agent", self.id)
    }

    /// The stack policy the chip's HIL images build under, relative to the
    /// repository root (`hil/targets/<id>/stack.toml`).
    pub fn hil_stack_policy(&self) -> PathBuf {
        Path::new(HIL_TARGETS).join(&self.id).join("stack.toml")
    }

    /// The chip's own stack policy (`platform/<id>/stack.toml`), relative
    /// to the root: the move limit and reserves of every image of the chip,
    /// which the HIL policy extends.
    pub fn stack_policy(&self) -> PathBuf {
        Path::new(PLATFORM).join(&self.id).join("stack.toml")
    }

    /// The input sections the chip's staged boot zeroes, comma separated,
    /// as the image linker reads them; empty for a chip without a staged
    /// boot.
    pub fn zeroed_inputs(&self) -> String {
        self.staged
            .as_ref()
            .map(|staged| staged.zeroed_inputs.join(","))
            .unwrap_or_default()
    }

    /// The manifest of that package, which declares the features the
    /// agent's images select from.
    pub fn hil_agent_manifest(&self, root: &Path) -> PathBuf {
        self.hil_agent_workspace(root).join("agent/Cargo.toml")
    }

    /// Every `(workspace, package)` a HIL image of the chip is built from:
    /// the HIL agent and the platform's bootstrap.
    pub fn hil_image_packages(&self, root: &Path) -> Vec<(PathBuf, String)> {
        vec![
            (self.hil_agent_workspace(root), self.hil_agent_package()),
            (self.platform_workspace(root), self.bootstrap_package()),
        ]
    }
}

/// The Rust target of `id`'s firmware, from its `chip.toml`.
pub fn rust_target(root: &Path, id: &str) -> Result<String> {
    Ok(Profile::load(root, id)?.rust_target)
}

/// The family of every supported chip, keyed by chip id.
pub fn families(root: &Path) -> Result<BTreeMap<String, String>> {
    Ok(Profile::all(root)?
        .into_iter()
        .map(|profile| (profile.id, profile.family))
        .collect())
}

/// A chip or family identifier: a lowercase ASCII letter, then lowercase
/// letters, digits or hyphens.
pub fn valid_identifier(id: &str) -> bool {
    id.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// Ids of the chips with a profile, sorted.
pub fn supported(root: &Path) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for entry in std::fs::read_dir(root.join(PLATFORM))? {
        let entry = entry?;
        if entry.path().join(PROFILE).is_file() {
            ids.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    ids.sort();
    Ok(ids)
}

fn profile_path(root: &Path, id: &str) -> PathBuf {
    root.join(PLATFORM).join(id).join(PROFILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repository() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn the_tracked_profiles_load() {
        let root = repository();
        let chips = Profile::all(&root).unwrap();
        assert!(!chips.is_empty());
        assert_eq!(families(&root).unwrap().len(), chips.len());
        for chip in &chips {
            assert!(valid_identifier(&chip.family), "{}", chip.id);
            if let Some(flash) = &chip.flash {
                assert!(flash.bootloader < flash.partition_table, "{}", chip.id);
                assert!(flash.partition_table < flash.application, "{}", chip.id);
                // The partition table the platform defines exists.
                assert!(root.join(&flash.partitions).is_file(), "{}", chip.id);
            }
            assert_eq!(
                chip.directory(&root, "verification"),
                root.join("verification").join(&chip.id)
            );
            if let Some(memory) = &chip.memory {
                assert!(memory.task_stack_region().is_some(), "{}", chip.id);
            }
            if let Some(placement) = &chip.placement {
                let memory = chip.memory.as_ref().unwrap();
                for rule in &placement.range {
                    assert!(memory.region(&rule.region).is_some(), "{}", chip.id);
                }
            }
        }
    }

    #[test]
    fn an_unknown_chip_names_the_supported_ones() {
        let root = tempfile::tempdir().unwrap();
        let platform = root.path().join(PLATFORM).join("esp32x9");
        std::fs::create_dir_all(&platform).unwrap();
        std::fs::write(
            platform.join(PROFILE),
            "schema = 1\nid = \"esp32x9\"\nrust-target = \"riscv32imac-unknown-none-elf\"\n\
             boot = \"staged\"\nespflash-chip = \"esp32x9\"\nrevisions = [\"rev0\"]\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.path().join(PLATFORM).join("no-profile")).unwrap();
        let error = Profile::load(root.path(), "esp32s2")
            .unwrap_err()
            .to_string();
        assert_eq!(error, "unsupported chip `esp32s2`; supported: esp32x9");
        assert_eq!(supported(root.path()).unwrap(), ["esp32x9"]);
    }

    #[test]
    fn a_profile_must_name_its_own_chip() {
        let root = tempfile::tempdir().unwrap();
        let platform = root.path().join(PLATFORM).join("esp32x9");
        std::fs::create_dir_all(&platform).unwrap();
        std::fs::write(
            platform.join(PROFILE),
            "schema = 1\nid = \"other\"\nrust-target = \"t\"\nboot = \"staged\"\n\
             espflash-chip = \"x\"\nrevisions = []\n",
        )
        .unwrap();
        assert!(Profile::load(root.path(), "esp32x9").is_err());
    }

    #[test]
    fn a_profile_must_name_a_valid_family() {
        let root = tempfile::tempdir().unwrap();
        let platform = root.path().join(PLATFORM).join("esp32x9");
        std::fs::create_dir_all(&platform).unwrap();
        let profile = |family: &str| {
            format!(
                "schema = 1\nid = \"esp32x9\"\nfamily = \"{family}\"\n\
                 rust-target = \"t\"\nboot = \"staged\"\nespflash-chip = \"x\"\n\
                 revisions = []\n[properties]\nwifi-bands = []\nbluetooth = []\n\
                 ieee802154 = false\ncores = 1\n"
            )
        };
        std::fs::write(platform.join(PROFILE), profile("espressif")).unwrap();
        assert_eq!(
            Profile::load(root.path(), "esp32x9").unwrap().family,
            "espressif"
        );
        std::fs::write(platform.join(PROFILE), profile("Espressif")).unwrap();
        let error = Profile::load(root.path(), "esp32x9")
            .unwrap_err()
            .to_string();
        assert!(error.contains("invalid family"), "{error}");
    }

    #[test]
    fn every_chip_s_hil_agent_workspace_declares_its_package() {
        let root = repository();
        for chip in Profile::all(&root).unwrap() {
            let workspace = chip.hil_agent_workspace(&root);
            let package = chip.hil_agent_package();
            let declared = std::fs::read_dir(&workspace)
                .unwrap()
                .filter_map(|entry| {
                    let manifest = entry.unwrap().path().join("Cargo.toml");
                    let text = std::fs::read_to_string(manifest).ok()?;
                    let manifest: toml::Table = toml::from_str(&text).ok()?;
                    Some(manifest.get("package")?.get("name")?.as_str()?.to_owned())
                })
                .any(|name| name == package);
            assert!(
                declared,
                "{} declares no package {package}",
                workspace.display()
            );
            assert!(chip.hil_agent_manifest(&root).is_file());
        }
    }

    #[test]
    fn every_image_package_is_declared_in_its_workspace() {
        let root = repository();
        for chip in Profile::all(&root).unwrap() {
            for (workspace, package) in chip.hil_image_packages(&root) {
                let text = std::fs::read_to_string(workspace.join("Cargo.toml")).unwrap();
                let members: toml::Table = toml::from_str(&text).unwrap();
                let listed = members["workspace"]["members"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|member| {
                        let manifest = workspace.join(member.as_str()?).join("Cargo.toml");
                        let manifest: toml::Table =
                            toml::from_str(&std::fs::read_to_string(manifest).ok()?).ok()?;
                        Some(manifest.get("package")?.get("name")?.as_str()?.to_owned())
                    })
                    .any(|name| name == package);
                assert!(listed, "{} has no member {package}", workspace.display());
            }
        }
    }
}

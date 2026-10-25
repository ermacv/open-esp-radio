//! The sections of the resolved facts, as every reader takes them.

use alloc::{collections::BTreeMap, format, string::String, vec::Vec};

use serde::{Deserialize, Serialize};

use crate::Result;

/// How an image starts an application.
///
/// Every chip's own images boot [`Boot::Staged`] (the family names no other);
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

/// The `[core-start]` table: the functions that release a core other than
/// core 0 from reset or stall. An image that links one has a core running code
/// of its own, which its stack policy must then declare with a proven root;
/// an image that declares every other core unused links none.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoreStart {
    /// Demangled function-name patterns, as the policy's `observed_functions`:
    /// `*` matches any substring, all other characters are literal.
    pub starters: Vec<String>,
    /// The functions of the cross-core call, same patterns. An image that
    /// links one runs code posted to another core, which no image does until
    /// the call is typed.
    pub ipc: Vec<String>,
}

/// The `[reset-cause]` table: the field of the chip's platform publication
/// (`chips/<id>/registers/published/platform.bindings.toml`) that holds why the
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

/// The address map host tools check images against.
///
/// The chip names its SRAM, its flash window and where PSRAM lies; the
/// module names the PSRAM's size; `runtime_psram` is the PSRAM after the
/// family's bootstrap probe page.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Memory {
    pub sram: Region,
    pub flash_xip: Region,
    /// The module's PSRAM in the cached external RAM aperture, on a chip
    /// that maps one apart from flash.
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
        core::iter::once(self.flash_xip).chain(self.psram).collect()
    }
}

/// A writer of interrupt levels or routes and the functions it may run in.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct LevelWriter {
    pub writer: String,
    pub contexts: Vec<String>,
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
    /// The CPU line the entry's source is routed to.
    pub table_line: [u32; 2],
    pub table_level: [u32; 2],
    /// The entry's handler class (`oer_interrupt_table::Class`).
    pub table_class: [u32; 2],
    pub table_core: [u32; 2],
    pub table_handler: u32,
    /// An exempt entry's reason is the bytes of the symbol named by this
    /// prefix and the entry's handler symbol.
    pub table_exemption_prefix: String,
    /// Every hart's hardware-vector table, one after another.
    pub vector_table_symbol: String,
    /// The words of one hart's table: the word of line `n` of hart `h` is
    /// word `h * vector_table_hart_slots + n`.
    pub vector_table_hart_slots: u32,
    pub exception_entry_symbol: String,
    pub harts: Vec<u32>,
    pub level_writers: Vec<LevelWriter>,
    /// Maximum allocation of each hart's dedicated SRAM interrupt stack.
    pub irq_stack_bytes: u32,
    /// Guard bytes keyed by hart, with exactly one entry for every `harts` member.
    #[serde(deserialize_with = "by_hart")]
    pub irq_stack_guard_bytes: BTreeMap<u32, u32>,
    pub irq_stack_margin_percent: u32,
}

/// A table keyed by hart number, as TOML keys are strings.
fn by_hart<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> core::result::Result<BTreeMap<u32, u32>, D::Error> {
    BTreeMap::<String, u32>::deserialize(deserializer)?
        .into_iter()
        .map(|(hart, bytes)| {
            hart.parse()
                .map(|hart| (hart, bytes))
                .map_err(|_| serde::de::Error::custom(format!("`{hart}` is not a hart number")))
        })
        .collect()
}

impl InterruptContract {
    /// Maximum usable bytes; final checks read the actual allocation from ELF.
    pub fn irq_stack_usable_bytes(&self, hart: u32) -> u32 {
        self.irq_stack_bytes - self.irq_stack_guard_bytes[&hart]
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let harts: alloc::collections::BTreeSet<_> = self.harts.iter().copied().collect();
        if harts.len() != self.harts.len()
            || harts.is_empty()
            || self
                .irq_stack_guard_bytes
                .keys()
                .copied()
                .collect::<alloc::collections::BTreeSet<_>>()
                != harts
        {
            return Err("interrupt guards must name every hart exactly once".into());
        }
        for (&hart, &bytes) in &self.irq_stack_guard_bytes {
            if bytes < 16 || !bytes.is_power_of_two() || bytes >= self.irq_stack_bytes {
                return Err(format!("hart {hart}'s interrupt guard must be a power of two of at least 16 bytes, smaller than its stack").into());
            }
        }
        Ok(())
    }
}

/// The `[staged]` table: the staged boot's host contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct StagedBoot {
    /// The input sections the runtime's linker script places in regions the
    /// boot zeroes: `name` or `name.*`.
    pub zeroed_inputs: Vec<String>,
    /// The PSRAM page the bootstrap probes before it copies stage two; stage
    /// two lies after it.
    pub bootstrap_psram_bytes: u32,
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
    /// The id of the pinned ROM ELF in `chips/<id>/vendor/artifacts.toml`
    /// (`oer-vendor-pins`) the stack analyses run with.
    pub elf: String,
    /// The reviewed ROM function summaries of the stack analyses, relative
    /// to the repository root; `None` for a chip with none reviewed yet.
    #[serde(default)]
    pub summaries: Option<String>,
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
    /// The regions every handler of the `[interrupts]` table lies in;
    /// `rom` names the mask ROM's code, as the `[rom]` ELF holds it.
    #[serde(default)]
    pub interrupt_handlers: Vec<String>,
    /// Allocated sections, by name prefix, that lie in a region.
    #[serde(default)]
    pub section: Vec<SectionRule>,
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
    /// Maximum size for a range whose allocation is chosen at link time.
    #[serde(default)]
    pub maximum_bytes: Option<u64>,
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
    /// Whether an image may lack the symbol.
    #[serde(default)]
    pub optional: bool,
}

/// Allocated sections whose name starts with `prefix`, each wholly in
/// `region`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct SectionRule {
    pub prefix: String,
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
            None => alloc::vec![self.name.clone()],
            Some([first, last]) => (first..=last)
                .map(|number| self.name.replace("{n}", &format!("{number}")))
                .collect(),
        }
    }
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

/// The `[properties]` table of a chip.
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
    pub partitions: String,
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
    /// The flash chip's size, in MiB: the module's.
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

/// The `[module]` table: the module that carries the chip, with its
/// flash and PSRAM.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Module {
    pub name: String,
    pub flash: ModuleFlash,
    /// `None` for a module without PSRAM.
    #[serde(default)]
    pub psram: Option<ModulePsram>,
}

/// The module's flash.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ModuleFlash {
    pub size_mib: u32,
}

/// The module's PSRAM.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct ModulePsram {
    pub size_mib: u32,
}

/// The `[board]` table: the board that carries the module.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Board {
    pub name: String,
}

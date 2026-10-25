//! The two file formats as written: a block type file and the chip root.

use std::collections::BTreeMap;

use oer_registers_contracts::{FactAccuracy, FactCompleteness, FactProvenance, NameOrigin};
use serde::Deserialize;

/// The schema of a block type file.
pub const TYPE_SCHEMA: u32 = 3;
/// The schema of a chip root file.
pub const ROOT_SCHEMA: u32 = 4;

/// One block type file: one hardware block, offsets from its base.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TypeFile {
    pub schema: u32,
    #[serde(rename = "type")]
    pub header: TypeHeader,
    #[serde(default)]
    pub interrupt_outputs: Vec<InterruptOutput>,
    #[serde(default)]
    pub review: TypeReview,
    #[serde(default)]
    pub registers: Vec<Register>,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub commands: Vec<Command>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TypeHeader {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub space: Space,
    pub sources: Vec<String>,
}

/// The address space a type's registers live in.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum Space {
    /// CPU memory-mapped registers of 8, 16 or 32 bits.
    #[default]
    Mmio,
    /// Analog-I2C bytes, addressed by block code and index.
    AnalogI2c,
}

impl Space {
    pub fn label(self) -> &'static str {
        match self {
            Self::Mmio => "mmio",
            Self::AnalogI2c => "analog-i2c",
        }
    }

    pub(crate) fn default_width(self) -> u32 {
        match self {
            Self::Mmio => 32,
            Self::AnalogI2c => 8,
        }
    }
}

/// A named interrupt output of a block; instances map it to sources.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct InterruptOutput {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub sources: Vec<String>,
}

/// The type's review: defaults and the overrides that differ from them.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TypeReview {
    #[serde(default)]
    pub sources: Option<Vec<String>>,
    #[serde(default)]
    pub provenance: Option<FactProvenance>,
    #[serde(default)]
    pub accuracy: Option<FactAccuracy>,
    #[serde(default)]
    pub completeness: Option<FactCompleteness>,
    #[serde(default)]
    pub naming: Option<NameOrigin>,
    /// The default of every register and field named `*_OPAQUE`.
    #[serde(default)]
    pub opaque: Option<OpaqueReview>,
    /// Overrides keyed `REGISTER`, `REGISTER.FIELD`, `GROUP.REGISTER` or
    /// `GROUP.REGISTER.FIELD`.
    #[serde(default)]
    pub entity: BTreeMap<String, ReviewOverride>,
}

/// The review default of opaque entities; their naming is `opaque`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct OpaqueReview {
    #[serde(default)]
    pub sources: Option<Vec<String>>,
    #[serde(default)]
    pub provenance: Option<FactProvenance>,
    #[serde(default)]
    pub accuracy: Option<FactAccuracy>,
    #[serde(default)]
    pub completeness: Option<FactCompleteness>,
}

/// The keys of one entity's review that differ from its default.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReviewOverride {
    /// No source is recorded for the entity, and why. Such a review states
    /// no sources, provenance, accuracy or completeness: it resolves to no
    /// sources, no provenance and `unknown` accuracy and completeness, and
    /// is no reviewed assertion.
    #[serde(default)]
    pub unsourced: Option<String>,
    #[serde(default)]
    pub sources: Option<Vec<String>>,
    #[serde(default)]
    pub provenance: Option<FactProvenance>,
    #[serde(default)]
    pub accuracy: Option<FactAccuracy>,
    #[serde(default)]
    pub completeness: Option<FactCompleteness>,
    #[serde(default)]
    pub naming: Option<NameOrigin>,
}

/// One register (or `dim` array of single words) of a type.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Register {
    pub name: String,
    /// From the block's base, or from the group's start.
    pub offset: u32,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub dim: Option<Dim>,
    #[serde(default)]
    pub reset: Option<Reset>,
    #[serde(default)]
    pub reserved: Option<Reserved>,
    #[serde(default)]
    pub fifo: Option<Sourced>,
    #[serde(default)]
    pub data_port: Option<Sourced>,
    #[serde(default)]
    pub read_side_effect: Option<Sourced>,
    #[serde(default)]
    pub read_safe: Option<ReadSafe>,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub views: Vec<View>,
}

/// A repeated single word: `count` elements `stride` bytes apart.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Dim {
    pub count: u32,
    pub stride: u32,
}

/// The reset value of the bits in `mask` (all bits when absent).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Reset {
    pub value: u32,
    #[serde(default)]
    pub mask: Option<u32>,
}

/// Bits a source marks reserved.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Reserved {
    pub mask: u32,
    pub sources: Vec<String>,
}

/// A register-level construct and its sources.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Sourced {
    pub sources: Vec<String>,
}

/// An analog byte whose read is qualified for a diagnostic capture.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReadSafe {
    pub epoch: String,
    #[serde(default)]
    pub comparison_mask: Option<u32>,
    pub sources: Vec<String>,
}

/// One field: bits and one class.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Field {
    pub name: String,
    /// `"n"` or `"high:low"`.
    pub bits: String,
    pub class: FieldClass,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub values: Vec<Value>,
    #[serde(default)]
    pub write_range: Option<WriteRange>,
    #[serde(default)]
    pub write_values_only: bool,
}

/// What a read and a write of a field do: the closed set of classes the
/// ESP-IDF headers and the curated model state.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(rename_all = "kebab-case")]
pub enum FieldClass {
    /// `R/W`.
    Plain,
    /// `RO`, `R`, `HRO`, `RO/SS`.
    ReadOnly,
    /// `WO`.
    WriteOnly,
    /// `WT`: a write starts an action; reads back nothing.
    WriteTrigger,
    /// `WTC`, `W1C`: a one written clears it.
    WriteOneToClear,
    /// `W1S`, `WTS`, `WS`, `R/W1`: a one written sets it.
    SetOnly,
    /// `R/W/SC`, `R/W/WTC`: software sets it, the hardware clears it.
    SelfClearing,
    /// `R/W/SS`, ...: writable, and the hardware also writes it.
    HardwareWritten,
    /// `R_SS_RC`, "cleared by a read".
    ReadClears,
    /// "Do not use it in applications": never written.
    DebugOnly,
    /// No source states what a read or a write does: neither is generated.
    Unknown,
}

impl FieldClass {
    /// Whether a read has a defined value, independently of read side effects.
    pub fn readable(self) -> bool {
        !matches!(self, Self::WriteOnly | Self::WriteTrigger | Self::Unknown)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::ReadOnly => "read-only",
            Self::WriteOnly => "write-only",
            Self::WriteTrigger => "write-trigger",
            Self::WriteOneToClear => "write-one-to-clear",
            Self::SetOnly => "set-only",
            Self::SelfClearing => "self-clearing",
            Self::HardwareWritten => "hardware-written",
            Self::ReadClears => "read-clears",
            Self::DebugOnly => "debug-only",
            Self::Unknown => "unknown",
        }
    }

    /// Whether software writes a value of the field's own choosing.
    pub fn written(self) -> bool {
        !matches!(
            self,
            Self::ReadOnly | Self::ReadClears | Self::DebugOnly | Self::Unknown
        )
    }
}

/// A named value of a field.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Value {
    pub name: String,
    pub value: u32,
    #[serde(default)]
    pub description: Option<String>,
}

/// The values a write may give a field, inclusive.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct WriteRange {
    pub min: u32,
    pub max: u32,
}

/// A sourced access to a strict subrange of one field of an analog byte.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct View {
    pub name: String,
    pub field: String,
    pub bits: String,
    pub sources: Vec<String>,
}

/// A repeated group of registers.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Group {
    /// Holds one `%s`, the element's index.
    pub name: String,
    pub offset: u32,
    pub count: u32,
    pub stride: u32,
    #[serde(default)]
    pub description: Option<String>,
    pub registers: Vec<Register>,
}

/// A command register: its starts, qualifiers and held bits.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Command {
    /// `REGISTER` or `GROUP.REGISTER`.
    pub register: String,
    pub commands: Vec<String>,
    #[serde(default)]
    pub qualifiers: Vec<String>,
    #[serde(default)]
    pub held: Vec<String>,
    pub sources: Vec<String>,
}

/// The chip root: includes, instances, interrupts, the analog-I2C space,
/// RAM regions and the placement of memory layouts.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RootFile {
    pub schema: u32,
    pub chip: String,
    pub types: Vec<String>,
    #[serde(default)]
    pub layouts: Vec<String>,
    #[serde(default)]
    pub instances: Vec<Instance>,
    #[serde(default)]
    pub interrupts: Vec<Interrupt>,
    pub interrupt_sources: InterruptSources,
    #[serde(default)]
    pub unbound_interrupts: Vec<UnboundInterrupt>,
    #[serde(default)]
    pub shared_controls: Vec<SharedControl>,
    #[serde(default)]
    pub gate_groups: Vec<GateGroup>,
    #[serde(default)]
    pub clock_sources: Vec<ClockSource>,
    #[serde(default)]
    pub analog_i2c: AnalogSpace,
    #[serde(default)]
    pub ram_regions: Vec<RamRegion>,
    #[serde(default)]
    pub layout_placements: Vec<LayoutPlacement>,
}

/// One MMIO instance of a type.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Instance {
    pub name: String,
    #[serde(rename = "type")]
    pub block_type: String,
    pub base: u64,
    pub extent: Extent,
    pub sources: Vec<String>,
    #[serde(default)]
    pub clocks: Vec<Control>,
    #[serde(default)]
    pub resets: Vec<Control>,
    #[serde(default)]
    pub muxes: Vec<Mux>,
}

/// A fixed gate or reset link to an expanded MMIO field identity.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Control {
    pub role: String,
    pub field: String,
    pub sources: Vec<String>,
}

/// A selector and its sourced input encodings. Unlisted encodings are unknown.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Mux {
    pub role: String,
    pub field: String,
    pub inputs: BTreeMap<String, String>,
    pub sources: Vec<String>,
}

/// One field shared by exactly these distinct MMIO instances.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SharedControl {
    pub name: String,
    pub field: String,
    pub instances: Vec<String>,
    pub sources: Vec<String>,
}

/// A vendor's fixed group of gate bits, independent of its consumers.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct GateGroup {
    pub name: String,
    pub fields: Vec<String>,
    pub sources: Vec<String>,
}

/// A clock-tree source; only known frequency and parent facts are stated.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ClockSource {
    pub name: String,
    #[serde(default)]
    pub frequency_hz: Option<u64>,
    #[serde(default)]
    pub parent: Option<String>,
    pub sources: Vec<String>,
}

/// All interrupt numbers are in `0..count`, each bound or explicitly unbound.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct InterruptSources {
    pub count: u32,
    pub sources: Vec<String>,
}

/// A source whose hardware owner or output is not established.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct UnboundInterrupt {
    pub source: u16,
    pub name: String,
    pub reason: String,
    pub sources: Vec<String>,
}

/// The size of an instance's window; a `partial` one is the part the
/// sources show, with its own sources.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Extent {
    pub size: u64,
    #[serde(default)]
    pub partial: bool,
    #[serde(default)]
    pub sources: Vec<String>,
}

/// One interrupt source of the chip, bound to an instance.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Interrupt {
    pub name: String,
    pub source: u16,
    pub instance: String,
    #[serde(default)]
    pub output: Option<String>,
    pub sources: Vec<String>,
}

/// The analog-I2C address space.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AnalogSpace {
    #[serde(default)]
    pub instances: Vec<AnalogInstance>,
}

/// One analog-I2C block, addressed by its code.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct AnalogInstance {
    pub name: String,
    #[serde(rename = "type")]
    pub block_type: String,
    pub code: u8,
    pub sources: Vec<String>,
    #[serde(default)]
    pub routes: Vec<crate::analog::Route>,
}

/// A RAM region memory layouts are placed in.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RamRegion {
    pub name: String,
    pub base: u64,
    pub size: u64,
    pub sources: Vec<String>,
}

/// The RAM regions one memory layout lies in.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LayoutPlacement {
    pub layout: String,
    pub regions: Vec<String>,
}

//! The difference in memory-mapped behavior between two builds of the same
//! firmware: what each function reads and writes inside the given address
//! windows, compared function by function. Symbol hashes, code layout,
//! branch offsets, the addresses of loaded data and the stored addresses
//! sized symbols contain are not part of it.
use crate::*;

/// Where an MMIO access lands.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MmioLocation {
    /// One exact word.
    At { address: u32 },
    /// An element of a register array the code selects at run time.
    Indexed { progression: IndexedAddress },
}

/// What an MMIO observation records.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MmioOperation {
    Load,
    Store,
    LoadReserved,
    StoreConditional,
    Atomic,
    FloatLoad,
    FloatStore,
    /// The bits a loaded register value keeps: its read mask.
    ReadSelection,
}

/// Where a run of stored bits comes from, without what differs between
/// builds of unchanged code.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum MmioStoredSource {
    /// Fixed bits; `value` is the run's bits, right-aligned. A whole word
    /// that lies in a sized symbol the build defines, such as a DMA
    /// descriptor or a handler, names it: the same object moved between
    /// builds stores another value at the same symbol and offset.
    Constant {
        value: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        symbol: Option<MmioSymbolOffset>,
    },
    /// Bits `low..` of a register's value at the function's entry.
    EntryRegister {
        register: u8,
        low: u8,
    },
    /// Bits `low..` of a loaded value: `address` only when it is in a window.
    Load {
        address: Option<u32>,
        width: u8,
        low: u8,
        same_word: bool,
    },
    Unknown,
}

/// An address as a symbol's stable name and the offset into it.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioSymbolOffset {
    pub name: String,
    pub offset: u32,
}

/// One run of a stored value's bits.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioStoredBits {
    pub low: u8,
    pub width: u8,
    pub source: MmioStoredSource,
}

/// One MMIO access of a function, without what differs between builds of
/// unchanged code: a stored run loaded from outside the windows keeps its
/// width and position but not its data address.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioAccess {
    pub location: MmioLocation,
    pub operation: MmioOperation,
    /// Bytes accessed; absent for a read selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<RegisterMask>,
    /// For a store: where each run of the stored bits comes from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stored: Vec<MmioStoredBits>,
}

/// Code of a build that was not analyzed, and why.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioGap {
    /// The build's input, by position among its `ROLE=PATH`s.
    pub input: u64,
    /// The object, when the gap is one object of the input.
    pub object: Option<ObjectId>,
    pub reason: String,
}

/// What a build knows of a function.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MmioFunctionState {
    /// Analyzed: its accesses are known.
    Analyzed,
    /// Defined but not analyzed: its accesses are unknown.
    Blocked,
    /// Not defined.
    Absent,
}

/// How one function's MMIO behavior differs between the builds. A function
/// one build lacks is all added or all removed; one a build blocks has no
/// accesses compared, only its state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioFunctionChange {
    /// The demangled name without crate disambiguators.
    pub function: String,
    /// What each build knows of it: old, new.
    pub state: [MmioFunctionState; 2],
    /// Whether each build's analysis of it is complete: an access missing
    /// from an incomplete one may only be unseen. A build that blocks or
    /// lacks the function has no complete analysis of it.
    pub complete: [bool; 2],
    pub removed: Vec<MmioAccess>,
    pub added: Vec<MmioAccess>,
    /// Accesses outside the stack whose address the analysis does not
    /// resolve, in each build: where the comparison sees less.
    pub unresolved: [u64; 2],
}

/// A function no difference is seen in, although a build blocks it or
/// analyzes it incompletely: an access may be unseen.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioUncertainFunction {
    pub function: String,
    /// What each build knows of it: old, new.
    pub state: [MmioFunctionState; 2],
    /// Whether each build's analysis of it is complete.
    pub complete: [bool; 2],
}

/// The MMIO difference of two builds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmioDiff {
    pub ranges: Vec<ImageRegion>,
    /// Functions with MMIO accesses in each build: old, new.
    pub functions: [u64; 2],
    /// Functions whose accesses differ, whose unresolved count does, or
    /// whose state (analyzed, blocked, absent) does where that hides or
    /// changes MMIO behavior, in name order.
    pub changes: Vec<MmioFunctionChange>,
    /// Functions both builds define that `changes` does not list but whose
    /// sameness is not shown: blocked or incompletely analyzed in a build,
    /// in name order.
    pub uncertain: Vec<MmioUncertainFunction>,
    /// Functions each build blocks, old and new: their accesses are unknown.
    pub blocked: [u64; 2],
    /// Why code of each build was not analyzed, old and new: code no
    /// function is selected or analyzed from, or facts a build lacks.
    pub gaps: [Vec<MmioGap>; 2],
}

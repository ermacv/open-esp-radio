//! Function analysis values and ISA port, with explicit object/address identity.
use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CodeAddressSpace {
    Section,
    Image,
}

/// Immutable load bytes in the selected image's virtual address space. The
/// caller owns the borrowed view. Mutable, unmapped and ambiguous bytes are unknown.
pub trait ImageMemory {
    fn read_constant(
        &self,
        address: u32,
        bytes: &mut [u8],
        control: &mut dyn RunControl,
    ) -> Result<bool>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeRange {
    pub start: u64,
    pub length: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FunctionCoverage {
    pub decoding: bool,
    pub control_flow: bool,
    pub references: bool,
}
impl Default for FunctionCoverage {
    fn default() -> Self {
        Self {
            decoding: true,
            control_flow: true,
            references: true,
        }
    }
}
impl FunctionCoverage {
    pub fn complete(self) -> bool {
        self.decoding && self.control_flow && self.references
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceTarget {
    pub binding: u8,
    pub definition: SymbolDefinition,
    pub symbol: SymbolId,
    #[serde(with = "crate::symbol_name")]
    pub name: Vec<u8>,
    pub section: Option<u32>,
    pub offset: u64,
    pub symbol_type: u8,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionRelocation {
    pub section: u32,
    pub index: u64,
    pub offset: u64,
    pub relocation_type: u32,
    pub addend: Option<i64>,
    pub target: std::sync::Arc<ReferenceTarget>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReferenceKind {
    Call,
    Branch,
    Address,
    Metadata,
    Unknown,
}
#[derive(Clone, Debug)]
pub struct NormalizedReference {
    pub kind: ReferenceKind,
    pub target: std::sync::Arc<ReferenceTarget>,
    pub addend: Option<i64>,
    pub paired: Option<(u32, u64)>,
    pub known: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
/// `link` identifies the ABI link registers x1/x5, not every nonzero destination.
pub enum InstructionFlow {
    Next,
    Branch {
        displacement: i32,
    },
    Jump {
        displacement: i32,
        link: bool,
    },
    /// A jump through a register that is not a return.
    Indirect {
        base: u8,
        offset: i32,
        link: bool,
    },
    /// A return to the caller: a jump through the link register `base` at
    /// offset zero that links nothing, as the ISA's return-address-stack
    /// hints define it. The decoder marks it, so no consumer tests registers.
    Return {
        base: u8,
    },
    Stop,
}

impl InstructionFlow {
    /// The base register, offset and link of a jump through a register, a
    /// return included.
    pub const fn through_register(self) -> Option<(u8, i32, bool)> {
        match self {
            Self::Indirect { base, offset, link } => Some((base, offset, link)),
            Self::Return { base } => Some((base, 0, false)),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecodedOp {
    pub length: u8,
    pub text: String,
    pub flow: InstructionFlow,
}
/// Structural classification when the semantic decoder does not model an encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedFlow {
    NonControl,
    Indirect,
    Unknown,
}
/// Receives captured bytes/structural records only. No project, filesystem or discovery authority.
pub trait FunctionDecoder {
    fn identity(&self) -> &'static str;
    /// Unknown is mandatory unless the ISA proves the encoding's control class.
    fn unsupported_flow(&self, _bytes: &[u8]) -> UnsupportedFlow {
        UnsupportedFlow::Unknown
    }
    fn decode(&self, bytes: &[u8]) -> Option<DecodedOp>;
    /// `all` is the complete section table sorted by offset, preserving physical IDs.
    fn reference(
        &self,
        relocation: &FunctionRelocation,
        all: &[FunctionRelocation],
        section: u32,
        control: &mut dyn RunControl,
    ) -> Result<NormalizedReference>;
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FunctionRecord {
    Fence {
        offset: u64,
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
    Condition {
        offset: u64,
        test: BranchTest,
        left: AbstractValue,
        right: AbstractValue,
    },
    Expression {
        id: u32,
        offset: u64,
        expression: Expression,
    },
    /// Register state before the transfer instruction: a call or tail call,
    /// a jump to a target outside the function, an image's jump to local
    /// targets the graph does not expand, and a relocatable object's jump
    /// the graph leaves without a target. A table dispatch is a `Dispatch`
    /// record. Its architectural write is a separate `Value` fact and must
    /// precede callee-entry substitution.
    CallInputs {
        offset: u64,
        registers: Vec<AbstractValue>,
    },
    ReturnValue {
        offset: u64,
        low: AbstractValue,
        high: AbstractValue,
    },
    /// An indirect jump through a table: its target is the word loaded from
    /// `base + (index << scale)`, the load, the addition and the scaling all
    /// in the jump's own block. `base` is the value the addition reads, a
    /// constant, an image address or an object's symbol; `index` the value
    /// the scaling reads, possibly unknown. A base the block or a loop's
    /// preheader forms gives the same record on every pass.
    Dispatch {
        offset: u64,
        base: AbstractValue,
        index: AbstractValue,
        scale: u8,
    },
    /// A call or out-of-function jump. A resolved target does not prove returns
    /// or effects; unresolved transfers remain visible.
    Transfer {
        offset: u64,
        target: AbstractValue,
        call: bool,
    },
    Value {
        offset: u64,
        register: u8,
        value: AbstractValue,
        relocation: Option<RelocationSite>,
    },
    MemoryAccess {
        offset: u64,
        access: MemoryKind,
        width: u8,
        address: AbstractValue,
        /// Value written, absent for reads. Unknown for an unmodeled atomic result.
        value: Option<AbstractValue>,
        relocation: Option<RelocationSite>,
    },
    SemanticGap {
        offset: u64,
        reason: SemanticGapReason,
    },
    Instruction {
        offset: u64,
        bytes: Vec<u8>,
        decoded: DecodedOp,
    },
    Block {
        id: u64,
        start: u64,
        end: u64,
    },
    Edge {
        from: u64,
        target: Option<u64>,
        relation: EdgeKind,
        external: bool,
    },
    Reference {
        raw: Box<FunctionRelocation>,
        reference_kind: ReferenceKind,
        target: std::sync::Arc<ReferenceTarget>,
        addend: Option<i64>,
        paired: Option<(u32, u64)>,
        known: bool,
    },
    Gap {
        start: u64,
        length: u64,
        reason: GapReason,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    Taken,
    Fallthrough,
    Jump,
    Call,
    PossibleContinuation,
    Indirect,
    Return,
    Stop,
    Conflict,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GapReason {
    Unvisited,
    UnsupportedInstruction,
    ConflictingBoundary,
    DeclaredData,
}
pub trait FunctionSink {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolDefinition {
    Null,
    Undefined,
    Section,
    Absolute,
    Common,
    Other,
}

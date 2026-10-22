//! Register-access observations are separate from physical hardware declarations.
use crate::*;

/// For a load: what happens to each bit of the loaded value inside the function.
/// Every bit of the access width is in exactly one class:
/// used > unknown > preserved > unused.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadUses {
    /// Bits that reach at least one sink.
    pub uses: Vec<ReadUse>,
    /// Bits not followed to the end, with the reason.
    pub unknown: Vec<ReadLimit>,
    /// Only back into the same word at the same position (RMW keeps them).
    pub preserved: u32,
    /// Proven to reach no sink.
    pub unused: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadUse {
    pub bits: u32,
    pub sink: ReadSink,
    pub precision: ReadPrecision,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ReadSink {
    /// Condition operand.
    Branch { site: u64 },
    /// Jump-table index.
    Dispatch { site: u64 },
    /// Indirect call or jump target.
    IndirectTarget { site: u64 },
    /// Address of another memory access.
    Address { site: u64 },
    /// Value at return.
    Return { register: u8 },
    /// Another word, or the same word at another position / after a
    /// non-identity transformation.
    Store {
        site: u64,
        address: Option<u32>,
        same_word: bool,
    },
    CallArgument {
        site: u64,
        register: u8,
        callee: Option<CallTarget>,
    },
}

/// The callee of a call or tail call, as far as the analysis resolves it.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallTarget {
    /// The target address when the transfer resolves to one (ROM included).
    pub address: Option<u32>,
    /// The defining function among the analyzed inputs, when it is one.
    pub function: Option<LibraryFunction>,
    /// The symbol at the target (an input's or the ROM's), when one names it.
    pub name: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadPrecision {
    /// Bit-for-bit flow: moves, and/or/xor with constants, shifts by a constant.
    Exact,
    /// Through arithmetic or comparison.
    May,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadLimit {
    pub bits: u32,
    pub limit: ReadLimitKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadLimitKind {
    PartialFunction,
    StackSpill,
    UnsupportedOperation,
    WorkLimit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RegisterMaskKind {
    /// Bits retained by a saved load-and-constant expression.
    ReadSelection,
    /// Stored bits not copied identically from the same word, in a write
    /// that preserves at least one bit of that word at its original position.
    /// This is an expression observation, not a claim of hardware RMW safety.
    WriteReplacement,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterMask {
    pub kind: RegisterMaskKind,
    pub bits: u32,
}
/// One run of bits of a stored value with a common source.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredBits {
    /// The run's lowest bit in the stored value.
    pub low: u8,
    pub width: u8,
    pub source: StoredBitsSource,
}

/// Where a run of stored bits comes from. These are expression observations
/// of one store, not a claim that the path reaching it runs.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum StoredBitsSource {
    /// Fixed bits; `value` is the run's bits, right-aligned.
    Constant { value: u32 },
    /// Bits `low..` of a register's value at the function's entry: an
    /// argument for `a0`..`a7`.
    EntryRegister { register: u8, low: u8 },
    /// Bits `low..` of a value the function loaded; `address` when it is one
    /// exact address, and `same_word` when that is the stored word itself
    /// (bits a read-modify-write keeps).
    Load {
        address: Option<u32>,
        width: u8,
        low: u8,
        same_word: bool,
    },
    /// No exact source.
    Unknown,
}

/// An address the analysis computes as `base + stride * index` from one
/// unknown index, every step exact modulo 2^32: an array element the code
/// selects at run time. Each address of the progression is a may-address; no
/// index value is claimed to occur.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedAddress {
    /// The address at index zero.
    pub base: u32,
    /// The distance between consecutive indices; never zero.
    pub stride: i32,
    /// The index lies in `0..count` when its expression proves that bound
    /// (a mask, a zero-extending load, an unsigned remainder or right
    /// shift); `None` when any 32-bit index is possible.
    pub count: Option<u32>,
}

impl IndexedAddress {
    /// The address at `index`.
    pub fn at(&self, index: u32) -> u32 {
        self.base
            .wrapping_add((self.stride as u32).wrapping_mul(index))
    }

    /// Whether some index inside the proven bound reaches the 32-bit word
    /// at `word`: one of its four bytes is the address at that index. An
    /// unbounded index wraps modulo 2^32, so it reaches every word its
    /// stride's power of two allows: a selection never hides a may-access.
    pub fn may_reach_word(&self, word: u32) -> bool {
        let word = word & !3;
        (0..4).any(|byte| self.index_of(word.wrapping_add(byte)).is_some())
    }

    /// The smallest index whose address is `address`, inside the bound.
    pub fn index_of(&self, address: u32) -> Option<u32> {
        let stride = self.stride as u32;
        let distance = address.wrapping_sub(self.base);
        // `stride * index == distance (mod 2^32)`: with `stride = 2^k * odd`,
        // a solution exists only when `2^k` divides the distance, and then
        // the smallest one is `(distance / 2^k) * odd^-1` modulo `2^(32-k)`.
        let shift = stride.trailing_zeros();
        if shift >= 32 || distance.trailing_zeros() < shift {
            return None;
        }
        let odd = stride >> shift;
        let modulus_mask = u32::MAX >> shift;
        let index = (distance >> shift).wrapping_mul(inverse(odd)) & modulus_mask;
        match self.count {
            Some(count) if index >= count => None,
            _ => Some(index),
        }
    }
}

/// The multiplicative inverse of an odd `value` modulo 2^32.
fn inverse(value: u32) -> u32 {
    // Newton's iteration doubles the correct low bits each step from the
    // three an odd number is its own inverse in.
    let mut inverse = value;
    for _ in 0..4 {
        inverse = inverse.wrapping_mul(2u32.wrapping_sub(value.wrapping_mul(inverse)));
    }
    inverse
}

/// A function one captured library input defines.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryFunction {
    /// Position of the defining input among the analyzed inputs.
    pub input: u64,
    pub symbol: SymbolId,
    #[serde(with = "oer_analysis_riscv_model::symbol_name::option")]
    pub name: Option<Vec<u8>>,
}

/// One outcome of analyzing every function of captured libraries for the
/// memory addresses they access, in input, object and symbol order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum RegisterAccess {
    /// One candidate address an analyzed function's record accesses.
    Observation {
        function: LibraryFunction,
        record: u64,
        fact: Box<FunctionRecord>,
        /// None retains an unresolved or nonnumeric address; never silently absent.
        address: Option<u32>,
        alternative: Option<u8>,
        /// For an unresolved address computed from one unknown index: the
        /// array progression it selects from.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        indexed: Option<IndexedAddress>,
        mask: Option<RegisterMask>,
        /// For a store: where each run of the stored bits comes from, low run
        /// first, covering the access width. Absent for loads.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stored: Option<Vec<StoredBits>>,
        /// For a load: uses, limits, RMW preservation and unused bits.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reads: Option<ReadUses>,
    },
    /// A function that cannot be analyzed; its accesses are unknown.
    Blocked {
        function: LibraryFunction,
        error: Error,
    },
    /// Code no function is selected or analyzed from.
    Gap {
        input: u64,
        object: Option<ObjectId>,
        reason: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureAuditIncomplete {
    pub function: LibraryFunction,
    pub reason: Error,
}

/// One Exact source-to-sink contradiction, retained even when other functions conflict.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureConflict {
    pub function: LibraryFunction,
    pub signature: String,
    pub declaration: Option<String>,
    pub origin: SignatureOrigin,
    pub usage: ReadUse,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SignatureOrigin {
    Argument {
        register: u8,
    },
    Result {
        site: u64,
        register: u8,
        callee: Option<CallTarget>,
    },
}
impl SignatureConflict {
    pub fn description(&self) -> String {
        let source = match &self.origin {
            SignatureOrigin::Argument { register } => {
                format!("entry a{}", register.saturating_sub(10))
            }
            SignatureOrigin::Result { site, register, .. } => format!(
                "absent result a{} of call +{site:#x}",
                register.saturating_sub(10)
            ),
        };
        format!(
            "function {}: signature {} ({:?}): {source} -> {:?}, bits {:#010x}, Exact",
            String::from_utf8_lossy(self.function.name.as_deref().unwrap_or(b"?")),
            self.signature,
            self.declaration,
            self.usage.sink,
            self.usage.bits
        )
    }
}

/// Why analyzed functions are partial. Each count is the number of partial
/// functions with that cause, so a function with several counts under each;
/// every partial function has at least one, `other` when none of the named
/// causes explains it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialCauses {
    /// Code the decoder could not read.
    pub decoding: u64,
    /// Control flow the graph could not follow: an unexpanded indirect jump
    /// or conflicting instruction boundaries.
    pub control_flow: u64,
    /// Relocations the analysis could not resolve.
    pub references: u64,
    /// Calls whose callee is not analyzed: registers and memory after them
    /// are unknown. Common, and no gap in the function's own code.
    pub opaque_calls: u64,
    /// Values whose alternatives exceeded their bound.
    pub value_limits: u64,
    /// Partial for no named cause.
    pub other: u64,
}

impl PartialCauses {
    /// Add `other`'s counts.
    pub fn add(&mut self, other: PartialCauses) {
        self.decoding += other.decoding;
        self.control_flow += other.control_flow;
        self.references += other.references;
        self.opaque_calls += other.opaque_calls;
        self.value_limits += other.value_limits;
        self.other += other.other;
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterAccessSummary {
    pub schema: u32,
    /// Distinct source bits per selected load with a sink removed directly by a signature.
    pub contract_narrowed_bits: u64,
    /// Distinct source bits per selected load whose computed May argument use was removed.
    pub contract_may_narrowed_bits: u64,
    pub signature_conflicts: Vec<SignatureConflict>,
    /// Audits that could not reach a fixed point within the existing resource bounds.
    pub signature_audit_incomplete: Vec<SignatureAuditIncomplete>,
    /// Explicit candidate intervals; empty selects every numeric address.
    pub ranges: Vec<ImageRegion>,
    pub functions: u64,
    /// Analyzed functions whose coverage or value semantics are incomplete.
    pub partial_functions: u64,
    /// `partial_functions` by cause.
    pub partial_causes: PartialCauses,
    pub blocked_functions: u64,
    pub gaps: u64,
    pub observations: u64,
    pub unresolved_addresses: u64,
    /// Unresolved addresses with an indexed progression.
    pub indexed_addresses: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_indexed_address_finds_the_index_of_each_reachable_address() {
        let descending = IndexedAddress {
            base: 0x2010_54fc,
            stride: -0x7c,
            count: None,
        };
        assert_eq!(descending.at(3), 0x2010_5388);
        assert_eq!(descending.index_of(0x2010_5388), Some(3));
        // An unbounded index wraps: every address its stride's power of two
        // divides the distance to is reachable, here every word.
        let wrapped = descending.index_of(0x2010_538c).unwrap();
        assert_eq!(descending.at(wrapped), 0x2010_538c);
        let queues = IndexedAddress {
            count: Some(8),
            ..descending
        };
        assert_eq!(queues.index_of(0x2010_538c), None, "between elements");
        assert!(queues.may_reach_word(0x2010_5388));
        assert!(!queues.may_reach_word(0x2010_538c));
        // An even stride reaches only addresses its power of two divides.
        let even = IndexedAddress {
            base: 0x100,
            stride: 12,
            count: Some(4),
        };
        assert_eq!(even.index_of(0x124), Some(3));
        assert_eq!(even.index_of(0x130), None, "index four is out of bound");
        assert_eq!(even.index_of(0x106), None);
        // An odd stride reaches every address within an unbounded index.
        let odd = IndexedAddress {
            base: 0,
            stride: 3,
            count: None,
        };
        assert_eq!(odd.at(odd.index_of(0x2010_0001).unwrap()), 0x2010_0001);
    }
}

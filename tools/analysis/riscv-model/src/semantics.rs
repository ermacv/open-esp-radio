//! Typed local semantics. No machine state, memory environment or repository access.
use crate::*;

/// RV32 operations; register operands are always in 0..32.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operand {
    Register(u8),
    Immediate(u32),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IntegerOp {
    Add,
    Sub,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    Sar,
    Lt,
    Ltu,
    Mul,
    Mulh,
    Mulhsu,
    Mulhu,
    Div,
    Divu,
    Rem,
    Remu,
    /// Zba `shNadd`: the left operand shifted by N, plus the right.
    ShiftAdd1,
    ShiftAdd2,
    ShiftAdd3,
    /// Zbb logic with an inverted right operand.
    AndNot,
    OrNot,
    XorNot,
    Min,
    Minu,
    Max,
    Maxu,
    RotateLeft,
    RotateRight,
    /// Zbb unary operations; the right operand is ignored.
    CountLeadingZeros,
    CountTrailingZeros,
    PopCount,
    SignExtendByte,
    SignExtendHalf,
    OrCombineBytes,
    ByteReverse,
    /// Zbs single-bit operations on the bit the right operand indexes.
    BitSet,
    BitClear,
    BitInvert,
    BitExtract,
}

impl IntegerOp {
    /// The RV32 result of this operation on two concrete words.
    pub fn evaluate(self, a: u32, b: u32) -> u32 {
        use IntegerOp::*;
        let (sa, sb) = (a as i32, b as i32);
        let bit = 1u32.wrapping_shl(b & 31);
        match self {
            Add => a.wrapping_add(b),
            Sub => a.wrapping_sub(b),
            And => a & b,
            Or => a | b,
            Xor => a ^ b,
            Shl => a.wrapping_shl(b & 31),
            Shr => a.wrapping_shr(b & 31),
            Sar => (sa >> (b & 31)) as u32,
            Lt => u32::from(sa < sb),
            Ltu => u32::from(a < b),
            Mul => a.wrapping_mul(b),
            Mulh => ((i64::from(sa) * i64::from(sb)) >> 32) as u32,
            Mulhsu => ((i64::from(sa) * i64::from(b)) >> 32) as u32,
            Mulhu => ((u64::from(a) * u64::from(b)) >> 32) as u32,
            Div => {
                if b == 0 {
                    u32::MAX
                } else {
                    sa.wrapping_div(sb) as u32
                }
            }
            Divu => a.checked_div(b).unwrap_or(u32::MAX),
            Rem => {
                if b == 0 {
                    a
                } else {
                    sa.wrapping_rem(sb) as u32
                }
            }
            Remu => {
                if b == 0 {
                    a
                } else {
                    a % b
                }
            }
            ShiftAdd1 => (a << 1).wrapping_add(b),
            ShiftAdd2 => (a << 2).wrapping_add(b),
            ShiftAdd3 => (a << 3).wrapping_add(b),
            AndNot => a & !b,
            OrNot => a | !b,
            XorNot => !(a ^ b),
            Min => sa.min(sb) as u32,
            Minu => a.min(b),
            Max => sa.max(sb) as u32,
            Maxu => a.max(b),
            RotateLeft => a.rotate_left(b & 31),
            RotateRight => a.rotate_right(b & 31),
            CountLeadingZeros => a.leading_zeros(),
            CountTrailingZeros => a.trailing_zeros(),
            PopCount => a.count_ones(),
            SignExtendByte => a as i8 as i32 as u32,
            SignExtendHalf => a as i16 as i32 as u32,
            OrCombineBytes => {
                u32::from_le_bytes(a.to_le_bytes().map(|b| if b == 0 { 0 } else { 0xff }))
            }
            ByteReverse => a.swap_bytes(),
            BitSet => a | bit,
            BitClear => a & !bit,
            BitInvert => a ^ bit,
            BitExtract => (a >> (b & 31)) & 1,
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemoryKind {
    Load,
    Store,
    LoadReserved,
    StoreConditional,
    Atomic,
    /// A word loaded into a floating-point register: no integer destination.
    FloatLoad,
    /// A floating-point register's word stored: no integer source.
    FloatStore,
}
/// An instruction's bounded local effect, before abstract interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticOp {
    Fence {
        fm: u8,
        predecessor: u8,
        successor: u8,
    },
    Integer {
        op: IntegerOp,
        dest: u8,
        left: Operand,
        right: Operand,
    },
    Upper {
        dest: u8,
        value: u32,
        pc_relative: bool,
    },
    Link {
        dest: u8,
    },
    Memory {
        kind: MemoryKind,
        base: u8,
        displacement: i32,
        width: u8,
        dest: Option<u8>,
        source: Option<u8>,
        swap: bool,
        signed: bool,
    },
    /// `dest` takes a value outside the integer model, such as a
    /// floating-point comparison, move or conversion result.
    Opaque {
        dest: u8,
    },
    None,
    Unsupported,
}

impl SemanticOp {
    /// Whether a described input names `register`, or the operation is
    /// `Unsupported` (which conservatively may read any register).
    ///
    /// This is not a complete machine-instruction read set: `Link` omits
    /// the indirect jump's base, `None` can represent a branch, and `Opaque`
    /// can omit inputs. A false result alone cannot prove a register unused;
    /// callers must retain their ISA and control-flow checks. Register zero
    /// is treated like any other explicitly named operand.
    pub fn reads_register(self, register: u8) -> bool {
        match self {
            Self::Integer { left, right, .. } => {
                [left, right].contains(&Operand::Register(register))
            }
            Self::Memory { base, source, .. } => base == register || source == Some(register),
            Self::Unsupported => true,
            Self::Fence { .. }
            | Self::Upper { .. }
            | Self::Link { .. }
            | Self::Opaque { .. }
            | Self::None => false,
        }
    }

    /// The described integer destination, including `x0`, even for an
    /// `Opaque` value. Callers decide whether to discard writes to zero.
    ///
    /// `None` means no destination is described, not that the machine
    /// instruction writes nothing. In particular, `Unsupported` must still
    /// invalidate proofs that require known effects.
    pub fn destination_register(self) -> Option<u8> {
        match self {
            Self::Integer { dest, .. }
            | Self::Upper { dest, .. }
            | Self::Link { dest }
            | Self::Opaque { dest } => Some(dest),
            Self::Memory { dest, .. } => dest,
            Self::Fence { .. } | Self::None | Self::Unsupported => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueRelocation {
    Ignore,
    UpperAbsolute,
    UpperPcRelative,
    CallUpper,
    LowerAbsolute,
    LowerPcRelative,
    Unsupported,
}
/// Extends structural decoding without adding filesystem or execution authority.
pub trait FunctionSemantics: FunctionDecoder {
    fn branch(&self, _bytes: &[u8]) -> Option<(BranchTest, Operand, Operand)> {
        None
    }
    fn semantic_identity(&self) -> &'static str;
    fn lift(&self, bytes: &[u8]) -> SemanticOp;
    fn value_relocation(
        &self,
        relocation: &FunctionRelocation,
        operation: SemanticOp,
    ) -> ValueRelocation;
}
/// Values are relative to this recipe's input/object unless an exact symbol is supplied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AbstractValue {
    Unknown,
    Alternatives { values: ValueAlternatives },
    Expression { id: u32 },
    Constant { value: u32 },
    ImageAddress { address: u32 },
    Section { section: u32, offset: i64 },
    Symbol { symbol: SymbolId, addend: i64 },
    EntryStack { offset: i64 },
}
/// Maximum exact alternatives retained by the finite-value profile.
pub const MAX_VALUE_ALTERNATIVES: usize = 8;
/// Nonrecursive exact leaves. Alternatives never contain expressions or other sets.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ValueAlternative {
    Constant { value: u32 },
    ImageAddress { address: u32 },
    Section { section: u32, offset: i64 },
    Symbol { symbol: SymbolId, addend: i64 },
    EntryStack { offset: i64 },
}
impl ValueAlternative {
    pub fn as_value(&self) -> AbstractValue {
        match self {
            Self::Constant { value } => AbstractValue::Constant { value: *value },
            Self::ImageAddress { address } => AbstractValue::ImageAddress { address: *address },
            Self::Section { section, offset } => AbstractValue::Section {
                section: *section,
                offset: *offset,
            },
            Self::Symbol { symbol, addend } => AbstractValue::Symbol {
                symbol: symbol.clone(),
                addend: *addend,
            },
            Self::EntryStack { offset } => AbstractValue::EntryStack { offset: *offset },
        }
    }
    pub fn from_value(value: &AbstractValue) -> Option<Self> {
        Some(match value {
            AbstractValue::Constant { value } => Self::Constant { value: *value },
            AbstractValue::ImageAddress { address } => Self::ImageAddress { address: *address },
            AbstractValue::Section { section, offset } => Self::Section {
                section: *section,
                offset: *offset,
            },
            AbstractValue::Symbol { symbol, addend } => Self::Symbol {
                symbol: symbol.clone(),
                addend: *addend,
            },
            AbstractValue::EntryStack { offset } => Self::EntryStack { offset: *offset },
            _ => return None,
        })
    }
}
/// Canonical, bounded set; serde rejects oversized, singleton and noncanonical sets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Hash)]
#[serde(transparent)]
pub struct ValueAlternatives(Vec<ValueAlternative>);
impl ValueAlternatives {
    pub fn new(mut values: Vec<ValueAlternative>) -> Result<Self> {
        if values.len() > MAX_VALUE_ALTERNATIVES {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "too many value alternatives",
            ));
        }
        values.sort_unstable();
        values.dedup();
        if values.len() < 2 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "alternatives require at least two distinct values",
            ));
        }
        Ok(Self(values))
    }
    pub fn values(&self) -> &[ValueAlternative] {
        &self.0
    }
    pub fn allocated_bytes(&self) -> u64 {
        (self.0.capacity() * std::mem::size_of::<ValueAlternative>()) as u64
            + self
                .0
                .iter()
                .map(|v| match v {
                    ValueAlternative::Symbol { symbol, .. } => {
                        symbol.object.artifact.allocated_bytes()
                    }
                    _ => 0,
                })
                .sum::<u64>()
    }
}
impl<'de> Deserialize<'de> for ValueAlternatives {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = ValueAlternatives;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("2..=8 sorted distinct exact alternatives")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                let mut values = Vec::with_capacity(MAX_VALUE_ALTERNATIVES);
                while let Some(value) = seq.next_element::<ValueAlternative>()? {
                    if values.len() == MAX_VALUE_ALTERNATIVES
                        || values.last().is_some_and(|v| v >= &value)
                    {
                        return Err(serde::de::Error::custom(
                            "oversized or noncanonical alternatives",
                        ));
                    }
                    values.push(value);
                }
                if values.len() < 2 {
                    return Err(serde::de::Error::custom("alternatives require two values"));
                }
                Ok(ValueAlternatives(values))
            }
        }
        deserializer.deserialize_seq(Visitor)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelocationSite {
    pub section: u32,
    pub index: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SemanticGapReason {
    UnsupportedInstruction,
    OpaqueCall,
    ConflictingBoundary,
    UnresolvedRelocation,
    UnexpandedControlFlow,
    AlternativeLimit,
}
/// None in an old manifest means analysis was not performed, not zero coverage.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticSummary {
    pub complete: bool,
    pub values: u64,
    pub known_values: u64,
    pub accesses: u64,
    pub known_addresses: u64,
    pub gaps: u64,
}

/// Explicit assumption about integer calling conventions; not inferred from ELF flags.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CallAbi {
    RiscvInteger,
}

impl CallAbi {
    /// Standard RV32 integer calling convention (not the reduced-register ABI).
    pub const fn stack_alignment(self) -> u32 {
        match self {
            Self::RiscvInteger => 16,
        }
    }
    /// The integer registers that carry a call's first argument words, in
    /// order (`a0` to `a7`).
    pub const fn arguments(self) -> [u8; 8] {
        match self {
            Self::RiscvInteger => [10, 11, 12, 13, 14, 15, 16, 17],
        }
    }

    /// The integer registers that carry a call's result (`a0`, `a1`).
    pub const fn results(self) -> [u8; 2] {
        match self {
            Self::RiscvInteger => [10, 11],
        }
    }

    /// The integer registers a callee preserves (`sp`, `gp`, `tp` and
    /// `s0` to `s11`).
    pub const fn preserved(self) -> [u8; 15] {
        match self {
            Self::RiscvInteger => [2, 3, 4, 8, 9, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27],
        }
    }

    /// The integer registers a call may clobber: every one but `zero` and
    /// the preserved ones.
    pub const fn clobbered(self) -> [u8; 16] {
        match self {
            Self::RiscvInteger => [1, 5, 6, 7, 10, 11, 12, 13, 14, 15, 16, 17, 28, 29, 30, 31],
        }
    }

    /// The argument word `register` carries, if it carries one.
    pub fn argument_word(self, register: u8) -> Option<usize> {
        self.arguments().iter().position(|&r| r == register)
    }
}
/// Flat DAG. References must point to earlier records in the same function.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Expression {
    EntryRegister {
        register: u8,
    },
    Integer {
        op: IntegerOp,
        left: AbstractValue,
        right: AbstractValue,
    },
    Load {
        address: AbstractValue,
        width: u8,
        signed: bool,
    },
    CallResult {
        callsite: u64,
        register: u8,
    },
    /// A word captured by a checked normal-return memory postcondition.
    /// The source names the callee's load-time memory, never entry contents.
    CallWord {
        callsite: u64,
        owner: u64,
        load: u64,
        load_order: u32,
        store: u64,
        store_order: u32,
        width: u8,
        source: AbstractValue,
    },
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BranchTest {
    Eq,
    Ne,
    Lt,
    Ge,
    Ltu,
    Geu,
}

impl BranchTest {
    /// The RV32 predicate on two concrete words, with the instruction's signedness.
    pub fn evaluate(self, left: u32, right: u32) -> bool {
        match self {
            Self::Eq => left == right,
            Self::Ne => left != right,
            Self::Lt => (left as i32) < (right as i32),
            Self::Ge => (left as i32) >= (right as i32),
            Self::Ltu => left < right,
            Self::Geu => left >= right,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_predicates_cover_equality_and_signed_boundaries() {
        // Eq, Ne, Lt, Ge, Ltu, Geu, in that order. Reversed operands
        // exercise both directions at zero, MIN, MAX and the sign boundary.
        for (left, right, expected) in [
            (0, 0, [true, false, false, true, false, true]),
            (u32::MAX, u32::MAX, [true, false, false, true, false, true]),
            (
                0x80000000,
                0x80000000,
                [true, false, false, true, false, true],
            ),
            (0, u32::MAX, [false, true, false, true, true, false]),
            (u32::MAX, 0, [false, true, true, false, false, true]),
            (
                0x7fffffff,
                0x80000000,
                [false, true, false, true, true, false],
            ),
            (
                0x80000000,
                0x7fffffff,
                [false, true, true, false, false, true],
            ),
            (
                0x80000000,
                u32::MAX,
                [false, true, true, false, true, false],
            ),
            (
                u32::MAX,
                0x80000000,
                [false, true, false, true, false, true],
            ),
            (0, 1, [false, true, true, false, true, false]),
            (1, 0, [false, true, false, true, false, true]),
        ] {
            for (test, expected) in [
                BranchTest::Eq,
                BranchTest::Ne,
                BranchTest::Lt,
                BranchTest::Ge,
                BranchTest::Ltu,
                BranchTest::Geu,
            ]
            .into_iter()
            .zip(expected)
            {
                assert_eq!(
                    test.evaluate(left, right),
                    expected,
                    "{test:?} {left:#x} {right:#x}"
                );
            }
        }
    }

    fn assert_registers(op: SemanticOp, inputs: &[u8], destination: Option<u8>) {
        assert_eq!(op.destination_register(), destination, "{op:?}");
        for register in 0..32 {
            assert_eq!(
                op.reads_register(register),
                inputs.contains(&register),
                "{op:?}, x{register}"
            );
        }
    }

    #[test]
    fn register_queries_retain_explicit_operands_and_zero_destinations() {
        for dest in [0, 10, 31] {
            assert_registers(
                SemanticOp::Integer {
                    op: IntegerOp::Add,
                    dest,
                    left: Operand::Register(0),
                    right: Operand::Immediate(10),
                },
                &[0],
                Some(dest),
            );
            assert_registers(
                SemanticOp::Integer {
                    op: IntegerOp::Add,
                    dest,
                    left: Operand::Register(11),
                    right: Operand::Register(12),
                },
                &[11, 12],
                Some(dest),
            );
            assert_registers(
                SemanticOp::Integer {
                    op: IntegerOp::Add,
                    dest,
                    left: Operand::Immediate(11),
                    right: Operand::Register(12),
                },
                &[12],
                Some(dest),
            );
            for op in [
                SemanticOp::Upper {
                    dest,
                    value: 11,
                    pc_relative: false,
                },
                SemanticOp::Upper {
                    dest,
                    value: 12,
                    pc_relative: true,
                },
                SemanticOp::Link { dest },
                SemanticOp::Opaque { dest },
            ] {
                assert_registers(op, &[], Some(dest));
            }
        }
        assert_registers(SemanticOp::None, &[], None);
        assert_registers(
            SemanticOp::Fence {
                fm: 0,
                predecessor: 15,
                successor: 15,
            },
            &[],
            None,
        );
    }

    #[test]
    fn memory_queries_cover_bases_sources_and_optional_integer_destinations() {
        for (kind, dest, source, inputs) in [
            (MemoryKind::Load, Some(10), None, &[11][..]),
            (MemoryKind::Store, None, Some(12), &[11, 12][..]),
            (MemoryKind::LoadReserved, Some(10), None, &[11][..]),
            (
                MemoryKind::StoreConditional,
                Some(10),
                Some(12),
                &[11, 12][..],
            ),
            (MemoryKind::Atomic, Some(10), Some(12), &[11, 12][..]),
            (MemoryKind::FloatLoad, None, None, &[11][..]),
            (MemoryKind::FloatStore, None, None, &[11][..]),
            (MemoryKind::Atomic, Some(0), Some(0), &[0, 11][..]),
        ] {
            assert_registers(
                SemanticOp::Memory {
                    kind,
                    base: 11,
                    displacement: 10,
                    width: 4,
                    dest,
                    source,
                    swap: false,
                    signed: false,
                },
                inputs,
                dest,
            );
        }
        assert_registers(
            SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: 0,
                displacement: 12,
                width: 4,
                dest: Some(10),
                source: None,
                swap: false,
                signed: false,
            },
            &[0],
            Some(10),
        );
    }

    #[test]
    fn unsupported_may_read_any_register_without_describing_a_destination() {
        assert_eq!(SemanticOp::Unsupported.destination_register(), None);
        for register in 0..32 {
            assert!(
                SemanticOp::Unsupported.reads_register(register),
                "x{register}"
            );
        }
    }

    #[test]
    fn the_psabi_register_roles_partition_the_integer_registers() {
        let abi = CallAbi::RiscvInteger;
        assert_eq!(abi.stack_alignment(), 16);
        let mut roles: Vec<u8> = abi.preserved().into_iter().chain(abi.clobbered()).collect();
        roles.sort_unstable();
        assert_eq!(roles, (1..32).collect::<Vec<u8>>());
        let clobbered = abi.clobbered();
        for register in abi.arguments().into_iter().chain(abi.results()) {
            assert!(clobbered.contains(&register), "x{register}");
        }
        assert_eq!(abi.argument_word(10), Some(0));
        assert_eq!(abi.argument_word(17), Some(7));
        assert_eq!(abi.argument_word(18), None);
        assert_eq!(abi.results(), [abi.arguments()[0], abi.arguments()[1]]);
    }
}

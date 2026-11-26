//! RV32 bit-manipulation (Zba, Zbb, Zbs), code-size (Zcb, Zcmp) and CSR
//! access (Zicsr) encodings, which the pinned rv-asm 0.2.1 does not decode.
//!
//! ESP-IDF builds for the `rv32imafc_zba_zbb_zbs_zcb_zcmp_zcmt` chip
//! targets. Zcmp occupies the 16-bit encoding
//! space rv-asm reads as C.FSDSP, and Zcb the reserved space of quadrants 0
//! and 1, so those spaces are classified here before rv-asm sees them. Zcmt
//! table jumps need the `jvt` CSR and stay unsupported.
//! Encodings: <https://docs.riscv.org/reference/isa/unpriv/b-st-ext.html> and
//! <https://docs.riscv.org/reference/isa/unpriv/zc.html> and
//! <https://docs.riscv.org/reference/isa/unpriv/zicsr.html>.
use crate::{ExtensionOp as IntegerOp, Operand, field, prime};
use core::fmt;

/// One decoded extension instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Extension {
    /// `dest = left op right`; unary operations ignore `right`.
    Integer {
        op: IntegerOp,
        dest: u8,
        left: u8,
        right: Operand,
    },
    /// Zcb byte and halfword loads and stores.
    Memory {
        load: bool,
        register: u8,
        base: u8,
        offset: u8,
        width: u8,
        signed: bool,
    },
    /// `cm.push`: store the listed registers below `sp`, then lower `sp`.
    Push { list: RegisterList, adjustment: u32 },
    /// `cm.pop`, `cm.popret` and `cm.popretz`: raise `sp` after loading the
    /// listed registers; the returning forms zero `a0` when asked and return
    /// through `ra`.
    Pop {
        list: RegisterList,
        adjustment: u32,
        ret: Option<bool>,
    },
    /// `cm.mvsa01`: two saved registers take `a0` and `a1`.
    MoveToSaved { first: u8, second: u8 },
    /// `cm.mva01s`: `a0` and `a1` take two saved registers.
    MoveFromSaved { first: u8, second: u8 },
    /// `csrrw`, `csrrs`, `csrrc` and their immediate forms: `dest` takes
    /// the old value of `csr`, which `source` then writes, sets or clears.
    Csr {
        op: CsrOp,
        dest: u8,
        source: Operand,
        csr: u16,
    },
}

/// How a Zicsr access changes its CSR.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CsrOp {
    Write,
    Set,
    Clear,
}

/// How a word falls into the extension spaces.
pub(crate) enum Classified {
    Extension(Extension, usize),
    /// Inside a space this module owns but not a supported encoding.
    Reserved,
    /// Not an extension encoding; the base decoder applies.
    Base,
}

pub const A0: u8 = 10;
pub const A1: u8 = 11;
pub const RA: u8 = 1;

/// The saved registers `s0`..`s11`, in their order.
const SAVED: [u8; 12] = [8, 9, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27];

/// Zcmp's three-bit saved-register field at bit `AT`: `s0`..`s7`.
const fn saved_prime<const AT: u32>(word: u32) -> u8 {
    match field::<AT, 3>(word) {
        0 => 8,
        1 => 9,
        2 => 18,
        3 => 19,
        4 => 20,
        5 => 21,
        6 => 22,
        _ => 23,
    }
}

/// The registers a Zcmp stack form stores or loads: `ra` alone, or `ra` and
/// the saved registers from `s0` to the one the variant names. No list ends
/// at `s10`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum RegisterList {
    Ra,
    S0,
    S1,
    S2,
    S3,
    S4,
    S5,
    S6,
    S7,
    S8,
    S9,
    S11,
}

impl RegisterList {
    /// The list the four-bit `rlist` field of `word` names; the values
    /// below 4 name none.
    const fn of(word: u32) -> Option<Self> {
        Some(match field::<4, 4>(word) {
            4 => Self::Ra,
            5 => Self::S0,
            6 => Self::S1,
            7 => Self::S2,
            8 => Self::S3,
            9 => Self::S4,
            10 => Self::S5,
            11 => Self::S6,
            12 => Self::S7,
            13 => Self::S8,
            14 => Self::S9,
            15 => Self::S11,
            _ => return None,
        })
    }

    /// The saved registers the list names besides `ra`.
    const fn saved(self) -> usize {
        match self {
            Self::Ra => 0,
            Self::S0 => 1,
            Self::S1 => 2,
            Self::S2 => 3,
            Self::S3 => 4,
            Self::S4 => 5,
            Self::S5 => 6,
            Self::S6 => 7,
            Self::S7 => 8,
            Self::S8 => 9,
            Self::S9 => 10,
            Self::S11 => 12,
        }
    }

    /// The stack adjustment of the list with the two-bit `spimm` field of
    /// `word`: the frame the registers need, rounded to the 16-byte stack
    /// alignment, and up to three further 16-byte blocks.
    const fn adjustment(self, word: u32) -> u32 {
        let [none, one, two, three] = match self {
            Self::Ra | Self::S0 | Self::S1 | Self::S2 => [16, 32, 48, 64],
            Self::S3 | Self::S4 | Self::S5 | Self::S6 => [32, 48, 64, 80],
            Self::S7 | Self::S8 | Self::S9 => [48, 64, 80, 96],
            Self::S11 => [64, 80, 96, 112],
        };
        match field::<2, 2>(word) {
            0 => none,
            1 => one,
            2 => two,
            _ => three,
        }
    }

    /// The assembler's register-list syntax.
    const fn name(self) -> &'static str {
        match self {
            Self::Ra => "{ra}",
            Self::S0 => "{ra, s0}",
            Self::S1 => "{ra, s0-s1}",
            Self::S2 => "{ra, s0-s2}",
            Self::S3 => "{ra, s0-s3}",
            Self::S4 => "{ra, s0-s4}",
            Self::S5 => "{ra, s0-s5}",
            Self::S6 => "{ra, s0-s6}",
            Self::S7 => "{ra, s0-s7}",
            Self::S8 => "{ra, s0-s8}",
            Self::S9 => "{ra, s0-s9}",
            Self::S11 => "{ra, s0-s11}",
        }
    }
}

/// Registers of a Zcmp list in the order the specification stores them from
/// the top of the frame down: the last saved register first, `ra` last.
pub fn list_registers(list: RegisterList) -> impl Iterator<Item = u8> {
    SAVED
        .into_iter()
        .take(list.saved())
        .rev()
        .chain(core::iter::once(RA))
}

/// Classify one instruction's bytes.
pub(crate) fn classify(bytes: &[u8]) -> Classified {
    let Some(&[low, high]) = bytes.first_chunk() else {
        return Classified::Base;
    };
    let half = u16::from_le_bytes([low, high]);
    if half & 3 != 3 {
        return compressed(half);
    }
    let Some(raw) = bytes.first_chunk() else {
        return Classified::Base;
    };
    match word(u32::from_le_bytes(*raw)) {
        Some(extension) => Classified::Extension(extension, 4),
        None => Classified::Base,
    }
}

fn compressed(half: u16) -> Classified {
    let word = u32::from(half);
    let found = |extension| Classified::Extension(extension, 2);
    match (field::<0, 2>(word), field::<13, 3>(word)) {
        // Zcb loads and stores.
        (0, 4) => {
            let (base, register) = (prime::<7>(word), prime::<2>(word));
            let (bit5, bit6) = (field::<5, 1>(word), field::<6, 1>(word));
            let byte_offset = bit6 | (bit5 << 1);
            let half_offset = bit5 << 1;
            let memory = |load, offset, width, signed| {
                found(Extension::Memory {
                    load,
                    register,
                    base,
                    offset,
                    width,
                    signed,
                })
            };
            match field::<10, 6>(word) {
                0b100000 => memory(true, byte_offset, 1, false),
                0b100001 => memory(true, half_offset, 2, bit6 == 1),
                0b100010 => memory(false, byte_offset, 1, false),
                0b100011 if bit6 == 0 => memory(false, half_offset, 2, false),
                _ => Classified::Reserved,
            }
        }
        // Zcb arithmetic; other quadrant-1 funct3=4 forms are base C.
        (1, 4) if field::<10, 6>(word) == 0b100111 => {
            let dest = prime::<7>(word);
            let integer = |op, right| {
                found(Extension::Integer {
                    op,
                    dest,
                    left: dest,
                    right,
                })
            };
            let unary = |op| integer(op, Operand::Immediate(0));
            match (field::<5, 2>(word), field::<2, 3>(word)) {
                (3, 0) => unary(IntegerOp::ZeroExtendByte),
                (3, 1) => unary(IntegerOp::SignExtendByte),
                (3, 2) => unary(IntegerOp::ZeroExtendHalf),
                (3, 3) => unary(IntegerOp::SignExtendHalf),
                (3, 5) => integer(IntegerOp::Xor, Operand::Immediate(u32::MAX)),
                (2, _) => integer(IntegerOp::Mul, Operand::Register(prime::<2>(word))),
                _ => Classified::Reserved,
            }
        }
        // Zcmp and Zcmt.
        (2, 5) => {
            let frame = RegisterList::of(word).map(|list| (list, list.adjustment(word)));
            let stack =
                |extension: Option<Extension>| extension.map_or(Classified::Reserved, found);
            let pop = |ret| {
                stack(frame.map(|(list, adjustment)| Extension::Pop {
                    list,
                    adjustment,
                    ret,
                }))
            };
            match (
                field::<8, 5>(word),
                field::<10, 6>(word),
                field::<5, 2>(word),
            ) {
                (0b11000, ..) => {
                    stack(frame.map(|(list, adjustment)| Extension::Push { list, adjustment }))
                }
                (0b11010, ..) => pop(None),
                (0b11100, ..) => pop(Some(true)),
                (0b11110, ..) => pop(Some(false)),
                (_, 0b101011, 1) if field::<7, 3>(word) != field::<2, 3>(word) => {
                    found(Extension::MoveToSaved {
                        first: saved_prime::<7>(word),
                        second: saved_prime::<2>(word),
                    })
                }
                (_, 0b101011, 3) => found(Extension::MoveFromSaved {
                    first: saved_prime::<7>(word),
                    second: saved_prime::<2>(word),
                }),
                _ => Classified::Reserved,
            }
        }
        _ => Classified::Base,
    }
}

/// The twelve bits of a CSR's number, the word's highest.
const fn csr(word: u32) -> u16 {
    let [low, high, ..] = (word >> 20).to_le_bytes();
    u16::from_le_bytes([low, high])
}

fn word(word: u32) -> Option<Extension> {
    let dest = field::<7, 5>(word);
    let funct3 = field::<12, 3>(word);
    let left = field::<15, 5>(word);
    let rs2 = field::<20, 5>(word);
    let funct7 = field::<25, 7>(word);
    let integer = |op, right| {
        Some(Extension::Integer {
            op,
            dest,
            left,
            right,
        })
    };
    let register = Operand::Register(rs2);
    let shamt = Operand::Immediate(u32::from(rs2));
    let none = Operand::Immediate(0);
    use IntegerOp::*;
    match (field::<0, 7>(word), funct7, funct3) {
        (0x33, 0x10, 2) => integer(ShiftAdd1, register),
        (0x33, 0x10, 4) => integer(ShiftAdd2, register),
        (0x33, 0x10, 6) => integer(ShiftAdd3, register),
        (0x33, 0x20, 7) => integer(AndNot, register),
        (0x33, 0x20, 6) => integer(OrNot, register),
        (0x33, 0x20, 4) => integer(XorNot, register),
        (0x33, 0x05, 4) => integer(Min, register),
        (0x33, 0x05, 5) => integer(Minu, register),
        (0x33, 0x05, 6) => integer(Max, register),
        (0x33, 0x05, 7) => integer(Maxu, register),
        (0x33, 0x30, 1) => integer(RotateLeft, register),
        (0x33, 0x30, 5) => integer(RotateRight, register),
        (0x33, 0x04, 4) if rs2 == 0 => integer(ZeroExtendHalf, none),
        (0x33, 0x14, 1) => integer(BitSet, register),
        (0x33, 0x24, 1) => integer(BitClear, register),
        (0x33, 0x34, 1) => integer(BitInvert, register),
        (0x33, 0x24, 5) => integer(BitExtract, register),
        (0x13, 0x30, 1) => match rs2 {
            0 => integer(CountLeadingZeros, none),
            1 => integer(CountTrailingZeros, none),
            2 => integer(PopCount, none),
            4 => integer(SignExtendByte, none),
            5 => integer(SignExtendHalf, none),
            _ => None,
        },
        (0x13, 0x14, 1) => integer(BitSet, shamt),
        (0x13, 0x24, 1) => integer(BitClear, shamt),
        (0x13, 0x34, 1) => integer(BitInvert, shamt),
        (0x13, 0x24, 5) => integer(BitExtract, shamt),
        (0x13, 0x30, 5) => integer(RotateRight, shamt),
        (0x13, 0x14, 5) if rs2 == 7 => integer(OrCombineBytes, none),
        (0x13, 0x34, 5) if rs2 == 0x18 => integer(ByteReverse, none),
        (0x73, _, 1..=3 | 5..=7) => Some(Extension::Csr {
            op: match funct3 & 3 {
                1 => CsrOp::Write,
                2 => CsrOp::Set,
                _ => CsrOp::Clear,
            },
            dest,
            source: if funct3 & 4 == 0 {
                Operand::Register(left)
            } else {
                Operand::Immediate(u32::from(left))
            },
            csr: csr(word),
        }),
        _ => None,
    }
}

const ABI: [&str; 32] = [
    "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
    "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
    "t5", "t6",
];

/// The ABI name of integer register `register` (`x0`..`x31`).
pub fn register_name(register: u8) -> &'static str {
    ABI[register as usize]
}

impl fmt::Display for Extension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Extension::Integer {
                op,
                dest,
                left,
                right,
            } => {
                use IntegerOp::*;
                let immediate = matches!(right, Operand::Immediate(_));
                let (mnemonic, unary) = match op {
                    ZeroExtendByte => ("zext.b", true),
                    ZeroExtendHalf => ("zext.h", true),
                    Xor => ("not", true),
                    Mul => ("mul", false),
                    ShiftAdd1 => ("sh1add", false),
                    ShiftAdd2 => ("sh2add", false),
                    ShiftAdd3 => ("sh3add", false),
                    AndNot => ("andn", false),
                    OrNot => ("orn", false),
                    XorNot => ("xnor", false),
                    Min => ("min", false),
                    Minu => ("minu", false),
                    Max => ("max", false),
                    Maxu => ("maxu", false),
                    RotateLeft => ("rol", false),
                    RotateRight if immediate => ("rori", false),
                    RotateRight => ("ror", false),
                    CountLeadingZeros => ("clz", true),
                    CountTrailingZeros => ("ctz", true),
                    PopCount => ("cpop", true),
                    SignExtendByte => ("sext.b", true),
                    SignExtendHalf => ("sext.h", true),
                    OrCombineBytes => ("orc.b", true),
                    ByteReverse => ("rev8", true),
                    BitSet if immediate => ("bseti", false),
                    BitSet => ("bset", false),
                    BitClear if immediate => ("bclri", false),
                    BitClear => ("bclr", false),
                    BitInvert if immediate => ("binvi", false),
                    BitInvert => ("binv", false),
                    BitExtract if immediate => ("bexti", false),
                    BitExtract => ("bext", false),
                };
                write!(
                    f,
                    "{mnemonic} {}, {}",
                    register_name(dest),
                    register_name(left)
                )?;
                match right {
                    _ if unary => Ok(()),
                    Operand::Register(r) => write!(f, ", {}", register_name(r)),
                    Operand::Immediate(v) => write!(f, ", {v}"),
                }
            }
            Extension::Csr {
                op,
                dest,
                source,
                csr,
            } => {
                let mnemonic = match op {
                    CsrOp::Write => "csrrw",
                    CsrOp::Set => "csrrs",
                    CsrOp::Clear => "csrrc",
                };
                match source {
                    Operand::Register(r) => {
                        write!(
                            f,
                            "{mnemonic} {}, {csr:#x}, {}",
                            register_name(dest),
                            register_name(r)
                        )
                    }
                    Operand::Immediate(v) => {
                        write!(f, "{mnemonic}i {}, {csr:#x}, {v}", register_name(dest))
                    }
                }
            }
            Extension::Memory {
                load,
                register,
                base,
                offset,
                width,
                signed,
            } => {
                let kind = match (load, width, signed) {
                    (true, 1, _) => "c.lbu",
                    (true, _, false) => "c.lhu",
                    (true, _, true) => "c.lh",
                    (false, 1, _) => "c.sb",
                    (false, ..) => "c.sh",
                };
                write!(
                    f,
                    "{kind} {}, {offset}({})",
                    register_name(register),
                    register_name(base)
                )
            }
            Extension::Push { list, adjustment } => {
                write!(f, "cm.push {}, -{adjustment}", list.name())
            }
            Extension::Pop {
                list,
                adjustment,
                ret,
            } => {
                let kind = match ret {
                    None => "cm.pop",
                    Some(false) => "cm.popret",
                    Some(true) => "cm.popretz",
                };
                write!(f, "{kind} {}, {adjustment}", list.name())
            }
            Extension::MoveToSaved { first, second } => {
                write!(
                    f,
                    "cm.mvsa01 {}, {}",
                    register_name(first),
                    register_name(second)
                )
            }
            Extension::MoveFromSaved { first, second } => {
                write!(
                    f,
                    "cm.mva01s {}, {}",
                    register_name(first),
                    register_name(second)
                )
            }
        }
    }
}

#[cfg(test)]
mod tests;

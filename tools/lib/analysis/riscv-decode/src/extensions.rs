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
use crate::{ExtensionOp as IntegerOp, Operand};
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
    Push { list: u8, adjustment: u32 },
    /// `cm.pop`, `cm.popret` and `cm.popretz`: raise `sp` after loading the
    /// listed registers; the returning forms zero `a0` when asked and return
    /// through `ra`.
    Pop {
        list: u8,
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

/// Saved register `s<index>`.
const fn saved(index: u8) -> u8 {
    match index {
        0 => 8,
        1 => 9,
        _ => 16 + index,
    }
}

/// Zcmp's three-bit saved-register field: `s0`, `s1`, then `s2..=s7`.
const fn saved_prime(field: u16) -> u8 {
    saved(field as u8)
}

/// Saved registers a Zcmp register list names besides `ra`.
const fn saved_count(list: u8) -> u8 {
    if list == 15 { 12 } else { list - 4 }
}

/// Registers of a Zcmp list in the order the specification stores them from
/// the top of the frame down: `s11` first, `ra` last.
pub fn list_registers(list: u8) -> impl Iterator<Item = u8> {
    (0..saved_count(list))
        .rev()
        .map(saved)
        .chain(core::iter::once(RA))
}

/// The frame a register list needs, rounded to the 16-byte stack alignment.
const fn base_adjustment(list: u8) -> u32 {
    let bytes = (saved_count(list) as u32 + 1) * 4;
    bytes.div_ceil(16) * 16
}

/// Classify one instruction's bytes.
pub(crate) fn classify(bytes: &[u8]) -> Classified {
    if bytes.len() < 2 {
        return Classified::Base;
    }
    let half = u16::from_le_bytes([bytes[0], bytes[1]]);
    if half & 3 != 3 {
        return compressed(half);
    }
    let Ok(raw) = <[u8; 4]>::try_from(bytes.get(..4).unwrap_or_default()) else {
        return Classified::Base;
    };
    match word(u32::from_le_bytes(raw)) {
        Some(extension) => Classified::Extension(extension, 4),
        None => Classified::Base,
    }
}

fn compressed(half: u16) -> Classified {
    let (quadrant, funct3) = (half & 3, half >> 13);
    let found = |extension| Classified::Extension(extension, 2);
    let prime = |shift: u16| 8 + ((half >> shift) & 7) as u8;
    match (quadrant, funct3) {
        // Zcb loads and stores.
        (0, 4) => {
            let (base, register) = (prime(7), prime(2));
            let byte_offset = (((half >> 6) & 1) | (((half >> 5) & 1) << 1)) as u8;
            let half_offset = (((half >> 5) & 1) << 1) as u8;
            let bit6 = (half >> 6) & 1;
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
            match half >> 10 {
                0b100000 => memory(true, byte_offset, 1, false),
                0b100001 => memory(true, half_offset, 2, bit6 == 1),
                0b100010 => memory(false, byte_offset, 1, false),
                0b100011 if bit6 == 0 => memory(false, half_offset, 2, false),
                _ => Classified::Reserved,
            }
        }
        // Zcb arithmetic; other quadrant-1 funct3=4 forms are base C.
        (1, 4) if half >> 10 == 0b100111 => {
            let dest = prime(7);
            let unary = |op, right| {
                found(Extension::Integer {
                    op,
                    dest,
                    left: dest,
                    right: Operand::Immediate(right),
                })
            };
            match ((half >> 5) & 3, (half >> 2) & 7) {
                (3, 0) => unary(IntegerOp::And, 0xff),
                (3, 1) => unary(IntegerOp::SignExtendByte, 0),
                (3, 2) => unary(IntegerOp::And, 0xffff),
                (3, 3) => unary(IntegerOp::SignExtendHalf, 0),
                (3, 5) => unary(IntegerOp::Xor, u32::MAX),
                (2, _) => found(Extension::Integer {
                    op: IntegerOp::Mul,
                    dest,
                    left: dest,
                    right: Operand::Register(prime(2)),
                }),
                _ => Classified::Reserved,
            }
        }
        // Zcmp and Zcmt.
        (2, 5) => {
            let list = ((half >> 4) & 0xf) as u8;
            let adjustment = base_adjustment(list.max(4)) + u32::from((half >> 2) & 3) * 16;
            let stack = |extension| {
                if list < 4 {
                    Classified::Reserved
                } else {
                    found(extension)
                }
            };
            match ((half >> 8) & 0x1f, half >> 10, (half >> 5) & 3) {
                (0b11000, ..) => stack(Extension::Push { list, adjustment }),
                (0b11010, ..) => stack(Extension::Pop {
                    list,
                    adjustment,
                    ret: None,
                }),
                (0b11100, ..) => stack(Extension::Pop {
                    list,
                    adjustment,
                    ret: Some(true),
                }),
                (0b11110, ..) => stack(Extension::Pop {
                    list,
                    adjustment,
                    ret: Some(false),
                }),
                (_, 0b101011, 1) if (half >> 7) & 7 != (half >> 2) & 7 => {
                    found(Extension::MoveToSaved {
                        first: saved_prime((half >> 7) & 7),
                        second: saved_prime((half >> 2) & 7),
                    })
                }
                (_, 0b101011, 3) => found(Extension::MoveFromSaved {
                    first: saved_prime((half >> 7) & 7),
                    second: saved_prime((half >> 2) & 7),
                }),
                _ => Classified::Reserved,
            }
        }
        _ => Classified::Base,
    }
}

fn word(word: u32) -> Option<Extension> {
    let dest = ((word >> 7) & 31) as u8;
    let funct3 = (word >> 12) & 7;
    let left = ((word >> 15) & 31) as u8;
    let field = (word >> 20) & 31;
    let funct7 = word >> 25;
    let integer = |op, right| {
        Some(Extension::Integer {
            op,
            dest,
            left,
            right,
        })
    };
    let register = Operand::Register(field as u8);
    let shamt = Operand::Immediate(field);
    let none = Operand::Immediate(0);
    use IntegerOp::*;
    match (word & 0x7f, funct7, funct3) {
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
        (0x33, 0x04, 4) if field == 0 => integer(And, Operand::Immediate(0xffff)),
        (0x33, 0x14, 1) => integer(BitSet, register),
        (0x33, 0x24, 1) => integer(BitClear, register),
        (0x33, 0x34, 1) => integer(BitInvert, register),
        (0x33, 0x24, 5) => integer(BitExtract, register),
        (0x13, 0x30, 1) => match field {
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
        (0x13, 0x14, 5) if field == 7 => integer(OrCombineBytes, none),
        (0x13, 0x34, 5) if field == 0x18 => integer(ByteReverse, none),
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
            csr: (word >> 20) as u16,
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
        // The assembler's register-list syntax: `{ra}`, `{ra, s0}` or
        // `{ra, s0-sN}`.
        let list = |f: &mut fmt::Formatter<'_>, list: u8| match saved_count(list) {
            0 => write!(f, "{{ra}}"),
            1 => write!(f, "{{ra, s0}}"),
            count => write!(f, "{{ra, s0-s{}}}", count - 1),
        };
        match *self {
            Extension::Integer {
                op,
                dest,
                left,
                right,
            } => {
                use IntegerOp::*;
                let immediate = matches!(right, Operand::Immediate(_));
                let (mnemonic, unary) = match (op, right) {
                    (And, Operand::Immediate(0xff)) => ("zext.b", true),
                    (And, Operand::Immediate(0xffff)) => ("zext.h", true),
                    (Xor, _) => ("not", true),
                    (Mul, _) => ("mul", false),
                    (ShiftAdd1, _) => ("sh1add", false),
                    (ShiftAdd2, _) => ("sh2add", false),
                    (ShiftAdd3, _) => ("sh3add", false),
                    (AndNot, _) => ("andn", false),
                    (OrNot, _) => ("orn", false),
                    (XorNot, _) => ("xnor", false),
                    (Min, _) => ("min", false),
                    (Minu, _) => ("minu", false),
                    (Max, _) => ("max", false),
                    (Maxu, _) => ("maxu", false),
                    (RotateLeft, _) => ("rol", false),
                    (RotateRight, _) if immediate => ("rori", false),
                    (RotateRight, _) => ("ror", false),
                    (CountLeadingZeros, _) => ("clz", true),
                    (CountTrailingZeros, _) => ("ctz", true),
                    (PopCount, _) => ("cpop", true),
                    (SignExtendByte, _) => ("sext.b", true),
                    (SignExtendHalf, _) => ("sext.h", true),
                    (OrCombineBytes, _) => ("orc.b", true),
                    (ByteReverse, _) => ("rev8", true),
                    (BitSet, _) if immediate => ("bseti", false),
                    (BitSet, _) => ("bset", false),
                    (BitClear, _) if immediate => ("bclri", false),
                    (BitClear, _) => ("bclr", false),
                    (BitInvert, _) if immediate => ("binvi", false),
                    (BitInvert, _) => ("binv", false),
                    (BitExtract, _) if immediate => ("bexti", false),
                    (BitExtract, _) => ("bext", false),
                    _ => unreachable!("extension decoding produces no other operation"),
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
            Extension::Push {
                list: l,
                adjustment,
            } => {
                write!(f, "cm.push ")?;
                list(f, l)?;
                write!(f, ", -{adjustment}")
            }
            Extension::Pop {
                list: l,
                adjustment,
                ret,
            } => {
                let kind = match ret {
                    None => "cm.pop",
                    Some(false) => "cm.popret",
                    Some(true) => "cm.popretz",
                };
                write!(f, "{kind} ")?;
                list(f, l)?;
                write!(f, ", {adjustment}")
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

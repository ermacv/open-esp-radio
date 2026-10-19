//! RV32 instruction decoding: instruction bytes to a typed instruction and its
//! length, with its assembler text.
//!
//! The pinned rv-asm 0.2.1 decodes the RV32I base with M, A and C, and this
//! crate decodes the forms it lacks: the Zba, Zbb and Zbs integer forms, the
//! Zcb loads, stores and arithmetic, the Zcmp push, pop and register moves,
//! the Zicsr CSR accesses and the single-precision F extension with its compressed loads and stores.
//! The caller selects the instruction set with [`Extensions`]; an encoding
//! outside it, a reserved encoding or an incomplete instruction decodes to
//! `None`. Neither this crate nor rv-asm has I/O, ELF knowledge or a `std`
//! dependency.
//!
//! Encodings: the unprivileged ISA, <https://docs.riscv.org/reference/isa/unpriv/>.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

use core::fmt;

mod extensions;
mod float;

pub use extensions::{A0, A1, CsrOp, Extension, RA, list_registers, register_name};
pub use float::{Float, Register};
pub use rv_asm::{AmoOp, AmoOrdering, Fence, FenceSet, Imm, Inst, Reg};

/// A set of instruction-set extensions beside the RV32I base.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Extensions(u16);

impl Extensions {
    /// The RV32I base alone.
    pub const NONE: Self = Self(0);
    pub const M: Self = Self(1 << 0);
    pub const A: Self = Self(1 << 1);
    /// Single precision; with C, also the compressed FP loads and stores.
    pub const F: Self = Self(1 << 2);
    pub const C: Self = Self(1 << 3);
    pub const ZBA: Self = Self(1 << 4);
    pub const ZBB: Self = Self(1 << 5);
    pub const ZBS: Self = Self(1 << 6);
    /// Requires C; `c.sext.b`, `c.zext.h` and `c.sext.h` also need Zbb and
    /// `c.mul` M.
    pub const ZCB: Self = Self(1 << 7);
    /// Requires C.
    pub const ZCMP: Self = Self(1 << 8);
    pub const ZICSR: Self = Self(1 << 9);
    /// RV32IMAC.
    pub const RV32IMAC: Self = Self::M.union(Self::A).union(Self::C);
    /// Every extension this crate decodes:
    /// `rv32imafc_zicsr_zba_zbb_zbs_zcb_zcmp` without Zcmt.
    pub const ALL: Self = Self::RV32IMAC
        .union(Self::F)
        .union(Self::ZICSR)
        .union(Self::ZBA)
        .union(Self::ZBB)
        .union(Self::ZBS)
        .union(Self::ZCB)
        .union(Self::ZCMP);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// An operation of an extension integer form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ExtensionOp {
    And,
    Xor,
    Mul,
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
    CountLeadingZeros,
    CountTrailingZeros,
    PopCount,
    SignExtendByte,
    SignExtendHalf,
    OrCombineBytes,
    ByteReverse,
    BitSet,
    BitClear,
    BitInvert,
    BitExtract,
}

/// The right operand of an extension integer form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Operand {
    Register(u8),
    Immediate(u32),
}

/// One decoded instruction: a base RV32IMAC form from rv-asm, or an
/// extension or single-precision form it lacks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Instruction {
    Base(Inst),
    Extension(Extension),
    Float(Float),
}

impl Instruction {
    /// The extensions the instruction needs beside the RV32I base and, for a
    /// 16-bit encoding, C; `compressed` for a 16-bit encoding.
    fn requires(&self, compressed: bool) -> Extensions {
        match *self {
            Instruction::Base(inst) => match inst {
                Inst::Mul { .. }
                | Inst::Mulh { .. }
                | Inst::Mulhsu { .. }
                | Inst::Mulhu { .. }
                | Inst::Div { .. }
                | Inst::Divu { .. }
                | Inst::Rem { .. }
                | Inst::Remu { .. } => Extensions::M,
                Inst::LrW { .. } | Inst::ScW { .. } | Inst::AmoW { .. } => Extensions::A,
                _ => Extensions::NONE,
            },
            Instruction::Extension(extension) => {
                use ExtensionOp::*;
                match extension {
                    Extension::Integer { op, right, .. } if compressed => match (op, right) {
                        (Mul, _) => Extensions::ZCB.union(Extensions::M),
                        (SignExtendByte | SignExtendHalf, _)
                        | (And, Operand::Immediate(0xffff)) => {
                            Extensions::ZCB.union(Extensions::ZBB)
                        }
                        _ => Extensions::ZCB,
                    },
                    Extension::Memory { .. } => Extensions::ZCB,
                    Extension::Integer { op, .. } => match op {
                        ShiftAdd1 | ShiftAdd2 | ShiftAdd3 => Extensions::ZBA,
                        BitSet | BitClear | BitInvert | BitExtract => Extensions::ZBS,
                        _ => Extensions::ZBB,
                    },
                    Extension::Push { .. }
                    | Extension::Pop { .. }
                    | Extension::MoveToSaved { .. }
                    | Extension::MoveFromSaved { .. } => Extensions::ZCMP,
                    Extension::Csr { .. } => Extensions::ZICSR,
                }
            }
            Instruction::Float(_) => Extensions::F,
        }
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Instruction::Base(inst) => fmt::Display::fmt(inst, f),
            Instruction::Extension(extension) => fmt::Display::fmt(extension, f),
            Instruction::Float(float) => fmt::Display::fmt(float, f),
        }
    }
}

/// Decode the instruction at the start of `bytes` within `extensions`: the
/// instruction and its length, 2 or 4 bytes.
///
/// `bytes` may extend past the instruction. `None` when the encoding is
/// outside the set, reserved, longer than 32 bits, or `bytes` is shorter than
/// the instruction.
pub fn decode(bytes: &[u8], extensions: Extensions) -> Option<(Instruction, usize)> {
    let (instruction, width) = decode_any(bytes)?;
    let required = instruction.requires(width == 2);
    let required = if width == 2 {
        required.union(Extensions::C)
    } else {
        required
    };
    extensions
        .contains(required)
        .then_some((instruction, width))
}

/// Every form this crate decodes, before the extension selection.
fn decode_any(bytes: &[u8]) -> Option<(Instruction, usize)> {
    match extensions::classify(bytes) {
        extensions::Classified::Extension(extension, width) => {
            return (bytes.len() >= width).then_some((Instruction::Extension(extension), width));
        }
        extensions::Classified::Reserved => return None,
        extensions::Classified::Base => {}
    }
    if bytes.len() < 2 {
        return None;
    }
    let half = u16::from_le_bytes([bytes[0], bytes[1]]);
    let float = if half & 3 != 3 {
        float::compressed(half).map(|float| (float, 2))
    } else {
        bytes
            .get(..4)
            .and_then(|word| float::word(u32::from_le_bytes(word.try_into().ok()?)))
            .map(|float| (float, 4))
    };
    if let Some((float, width)) = float {
        return Some((Instruction::Float(float), width));
    }
    let width = if half & 3 != 3 {
        2
    } else if half & 0x1f != 0x1f {
        4
    } else {
        return None;
    };
    if bytes.len() < width {
        return None;
    }
    // C.ADDI16SP with a zero immediate is reserved; rv-asm 0.2.1 decodes it
    // as `addi sp, sp, 0`.
    if half == 0x6101 {
        return None;
    }
    let mut code = [0; 4];
    code[..width].copy_from_slice(&bytes[..width]);
    let (inst, compressed) = Inst::decode(u32::from_le_bytes(code), rv_asm::Xlen::Rv32).ok()?;
    if (compressed == rv_asm::IsCompressed::Yes) != (width == 2) {
        return None;
    }
    // rv-asm 0.2.1 zero-extends C.ANDI's six-bit immediate. Normalize here so
    // every consumer sees the ISA's signed immediate (C extension, integer ALU).
    let inst = match (width, inst) {
        (2, Inst::Andi { imm, dest, src1 }) => Inst::Andi {
            imm: Imm::new_i32(((imm.as_u32() << 26) as i32) >> 26),
            dest,
            src1,
        },
        (_, inst) => inst,
    };
    Some((Instruction::Base(inst), width))
}

#[cfg(test)]
mod tests;

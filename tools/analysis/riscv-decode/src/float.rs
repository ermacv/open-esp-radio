//! RV32F single-precision encodings, which the pinned rv-asm 0.2.1 does not
//! decode.
//!
//! A chip that builds for `rv32imafc` runs compiled Rust and vendor code
//! contain FP loads, stores, arithmetic and moves between the register
//! files. Only their structure and integer-register effects are modeled: the
//! FP register file is not. Encodings:
//! <https://docs.riscv.org/reference/isa/unpriv/f-st-ext.html> and the C
//! extension's FP loads and stores,
//! <https://docs.riscv.org/reference/isa/unpriv/c-st-ext.html>. The D, Q and
//! Zfh formats and `C.FLD`/`C.FSD` are not single precision and stay
//! undecoded.
use crate::{field, prime};
use core::fmt;

/// One operand register and the file it names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Register {
    Integer(u8),
    Float(u8),
}

/// A static rounding mode, by the name the assembler writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Rounding {
    /// `rne`: to nearest, ties to even.
    NearestEven,
    /// `rtz`: towards zero.
    TowardZero,
    /// `rdn`: down.
    Down,
    /// `rup`: up.
    Up,
    /// `rmm`: to nearest, ties to the greater magnitude.
    NearestMaxMagnitude,
}

impl Rounding {
    const fn name(self) -> &'static str {
        match self {
            Self::NearestEven => "rne",
            Self::TowardZero => "rtz",
            Self::Down => "rdn",
            Self::Up => "rup",
            Self::NearestMaxMagnitude => "rmm",
        }
    }
}

/// One decoded RV32F instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Float {
    /// `flw` and its compressed forms: FP `dest` takes the word at `base + offset`.
    Load { dest: u8, base: u8, offset: i32 },
    /// `fsw` and its compressed forms: FP `source` is stored at `base + offset`.
    Store { source: u8, base: u8, offset: i32 },
    /// Every other form. The first operand is the destination; an integer
    /// destination is the only integer-register effect.
    Operation {
        mnemonic: &'static str,
        operands: [Option<Register>; 4],
        /// A static rounding mode; `None` for the dynamic mode or a form without one.
        rounding: Option<Rounding>,
    },
}

/// The rounding-mode field of `word`: 0–4 name a mode, 7 selects `frm`, 5
/// and 6 are reserved.
const fn rounding(word: u32) -> Option<Option<Rounding>> {
    match field::<12, 3>(word) {
        0 => Some(Some(Rounding::NearestEven)),
        1 => Some(Some(Rounding::TowardZero)),
        2 => Some(Some(Rounding::Down)),
        3 => Some(Some(Rounding::Up)),
        4 => Some(Some(Rounding::NearestMaxMagnitude)),
        7 => Some(None),
        _ => None,
    }
}

/// Decode a 32-bit word in the F major opcodes; `None` for any other word
/// and for reserved or non-single-precision encodings.
pub(crate) fn word(word: u32) -> Option<Float> {
    use Register::{Float as F, Integer as X};
    let rd = field::<7, 5>(word);
    let funct3 = field::<12, 3>(word);
    let rs1 = field::<15, 5>(word);
    let rs2 = field::<20, 5>(word);
    let funct7 = field::<25, 7>(word);
    let operation = |mnemonic, operands, rounding| {
        Some(Float::Operation {
            mnemonic,
            operands,
            rounding,
        })
    };
    let fused = |mnemonic| {
        if field::<25, 2>(word) != 0 {
            return None;
        }
        let rs3 = field::<27, 5>(word);
        operation(
            mnemonic,
            [Some(F(rd)), Some(F(rs1)), Some(F(rs2)), Some(F(rs3))],
            rounding(word)?,
        )
    };
    match field::<0, 7>(word) {
        0x07 if funct3 == 2 => Some(Float::Load {
            dest: rd,
            base: rs1,
            offset: word.cast_signed() >> 20,
        }),
        0x27 if funct3 == 2 => Some(Float::Store {
            source: rs2,
            base: rs1,
            offset: ((word & 0xfe00_0000).cast_signed() >> 20) | i32::from(rd),
        }),
        0x43 => fused("fmadd.s"),
        0x47 => fused("fmsub.s"),
        0x4b => fused("fnmsub.s"),
        0x4f => fused("fnmadd.s"),
        0x53 => {
            let binary = [Some(F(rd)), Some(F(rs1)), Some(F(rs2)), None];
            let to_float = [Some(F(rd)), Some(X(rs1)), None, None];
            let to_integer = [Some(X(rd)), Some(F(rs1)), None, None];
            let compared = [Some(X(rd)), Some(F(rs1)), Some(F(rs2)), None];
            let rounded = |mnemonic, operands| operation(mnemonic, operands, rounding(word)?);
            let exact = |mnemonic, operands| operation(mnemonic, operands, None);
            match (funct7, funct3, rs2) {
                (0x00, ..) => rounded("fadd.s", binary),
                (0x04, ..) => rounded("fsub.s", binary),
                (0x08, ..) => rounded("fmul.s", binary),
                (0x0c, ..) => rounded("fdiv.s", binary),
                (0x2c, _, 0) => rounded("fsqrt.s", [Some(F(rd)), Some(F(rs1)), None, None]),
                (0x10, 0, _) => exact("fsgnj.s", binary),
                (0x10, 1, _) => exact("fsgnjn.s", binary),
                (0x10, 2, _) => exact("fsgnjx.s", binary),
                (0x14, 0, _) => exact("fmin.s", binary),
                (0x14, 1, _) => exact("fmax.s", binary),
                (0x60, _, 0) => rounded("fcvt.w.s", to_integer),
                (0x60, _, 1) => rounded("fcvt.wu.s", to_integer),
                (0x68, _, 0) => rounded("fcvt.s.w", to_float),
                (0x68, _, 1) => rounded("fcvt.s.wu", to_float),
                (0x50, 0, _) => exact("fle.s", compared),
                (0x50, 1, _) => exact("flt.s", compared),
                (0x50, 2, _) => exact("feq.s", compared),
                (0x70, 0, 0) => exact("fmv.x.w", to_integer),
                (0x70, 1, 0) => exact("fclass.s", to_integer),
                (0x78, 0, 0) => exact("fmv.w.x", to_float),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Decode a 16-bit FP load or store: `C.FLW`, `C.FSW`, `C.FLWSP` and
/// `C.FSWSP`. `None` for every other halfword.
pub(crate) fn compressed(half: u16) -> Option<Float> {
    let word = u32::from(half);
    // C.FLW/C.FSW: uimm[5:3] = [12:10], uimm[2] = [6], uimm[6] = [5].
    let register_offset = (i32::from(field::<10, 3>(word)) << 3)
        | (i32::from(field::<6, 1>(word)) << 2)
        | (i32::from(field::<5, 1>(word)) << 6);
    match (field::<0, 2>(word), field::<13, 3>(word)) {
        (0, 3) => Some(Float::Load {
            dest: prime::<2>(word),
            base: prime::<7>(word),
            offset: register_offset,
        }),
        (0, 7) => Some(Float::Store {
            source: prime::<2>(word),
            base: prime::<7>(word),
            offset: register_offset,
        }),
        // C.FLWSP: uimm[5] = [12], uimm[4:2] = [6:4], uimm[7:6] = [3:2].
        (2, 3) => Some(Float::Load {
            dest: field::<7, 5>(word),
            base: 2,
            offset: (i32::from(field::<12, 1>(word)) << 5)
                | (i32::from(field::<4, 3>(word)) << 2)
                | (i32::from(field::<2, 2>(word)) << 6),
        }),
        // C.FSWSP: uimm[5:2] = [12:9], uimm[7:6] = [8:7].
        (2, 7) => Some(Float::Store {
            source: field::<2, 5>(word),
            base: 2,
            offset: (i32::from(field::<9, 4>(word)) << 2) | (i32::from(field::<7, 2>(word)) << 6),
        }),
        _ => None,
    }
}

const FLOAT_ABI: [&str; 32] = [
    "ft0", "ft1", "ft2", "ft3", "ft4", "ft5", "ft6", "ft7", "fs0", "fs1", "fa0", "fa1", "fa2",
    "fa3", "fa4", "fa5", "fa6", "fa7", "fs2", "fs3", "fs4", "fs5", "fs6", "fs7", "fs8", "fs9",
    "fs10", "fs11", "ft8", "ft9", "ft10", "ft11",
];

impl fmt::Display for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Register::Integer(r) => f.write_str(super::extensions::register_name(r)),
            Register::Float(r) => f.write_str(FLOAT_ABI[r as usize]),
        }
    }
}

impl fmt::Display for Float {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let base = |r| Register::Integer(r);
        match *self {
            Float::Load {
                dest,
                base: b,
                offset,
            } => {
                write!(f, "flw {}, {offset}({})", Register::Float(dest), base(b))
            }
            Float::Store {
                source,
                base: b,
                offset,
            } => write!(f, "fsw {}, {offset}({})", Register::Float(source), base(b)),
            Float::Operation {
                mnemonic,
                operands,
                rounding,
            } => {
                write!(f, "{mnemonic}")?;
                for (index, operand) in operands.iter().flatten().enumerate() {
                    write!(f, "{}{operand}", if index == 0 { " " } else { ", " })?;
                }
                match rounding {
                    Some(mode) => write!(f, ", {}", mode.name()),
                    None => Ok(()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

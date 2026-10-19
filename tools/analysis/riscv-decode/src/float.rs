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
use core::fmt;

/// One operand register and the file it names.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Register {
    Integer(u8),
    Float(u8),
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
        rounding: Option<u8>,
    },
}

/// The rounding-mode field: 0–4 name a mode, 7 selects `frm`, 5 and 6 are reserved.
fn rounding(funct3: u32) -> Option<Option<u8>> {
    match funct3 {
        0..=4 => Some(Some(funct3 as u8)),
        7 => Some(None),
        _ => None,
    }
}

/// Decode a 32-bit word in the F major opcodes; `None` for any other word
/// and for reserved or non-single-precision encodings.
pub(crate) fn word(word: u32) -> Option<Float> {
    use Register::{Float as F, Integer as X};
    let rd = ((word >> 7) & 31) as u8;
    let funct3 = (word >> 12) & 7;
    let rs1 = ((word >> 15) & 31) as u8;
    let rs2 = ((word >> 20) & 31) as u8;
    let funct7 = word >> 25;
    let operation = |mnemonic, operands, rounding| {
        Some(Float::Operation {
            mnemonic,
            operands,
            rounding,
        })
    };
    match word & 0x7f {
        0x07 if funct3 == 2 => Some(Float::Load {
            dest: rd,
            base: rs1,
            offset: (word as i32) >> 20,
        }),
        0x27 if funct3 == 2 => Some(Float::Store {
            source: rs2,
            base: rs1,
            offset: (((word & 0xfe00_0000) as i32) >> 20) | ((word >> 7) & 31) as i32,
        }),
        major @ (0x43 | 0x47 | 0x4b | 0x4f) if funct7 & 3 == 0 => {
            let mnemonic = match major {
                0x43 => "fmadd.s",
                0x47 => "fmsub.s",
                0x4b => "fnmsub.s",
                _ => "fnmadd.s",
            };
            let rs3 = (word >> 27) as u8;
            operation(
                mnemonic,
                [Some(F(rd)), Some(F(rs1)), Some(F(rs2)), Some(F(rs3))],
                rounding(funct3)?,
            )
        }
        0x53 => {
            let binary =
                |mnemonic| Some((mnemonic, [Some(F(rd)), Some(F(rs1)), Some(F(rs2)), None]));
            let rounded = |mnemonic| {
                let (mnemonic, operands) = binary(mnemonic)?;
                operation(mnemonic, operands, rounding(funct3)?)
            };
            let exact = |mnemonic, operands| operation(mnemonic, operands, None);
            match (funct7, funct3, rs2) {
                (0x00, ..) => rounded("fadd.s"),
                (0x04, ..) => rounded("fsub.s"),
                (0x08, ..) => rounded("fmul.s"),
                (0x0c, ..) => rounded("fdiv.s"),
                (0x2c, _, 0) => operation(
                    "fsqrt.s",
                    [Some(F(rd)), Some(F(rs1)), None, None],
                    rounding(funct3)?,
                ),
                (0x10, 0..=2, _) => {
                    let mnemonic = ["fsgnj.s", "fsgnjn.s", "fsgnjx.s"][funct3 as usize];
                    exact(mnemonic, binary(mnemonic)?.1)
                }
                (0x14, 0..=1, _) => {
                    let mnemonic = ["fmin.s", "fmax.s"][funct3 as usize];
                    exact(mnemonic, binary(mnemonic)?.1)
                }
                (0x60, _, 0..=1) => operation(
                    ["fcvt.w.s", "fcvt.wu.s"][rs2 as usize],
                    [Some(X(rd)), Some(F(rs1)), None, None],
                    rounding(funct3)?,
                ),
                (0x68, _, 0..=1) => operation(
                    ["fcvt.s.w", "fcvt.s.wu"][rs2 as usize],
                    [Some(F(rd)), Some(X(rs1)), None, None],
                    rounding(funct3)?,
                ),
                (0x50, 0..=2, _) => exact(
                    ["fle.s", "flt.s", "feq.s"][funct3 as usize],
                    [Some(X(rd)), Some(F(rs1)), Some(F(rs2)), None],
                ),
                (0x70, 0, 0) => exact("fmv.x.w", [Some(X(rd)), Some(F(rs1)), None, None]),
                (0x70, 1, 0) => exact("fclass.s", [Some(X(rd)), Some(F(rs1)), None, None]),
                (0x78, 0, 0) => exact("fmv.w.x", [Some(F(rd)), Some(X(rs1)), None, None]),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Decode a 16-bit FP load or store: `C.FLW`, `C.FSW`, `C.FLWSP` and
/// `C.FSWSP`. `None` for every other halfword.
pub(crate) fn compressed(half: u16) -> Option<Float> {
    let bit = |n: u16| u32::from((half >> n) & 1);
    let prime = |shift: u16| 8 + ((half >> shift) & 7) as u8;
    // C.FLW/C.FSW: uimm[5:3] = [12:10], uimm[2] = [6], uimm[6] = [5].
    let register_offset =
        || ((u32::from((half >> 10) & 7) << 3) | (bit(6) << 2) | (bit(5) << 6)) as i32;
    match (half & 3, half >> 13) {
        (0, 3) => Some(Float::Load {
            dest: prime(2),
            base: prime(7),
            offset: register_offset(),
        }),
        (0, 7) => Some(Float::Store {
            source: prime(2),
            base: prime(7),
            offset: register_offset(),
        }),
        // C.FLWSP: uimm[5] = [12], uimm[4:2] = [6:4], uimm[7:6] = [3:2].
        (2, 3) => Some(Float::Load {
            dest: ((half >> 7) & 31) as u8,
            base: 2,
            offset: ((bit(12) << 5)
                | (u32::from((half >> 4) & 7) << 2)
                | (u32::from((half >> 2) & 3) << 6)) as i32,
        }),
        // C.FSWSP: uimm[5:2] = [12:9], uimm[7:6] = [8:7].
        (2, 7) => Some(Float::Store {
            source: ((half >> 2) & 31) as u8,
            base: 2,
            offset: ((u32::from((half >> 9) & 15) << 2) | (u32::from((half >> 7) & 3) << 6)) as i32,
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
                    Some(mode) => write!(
                        f,
                        ", {}",
                        ["rne", "rtz", "rdn", "rup", "rmm"][mode as usize]
                    ),
                    None => Ok(()),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;

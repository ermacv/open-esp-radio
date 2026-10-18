//! Placement-independent listings of linked functions.
//!
//! [`normalize`] decodes a function's bytes and writes one line per
//! instruction in which every address the code forms is replaced by the
//! location the caller's [`Placement`] names: branch and jump targets,
//! `lui`/`auipc` pairs with the instructions that complete them (tracked
//! through registers until a call or another write clobbers them), and
//! words in code that hold an address. Two links of the same code that
//! differ only in where functions and data were placed list alike; that is
//! what `cargo fw compare` compares. Compressed and full encodings of
//! one instruction list alike too.

use oer_riscv_decode::{Extension, Float, Inst, Instruction, Reg, Register};
use std::collections::HashMap;

/// Where the image placed what an address names.
pub trait Placement {
    /// The location of `address`: a symbol and offset, `data`, or the raw
    /// address when the image holds nothing there.
    fn locate(&self, address: u64) -> String;
    /// Whether `address` lies in the image's allocated contents.
    fn contains(&self, address: u64) -> bool;
}

/// Registers a call may clobber: `ra`, `t0`–`t6` and `a0`–`a7`.
const CALLER_SAVED: [u8; 16] = [1, 5, 6, 7, 10, 11, 12, 13, 14, 15, 16, 17, 28, 29, 30, 31];

/// The listing of the function at `address` with code `bytes`.
pub fn normalize(address: u64, bytes: &[u8], placement: &dyn Placement) -> Vec<String> {
    let mut pending: HashMap<u8, u32> = HashMap::new();
    let mut lines = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let pc = (address + at as u64) as u32;
        let rest = &bytes[at..];
        let Some((instruction, width)) = crate::decode_instruction(rest) else {
            // Data in code: an address word is located, anything else kept.
            if rest.len() >= 4 && rest[0] & 3 == 3 {
                let value = u32::from_le_bytes([rest[0], rest[1], rest[2], rest[3]]);
                lines.push(if placement.contains(u64::from(value)) {
                    format!(".word {}", placement.locate(u64::from(value)))
                } else {
                    format!(".word {value:#x}")
                });
                at += 4;
            } else {
                let half = u16::from_le_bytes([rest[0], *rest.get(1).unwrap_or(&0)]);
                lines.push(format!(".half {half:#x}"));
                at += 2;
            }
            continue;
        };
        lines.push(line(pc, instruction, &mut pending, placement));
        at += width;
    }
    lines
}

fn line(
    pc: u32,
    instruction: Instruction,
    pending: &mut HashMap<u8, u32>,
    placement: &dyn Placement,
) -> String {
    let locate = |address: u32| placement.locate(u64::from(address));
    let resolved = |base: Reg, offset: i32, pending: &HashMap<u8, u32>| {
        pending
            .get(&base.0)
            .map(|high| high.wrapping_add_signed(offset))
    };
    match instruction {
        Instruction::Base(inst) => match inst {
            Inst::Jal { offset, dest } => {
                let text = format!(
                    "jal {dest} {}",
                    locate(pc.wrapping_add_signed(offset.as_i32()))
                );
                clobber(pending);
                text
            }
            Inst::Jalr { offset, base, dest } => {
                let text = match resolved(base, offset.as_i32(), pending) {
                    Some(target) => format!("jalr {dest} {}", locate(target)),
                    None => inst.to_string(),
                };
                clobber(pending);
                pending.remove(&dest.0);
                text
            }
            Inst::Beq { offset, src1, src2 }
            | Inst::Bne { offset, src1, src2 }
            | Inst::Blt { offset, src1, src2 }
            | Inst::Bge { offset, src1, src2 }
            | Inst::Bltu { offset, src1, src2 }
            | Inst::Bgeu { offset, src1, src2 } => {
                let op = match inst {
                    Inst::Beq { .. } => "beq",
                    Inst::Bne { .. } => "bne",
                    Inst::Blt { .. } => "blt",
                    Inst::Bge { .. } => "bge",
                    Inst::Bltu { .. } => "bltu",
                    _ => "bgeu",
                };
                let target = pc.wrapping_add_signed(offset.as_i32());
                format!("{op} {src1},{src2} {}", locate(target))
            }
            Inst::Lui { uimm, dest } => {
                pending.insert(dest.0, uimm.as_u32());
                format!("lui {dest} <pair>")
            }
            Inst::Auipc { uimm, dest } => {
                pending.insert(dest.0, pc.wrapping_add(uimm.as_u32()));
                format!("auipc {dest} <pair>")
            }
            Inst::Addi { imm, dest, src1 } => match resolved(src1, imm.as_i32(), pending) {
                // An address computed from a tracked base is itself a base.
                Some(target) => {
                    pending.insert(dest.0, target);
                    format!("addi {dest},{src1} {}", locate(target))
                }
                None => {
                    pending.remove(&dest.0);
                    inst.to_string()
                }
            },
            Inst::Lb { offset, dest, base }
            | Inst::Lbu { offset, dest, base }
            | Inst::Lh { offset, dest, base }
            | Inst::Lhu { offset, dest, base }
            | Inst::Lw { offset, dest, base } => {
                let op = match inst {
                    Inst::Lb { .. } => "lb",
                    Inst::Lbu { .. } => "lbu",
                    Inst::Lh { .. } => "lh",
                    Inst::Lhu { .. } => "lhu",
                    _ => "lw",
                };
                let text = match resolved(base, offset.as_i32(), pending) {
                    Some(target) => format!("{op} {dest} {}", locate(target)),
                    None => inst.to_string(),
                };
                // A value loaded through an address is no address.
                pending.remove(&dest.0);
                text
            }
            Inst::Sb { offset, src, base }
            | Inst::Sh { offset, src, base }
            | Inst::Sw { offset, src, base } => {
                let op = match inst {
                    Inst::Sb { .. } => "sb",
                    Inst::Sh { .. } => "sh",
                    _ => "sw",
                };
                match resolved(base, offset.as_i32(), pending) {
                    Some(target) => format!("{op} {src} {}", locate(target)),
                    None => inst.to_string(),
                }
            }
            _ => {
                if let Some(dest) = destination(&inst) {
                    pending.remove(&dest);
                }
                inst.to_string()
            }
        },
        Instruction::Extension(extension) => {
            match extension {
                Extension::Integer { dest, .. } | Extension::Csr { dest, .. } => {
                    pending.remove(&dest);
                }
                Extension::Memory {
                    load: true,
                    register,
                    ..
                } => {
                    pending.remove(&register);
                }
                Extension::Memory { load: false, .. } => {}
                // The stack and register-list forms rewrite `sp`, `ra`, the
                // saved registers and `a0`/`a1`: nothing tracked survives.
                Extension::Push { .. }
                | Extension::Pop { .. }
                | Extension::MoveToSaved { .. }
                | Extension::MoveFromSaved { .. } => pending.clear(),
            }
            extension.to_string()
        }
        Instruction::Float(float) => match float {
            Float::Load { dest, base, offset } => match resolved(Reg(base), offset, pending) {
                Some(target) => format!("flw f{dest} {}", locate(target)),
                None => float.to_string(),
            },
            Float::Store {
                source,
                base,
                offset,
            } => match resolved(Reg(base), offset, pending) {
                Some(target) => format!("fsw f{source} {}", locate(target)),
                None => float.to_string(),
            },
            Float::Operation { operands, .. } => {
                if let Some(Register::Integer(dest)) = operands[0] {
                    pending.remove(&dest);
                }
                float.to_string()
            }
        },
    }
}

fn clobber(pending: &mut HashMap<u8, u32>) {
    for register in CALLER_SAVED {
        pending.remove(&register);
    }
}

/// The integer register a base instruction writes, if any.
fn destination(inst: &Inst) -> Option<u8> {
    use Inst::*;
    match *inst {
        AddiW { dest, .. }
        | Slti { dest, .. }
        | Sltiu { dest, .. }
        | Xori { dest, .. }
        | Ori { dest, .. }
        | Andi { dest, .. }
        | Slli { dest, .. }
        | SlliW { dest, .. }
        | Srli { dest, .. }
        | SrliW { dest, .. }
        | Srai { dest, .. }
        | SraiW { dest, .. }
        | Add { dest, .. }
        | AddW { dest, .. }
        | Sub { dest, .. }
        | SubW { dest, .. }
        | Sll { dest, .. }
        | SllW { dest, .. }
        | Slt { dest, .. }
        | Sltu { dest, .. }
        | Xor { dest, .. }
        | Srl { dest, .. }
        | SrlW { dest, .. }
        | Sra { dest, .. }
        | SraW { dest, .. }
        | Or { dest, .. }
        | And { dest, .. }
        | Mul { dest, .. }
        | MulW { dest, .. }
        | Mulh { dest, .. }
        | Mulhsu { dest, .. }
        | Mulhu { dest, .. }
        | Div { dest, .. }
        | DivW { dest, .. }
        | Divu { dest, .. }
        | DivuW { dest, .. }
        | Rem { dest, .. }
        | RemW { dest, .. }
        | Remu { dest, .. }
        | RemuW { dest, .. }
        | Lwu { dest, .. }
        | Ld { dest, .. }
        | LrW { dest, .. }
        | ScW { dest, .. }
        | AmoW { dest, .. } => Some(dest.0),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Symbols by start address, each `size` bytes.
    struct Symbols(Vec<(u64, u64, &'static str)>);

    impl Placement for Symbols {
        fn locate(&self, address: u64) -> String {
            self.0
                .iter()
                .find(|(start, size, _)| *start <= address && address < start + size)
                .map_or_else(
                    || format!("?{address:#x}"),
                    |(start, _, name)| format!("{name}+{:#x}", address - start),
                )
        }
        fn contains(&self, address: u64) -> bool {
            self.0
                .iter()
                .any(|(start, size, _)| *start <= address && address < start + size)
        }
    }

    fn u_type(opcode: u32, rd: u32, value: u32) -> u32 {
        (value & 0xffff_f000) | (rd << 7) | opcode
    }

    fn i_type(opcode: u32, funct3: u32, rd: u32, rs1: u32, imm: i32) -> u32 {
        ((imm as u32 & 0xfff) << 20) | (rs1 << 15) | (funct3 << 12) | (rd << 7) | opcode
    }

    fn jal(rd: u32, offset: i32) -> u32 {
        let o = offset as u32;
        ((o >> 20 & 1) << 31)
            | ((o >> 1 & 0x3ff) << 21)
            | ((o >> 11 & 1) << 20)
            | ((o >> 12 & 0xff) << 12)
            | (rd << 7)
            | 0x6f
    }

    /// `f` at `base` loads `DATA` at `data` through `lui`+`addi`, reads a
    /// word through the result, calls `callee` at `callee`, and returns.
    fn image(base: u32, data: u32, callee: u32) -> (Vec<u8>, Symbols) {
        let high = data.wrapping_add(0x800) & 0xffff_f000;
        let low = data.wrapping_sub(high) as i32;
        let words = [
            u_type(0x37, 10, high),                        // lui a0, %hi(DATA)
            i_type(0x13, 0, 10, 10, low),                  // addi a0, a0, %lo(DATA)
            i_type(0x03, 2, 11, 10, 4),                    // lw a1, 4(a0)
            jal(1, callee.wrapping_sub(base + 12) as i32), // jal ra, callee
            i_type(0x13, 0, 12, 10, 8),                    // addi a2, a0, 8: a0 is clobbered
            0x0000_8067,                                   // ret
        ];
        let bytes = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let symbols = Symbols(vec![
            (u64::from(base), 24, "f"),
            (u64::from(data), 16, "DATA"),
            (u64::from(callee), 4, "callee"),
        ]);
        (bytes, symbols)
    }

    #[test]
    fn two_placements_of_one_function_list_alike() {
        let (first, at_first) = image(0x4000_0000, 0x4080_1234, 0x4000_0800);
        let (second, at_second) = image(0x4200_0100, 0x3fc8_8f00, 0x4200_4000);
        let a = normalize(0x4000_0000, &first, &at_first);
        let b = normalize(0x4200_0100, &second, &at_second);
        assert_ne!(first, second, "the placements change the bytes");
        assert_eq!(a, b);
        assert_eq!(
            a,
            [
                "lui a0 <pair>",
                "addi a0,a0 DATA+0x0",
                "lw a1 DATA+0x4",
                "jal ra callee+0x0",
                "addi a2, a0, 8",
                "ret",
            ]
        );
    }

    #[test]
    fn a_changed_instruction_lists_differently() {
        let (mut bytes, symbols) = image(0x4000_0000, 0x4080_1234, 0x4000_0800);
        let a = normalize(0x4000_0000, &bytes, &symbols);
        bytes[8..12].copy_from_slice(&i_type(0x03, 2, 11, 10, 8).to_le_bytes());
        let b = normalize(0x4000_0000, &bytes, &symbols);
        assert_ne!(a, b);
        assert_eq!(b[2], "lw a1 DATA+0x8");
    }

    #[test]
    fn compressed_and_full_encodings_list_alike() {
        let symbols = Symbols(Vec::new());
        // c.addi a0, 1 and addi a0, a0, 1.
        let compressed = normalize(0, &0x0505_u16.to_le_bytes(), &symbols);
        let full = normalize(0, &i_type(0x13, 0, 10, 10, 1).to_le_bytes(), &symbols);
        assert_eq!(compressed, full);
    }
}

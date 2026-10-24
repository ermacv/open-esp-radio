//! The end of the bootstrap's stack path: its jump into stage two.
//!
//! The bootstrap enters stage two at an address it knows at build time and
//! never uses its own stack again. The stack policy names the entry's symbol
//! in the stage-two ELF (`handoff`); the path ends at the entry only after
//! both facts are verified, each in the ELF that holds it:
//!
//! 1. in the bootstrap, an indirect jump whose register provably holds the
//!    entry's address;
//! 2. in stage two, every path from the entry installs the stack pointer
//!    (`sp` = the CPU0 task stack's top) before anything reads `sp` or
//!    leaves the function other than through a call that does the same.
//!
//! Anything else stays a hole, as before.

use std::collections::BTreeSet;
use std::path::Path;

use oer_analysis_elf::Elf;
use oer_analysis_riscv_decode::{Extensions, Inst, Instruction, decode};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::{Fact, FunctionSemantics, SemanticOp};

use super::{Image, Result};

/// The verified end of a bootstrap stack path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Handoff {
    pub symbol: String,
    pub entry: u32,
    /// The bootstrap's jump sites to the entry, with their functions.
    pub sites: Vec<(u32, String)>,
}

impl Handoff {
    pub fn render(&self) -> String {
        let mut out = format!(
            "path end: stage-two entry `{}` ({:#010x}); it installs its stack pointer before any \
             stack access\n",
            self.symbol, self.entry
        );
        for (site, function) in &self.sites {
            out.push_str(&format!("  jump at {site:#010x} in {function}\n"));
        }
        out
    }
}

impl Image {
    /// End the stack path of every jump out of this (bootstrap) image whose
    /// register holds the address of `symbol` in `stage_two`, after verifying
    /// that the entry installs `stack_top` as its stack pointer first.
    pub fn end_path_at_stage_two(
        &mut self,
        symbol: &str,
        stage_two: &Path,
        stack_top: &str,
    ) -> Result<Handoff> {
        let bytes = std::fs::read(stage_two)?;
        let elf = Elf::executable(&bytes)?;
        let entry = u32::try_from(elf.address(symbol).ok_or_else(|| {
            format!("the stage-two ELF has no `{symbol}`, the bootstrap policy's handoff")
        })?)?;
        let top = u32::try_from(
            elf.address(stack_top)
                .ok_or_else(|| format!("the stage-two ELF has no `{stack_top}`"))?,
        )?;
        installs_stack_pointer(&elf, entry, top, 0, &mut BTreeSet::new())
            .map_err(|problem| format!("stage-two entry `{symbol}`: {problem}"))?;

        let own = Elf::executable(&self.elf)?;
        let mut sites = Vec::new();
        for facts in self.analysis.functions.values() {
            for transfer in &facts.transfers {
                // The analysis may have found the constant itself; otherwise
                // the jump is a hole and the constant is read from the code.
                let goes_there = match transfer.target {
                    Some(target) => target == entry,
                    None => {
                        constant_jump_target(&own, &facts.function, transfer.site) == Some(entry)
                    }
                };
                if transfer.kind == oer_analysis_riscv_stack::TransferKind::Tail && goes_there {
                    sites.push((transfer.site, self.function_at(transfer.site)));
                }
            }
        }
        if sites.is_empty() {
            return Err(format!(
                "the bootstrap policy names handoff `{symbol}` ({entry:#010x}), but no jump of the \
                 bootstrap provably goes there"
            )
            .into());
        }
        for (site, _) in &sites {
            self.resolutions.add(*site, Fact::StageTwoEntry, [entry]);
        }
        self.resolutions.add_exit(entry);
        Ok(Handoff {
            symbol: symbol.to_owned(),
            entry,
            sites,
        })
    }
}

/// The file bytes of `function`: found by its address, in the section that
/// holds all of them.
pub(super) fn function_code<'a>(
    elf: &Elf<'a>,
    function: &oer_analysis_elf::Function,
) -> Option<&'a [u8]> {
    let address = u64::from(function.address);
    let section = elf.sections().find(|section| section.contains(address))?;
    let start = usize::try_from(address.checked_sub(section.address)?).ok()?;
    section
        .data
        .get(start..start.checked_add(function.size as usize)?)
}

/// The address a `jr` at `site` in `function` jumps to, when it is the
/// `lui`/`addi` constant (`li`) written to its register by the two
/// instructions right before it, and no branch or jump of the function lands
/// on either the `addi` or the `jr` (the constant is the only way in).
pub(super) fn constant_jump_target(
    elf: &Elf<'_>,
    function: &oer_analysis_elf::Function,
    site: u32,
) -> Option<u32> {
    let code = function_code(elf, function)?;
    let mut instructions = Vec::new();
    let mut offset = 0;
    while offset < code.len() {
        // An encoding outside the decoder's set still has its length in its
        // low bits and cannot be a branch the scan misses.
        let (instruction, length) = decode(&code[offset..], Extensions::ALL).unwrap_or((
            Instruction::Unimp,
            if code[offset] & 3 == 3 { 4 } else { 2 },
        ));
        instructions.push((function.address + offset as u32, instruction, length as u32));
        offset += length;
    }
    let at = instructions.iter().position(|&(pc, ..)| pc == site)?;
    let [(lui_pc, lui, _), (addi_pc, addi, _), (_, jump, _)] =
        instructions.get(at.checked_sub(2)?..=at)?
    else {
        return None;
    };
    let Instruction::Base(Inst::Jalr { offset, base, dest }) = *jump else {
        return None;
    };
    let (
        Instruction::Base(Inst::Lui { uimm, dest: upper }),
        Instruction::Base(Inst::Addi {
            imm,
            dest: low,
            src1,
        }),
    ) = (*lui, *addi)
    else {
        return None;
    };
    if dest.0 != 0 || upper.0 != base.0 || low.0 != base.0 || src1.0 != base.0 {
        return None;
    }
    // An `li`-then-`jr` pair jumps to a constant; a return jumps out.
    let constant_jump = |index: usize| {
        let Some(previous) = index
            .checked_sub(2)
            .and_then(|first| instructions.get(first..index))
        else {
            return false;
        };
        matches!(
            (previous[0].1, previous[1].1, instructions[index].1),
            (
                Instruction::Base(Inst::Lui { dest: a, .. }),
                Instruction::Base(Inst::Addi { dest: b, src1: c, .. }),
                Instruction::Base(Inst::Jalr { base, .. }),
            ) if a.0 == b.0 && b.0 == c.0 && c.0 == base.0
        )
    };
    let joined = instructions
        .iter()
        .enumerate()
        .any(|(index, &(pc, instruction, _))| match instruction {
            Instruction::Base(
                Inst::Jal { offset, .. }
                | Inst::Beq { offset, .. }
                | Inst::Bne { offset, .. }
                | Inst::Blt { offset, .. }
                | Inst::Bge { offset, .. }
                | Inst::Bltu { offset, .. }
                | Inst::Bgeu { offset, .. },
            ) => [*addi_pc, site].contains(&pc.wrapping_add(offset.as_u32())),
            // A computed jump inside the function could land anywhere.
            Instruction::Base(Inst::Jalr { dest, base, .. }) => {
                dest.0 == 0 && !matches!(base.0, 1 | 5) && !constant_jump(index)
            }
            _ => false,
        });
    let _ = lui_pc;
    (!joined).then(|| {
        uimm.as_u32()
            .wrapping_add(imm.as_u32())
            .wrapping_add(offset.as_u32())
            & !1
    })
}

/// How deep calls before the installation may nest.
const CALL_DEPTH: usize = 4;
/// Most instructions the scan visits in one entry.
const STEP_LIMIT: usize = 4096;

/// Whether every path from `entry` installs `sp = top` before reading `sp`,
/// leaving the function, or reaching an instruction the scan does not know.
/// A call is followed into its callee, which must do the same; the
/// installation then holds on return.
pub(super) fn installs_stack_pointer(
    elf: &Elf<'_>,
    entry: u32,
    top: u32,
    depth: usize,
    calls: &mut BTreeSet<u32>,
) -> std::result::Result<(), String> {
    if depth > CALL_DEPTH {
        return Err("calls nest too deeply before the stack pointer is installed".into());
    }
    let function = elf
        .functions()
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|function| function.address == entry)
        .ok_or_else(|| format!("{entry:#010x} is not the start of a function"))?;
    let code = function_code(elf, &function).ok_or("the function's bytes are not in the file")?;
    let end = entry + function.size;
    let mut seen = BTreeSet::new();
    let mut pending = vec![(entry, None::<(u8, u32)>)];
    while let Some((pc, upper)) = pending.pop() {
        if !seen.insert((pc, upper)) {
            continue;
        }
        if seen.len() > STEP_LIMIT {
            return Err("the scan exceeds its step limit".into());
        }
        let offset = (pc - entry) as usize;
        let Some(bytes) = code.get(offset..) else {
            return Err(format!("control leaves the function at {pc:#010x}"));
        };
        let (instruction, length) = decode(bytes, Extensions::ALL)
            .ok_or_else(|| format!("undecodable instruction at {pc:#010x}"))?;
        let next = pc + length as u32;
        let relative = |displacement: u32| pc.wrapping_add(displacement);
        let within = |target: u32| target >= entry && target < end;
        match instruction {
            Instruction::Base(Inst::Jal { offset, dest }) => {
                let target = relative(offset.as_u32());
                if dest.0 == 0 {
                    if !within(target) {
                        return Err(format!("the jump at {pc:#010x} leaves the function"));
                    }
                    pending.push((target, None));
                } else {
                    // A call: the callee installs the stack pointer, so the path ends.
                    call(elf, target, top, depth, calls, pc)?;
                }
                continue;
            }
            Instruction::Base(Inst::Jalr { offset, base, dest }) => match (upper, dest.0) {
                (Some((register, value)), 1) if register == base.0 => {
                    let target = value.wrapping_add(offset.as_u32()) & !1;
                    call(elf, target, top, depth, calls, pc)?;
                    continue;
                }
                _ => {
                    return Err(format!(
                        "the indirect jump at {pc:#010x} is not a call to a known function"
                    ));
                }
            },
            Instruction::Base(
                Inst::Beq { offset, src1, src2 }
                | Inst::Bne { offset, src1, src2 }
                | Inst::Blt { offset, src1, src2 }
                | Inst::Bge { offset, src1, src2 }
                | Inst::Bltu { offset, src1, src2 }
                | Inst::Bgeu { offset, src1, src2 },
            ) => {
                if src1.0 == 2 || src2.0 == 2 {
                    return Err(format!("the branch at {pc:#010x} reads sp"));
                }
                let target = relative(offset.as_u32());
                if !within(target) {
                    return Err(format!("the branch at {pc:#010x} leaves the function"));
                }
                pending.push((target, None));
                pending.push((next, None));
                continue;
            }
            _ => {}
        }
        // The installation: `auipc sp, hi` followed by `addi sp, sp, lo`
        // forms `top`; nothing else may write or read `sp`.
        if let Instruction::Base(Inst::Auipc { uimm, dest }) = instruction
            && dest.0 == 2
        {
            let value = relative(uimm.as_u32());
            let Some(&(Instruction::Base(Inst::Addi { imm, dest, src1 }), _)) = code
                .get(offset + length..)
                .and_then(|rest| decode(rest, Extensions::ALL))
                .as_ref()
            else {
                return Err(format!(
                    "the write of sp at {pc:#010x} is not `la sp, symbol`"
                ));
            };
            if dest.0 != 2 || src1.0 != 2 || value.wrapping_add(imm.as_u32()) != top {
                return Err(format!(
                    "sp is not set to the CPU0 task stack's top at {pc:#010x}"
                ));
            }
            continue;
        }
        let operation = RiscvDecoder.lift(bytes);
        let upper_now = match instruction {
            Instruction::Base(Inst::Lui { uimm, dest }) => Some((dest.0, uimm.as_u32())),
            Instruction::Base(Inst::Auipc { uimm, dest }) => {
                Some((dest.0, relative(uimm.as_u32())))
            }
            _ => None,
        };
        // A CSR access (`csrr t0, mhartid`) touches no memory: it may not
        // write `sp`, and the `reads_register` check below covers its reads.
        if let Instruction::Extension(oer_analysis_riscv_decode::Extension::Csr { .. }) =
            instruction
        {
            if matches!(operation, SemanticOp::Opaque { dest: 2 }) {
                return Err(format!("sp is written at {pc:#010x} by a CSR access"));
            }
            pending.push((next, None));
            continue;
        }
        match operation {
            SemanticOp::Integer { dest, .. } | SemanticOp::Upper { dest, .. } if dest == 2 => {
                return Err(format!(
                    "sp is written at {pc:#010x} other than by `la sp, symbol`"
                ));
            }
            SemanticOp::Integer { .. } | SemanticOp::Upper { .. } | SemanticOp::Fence { .. } => {}
            SemanticOp::Memory { dest, .. } if dest == Some(2) => {
                return Err(format!("sp is loaded at {pc:#010x}"));
            }
            SemanticOp::Memory { .. } => {}
            _ => {
                return Err(format!(
                    "the instruction at {pc:#010x} is not one the scan knows"
                ));
            }
        }
        if operation.reads_register(2) {
            return Err(format!("sp is read at {pc:#010x} before it is installed"));
        }
        pending.push((
            next,
            upper_now.filter(|_| matches!(operation, SemanticOp::Upper { .. })),
        ));
    }
    Ok(())
}

fn call(
    elf: &Elf<'_>,
    target: u32,
    top: u32,
    depth: usize,
    calls: &mut BTreeSet<u32>,
    site: u32,
) -> std::result::Result<(), String> {
    if !calls.insert(target) {
        return Err(format!(
            "the call at {site:#010x} repeats a callee on the path"
        ));
    }
    let result = installs_stack_pointer(elf, target, top, depth + 1, calls)
        .map_err(|problem| format!("callee {target:#010x} of the call at {site:#010x}: {problem}"));
    calls.remove(&target);
    result
}

//! Triage aid for untriaged vendor locations.
//!
//! For every location the scenarios leave untriaged, the report shows the
//! instructions around it, the in-function definitions of the registers the
//! instruction at the location reads, and whether the block it opens only
//! calls reviewed diagnostic functions and stores nothing outside the stack.
//! Instructions are decoded and lifted by Blobray's RISC-V decoder; a forward
//! pass over each function folds constants, so the report shows the
//! addresses, register names and fields an instruction touches. These are
//! proposals for a reviewer: exclusions stay reviewed decisions.
use crate::inspect::{Corpus, STACK, Step, ZERO};
use oer_riscv_model::{InstructionFlow, MemoryKind, Operand, SemanticOp};
use oer_vendor_evidence_shard::{Location, LocationKind};
use oer_vendor_scenario_engine::harness::Result;
use oer_vendor_scenario_engine::registers::Registers;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Instructions shown before and after a location.
const BEFORE: usize = 10;
const AFTER: usize = 3;
/// Definitions followed back from a location's operands.
const DEPTH: usize = 2;
/// Origins of the linked production image and the vendor ROM, in the order
/// [`Code::of`] takes them: an earlier image wins a function name.
const ORIGINS: [&str; 2] = ["image", "rom"];

/// One decoded instruction of a function, annotated by the inspection pass.
pub(crate) type Line = Step;

/// The functions of the linked images and the reviewed diagnostic output.
pub struct Code {
    corpus: Corpus,
    diagnostic: BTreeSet<String>,
}

impl Code {
    /// The code functions of `elfs`, the linked image then the ROM; the
    /// chip's published `registers`; the functions reviewed decisions name
    /// as `diagnostic` output.
    pub fn of(elfs: &[&[u8]], registers: Registers, diagnostic: BTreeSet<String>) -> Result<Self> {
        let objects: Vec<(&str, &[u8])> =
            ORIGINS.iter().copied().zip(elfs.iter().copied()).collect();
        Ok(Self {
            corpus: Corpus::of(&objects, registers)?,
            diagnostic,
        })
    }

    /// The decoding of `function` from the first image that defines it,
    /// each line annotated with what the inspection pass resolves.
    pub(crate) fn lines(&self, function: &str) -> Option<Vec<Line>> {
        Some(self.corpus.walk(self.corpus.function(function)?))
    }

    /// How the straight-line code from `index` ends, when it only calls
    /// reviewed diagnostic functions, stores nothing outside the stack, and
    /// reads nothing outside the stack after its last call: work after the
    /// output is behavior, not diagnostics.
    fn diagnostic_only(&self, lines: &[Line], index: usize) -> Option<Ending> {
        let mut calls = 0;
        for line in &lines[index..] {
            if let SemanticOp::Memory {
                kind, base, dest, ..
            } = line.op
            {
                let stack_store = base == STACK && dest.is_none() && kind == MemoryKind::Store;
                let late_read = kind == MemoryKind::Load && base != STACK && calls > 0;
                if (kind != MemoryKind::Load && !stack_store) || late_read {
                    return None;
                }
            }
            match line.flow {
                InstructionFlow::Jump { link: true, .. }
                | InstructionFlow::Indirect { link: true, .. } => {
                    calls += 1;
                    if !line
                        .callee
                        .as_ref()
                        .is_some_and(|name| self.diagnostic.contains(name))
                    {
                        return None;
                    }
                }
                InstructionFlow::Next => {}
                InstructionFlow::Jump {
                    displacement: 0,
                    link: false,
                } => return (calls > 0).then_some(Ending::Spins),
                InstructionFlow::Indirect { link: false, .. }
                | InstructionFlow::Return { .. }
                | InstructionFlow::Stop => {
                    return (calls > 0).then_some(Ending::Returns);
                }
                InstructionFlow::Branch { .. } | InstructionFlow::Jump { .. } => {
                    return (calls > 0).then_some(Ending::Continues);
                }
            }
        }
        None
    }
}

/// How a diagnostic-only block ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Ending {
    /// Jumps to itself: an assertion that logs and stops the core.
    Spins,
    /// Returns from its function after the output.
    Returns,
    /// Continues on another path after the output.
    Continues,
}

impl Ending {
    fn label(self) -> &'static str {
        match self {
            Ending::Spins => "assertion: reviewed diagnostic output, then the core spins",
            Ending::Returns => {
                "reviewed diagnostic output, then a return: the early return is behavior"
            }
            Ending::Continues => "reviewed diagnostic output, then another path",
        }
    }
}

/// Registers `line` writes and reads.
fn operands(line: &Line) -> (Option<u8>, Vec<u8>) {
    let register = |operand: Operand| match operand {
        Operand::Register(r) => Some(r),
        Operand::Immediate(_) => None,
    };
    match line.op {
        SemanticOp::Integer {
            dest, left, right, ..
        } => (
            Some(dest),
            [left, right].into_iter().filter_map(register).collect(),
        ),
        SemanticOp::Upper { dest, .. } | SemanticOp::Link { dest } => (Some(dest), vec![]),
        // Its inputs are FP registers, which the integer model does not track.
        SemanticOp::Opaque { dest } => (Some(dest), vec![]),
        SemanticOp::Memory {
            base, dest, source, ..
        } => (dest, [Some(base), source].into_iter().flatten().collect()),
        SemanticOp::Fence { .. } | SemanticOp::None | SemanticOp::Unsupported => {
            (None, line.branch.clone())
        }
    }
}

fn name(register: u8) -> String {
    const ABI: [&str; 32] = [
        "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3", "a4",
        "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11", "t3", "t4",
        "t5", "t6",
    ];
    ABI.get(usize::from(register))
        .map_or_else(|| format!("x{register}"), |name| (*name).to_owned())
}

/// The in-function definitions of the registers the instruction at `index`
/// reads, followed `DEPTH` steps back.
fn definitions(lines: &[Line], index: usize) -> Vec<String> {
    let wanted_of = |at: usize, depth: usize| {
        operands(&lines[at])
            .1
            .into_iter()
            .filter(|r| *r != ZERO && *r != STACK)
            .map(move |r| (r, at, depth))
            .collect::<Vec<_>>()
    };
    let mut found = vec![];
    let mut wanted = wanted_of(index, 0);
    let mut seen = BTreeSet::new();
    while let Some((register, from, depth)) = wanted.pop() {
        let Some(at) = (0..from)
            .rev()
            .find(|i| operands(&lines[*i]).0 == Some(register))
        else {
            found.push(format!(
                "{}: an argument or not defined in this function",
                name(register)
            ));
            continue;
        };
        if !seen.insert(at) {
            continue;
        }
        found.push(format!(
            "{} <- +{:#x}: {}",
            name(register),
            lines[at].offset,
            lines[at].display()
        ));
        if depth + 1 < DEPTH {
            wanted.extend(wanted_of(at, depth + 1));
        }
    }
    found
}

/// The index of the transfer that ends the block opened at `index`: a
/// branch, jump, return or other stop; calls do not end it.
fn block_end(lines: &[Line], index: usize) -> usize {
    lines[index..]
        .iter()
        .position(|line| {
            !matches!(
                line.flow,
                InstructionFlow::Next
                    | InstructionFlow::Jump { link: true, .. }
                    | InstructionFlow::Indirect { link: true, .. }
            )
        })
        .map_or(lines.len().saturating_sub(1), |offset| index + offset)
}

/// The report of `untriaged` over `code`.
pub fn report(code: &Code, untriaged: &BTreeSet<Location>) -> String {
    let mut text = String::new();
    let mut by_function: BTreeMap<&str, Vec<&Location>> = BTreeMap::new();
    for location in untriaged {
        by_function
            .entry(&location.function)
            .or_default()
            .push(location);
    }
    for (function, locations) in by_function {
        text.push_str(&format!("\n== {function}: {} locations\n", locations.len()));
        let Some(lines) = code.lines(function) else {
            text.push_str("  (no code in the linked image or ROM)\n");
            continue;
        };
        for location in locations {
            let Some(index) = lines.iter().position(|l| l.offset >= location.offset) else {
                continue;
            };
            let ending = matches!(location.kind, LocationKind::Block)
                .then(|| code.diagnostic_only(&lines, index))
                .flatten();
            let candidate = ending.is_some();
            text.push_str(&format!(
                "\n  +{:#x} {:?}{}\n",
                location.offset,
                location.kind,
                ending.map_or_else(String::new, |e| format!("  [candidate: {}]", e.label()))
            ));
            let start = index.saturating_sub(BEFORE);
            // A candidate shows its whole block through the transfer that
            // ends it: whether it rejoins the other path or returns is the
            // reviewer's question.
            let end = if candidate {
                block_end(&lines, index) + AFTER
            } else {
                index + AFTER
            };
            for (i, line) in lines
                .iter()
                .enumerate()
                .take((end + 1).min(lines.len()))
                .skip(start)
            {
                let mark = if i == index { ">" } else { " " };
                text.push_str(&format!(
                    "   {mark} +{:04x}  {}\n",
                    line.offset,
                    line.display()
                ));
            }
            for definition in definitions(&lines, index) {
                text.push_str(&format!("      {definition}\n"));
            }
        }
    }
    text
}

/// Every function of `untriaged` in full, each instruction marked by the
/// locations at its offset: `U` untriaged, `E` excluded by a reviewed
/// decision (its reason follows the listing, by number), `C` reached only
/// through excluded functions; unmarked code is covered.
pub fn functions(
    code: &Code,
    untriaged: &BTreeSet<Location>,
    uncovered: &BTreeSet<Location>,
    consequential: &BTreeSet<Location>,
    reason: impl Fn(&Location) -> Option<&'static str>,
) -> String {
    let mut text = String::new();
    let names: BTreeSet<&str> = untriaged.iter().map(|l| l.function.as_str()).collect();
    for function in names {
        let Some(lines) = code.lines(function) else {
            continue;
        };
        let mut reasons: Vec<&'static str> = vec![];
        let at = |offset: u32| {
            uncovered
                .iter()
                .filter(move |l| l.function == function && l.offset == offset)
        };
        text.push_str(&format!("\n== {function}\n"));
        for line in &lines {
            let mut marks = vec![];
            for location in at(line.offset) {
                let kind = match location.kind {
                    LocationKind::Block => "block",
                    LocationKind::Taken => "taken",
                    LocationKind::Fallthrough => "fallthrough",
                    LocationKind::Followed => "followed",
                    LocationKind::Unresolved => "unresolved",
                };
                let mark = if untriaged.contains(location) {
                    format!("U {kind}")
                } else if consequential.contains(location) {
                    format!("C {kind}")
                } else if let Some(reason) = reason(location) {
                    let number = reasons
                        .iter()
                        .position(|r| *r == reason)
                        .unwrap_or_else(|| {
                            reasons.push(reason);
                            reasons.len() - 1
                        });
                    format!("E{} {kind}", number + 1)
                } else {
                    format!("? {kind}")
                };
                marks.push(mark);
            }
            text.push_str(&format!(
                "  {:<24} +{:04x}  {}\n",
                marks.join(", "),
                line.offset,
                line.display()
            ));
        }
        for (number, reason) in reasons.iter().enumerate() {
            text.push_str(&format!("  E{}: {reason}\n", number + 1));
        }
    }
    text
}

/// Write the report of `untriaged` below `run`; its path.
pub fn write(
    run: &Path,
    scenario: &str,
    code: &Code,
    untriaged: &BTreeSet<Location>,
) -> std::io::Result<PathBuf> {
    let path = run.join(format!("untriaged-{scenario}.txt"));
    std::fs::write(&path, report(code, untriaged))?;
    Ok(path)
}

#[cfg(test)]
mod tests;

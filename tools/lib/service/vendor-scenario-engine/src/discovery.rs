//! Every undeclared memory a failing request reaches, from one run.
//!
//! A case side that stops at an access outside its declared memory reports
//! only that first access. When a request misses its expected verdict, the
//! discovery reruns it with each such access mapped: a data access to its
//! containing symbol, zero-filled, and a fetch to a call that returns zero,
//! until no side stops at an undeclared access. The failure report then lists
//! every missing symbol at once. The mapped bytes and answers are placeholders
//! for the declaration a scenario still owes; they never produce evidence.
use crate::harness::Result;
use blobray_domain::{
    CallBinding, CallBoundary, CallDeclaration, CallRepetition, CallResponse, ExecutionCase,
    ExecutionEvidence, ExecutionGap, ExecutionStop, Invocation, MemoryAccess, RegionLifetime,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Reruns one discovery may take.
pub const ATTEMPTS: usize = 32;
/// Bytes mapped for an access no symbol contains.
const WORD: u32 = 4;
/// Argument words a placeholder call receives.
const ARGUMENT_WORDS: u16 = 2;

/// One undeclared access the request reached.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Missing {
    pub case: u32,
    pub name: String,
    pub replacement: bool,
    pub access: String,
    pub address: u32,
    /// The symbol or register that holds the address, with its extent.
    pub owner: String,
    pub start: u32,
    pub length: u32,
    /// Why discovery stopped here without mapping it, when it did.
    pub unresolved: Option<String>,
}

/// Named address ranges of the linked images, the ROM and the chip's
/// registers.
#[derive(Default)]
pub struct Symbols {
    ranges: BTreeMap<u32, (String, u32)>,
    registers: crate::registers::Registers,
}

impl Symbols {
    /// The defined, sized symbols of every ELF in `elfs`; an earlier ELF
    /// wins an address.
    pub fn of(elfs: &[&[u8]], registers: crate::registers::Registers) -> Self {
        let mut ranges = BTreeMap::new();
        for bytes in elfs {
            let Ok(file) = oer_elf::Elf::parse(bytes) else {
                continue;
            };
            for symbol in file.symbols() {
                let name = symbol.name;
                if symbol.section.is_none()
                    || !matches!(
                        symbol.kind,
                        oer_elf::SymbolKind::Data | oer_elf::SymbolKind::Text
                    )
                    || symbol.size == 0
                {
                    continue;
                }
                let (Ok(address), Ok(size)) =
                    (u32::try_from(symbol.address), u32::try_from(symbol.size))
                else {
                    continue;
                };
                ranges
                    .entry(address)
                    .or_insert_with(|| (name.to_owned(), size));
            }
        }
        Self { ranges, registers }
    }

    /// The owner of `address`: its symbol with start and length, a
    /// published register word, or the bare word.
    pub fn owner(&self, address: u32) -> (String, u32, u32) {
        if let Some((start, (name, size))) = self.ranges.range(..=address).next_back()
            && address - start < *size
        {
            return (format!("{name}+{:#x}", address - start), *start, *size);
        }
        let word = address & !(WORD - 1);
        (self.registers.name(address), word, WORD)
    }
}

/// The first case side that stopped at an access outside its memory.
fn first_gap(records: &[ExecutionEvidence]) -> Option<(u32, bool, u32, MemoryAccess)> {
    records.iter().find_map(|record| match record {
        ExecutionEvidence::Outcome {
            case,
            replacement,
            stop:
                ExecutionStop::Incomplete {
                    reason: ExecutionGap::Memory { address, access },
                    ..
                },
            ..
        } => Some((*case, *replacement, *address, *access)),
        _ => None,
    })
}

/// One side of `case`.
fn side(case: &mut ExecutionCase, replacement: bool) -> Option<&mut Invocation> {
    if replacement {
        case.replacement.as_mut()
    } else {
        Some(&mut case.vendor)
    }
}

fn declared(invocation: &Invocation, address: u32) -> bool {
    invocation.memory.iter().any(|region| {
        let seed = &region.seed;
        address >= seed.address && u64::from(address - seed.address) < u64::from(seed.length)
    })
}

/// A call at `address` that returns zero, for as long as it is called.
fn placeholder_call(owner: &str, address: u32) -> CallDeclaration {
    CallDeclaration {
        id: format!("discovered {owner}"),
        applicability: "undeclared callee mapped by discovery; never evidence".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address,
            boundary: CallBoundary::Unmapped,
            allow_tail: true,
        },
        argument_words: ARGUMENT_WORDS,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Unbounded,
    }
}

/// Every undeclared access of a request's `cases` whose first run gave
/// `records`, rerunning the cases through `run` with each found access
/// mapped.
pub fn discover(
    cases: &[ExecutionCase],
    records: &[ExecutionEvidence],
    symbols: &Symbols,
    mut run: impl FnMut(&[ExecutionCase]) -> Result<Vec<ExecutionEvidence>>,
) -> Vec<Missing> {
    let mut cases = cases.to_vec();
    let mut records: Vec<ExecutionEvidence> =
        records.iter().filter(|r| is_outcome(r)).cloned().collect();
    let chains = chains(&cases);
    let mut found: Vec<Missing> = vec![];
    let mut seen = BTreeSet::new();
    for _ in 0..ATTEMPTS {
        let Some((case, replacement, address, access)) = first_gap(&records) else {
            break;
        };
        let (owner, start, length) = symbols.owner(address);
        let mut missing = Missing {
            case,
            name: cases
                .get(case as usize)
                .map_or_else(String::new, |c| c.name.clone()),
            replacement,
            access: format!("{access:?}").to_lowercase(),
            address,
            owner: owner.clone(),
            start,
            length,
            unresolved: None,
        };
        let Some(entry) = cases
            .get_mut(case as usize)
            .and_then(|c| side(c, replacement))
            .map(|s| s.entry)
        else {
            missing.unresolved = Some("the case side is not in the request".into());
            found.push(missing);
            break;
        };
        if !seen.insert((entry, replacement, address)) {
            missing.unresolved = Some("mapping it did not remove the stop".into());
            found.push(missing);
            break;
        }
        let stopped = cases
            .get_mut(case as usize)
            .and_then(|c| side(c, replacement));
        if stopped.is_some_and(|stopped| declared(stopped, address)) {
            missing.unresolved =
                Some("inside a declared region whose bytes are unknown there".into());
            found.push(missing);
            break;
        }
        // Every case entering the same function on this side reaches the
        // same undeclared memory: map it for all of them in one rerun.
        let region = if access == MemoryAccess::Fetch {
            None
        } else {
            match crate::harness::filled(start, length, 0) {
                Ok(region) => Some(region),
                Err(error) => {
                    missing.unresolved = Some(error.to_string());
                    found.push(missing);
                    break;
                }
            }
        };
        let mut changed = BTreeSet::new();
        for (index, invocation) in cases
            .iter_mut()
            .enumerate()
            .filter_map(|(i, c)| side(c, replacement).map(|s| (i, s)))
        {
            if invocation.entry != entry || declared(invocation, address) {
                continue;
            }
            changed.insert(index);
            match &region {
                Some(region) => invocation.memory.push(region.clone()),
                None if !invocation
                    .calls
                    .iter()
                    .any(|call| call.binding.address == address) =>
                {
                    invocation.calls.push(placeholder_call(&owner, address));
                }
                None => {}
            }
        }
        found.push(missing.clone());
        // Only the chains of cold-started cases that gained a mapping rerun;
        // the others keep their outcomes.
        let mut failed = None;
        for range in chains
            .iter()
            .filter(|range| changed.range((*range).clone()).next().is_some())
        {
            match run(&cases[range.clone()]) {
                Ok(next) => {
                    records.retain(|r| {
                        !outcome_case(r).is_some_and(|c| range.contains(&(c as usize)))
                    });
                    records.extend(next.into_iter().filter(is_outcome).map(|mut r| {
                        if let ExecutionEvidence::Outcome { case, .. } = &mut r {
                            *case += range.start as u32;
                        }
                        r
                    }));
                }
                Err(error) => {
                    failed = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = failed {
            if let Some(last) = found.last_mut() {
                last.unresolved = Some(format!("the rerun with it mapped failed: {error}"));
            }
            break;
        }
        records.sort_by_key(|r| outcome_case(r).unwrap_or(u32::MAX));
    }
    found
}

fn is_outcome(record: &ExecutionEvidence) -> bool {
    matches!(record, ExecutionEvidence::Outcome { .. })
}

fn outcome_case(record: &ExecutionEvidence) -> Option<u32> {
    match record {
        ExecutionEvidence::Outcome { case, .. } => Some(*case),
        _ => None,
    }
}

/// The case ranges that each start with a cold reset: a warm case depends
/// on the cases before it back to its chain's start.
fn chains(cases: &[ExecutionCase]) -> Vec<std::ops::Range<usize>> {
    let mut starts: Vec<usize> = cases
        .iter()
        .enumerate()
        .filter(|(_, c)| c.reset == blobray_domain::SessionReset::Cold)
        .map(|(i, _)| i)
        .collect();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    starts
        .iter()
        .zip(starts.iter().skip(1).chain([&cases.len()]))
        .map(|(start, end)| *start..*end)
        .collect()
}

/// The readable form of `missing`.
pub fn render(missing: &[Missing]) -> String {
    if missing.is_empty() {
        return String::new();
    }
    let mut text = String::from(
        "\nundeclared accesses (each rerun mapped the previous ones as placeholders):\n",
    );
    for m in missing {
        text.push_str(&format!(
            "  {} {} {:#010x} in {} ({:#010x}, {} bytes), first in case {} `{}`{}\n",
            if m.replacement {
                "production"
            } else {
                "vendor"
            },
            m.access,
            m.address,
            m.owner,
            m.start,
            m.length,
            m.case,
            m.name,
            m.unresolved
                .as_ref()
                .map_or_else(String::new, |why| format!("; unresolved: {why}"))
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use blobray_domain::{ExecutionGoal, SessionReset};

    fn outcome(address: Option<(u32, MemoryAccess)>) -> Vec<ExecutionEvidence> {
        let stop = match address {
            Some((address, access)) => ExecutionStop::Incomplete {
                pc: 0x4000_0000,
                reason: ExecutionGap::Memory { address, access },
            },
            None => ExecutionStop::Returned {
                low: Some(0),
                high: None,
            },
        };
        vec![ExecutionEvidence::Outcome {
            case: 0,
            replacement: false,
            stop,
            steps: 1,
        }]
    }

    fn cases() -> Vec<ExecutionCase> {
        let vendor = Invocation {
            entry: 0x4000_0000,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            preload: vec![],
            models: vec![],
            calls: vec![],
            observe_memory: vec![],
            observe_calls: None,
            observe_timeline: crate::harness::TIMELINE,
        };
        let case = ExecutionCase {
            name: "append".into(),
            reset: SessionReset::Cold,
            stack_fill: None,
            vendor,
            replacement: None,
            relation: None,
        };
        // A warm case of the same chain, then a chain of its own entering
        // another function.
        let mut other = case.clone();
        other.name = "append-other".into();
        other.reset = SessionReset::Warm;
        let mut unrelated = case.clone();
        unrelated.name = "unrelated".into();
        unrelated.vendor.entry = 0x4000_1000;
        vec![case, other, unrelated]
    }

    fn symbols() -> Symbols {
        let mut symbols = Symbols::default();
        symbols
            .ranges
            .insert(0x3fc0_0100, ("g_intr_lock_mux".into(), 4));
        symbols
            .ranges
            .insert(0x3fc0_0200, ("esp_test_rx_statistics".into(), 8));
        symbols
    }

    #[test]
    fn every_undeclared_symbol_is_found_in_one_discovery() {
        let mut answers = vec![
            outcome(Some((0x3fc0_0204, MemoryAccess::Write))),
            outcome(Some((0x4000_8000, MemoryAccess::Fetch))),
            outcome(None),
        ]
        .into_iter();
        let mut runs = vec![];
        let found = discover(
            &cases(),
            &outcome(Some((0x3fc0_0100, MemoryAccess::Read))),
            &symbols(),
            |cases| {
                runs.push(cases.to_vec());
                Ok(answers.next().unwrap())
            },
        );
        let owners: Vec<_> = found.iter().map(|m| m.owner.as_str()).collect();
        assert_eq!(
            owners,
            [
                "g_intr_lock_mux+0x0",
                "esp_test_rx_statistics+0x4",
                "0x40008000"
            ]
        );
        assert!(found.iter().all(|m| m.unresolved.is_none()));
        // Only the chain entering the mapped function reruns; its last rerun
        // maps both symbols and answers the fetch in each of its cases.
        assert_eq!(runs.len(), 3);
        assert!(runs.iter().all(|run| run.len() == 2));
        for case in runs.last().unwrap() {
            let last = &case.vendor;
            assert_eq!(last.memory.len(), 2);
            assert_eq!(last.memory[1].seed.address, 0x3fc0_0200);
            assert_eq!(last.memory[1].seed.length, 8);
            assert_eq!(last.calls[0].binding.address, 0x4000_8000);
        }
        assert!(render(&found).contains("esp_test_rx_statistics+0x4"));
    }

    #[test]
    fn a_stop_mapping_does_not_remove_is_unresolved() {
        let found = discover(
            &cases(),
            &outcome(Some((0x3fc0_0100, MemoryAccess::Read))),
            &symbols(),
            |_| Ok(outcome(Some((0x3fc0_0100, MemoryAccess::Read)))),
        );
        assert_eq!(found.len(), 2);
        assert!(found[1].unresolved.is_some());
    }
}

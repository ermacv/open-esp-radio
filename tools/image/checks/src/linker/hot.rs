//! Hot task code an image lists for SRAM.
//!
//! An image may name, in its manifest (`[package.metadata.open-radio]
//! hot-code`), a list of functions that run on its busiest path, most
//! valuable first, with a byte budget. After the strict selection the image
//! linker moves each listed function's whole input section into `.hot.text`,
//! which executes from SRAM, through the generated [`FRAGMENT`]. A function
//! the list names but the link does not define, one already in SRAM or in
//! the strict selection, one without a single defining input section and one
//! beyond the budget or the free SRAM are reported in the placement record
//! and left where they are: the list never fails a build and changes no
//! other placement.
//!
//! [`HotCode::from_samples`] writes such a list from a PC profile: the
//! cached or already moved functions with the most samples per byte, until
//! the budget. The
//! hottest functions of a busy path are often large (a task's whole future
//! inlined into one), so ranking by samples alone would spend the budget on
//! one of them; ranking by density covers the most samples per SRAM byte.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::strict::{InputSection, PlacedFunction, script_name};
use crate::Result;
use oer_os_durable::inputs::Inputs;

/// The generated linker fragment `.hot.text` includes.
pub const FRAGMENT: &str = "hot-text.x";

/// SRAM the selection leaves unused: the 64-byte alignment of `.hot.text`
/// and of each SRAM output section after it may grow by up to 63 bytes once
/// the section moves.
pub const ALIGNMENT_RESERVE: u64 = 512;

/// The output section that holds an image's cached code.
pub const CACHED_TEXT: &str = ".text";

/// The output section the list moves its functions into.
pub const HOT_TEXT: &str = ".hot.text";

/// The output sections a profile ranks: the cached code and the functions
/// an earlier list already moved, so a list regenerated from an image that
/// applied one keeps them on their samples. Strict handler code, which
/// executes from SRAM whatever the list says, is left out.
pub const RANKED_TEXT: [&str; 2] = [CACHED_TEXT, HOT_TEXT];

/// The samples a function needs to be listed: one sample is noise.
pub const MINIMUM_SAMPLES: u64 = 2;

/// An image's hot-code list.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HotCode {
    /// Bytes of whole input sections the list may move into SRAM.
    pub budget_bytes: u64,
    /// Demangled function names without crate hashes, most valuable first.
    pub functions: Vec<String>,
}

impl HotCode {
    pub fn read(path: &Path, inputs: &mut Inputs) -> Result<Self> {
        let text = inputs.read_to_string(path)?;
        toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()).into())
    }

    /// The list a PC profile ranks: `pcs` sampled in the image whose
    /// functions are `functions`, one entry per sample. Functions outside
    /// [`RANKED_TEXT`] or with fewer than [`MINIMUM_SAMPLES`] are left out;
    /// the rest go in by samples per byte (then samples, then name), each
    /// while its size fits the remaining `budget_bytes`.
    pub fn from_samples(
        functions: &[ProfiledFunction],
        pcs: impl IntoIterator<Item = u32>,
        budget_bytes: u64,
    ) -> Self {
        let mut by_address: Vec<&ProfiledFunction> = functions
            .iter()
            .filter(|function| {
                RANKED_TEXT.contains(&function.section.as_str()) && function.size > 0
            })
            .collect();
        by_address.sort_by_key(|function| function.address);
        let mut samples: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for pc in pcs {
            let index = by_address.partition_point(|function| function.address <= pc);
            let Some(function) = index.checked_sub(1).and_then(|index| by_address.get(index))
            else {
                continue;
            };
            if pc.saturating_sub(function.address) < function.size {
                let entry = samples
                    .entry(function.name.as_str())
                    .or_insert((0, u64::from(function.size)));
                entry.0 += 1;
            }
        }
        let mut ranked: Vec<_> = samples
            .into_iter()
            .filter(|(_, (count, _))| *count >= MINIMUM_SAMPLES)
            .collect();
        // Density compares as `left_samples / left_size` against the right
        // one, cross-multiplied in u128 so it never overflows or divides.
        ranked.sort_by(
            |(left, (left_samples, left_size)), (right, (right_samples, right_size))| {
                (u128::from(*right_samples) * u128::from(*left_size))
                    .cmp(&(u128::from(*left_samples) * u128::from(*right_size)))
                    .then(right_samples.cmp(left_samples))
                    .then(left.cmp(right))
            },
        );
        let mut used = 0_u64;
        let mut listed = Vec::new();
        for (name, (_, size)) in ranked {
            if let Some(total) = used
                .checked_add(size)
                .filter(|total| *total <= budget_bytes)
            {
                used = total;
                listed.push(name.to_owned());
            }
        }
        Self {
            budget_bytes,
            functions: listed,
        }
    }

    /// The list as its file, under a comment naming where it came from.
    pub fn render(&self, origin: &str) -> Result<String> {
        let mut text = String::new();
        for line in origin.lines() {
            let _ = writeln!(text, "# {line}");
        }
        text.push_str(&toml::to_string_pretty(self)?);
        Ok(text)
    }
}

/// One function of a profiled image: its build-stable name, its address and
/// size, and the output section that holds it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfiledFunction {
    pub name: String,
    pub address: u32,
    pub size: u32,
    pub section: String,
}

/// The functions of `elf` by their build-stable name: the smallest demangled
/// name (without crate hashes) among each function's symbols.
pub fn profiled_functions(elf: &oer_analysis_elf::Elf<'_>) -> Result<Vec<ProfiledFunction>> {
    let sections: BTreeMap<usize, String> = elf
        .sections()
        .map(|section| (section.index, section.name.to_owned()))
        .collect();
    Ok(elf
        .functions()?
        .into_iter()
        .filter_map(|function| {
            Some(ProfiledFunction {
                name: key(&function.names)?,
                address: function.address,
                size: function.size,
                section: sections.get(&function.section).cloned().unwrap_or_default(),
            })
        })
        .collect())
}

/// The build-stable name of a function with `symbols`.
pub fn key(symbols: &[String]) -> Option<String> {
    symbols
        .iter()
        .map(|symbol| oer_analysis_elf::demangle(symbol))
        .min()
}

/// Why a listed function stayed where it was.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkipReason {
    /// The link defines no function of that name.
    Unmatched,
    /// It already executes from SRAM.
    InSram,
    /// The strict selection already moves its section.
    Strict,
    /// It has no single defining input section.
    NoSingleSection,
    /// Its symbol or section needs linker-script escaping.
    ScriptName,
    /// Its section does not fit the budget or the free SRAM.
    NoRoom,
}

/// One listed function the link did not move.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Skipped {
    pub name: String,
    pub reason: SkipReason,
    /// Bytes of its input section, when it has one.
    #[serde(default)]
    pub section_bytes: u64,
}

/// One function of the first link as the selection sees it.
#[derive(Clone, Debug)]
pub(super) struct Candidate {
    pub key: String,
    pub symbols: Vec<String>,
    pub size: u64,
    pub in_sram: bool,
}

/// What a list moves: the placed functions and the skipped ones.
#[derive(Debug, Default)]
pub(super) struct Selection {
    pub placed: Vec<PlacedFunction>,
    pub skipped: Vec<Skipped>,
}

/// Select `list`'s functions among `candidates`, whose input sections are
/// `sections`, leaving `strict` sections to the strict selection, within
/// `free` bytes of SRAM and the list's budget.
pub(super) fn select(
    list: &HotCode,
    candidates: &[Candidate],
    sections: &[InputSection],
    strict: &BTreeSet<(String, String)>,
    free: u64,
) -> Selection {
    let room = free.min(list.budget_bytes);
    let mut selection = Selection::default();
    let mut moved: BTreeSet<(String, String)> = BTreeSet::new();
    let mut used = 0_u64;
    let skip = |selection: &mut Selection, name: &str, reason, section_bytes| {
        selection.skipped.push(Skipped {
            name: name.to_owned(),
            reason,
            section_bytes,
        });
    };
    for name in &list.functions {
        let matches: Vec<_> = candidates
            .iter()
            .filter(|candidate| &candidate.key == name)
            .collect();
        if matches.is_empty() {
            skip(&mut selection, name, SkipReason::Unmatched, 0);
            continue;
        }
        for candidate in matches {
            if candidate.in_sram {
                skip(&mut selection, name, SkipReason::InSram, 0);
                continue;
            }
            let defining: Vec<_> = sections
                .iter()
                .filter(|section| {
                    candidate
                        .symbols
                        .iter()
                        .any(|symbol| section.symbols.contains(symbol))
                })
                .collect();
            let [section] = defining.as_slice() else {
                skip(&mut selection, name, SkipReason::NoSingleSection, 0);
                continue;
            };
            let group: Vec<_> = sections
                .iter()
                .filter(|other| other.file == section.file && other.name == section.name)
                .collect();
            let section_bytes = group.iter().map(|section| section.size).sum::<u64>();
            let place = (section.file.clone(), section.name.clone());
            if strict.contains(&place) {
                skip(&mut selection, name, SkipReason::Strict, section_bytes);
                continue;
            }
            let Some(symbol) = candidate
                .symbols
                .iter()
                .find(|symbol| section.symbols.contains(symbol))
            else {
                skip(&mut selection, name, SkipReason::NoSingleSection, 0);
                continue;
            };
            if script_name(symbol, name, false).is_err()
                || script_name(&section.name, name, false).is_err()
                || script_name(&section.file, name, true).is_err()
            {
                skip(&mut selection, name, SkipReason::ScriptName, section_bytes);
                continue;
            }
            if !moved.contains(&place) {
                match used
                    .checked_add(section_bytes)
                    .filter(|total| *total <= room)
                {
                    Some(total) => used = total,
                    None => {
                        skip(&mut selection, name, SkipReason::NoRoom, section_bytes);
                        continue;
                    }
                }
                moved.insert(place);
            }
            selection.placed.push(PlacedFunction {
                name: name.clone(),
                symbol: symbol.clone(),
                file: section.file.clone(),
                section: section.name.clone(),
                section_index: section.index,
                size: candidate.size,
                section_bytes,
                other_symbols: group
                    .iter()
                    .flat_map(|section| &section.symbols)
                    .filter(|other| *other != symbol)
                    .cloned()
                    .collect(),
            });
        }
    }
    selection
}

#[cfg(test)]
mod tests;

//! The worst-case interrupt stack of each hart, from the image's interrupt
//! table.
//!
//! A hart takes interrupts at the levels of its table entries; a level
//! preempts only a lower one, so the stack holds at most one interrupt per level at a time,
//! with an exception on top. Each level costs its worst hardware-vector entry
//! among the lines its table entries bind: the entry's frame below the
//! interrupt stack's position plus the bound of the handler it calls. One
//! line, one source, one direct call: the vector of an entry's line must call
//! the entry's handler, or the stacks are refused.
//!
//! The sum over levels rests on the interrupt controller (CLIC): an interrupt
//! preempts only above both the threshold and the running level
//! `mintstatus.mil`, which the hardware sets on entry. Lowering the threshold
//! or setting `mstatus.MIE` inside a handler therefore lets no interrupt of
//! its own level in; only an instruction that lowers `mil` (a trap return, a
//! write of `mnxti` or `mintstatus`) can. The trap-entry check refuses one on
//! the way to the handler, and each level's bound lists those its handlers
//! reach ([`Bound::level_drops`]): a hart with none is checked, one with any
//! holds only under the nesting rule those sites may break. The rule's other
//! half, that each line's CLIC level is its table entry's and stays so, is
//! the image's configuration: the firmware gate checks where its writers run
//! ([`crate::instances`]).
use crate::{Analysis, Assumption, Bound, LevelDrop, Resolutions, TableEntry, TrapEntry};
use oer_analysis_riscv_model::{Error, ErrorCode, Ledger, Result, RunControl};
use std::collections::{BTreeMap, BTreeSet};

/// What the stacks of an image are bounded from.
pub struct Stacks<'a> {
    pub table: &'a [TableEntry],
    /// The harts, by the table's core numbers.
    pub cores: &'a [u32],
    /// The checked entries of the hardware vectors, by hart and line.
    pub vectors: &'a [Vector],
    /// The checked entry of exceptions.
    pub exception: TrapEntry,
    /// Targets of indirect sites, the same in every context.
    pub resolutions: &'a Resolutions,
}

/// The checked entry of one hart's line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Vector {
    pub hart: u32,
    pub line: u32,
    pub entry: TrapEntry,
}

/// One level of one hart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LevelStack {
    pub level: u32,
    /// Bytes the level adds; `None` when its bound is unknown.
    pub bytes: Option<u64>,
    /// Bytes the level adds over what is resolved (`partial + ?` when
    /// [`Self::bytes`] is unknown).
    pub partial: u64,
    /// The worst entry's frame and its handler's bound.
    pub frame: u64,
    pub bound: Bound,
}

/// One hart's interrupt stack.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HartStack {
    pub core: u32,
    pub levels: Vec<LevelStack>,
    /// The exception on top of every level.
    pub exception: LevelStack,
    /// The sum of every level and the exception; `None` when one is unknown.
    pub bytes: Option<u64>,
    /// The sum over what is resolved (`partial + ?` when [`Self::bytes`] is
    /// unknown).
    pub partial: u64,
}

impl HartStack {
    /// The instructions that can lower the interrupt level in a handler of
    /// any level or the exception: the sites where the nesting rule the sum
    /// rests on may break. Empty when the rule is checked.
    pub fn level_drops(&self) -> Vec<(u32, LevelDrop)> {
        let mut drops: Vec<(u32, LevelDrop)> = self
            .levels
            .iter()
            .chain([&self.exception])
            .flat_map(|level| level.bound.level_drops.iter().copied())
            .collect();
        drops.sort_unstable();
        drops.dedup();
        drops
    }

    /// What the bounds of its levels and the exception assume: a hart with
    /// a bound and none is proven, with some conditional on them.
    pub fn assumptions(&self) -> BTreeSet<Assumption> {
        self.levels
            .iter()
            .chain([&self.exception])
            .flat_map(|level| level.bound.assumptions.iter().copied())
            .collect()
    }
}

impl Stacks<'_> {
    /// The dispatch facts of a level's bound: the handlers are direct calls
    /// of their lines' entries, so every context shares the image's.
    pub fn resolutions_for(&self, _analysis: &Analysis, _core: u32, _level: u32) -> Resolutions {
        self.resolutions.clone()
    }

    /// The checked entries of the lines `core`'s entries at `level` bind.
    ///
    /// # Errors
    ///
    /// A line with no vector, or whose vector calls another function than
    /// its entry's handler.
    fn entries_of(&self, core: u32, level: u32) -> Result<Vec<TrapEntry>> {
        let invalid = |message: String| Error::new(ErrorCode::Integrity, message);
        let mut entries = Vec::new();
        for binding in self
            .table
            .iter()
            .filter(|entry| entry.core == core && entry.level == level)
        {
            let Some(handler) = binding.handler else {
                continue;
            };
            let vector = self
                .vectors
                .iter()
                .find(|vector| vector.hart == core && vector.line == binding.line)
                .ok_or_else(|| {
                    invalid(format!(
                        "hart {core} has no vector for line {} of source {}",
                        binding.line, binding.source
                    ))
                })?;
            if vector.entry.handler != handler {
                return Err(invalid(format!(
                    "the vector of line {} on hart {core} calls {:#010x}, the table binds \
                     source {} to {handler:#010x}",
                    binding.line, vector.entry.handler, binding.source
                )));
            }
            entries.push(vector.entry);
        }
        Ok(entries)
    }
}

/// The interrupt stack of each hart.
pub fn interrupt_stacks(
    analysis: &Analysis,
    stacks: &Stacks<'_>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Vec<HartStack>> {
    let mut harts = Vec::new();
    for &core in stacks.cores {
        let levels: BTreeSet<u32> = stacks
            .table
            .iter()
            .filter(|entry| entry.core == core && entry.handler.is_some())
            .map(|entry| entry.level)
            .collect();
        let mut level_stacks = Vec::new();
        for level in levels {
            let entries = stacks.entries_of(core, level)?;
            level_stacks.push(worst(
                analysis,
                level,
                &entries,
                stacks.resolutions,
                keep,
                control,
            )?);
        }
        let exception = worst(
            analysis,
            0,
            &[stacks.exception],
            stacks.resolutions,
            keep,
            control,
        )?;
        let bytes = level_stacks
            .iter()
            .chain([&exception])
            .map(|level| level.bytes)
            .sum::<Option<u64>>();
        let partial = level_stacks
            .iter()
            .chain([&exception])
            .map(|level| level.partial)
            .sum();
        harts.push(HartStack {
            core,
            levels: level_stacks,
            exception,
            bytes,
            partial,
        });
    }
    Ok(harts)
}

/// The deepest of `entries` at `level`: an unknown bound, else the most
/// bytes.
fn worst(
    analysis: &Analysis,
    level: u32,
    entries: &[TrapEntry],
    resolutions: &Resolutions,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<LevelStack> {
    let mut worst: Option<LevelStack> = None;
    let mut by_handler: BTreeMap<u32, Bound> = BTreeMap::new();
    for entry in entries {
        let bound = match by_handler.get(&entry.handler) {
            Some(bound) => bound.clone(),
            None => {
                let bound = analysis.bound_with(entry.handler, resolutions, keep, control)?;
                by_handler.insert(entry.handler, bound.clone());
                bound
            }
        };
        let candidate = LevelStack {
            level,
            bytes: bound.bytes.map(|bytes| entry.frame + bytes),
            partial: entry.frame + bound.partial,
            frame: entry.frame,
            bound,
        };
        // An unknown entry is the worst; among unknown ones, the deepest
        // resolved part.
        let deeper = match &worst {
            None => true,
            Some(current) => match (current.bytes, candidate.bytes) {
                (Some(_), None) => true,
                (Some(current), Some(bytes)) => bytes > current,
                (None, None) => candidate.partial > current.partial,
                (None, Some(_)) => false,
            },
        };
        if deeper {
            worst = Some(candidate);
        }
    }
    Ok(worst.expect("at least one entry"))
}

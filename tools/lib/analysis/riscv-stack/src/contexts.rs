//! The worst-case interrupt stack of each hart, from the image's interrupt
//! table.
//!
//! A hart takes interrupts at the levels of its table entries, and at the
//! levels the platform always uses (the IPC line's); a level preempts only a
//! lower one, so the stack holds at most one interrupt per level at a time,
//! with an exception on top. Each level costs its worst hardware-vector entry:
//! the entry's frame below the interrupt stack's position plus the bound of
//! the handler it calls, where the dispatcher's calls through the source
//! table reach only the handlers the table routes to that hart at that level.
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
use crate::{Analysis, Assumption, Bound, Fact, LevelDrop, Resolutions, TableEntry, TrapEntry};
use oer_riscv_model::Result;
use std::collections::{BTreeMap, BTreeSet};

/// What the stacks of an image are bounded from.
pub struct Stacks<'a> {
    pub table: &'a [TableEntry],
    /// The harts, by the table's core numbers.
    pub cores: &'a [u32],
    /// Levels every hart takes whatever the table holds.
    pub always: &'a [u32],
    /// The checked entries of the hardware vectors.
    pub vectors: &'a [TrapEntry],
    /// The checked entry of exceptions.
    pub exception: TrapEntry,
    /// The base of the dispatcher's source table: a call through it reaches
    /// the handler of the source dispatched.
    pub sources: u32,
    /// Targets of other indirect sites, the same in every context.
    pub resolutions: &'a Resolutions,
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

/// The interrupt stack of each hart.
pub fn interrupt_stacks(analysis: &Analysis, stacks: &Stacks<'_>) -> Result<Vec<HartStack>> {
    let dispatch_sites: BTreeSet<u32> = analysis
        .functions
        .values()
        .flat_map(|facts| facts.table_bases.iter())
        .filter(|&(_, &base)| base == stacks.sources)
        .map(|(&site, _)| site)
        .collect();
    let mut harts = Vec::new();
    for &core in stacks.cores {
        let mut levels: BTreeSet<u32> = stacks
            .table
            .iter()
            .filter(|entry| entry.core == core && entry.handler.is_some())
            .map(|entry| entry.level)
            .collect();
        levels.extend(stacks.always);
        let mut level_stacks = Vec::new();
        for level in levels {
            let handlers: BTreeSet<u32> = stacks
                .table
                .iter()
                .filter(|entry| entry.core == core && entry.level == level)
                .filter_map(|entry| entry.handler)
                .collect();
            let mut resolutions = stacks.resolutions.clone();
            for &site in &dispatch_sites {
                resolutions.add(site, Fact::InterruptTable, handlers.iter().copied());
            }
            level_stacks.push(worst(analysis, level, stacks.vectors, &resolutions)?);
        }
        let exception = worst(analysis, 0, &[stacks.exception], stacks.resolutions)?;
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
) -> Result<LevelStack> {
    let mut worst: Option<LevelStack> = None;
    let mut by_handler: BTreeMap<u32, Bound> = BTreeMap::new();
    for entry in entries {
        let bound = match by_handler.get(&entry.handler) {
            Some(bound) => bound.clone(),
            None => {
                let bound = analysis.bound_with(entry.handler, resolutions)?;
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

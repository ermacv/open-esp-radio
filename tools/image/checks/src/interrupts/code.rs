//! The executor/task boundary comes from resolution facts, never names.

use super::Result;
use oer_analysis_riscv_stack::{
    Analysis, Class, Function, HartStack, Resolutions, Stacks, task_poll_sites,
};
use oer_chip_profile::Memory;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::{self, Write as _};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Role {
    Strict,
    Task,
}

impl fmt::Display for Role {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Strict => "strict",
            Self::Task => "task",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Location {
    Sram,
    Rom,
    Psram,
    Flash,
    Other,
}

impl Location {
    pub(super) fn cached(self) -> bool {
        matches!(self, Self::Psram | Self::Flash)
    }
}

impl fmt::Display for Location {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Sram => "SRAM",
            Self::Rom => "ROM",
            Self::Psram => "PSRAM",
            Self::Flash => "flash XIP",
            Self::Other => "other",
        })
    }
}

#[derive(Debug)]
pub(super) struct PlacedFunction {
    pub size: u32,
    pub role: Role,
    pub location: Location,
}

#[derive(Debug)]
pub(super) struct ContextCode {
    pub hart: u32,
    pub level: u32,
    /// The level's bound root: its worst trap entry's handler.
    pub root: u32,
    /// The level's class ([`super::classes::context_class`]); `None` for the
    /// exception.
    pub class: Option<Class>,
    pub strict: BTreeSet<u32>,
    pub tasks: BTreeSet<u32>,
    pub task_roots: BTreeSet<u32>,
    pub cached_paths: BTreeMap<u32, Vec<u32>>,
    /// Calls into the panic machinery from the strict code, by caller and
    /// callee: reported, not gated.
    pub panics: Vec<(u32, u32)>,
}

impl fmt::Display for ContextCode {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "hart {} ", self.hart)?;
        match self.class {
            None => out.write_str("exception"),
            Some(class) => write!(out, "P{} ({class})", self.level),
        }
    }
}

/// Resolved interrupt code partitioned at the task-poll sites of each
/// executor's level.
/// A function with any strict path keeps the strict classification.
#[derive(Debug, Default)]
pub struct CodePlacement {
    pub(super) contexts: Vec<ContextCode>,
    pub(super) functions: BTreeMap<u32, PlacedFunction>,
    pub(super) panic: BTreeSet<u32>,
}

impl CodePlacement {
    /// The resolved strict functions still in cached memory, by address and size.
    /// The image linker consumes this same partition as the final check.
    pub fn strict_cached(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.functions.iter().filter_map(|(&address, function)| {
            (function.role == Role::Strict && function.location.cached())
                .then_some((address, function.size))
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn of(
        analysis: &Analysis,
        own: &[Function],
        harts: &[HartStack],
        inputs: &Stacks<'_>,
        panic: BTreeSet<u32>,
        memory: &Memory,
        keep: &mut oer_analysis_riscv_model::Ledger<'_>,
        control: &mut dyn oer_analysis_riscv_model::RunControl,
    ) -> Result<Self> {
        let mut contexts = Vec::new();
        let mut strict = panic.clone();
        let mut tasks = BTreeSet::new();
        for hart in harts {
            for level in hart.levels.iter().chain([&hart.exception]) {
                let class = (level.level != 0)
                    .then(|| super::classes::context_class(inputs.table, hart.core, level.level));
                // Only an executor's level grants the task-side PSRAM
                // permission. A poll on any other level cannot relax its
                // interrupt placement.
                let executor = class == Some(Class::Executor);
                let resolutions = inputs.resolutions_for(analysis, hart.core, level.level);
                let cut = task_poll_sites(&resolutions);
                let mut reached =
                    analysis.reached_cut(level.bound.root, &resolutions, &cut, keep, control)?;
                if !executor {
                    reached.extend(&level.bound.reached);
                }
                let task_roots = if executor {
                    reached
                        .iter()
                        .filter_map(|address| analysis.functions.get(address))
                        .flat_map(|facts| &facts.transfers)
                        .filter(|transfer| {
                            transfer.target.is_none() && cut.contains(&transfer.site)
                        })
                        .filter_map(|transfer| resolutions.get(transfer.site))
                        .filter(|resolution| resolution.resolves())
                        .flat_map(|resolution| resolution.targets.iter().copied())
                        .collect()
                } else {
                    BTreeSet::new()
                };
                let task_code: BTreeSet<_> =
                    level.bound.reached.difference(&reached).copied().collect();
                let cached: BTreeSet<_> = reached
                    .iter()
                    .copied()
                    .filter(|address| {
                        memory.cached().iter().any(|region| {
                            region.contains_range(u64::from(*address), u64::from(*address) + 4)
                        })
                    })
                    .collect();
                let cached_paths = paths(
                    analysis,
                    level.bound.root,
                    &resolutions,
                    &if executor { cut } else { BTreeSet::new() },
                    &reached,
                    &cached,
                );
                strict.extend(&reached);
                tasks.extend(&task_code);
                contexts.push(ContextCode {
                    hart: hart.core,
                    level: level.level,
                    root: level.bound.root,
                    class,
                    strict: reached,
                    tasks: task_code,
                    task_roots,
                    cached_paths,
                    panics: Vec::new(),
                });
            }
        }
        let own: BTreeSet<_> = own.iter().map(|function| function.address).collect();
        let functions = strict
            .union(&tasks)
            .filter_map(|&address| {
                let facts = analysis.functions.get(&address)?;
                let start = u64::from(address);
                let end = start + u64::from(facts.function.size.max(1));
                let location = if memory.sram.contains_range(start, end) {
                    Location::Sram
                } else if memory.flash_xip.contains_range(start, end) {
                    Location::Flash
                } else if memory
                    .psram
                    .is_some_and(|region| region.contains_range(start, end))
                {
                    Location::Psram
                } else if !own.contains(&address) {
                    Location::Rom
                } else {
                    Location::Other
                };
                Some((
                    address,
                    PlacedFunction {
                        size: facts.function.size,
                        role: if strict.contains(&address) {
                            Role::Strict
                        } else {
                            Role::Task
                        },
                        location,
                    },
                ))
            })
            .collect();
        Ok(Self {
            contexts,
            functions,
            panic,
        })
    }

    pub(super) fn summary(&self, name: impl Fn(u32) -> String) -> String {
        let count = |addresses: &BTreeSet<u32>| {
            let functions: Vec<_> = addresses
                .iter()
                .filter_map(|address| self.functions.get(address))
                .filter(|function| function.role == Role::Strict && function.location.cached())
                .collect();
            (
                functions.len(),
                functions
                    .iter()
                    .map(|function| u64::from(function.size))
                    .sum::<u64>(),
            )
        };
        let strict: Vec<_> = self
            .functions
            .values()
            .filter(|function| function.role == Role::Strict && function.location.cached())
            .collect();
        let tasks: Vec<_> = self
            .functions
            .values()
            .filter(|function| function.role == Role::Task && function.location == Location::Psram)
            .collect();
        let mut levels: Vec<_> = self
            .contexts
            .iter()
            .map(|context| {
                let (functions, bytes) = count(&context.strict);
                format!("{context}: {functions} functions, {bytes} bytes")
            })
            .collect();
        if !self.panic.is_empty() {
            let (functions, bytes) = count(&self.panic);
            levels.push(format!("panic path: {functions} functions, {bytes} bytes"));
        }
        let roots: BTreeSet<_> = self
            .contexts
            .iter()
            .flat_map(|context| context.task_roots.iter().copied())
            .collect();
        format!(
            "strict handler code in cached memory: {} functions, {} bytes; by level: {}\nexecutor task code in PSRAM: {} functions, {} bytes; tasks: {}\n",
            strict.len(),
            strict
                .iter()
                .map(|function| u64::from(function.size))
                .sum::<u64>(),
            if levels.is_empty() {
                "none".into()
            } else {
                levels.join("; ")
            },
            tasks.len(),
            tasks
                .iter()
                .map(|function| u64::from(function.size))
                .sum::<u64>(),
            if roots.is_empty() {
                "none".into()
            } else {
                roots.into_iter().map(name).collect::<Vec<_>>().join(", ")
            }
        )
    }

    pub(super) fn totals(&self) -> String {
        let mut out = "Strict handler code by region\nregion\tfunctions\tbytes\n".to_owned();
        for location in [
            Location::Sram,
            Location::Rom,
            Location::Psram,
            Location::Flash,
            Location::Other,
        ] {
            let functions: Vec<_> = self
                .functions
                .values()
                .filter(|function| function.role == Role::Strict && function.location == location)
                .collect();
            let _ = writeln!(
                out,
                "{location}\t{}\t{}",
                functions.len(),
                functions
                    .iter()
                    .map(|function| u64::from(function.size))
                    .sum::<u64>()
            );
        }
        out
    }

    pub(super) fn render_paths(&self, name: impl Fn(u32) -> String) -> String {
        let mut out =
            "Strict cached handler paths\nfunction\tcrate\taddress\tsize\treached from\tpath\n"
                .to_owned();
        for context in &self.contexts {
            for (&address, path) in &context.cached_paths {
                let label = name(address);
                let krate = label
                    .split("::")
                    .next()
                    .unwrap_or(&label)
                    .trim_start_matches('<');
                let size = self
                    .functions
                    .get(&address)
                    .map_or(0, |function| function.size);
                let path = path
                    .iter()
                    .map(|address| format!("{} ({address:#010x})", name(*address)))
                    .collect::<Vec<_>>()
                    .join(" -> ");
                let _ = writeln!(
                    out,
                    "{label}\t{krate}\t{address:#010x}\t{size}\t{context}\t{path}"
                );
            }
        }
        out
    }
}

#[cfg(test)]
pub(super) mod tests;

/// Shortest resolved paths for diagnostics, restricted to the cut inventory.
/// Direct targets take precedence over supplemental resolutions, as in Walk.
fn paths(
    analysis: &Analysis,
    root: u32,
    resolutions: &Resolutions,
    cut: &BTreeSet<u32>,
    allowed: &BTreeSet<u32>,
    wanted: &BTreeSet<u32>,
) -> BTreeMap<u32, Vec<u32>> {
    if wanted.is_empty() {
        return BTreeMap::new();
    }
    let mut parents = BTreeMap::from([(root, root)]);
    let mut queue = VecDeque::from([root]);
    while let Some(address) = queue.pop_front() {
        let Some(facts) = analysis.functions.get(&address) else {
            continue;
        };
        for transfer in &facts.transfers {
            if cut.contains(&transfer.site) {
                continue;
            }
            let targets = match transfer.target {
                Some(target) => BTreeSet::from([target]),
                None => resolutions
                    .get(transfer.site)
                    .filter(|resolution| resolution.resolves())
                    .map(|resolution| resolution.targets.clone())
                    .unwrap_or_default(),
            };
            for target in targets {
                if allowed.contains(&target) && !parents.contains_key(&target) {
                    parents.insert(target, address);
                    queue.push_back(target);
                }
            }
        }
    }
    wanted
        .iter()
        .filter_map(|&address| {
            let mut path = vec![address];
            let mut at = address;
            while at != root {
                at = *parents.get(&at)?;
                path.push(at);
            }
            path.reverse();
            Some((address, path))
        })
        .collect()
}

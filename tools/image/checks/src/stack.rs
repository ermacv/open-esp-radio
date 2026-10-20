//! The stack gate of an image build. One analysis of each ELF
//! (`oer-analysis-riscv-stack`, with the pinned ROM ELF and its reviewed summaries)
//! bounds every stack the image runs on:
//!
//! - the runtime's interrupt stacks ([`crate::interrupts`]);
//! - each task stack the policy names, from the function that runs on it
//!   (its root, by symbol): the root's worst-case call chain must leave the
//!   stack's storage the policy's minimum free bytes;
//! - the bootstrap image's stack, from its root likewise.
//!
//! A task stack's bound may be `partial + ?`: the root reaches indirect sites
//! no fact resolves (an executor's task polls, trait objects). The gate then
//! accepts it, holds the part it proves to the budget and lists every
//! unresolved site with its reason; the runtime's stack painting stays the
//! check of the exercised call chains. A root symbol the image lacks fails,
//! as does an image without compiler frame records or a linked function
//! without a record that the coverage review does not explain.

pub mod budget;
pub mod contract;
mod coverage;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::rc::Rc;

use oer_analysis_riscv_model::{Ledger, RunControl, WorkingMemory};
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_stack::{
    Analysis, Bound, DebugMetadata, Dwarf, Function, Reason, Resolutions, Summary, analyze,
    image_resolutions, parse_summaries, read_debug, stack_sizes, taken_addresses,
};

use crate::stack::budget::{Budget, MEMORY_LIMIT, TIMEOUT, Usage, WORK_UNITS};

pub use coverage::{Coverage, CoverageCategory, CoveragePolicy, CoverageReview};
use oer_chip_profile::policy::{StackPolicy, Storage, TaskStack};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

use crate::stack::contract::Contract;

/// The assumptions a check admits in a bound, each by name: a bound resting
/// on one is conditional and passes with a warning; any other fails. Each
/// leaves this list in the pull request that proves it.
pub const ADMITTED: &[oer_analysis_riscv_stack::Assumption] =
    &[oer_analysis_riscv_stack::Assumption::ExecutorInvariant];

/// The runtime task stacks' report.
pub const RUNTIME_REPORT: &str = "runtime-stack.txt";
/// The bootstrap stack's report.
pub const BOOTSTRAP_REPORT: &str = "bootstrap-stack.txt";

/// Bound the runtime `image`'s task stacks, write [`RUNTIME_REPORT`] into
/// `output`, then check them and the frame coverage under `policy`; returns
/// the warnings they passed with.
pub fn audit_runtime_stacks(
    image: &Image,
    policy: &StackPolicy,
    output: &Path,
) -> Result<Vec<String>> {
    let stacks = StackAudit::of(image, policy, &policy.runtime_task_stacks())?;
    std::fs::write(output.join(RUNTIME_REPORT), stacks.render())?;
    stacks.check()
}

/// Bound the bootstrap ELF `elf`'s stack, write [`BOOTSTRAP_REPORT`] into
/// `output`, then check it under `policy`. Returns the warnings it passed
/// with; `None` when the policy names no bootstrap stack.
pub fn audit_bootstrap_stack(
    root: &Path,
    profile: &oer_chip_profile::Profile,
    elf: &Path,
    policy: &StackPolicy,
    output: &Path,
) -> Result<Option<Vec<String>>> {
    let Some(bootstrap) = policy.stacks()?.bootstrap_stack else {
        return Ok(None);
    };
    let image = Image::read(root, profile, elf)?;
    let audit = StackAudit::of(&image, policy, &[("bootstrap stack", bootstrap)])?;
    std::fs::write(output.join(BOOTSTRAP_REPORT), audit.render())?;
    audit.check().map(Some)
}

/// One image's analysis with its pinned ROM, and the facts outside the
/// machine code every stack bound resolves indirect sites with.
pub struct Image {
    pub elf: Rc<[u8]>,
    pub analysis: Analysis,
    /// The image's own functions.
    pub functions: Vec<Function>,
    /// The compiler's frame record of each function (`.stack_sizes`).
    pub stack_sizes: BTreeMap<u32, u64>,
    /// The image's function names, demangled, by address.
    pub names: BTreeMap<u32, String>,
    pub dwarf: Dwarf,
    pub resolutions: Resolutions,
    /// The chip's address map and interrupt contract, when it names them.
    pub contract: Option<Contract>,
    /// What the analysis used of its [`budget`].
    pub usage: Usage,
}

impl Image {
    /// The analysis of `elf` with the chip's pinned ROM ELF and its
    /// reviewed summaries (`[rom]` of the chip profile).
    pub fn read(root: &Path, profile: &oer_chip_profile::Profile, elf: &Path) -> Result<Self> {
        let elf = std::fs::read(elf)
            .map_err(|error| format!("cannot read {}: {error}", elf.display()))?;
        Self::of_bytes(root, profile, elf)
    }

    /// [`Self::read`] of the ELF bytes `elf`.
    pub fn of_bytes(
        root: &Path,
        profile: &oer_chip_profile::Profile,
        elf: Vec<u8>,
    ) -> Result<Self> {
        let rom = profile
            .rom
            .as_ref()
            .ok_or_else(|| format!("chips/{}/platform/chip.toml names no [rom] ELF", profile.id))?;
        let rom_elf = std::fs::read(oer_tool_pins::artifacts::ensure(
            root,
            &profile.id,
            &rom.elf,
        )?)?;
        let summaries = match &rom.summaries {
            Some(path) => parse_summaries(&std::fs::read_to_string(root.join(path))?)?,
            None => Vec::new(),
        };
        Self::new(elf, &[&rom_elf], &summaries, Contract::of(profile))
    }

    /// The analysis of `elf` with its `companions` and their `summaries`,
    /// within the [`budget`] of one image analysis.
    fn new(
        elf: Vec<u8>,
        companions: &[&[u8]],
        summaries: &[Summary],
        contract: Option<Contract>,
    ) -> Result<Self> {
        let memory = WorkingMemory::new(MEMORY_LIMIT)?;
        let mut control = Budget::new(WORK_UNITS, TIMEOUT);
        // The image's and its companions' bytes are the analysis's too: its
        // DWARF reads borrow them.
        let input = elf.len()
            + companions
                .iter()
                .map(|companion| companion.len())
                .sum::<usize>();
        let _input = memory.reserve(input as u64, control.position())?;
        // The image's bytes move into the buffer the DWARF readers share,
        // which copies them once.
        let elf: Rc<[u8]> = {
            let _copy = memory.reserve(elf.len() as u64, control.position())?;
            Rc::from(elf)
        };
        let facts = ImageFacts::new(&elf, &[], &memory, &mut control)?;
        let companions = companions
            .iter()
            .map(|companion| ImageFacts::new(companion, &[], &memory, &mut control))
            .collect::<oer_analysis_riscv_model::Result<Vec<_>>>()?;
        let companions: Vec<&ImageFacts<'_>> = companions.iter().collect();
        // What the image keeps of the analysis, admitted while it counts
        // against the budget.
        let mut keep = Ledger::new(&memory, &mut control)?;
        let (dwarf, types) =
            read_debug(&elf, DebugMetadata::TypedLocations, &mut keep, &mut control)?;
        let analysis = analyze(
            &facts,
            &companions,
            summaries,
            &dwarf,
            &mut keep,
            &mut control,
        )?;
        let mut functions = Vec::new();
        let mut names = BTreeMap::new();
        for function in facts.functions() {
            control.checkpoint(1)?;
            keep.admit(
                function.names.iter().map(String::len).sum::<usize>()
                    + function.names.len() * size_of::<String>(),
                &mut control,
            )?;
            keep.push(&mut functions, function.clone(), &mut control)?;
            if let Some(name) = function.names.first() {
                let name = keep.string(&oer_analysis_elf::demangle(name), &mut control)?;
                keep.insert(&mut names, function.address, name, &mut control)?;
            }
        }
        let taken = taken_addresses(&facts, &mut keep, &mut control)?;
        let mut resolutions = image_resolutions(
            &facts,
            &analysis,
            &dwarf,
            &types,
            names
                .iter()
                .map(|(&address, name)| (address, name.as_str())),
            &mut keep,
            &mut control,
        )?;
        drop(types);
        let stack_sizes = stack_sizes(&facts, &mut keep, &mut control)?;
        drop(companions);
        drop(facts);
        let mut image = Self {
            elf,
            analysis,
            functions,
            stack_sizes,
            names,
            dwarf,
            resolutions: Resolutions::new(),
            contract,
            usage: Usage::default(),
        };
        resolutions.extend(image.ipc_resolutions(&taken, &mut keep, &mut control)?);
        image.usage = Usage {
            memory_bytes: memory.peak(),
            work_units: control.used(),
        };
        image.resolutions = resolutions;
        Ok(image)
    }

    /// The image's function that carries the symbol `name`.
    pub fn symbol(&self, name: &str) -> Option<u32> {
        self.functions
            .iter()
            .find(|function| function.names.iter().any(|candidate| candidate == name))
            .map(|function| function.address)
    }

    /// The image's functions whose demangled name is `path`.
    pub fn address_of(&self, path: &str) -> Vec<u32> {
        self.names
            .iter()
            .filter(|(_, name)| name.as_str() == path)
            .map(|(&address, _)| address)
            .collect()
    }

    /// The name of the function (of the image or a companion) that holds
    /// `address`.
    fn function_at(&self, address: u32) -> String {
        let Some((&start, facts)) = self.analysis.functions.range(..=address).next_back() else {
            return format!("{address:#010x}");
        };
        if address - start >= facts.function.size {
            return format!("{address:#010x}");
        }
        let name = self
            .names
            .get(&start)
            .cloned()
            .unwrap_or_else(|| oer_analysis_elf::demangle(&facts.function.label()));
        if address == start {
            name
        } else {
            format!("{name}+{:#x}", address - start)
        }
    }

    /// The linked bounds of `storage`, or `None` when the image links
    /// none of its symbols.
    pub fn storage(&self, storage: Storage<'_>) -> Result<Option<oer_analysis_elf::StorageBounds>> {
        let file = oer_analysis_elf::Elf::parse(&self.elf)?;
        let bounds = match storage {
            Storage::Symbol(name) => oer_analysis_elf::StorageBounds::symbol(&file, name)?,
            Storage::Range { bottom, top } => {
                oer_analysis_elf::StorageBounds::range(&file, bottom, top)?
            }
        };
        Ok(bounds
            .map(oer_analysis_elf::StorageBounds::nonempty)
            .transpose()?)
    }

    /// The targets of the IPC dispatch's call of the posted callback: the
    /// `handler` argument of every direct call of the only function that
    /// posts one; none when the image posts none.
    fn ipc_resolutions(
        &self,
        taken: &BTreeSet<u32>,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<Resolutions> {
        let resolutions = Resolutions::new();
        let Some(contract) = &self.contract else {
            return Ok(resolutions);
        };
        let Some(ipc) = &contract.interrupts.ipc else {
            return Ok(resolutions);
        };
        Ok(oer_analysis_riscv_stack::ipc_resolutions(
            &self.analysis,
            taken,
            self.address_of(&ipc.post),
            ipc.post_handler_argument,
            ipc.dispatch
                .iter()
                .flat_map(|dispatch| self.address_of(dispatch)),
            keep,
            control,
        )?)
    }
}

/// The frame coverage and task stack bounds of one image.
#[derive(Debug)]
pub struct StackAudit {
    pub coverage: Coverage,
    pub stacks: Vec<StackBound>,
    /// What the image's analysis used of its [`budget`].
    pub usage: Usage,
}

/// One stack's budget and its root's bound.
#[derive(Debug)]
pub struct StackBound {
    pub name: &'static str,
    pub root: String,
    /// The storage symbol, or the bottom and top symbols.
    pub storage: String,
    pub minimum_free_bytes: u64,
    /// The stack's bytes and its root's bound.
    pub linked: (u64, Bound),
    /// The deepest path, root first: each function's frame and name.
    pub path: Vec<(u64, String)>,
    /// Each unresolved site (or function) the root reaches: its reason, its
    /// address and the function that holds it.
    pub unresolved: Vec<(Reason, u32, String)>,
}

impl StackAudit {
    fn of(
        image: &Image,
        policy: &StackPolicy,
        stacks: &[(&'static str, &TaskStack)],
    ) -> Result<Self> {
        let coverage = CoveragePolicy::load(policy.stacks()?.coverage_policy)?
            .coverage(&image.functions, &image.stack_sizes)?;
        let mut bounds = Vec::new();
        for &(name, stack) in stacks {
            let storage = match stack.storage() {
                Storage::Symbol(symbol) => format!("`{symbol}`"),
                Storage::Range { bottom, top } => format!("`{bottom}`..`{top}`"),
            };
            let Some(storage_bounds) = image.storage(stack.storage())? else {
                return Err(format!("the image links no {name} ({storage})").into());
            };
            let root = image.symbol(&stack.root).ok_or_else(|| {
                format!(
                    "the image has no `{}`, the root the policy names for its {name}",
                    stack.root
                )
            })?;
            let walked = image.analysis.bound_with(root, &image.resolutions)?;
            let path = walked
                .path
                .iter()
                .map(|&(function, frame)| (frame, image.function_at(function)))
                .collect();
            let mut unresolved = walked
                .unresolved
                .iter()
                .map(|&(site, reason)| (reason, site, image.function_at(site)))
                .collect::<Vec<_>>();
            unresolved.sort();
            bounds.push(StackBound {
                name,
                root: stack.root.clone(),
                storage,
                minimum_free_bytes: u64::from(stack.minimum_free_bytes),
                linked: (storage_bounds.capacity, walked),
                path,
                unresolved,
            });
        }
        Ok(Self {
            coverage,
            stacks: bounds,
            usage: image.usage,
        })
    }

    /// The gate: full frame coverage, and each linked stack's bound, or the
    /// part a `partial + ?` bound proves, within its storage less its
    /// minimum free bytes, on no assumption the gate does not admit. Returns
    /// a warning for each bound left `partial + ?` or conditional.
    pub fn check(&self) -> Result<Vec<String>> {
        if !self.coverage.unreviewed.is_empty() {
            return Err(format!(
                "linked functions without a compiler frame record need exactly one coverage \
                 review:\n  {}",
                self.coverage.unreviewed.join("\n  ")
            )
            .into());
        }
        let mut warnings = Vec::new();
        for stack in &self.stacks {
            let (capacity, bound) = &stack.linked;
            let budget = capacity
                .checked_sub(stack.minimum_free_bytes)
                .filter(|budget| *budget != 0)
                .ok_or_else(|| {
                    format!(
                        "the {} has {capacity} bytes ({}) and cannot keep {} bytes free",
                        stack.name, stack.storage, stack.minimum_free_bytes
                    )
                })?;
            if let Some(assumption) = bound
                .assumptions
                .iter()
                .find(|assumption| !ADMITTED.contains(assumption))
            {
                return Err(format!(
                    "the {}'s bound assumes {assumption}, which the gate does not admit",
                    stack.name
                )
                .into());
            }
            for assumption in &bound.assumptions {
                warnings.push(format!(
                    "the {}'s bound is conditional on {assumption}",
                    stack.name
                ));
            }
            let (bytes, shown) = match bound.bytes {
                Some(bytes) => (bytes, bytes.to_string()),
                None => (bound.partial, format!("{} + ?", bound.partial)),
            };
            if bytes > budget {
                return Err(format!(
                    "the {} needs {shown} bytes from `{}`, more than its {budget}-byte budget \
                     ({capacity} bytes of {} less {} bytes free):\n{}",
                    stack.name,
                    stack.root,
                    stack.storage,
                    stack.minimum_free_bytes,
                    stack.render()
                )
                .into());
            }
            if bound.bytes.is_none() {
                warnings.push(format!(
                    "the {} is {shown} bytes from `{}`: its {} unresolved sites are in the report",
                    stack.name,
                    stack.root,
                    stack.unresolved.len()
                ));
            }
        }
        Ok(warnings)
    }

    /// The coverage and each stack's bound, budget, headroom, deepest path
    /// and unresolved sites.
    pub fn render(&self) -> String {
        let coverage = &self.coverage;
        let mut out = format!(
            "analysis: {} of {WORK_UNITS} work units, {} of {MEMORY_LIMIT} bytes of working memory\n",
            self.usage.work_units, self.usage.memory_bytes
        );
        let _ = writeln!(
            out,
            "frame coverage: {} of {} functions have a compiler frame record; {} reviewed, {} \
             unreviewed",
            coverage.measured,
            coverage.functions,
            coverage.reviewed.len(),
            coverage.unreviewed.len()
        );
        for (address, name, category) in &coverage.reviewed {
            let _ = writeln!(out, "  reviewed {category:?} {address:#010x} {name}");
        }
        for problem in &coverage.unreviewed {
            let _ = writeln!(out, "  unreviewed {problem}");
        }
        for stack in &self.stacks {
            out.push_str(&stack.render());
        }
        out
    }
}

impl StackBound {
    /// This stack's section of the report.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let (capacity, bound) = &self.linked;
        let budget = capacity.saturating_sub(self.minimum_free_bytes);
        let (bytes, state) = match bound.bytes {
            Some(bytes) if bound.assumptions.is_empty() => (bytes.to_string(), "proven"),
            Some(bytes) => (bytes.to_string(), "conditional"),
            None => (format!("{} + ?", bound.partial), "partial"),
        };
        let used = bound.bytes.unwrap_or(bound.partial);
        let headroom = if used <= budget {
            format!("{} bytes of headroom", budget - used)
        } else {
            format!("{} bytes over", used - budget)
        };
        let _ = writeln!(
            out,
            "{}: `{}` needs {bytes} bytes, {state}; budget {budget} bytes ({capacity} bytes of {} \
             less {} bytes free), {headroom}",
            self.name, self.root, self.storage, self.minimum_free_bytes
        );
        for assumption in &bound.assumptions {
            let _ = writeln!(out, "  assumes {assumption}");
        }
        let _ = writeln!(out, "  deepest path:");
        for (frame, function) in &self.path {
            let _ = writeln!(out, "    {frame:>6} {function}");
        }
        if !self.unresolved.is_empty() {
            let reasons = bound
                .reasons()
                .iter()
                .map(|(reason, count)| format!("{count} {reason}"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(
                out,
                "  unresolved: {} ({reasons}); a hole may reach deeper",
                self.unresolved.len()
            );
            for (reason, site, function) in &self.unresolved {
                let _ = writeln!(
                    out,
                    "    {reason} at {site:#010x} in {function} (closed by {})",
                    reason.closed_by()
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;

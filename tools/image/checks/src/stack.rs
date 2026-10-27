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
//! Every used hart requires a complete task bound. An unresolved indirect
//! site is reported with its reason and rejects the stack, regardless of how
//! its capacity was selected. Bootstrap paths retain their own handoff proof.
//! A root symbol the image lacks fails,
//! as does an image without compiler frame records or a linked function
//! without a record that the coverage review does not explain.

pub mod budget;
pub mod contract;
pub mod cores;
mod coverage;
pub mod frames;
pub mod handoff;
pub mod reachability;
pub mod rom;
pub mod sites;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;
use std::rc::Rc;

use oer_analysis_riscv_model::{Ledger, RunControl, WorkingMemory};
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_stack::{
    Analysis, Bound, DebugMetadata, Dwarf, Function, Reason, Resolutions, Summary, analyze,
    image_resolutions, parse_summaries, read_debug, stack_sizes,
};

use crate::stack::budget::{Budget, LIMITS};

pub use coverage::{Coverage, CoverageCategory, CoveragePolicy, CoverageReview};
#[cfg(test)]
use oer_chip_profile::policy::TaskStack;
use oer_chip_profile::policy::{StackPolicy, Storage};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

use crate::stack::contract::Contract;

/// The assumptions a check admits in a bound, each by name: a bound resting
/// on one is conditional and passes with a warning; any other fails. Each
/// leaves this list in the pull request that proves it.
pub const ADMITTED: &[oer_analysis_riscv_stack::Assumption] =
    &[oer_analysis_riscv_stack::Assumption::ExecutorInvariant];

/// What a bound with a number rests on, by short name: each assumption, and
/// `nesting` where a handler may lower the interrupt level. `None` when it
/// rests on nothing: the bound is proven.
pub fn conditions(
    assumptions: &BTreeSet<oer_analysis_riscv_stack::Assumption>,
    nesting: bool,
) -> Option<String> {
    let names: Vec<&str> = assumptions
        .iter()
        .map(|assumption| assumption.name())
        .chain(nesting.then_some("nesting"))
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// A bound's state in a report: `proven` only when it rests on nothing,
/// `conditional (…)` naming what it rests on, `partial` without a number.
pub fn bound_state(
    bytes: Option<u64>,
    assumptions: &BTreeSet<oer_analysis_riscv_stack::Assumption>,
    nesting: bool,
) -> String {
    match (bytes, conditions(assumptions, nesting)) {
        (None, _) => "partial".to_owned(),
        (Some(_), None) => "proven".to_owned(),
        (Some(_), Some(conditions)) => format!("conditional ({conditions})"),
    }
}

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
    std::fs::write(
        output.join(reachability::RUNTIME_REPORT),
        stacks.render_reachability(image, policy),
    )?;
    stacks.check()
}

/// Bound the bootstrap `image`'s stack, write [`BOOTSTRAP_REPORT`] into
/// `output`, then check it under `policy`. Returns the warnings it passed
/// with; `None` when the policy names no bootstrap stack.
pub fn audit_bootstrap_stack(
    image: &Image,
    policy: &StackPolicy,
    output: &Path,
) -> Result<Option<Vec<String>>> {
    let Some(bootstrap) = policy.stacks()?.bootstrap_stack else {
        return Ok(None);
    };
    let audit = StackAudit::of(
        image,
        policy,
        &[oer_chip_profile::policy::StackRoot::bootstrap(bootstrap)],
    )?;
    std::fs::write(output.join(BOOTSTRAP_REPORT), audit.render())?;
    std::fs::write(
        output.join(reachability::BOOTSTRAP_REPORT),
        audit.render_reachability(image, policy),
    )?;
    audit.check().map(Some)
}

/// One image's analysis with its pinned ROM, and the facts outside the
/// machine code every stack bound resolves indirect sites with.
pub struct Image {
    pub traps: oer_analysis_riscv_stack::TrapFact,
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
    pub resources: Box<oer_analysis_riscv_model::AnalysisResources>,
    /// Retained analysis and ELF allocations, reserved beside bound evidence.
    pub retained_bytes: u64,
    /// The family's selectors, which apply even when an image supplies its
    /// own stack layout.
    pub observed_functions: Vec<String>,
}

impl Image {
    /// The analysis of `elf` with the chip's pinned ROM ELF and its
    /// reviewed summaries (`[rom]` of the chip profile).
    pub fn read(root: &Path, profile: &oer_chip_profile::Profile, elf: &Path) -> Result<Self> {
        Self::read_with_inputs(
            root,
            profile,
            elf,
            &mut oer_os_durable::inputs::Inputs::default(),
        )
    }

    /// Record the analysis sources; the output ELF is fingerprinted separately
    /// after the final link, since placement can change its first-pass bytes.
    pub fn read_with_inputs(
        root: &Path,
        profile: &oer_chip_profile::Profile,
        elf: &Path,
        inputs: &mut oer_os_durable::inputs::Inputs,
    ) -> Result<Self> {
        let elf = std::fs::read(elf)
            .map_err(|error| format!("cannot read {}: {error}", elf.display()))?;
        Self::of_bytes_with_inputs(root, profile, elf, inputs)
    }

    /// [`Self::read`] of the ELF bytes `elf`.
    pub fn of_bytes(
        root: &Path,
        profile: &oer_chip_profile::Profile,
        elf: Vec<u8>,
    ) -> Result<Self> {
        Self::of_bytes_with_inputs(
            root,
            profile,
            elf,
            &mut oer_os_durable::inputs::Inputs::default(),
        )
    }

    fn of_bytes_with_inputs(
        root: &Path,
        profile: &oer_chip_profile::Profile,
        elf: Vec<u8>,
        inputs: &mut oer_os_durable::inputs::Inputs,
    ) -> Result<Self> {
        let rom = profile.facts.rom.as_ref().ok_or_else(|| {
            format!(
                "chips/{}/platform names no ROM layer with a [rom] ELF",
                profile.facts.id
            )
        })?;
        let rom_path = oer_tool_pins::artifacts::ensure_with_inputs(
            root,
            &profile.facts.id,
            &rom.elf,
            inputs,
        )?;
        let rom_elf = inputs.read(&rom_path)?;
        let summaries = match &rom.summaries {
            Some(path) => parse_summaries(&inputs.read_to_string(&root.join(path))?)?,
            None => Vec::new(),
        };
        let mut image = Self::new(elf, &[&rom_elf], &summaries, Contract::of(profile))?;
        image.observed_functions = profile.facts.observed_functions.clone();
        Ok(image)
    }

    /// The analysis of `elf` with its `companions` and their `summaries`,
    /// within the [`budget`] of one image analysis.
    fn new(
        elf: Vec<u8>,
        companions: &[&[u8]],
        summaries: &[Summary],
        contract: Option<Contract>,
    ) -> Result<Self> {
        let memory = WorkingMemory::new(LIMITS.working_memory_bytes)?;
        let mut control = Budget::new(
            LIMITS.work_units,
            std::time::Duration::from_millis(LIMITS.timeout_ms),
        );
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
        let mut preliminary = BTreeMap::new();
        let traps = if let Some(contract) = &contract {
            let interrupts = &contract.interrupts;
            let mut proof = Ledger::new(&memory, &mut control)?;
            let mut entry = None;
            for function in facts.functions() {
                control.checkpoint(1 + function.names.len() as u64)?;
                if function.names.contains(&interrupts.exception_entry_symbol) {
                    entry = Some(function.address);
                    break;
                }
            }
            let mut entries = Vec::new();
            for &hart in &interrupts.harts {
                control.checkpoint(1)?;
                proof.push(&mut entries, (hart, entry), &mut control)?;
            }
            let unknown = analyze(
                &facts,
                &companions,
                summaries,
                &dwarf,
                &mut proof,
                &mut control,
            )?;
            for (&address, function) in &unknown.functions {
                if function.limit_events != [0; 2] {
                    preliminary.insert(address, function.limit_events);
                }
            }
            oer_analysis_riscv_stack::prove_traps(
                &facts,
                &unknown,
                &entries,
                &dwarf,
                &mut keep,
                &mut control,
            )?
        } else {
            oer_analysis_riscv_stack::TrapFact {
                failure: Some(oer_analysis_riscv_stack::TrapFailure::Entry {
                    hart: None,
                    entry: None,
                }),
                ..Default::default()
            }
        };
        let analysis = oer_analysis_riscv_stack::analyze_with_traps(
            &facts,
            &companions,
            summaries,
            &dwarf,
            traps.semantics,
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
        let resolutions =
            image_resolutions(&facts, &analysis, &dwarf, &types, &mut keep, &mut control)?;
        drop(types);
        let stack_sizes = stack_sizes(&facts, &mut keep, &mut control)?;
        drop(companions);
        drop(facts);
        let mut image = Self {
            traps,
            elf,
            analysis,
            functions,
            stack_sizes,
            names,
            dwarf,
            resolutions: Resolutions::new(),
            contract,
            resources: Box::new(budget::resources(0, 0, Default::default())),
            retained_bytes: 0,
            observed_functions: Vec::new(),
        };
        let mut limited = oer_analysis_riscv_model::AnalysisLimited::default();
        for (&address, function) in &image.analysis.functions {
            let first = preliminary.remove(&address).unwrap_or_default();
            let limits = oer_analysis_riscv_model::FunctionLimits::value_events([
                first[0] + function.limit_events[0],
                first[1] + function.limit_events[1],
            ]);
            limited.function(&limits);
        }
        image.resources = Box::new(budget::resources(control.used(), memory.peak(), limited));
        image.resolutions = resolutions;
        image.retained_bytes = keep.bytes() + image.elf.len() as u64;
        Ok(image)
    }

    /// Admit witness allocations beside the retained image facts, using the
    /// existing limits. Bounds outlive their builder's operation, like Image.
    pub(crate) fn with_bounds<T>(
        &self,
        build: impl FnOnce(&mut Ledger<'_>, &mut Budget) -> Result<T>,
    ) -> Result<(T, u64)> {
        let memory = WorkingMemory::new(LIMITS.working_memory_bytes)?;
        let mut control = Budget::new(
            LIMITS.work_units,
            std::time::Duration::from_millis(LIMITS.timeout_ms),
        );
        let _retained = memory.reserve(self.retained_bytes, control.position())?;
        let mut keep = Ledger::new(&memory, &mut control)?;
        let value = build(&mut keep, &mut control)?;
        Ok((
            value,
            memory.peak().max(self.resources.usage.peak_admitted_bytes),
        ))
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
        match self.containing(address) {
            Some((name, 0)) => name,
            Some((name, offset)) => format!("{name}+{offset:#x}"),
            None => format!("{address:#010x}"),
        }
    }

    /// The name of the function (of the image or a companion) that holds
    /// `address`, and the address's offset in it.
    pub(crate) fn containing(&self, address: u32) -> Option<(String, u32)> {
        let (&start, facts) = self.analysis.functions.range(..=address).next_back()?;
        if address - start >= facts.function.size {
            return None;
        }
        let name = self
            .names
            .get(&start)
            .cloned()
            .unwrap_or_else(|| oer_analysis_elf::demangle(&facts.function.label()));
        Some((name, address - start))
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
}

/// The frame coverage and task stack bounds of one image.
#[derive(Debug)]
pub struct StackAudit {
    pub expression_limited_functions: usize,
    pub traps: oer_analysis_riscv_stack::TrapFact,
    pub coverage: Coverage,
    pub stacks: Vec<StackBound>,
    /// What the image's analysis used of its [`budget`].
    pub resources: Box<oer_analysis_riscv_model::AnalysisResources>,
    pub witness_memory_peak: u64,
    /// Independent task roots, reported without changing the stack gate.
    pub task_bounds: Vec<Bound>,
    /// The image declares these stacks proven: [`Self::check`] fails on any
    /// unresolved path in them.
    pub declared_proven: bool,
    /// Where the task stack's size came from, when the image's link says.
    pub size_sources: BTreeMap<u32, crate::linker::task_stack::SizeSource>,
    /// The verified jump into stage two that ends the bootstrap stack's path.
    pub handoff: Option<handoff::Handoff>,
}

/// One stack's budget and its root's bound.
#[derive(Debug)]
pub struct StackBound {
    pub name: String,
    pub hart: Option<u32>,
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
    pub(crate) fn render_reachability(&self, image: &Image, policy: &StackPolicy) -> String {
        let report = reachability::Report::new(image, policy);
        let mut out = report.header(self.witness_memory_peak);
        for (label, bound) in self.roots(policy) {
            out.push_str(&report.root(&label, bound));
        }
        out
    }

    /// Each root's report label and bound: the linked stacks, then the
    /// independent task roots.
    pub(crate) fn roots(&self, _policy: &StackPolicy) -> Vec<(String, &Bound)> {
        let mut roots = Vec::new();
        for stack in &self.stacks {
            let label = stack
                .hart
                .map_or_else(|| "bootstrap".into(), |hart| format!("CPU{hart}"));
            roots.push((label, &stack.linked.1));
        }
        for (index, bound) in self.task_bounds.iter().enumerate() {
            roots.push((format!("T{index}"), bound));
        }
        roots
    }
    pub(crate) fn of(
        image: &Image,
        policy: &StackPolicy,
        stacks: &[oer_chip_profile::policy::StackRoot<'_>],
    ) -> Result<Self> {
        let coverage = CoveragePolicy::load(policy.stacks()?.coverage_policy)?
            .coverage(&image.functions, &image.stack_sizes)?;
        let ((bounds, task_bounds), witness_memory_peak) = image.with_bounds(|keep, control| {
            let mut bounds = Vec::new();
            let mut ranges: Vec<(u64, u64, &str)> = Vec::new();
            let mut harts = BTreeSet::new();
            for root in stacks {
                if let Some(hart) = root.hart
                    && !harts.insert(hart)
                {
                    return Err(format!("duplicate task root for hart {hart}").into());
                }
                let name = &root.name;
                let stack = root.stack;
                let storage = match stack.storage() {
                    Storage::Symbol(symbol) => format!("`{symbol}`"),
                    Storage::Range { bottom, top } => format!("`{bottom}`..`{top}`"),
                };
                let Some(storage_bounds) = image.storage(stack.storage())? else {
                    return Err(format!("the image links no {name} ({storage})").into());
                };
                for &(start, end, previous) in &ranges {
                    if storage_bounds.start < end && start < storage_bounds.end {
                        return Err(format!("{name} overlaps {previous}").into());
                    }
                }
                ranges.push((storage_bounds.start, storage_bounds.end, name));
                let root_address = image.symbol(&stack.root).ok_or_else(|| {
                    format!(
                        "the image has no `{}`, the root the policy names for its {name}",
                        stack.root
                    )
                })?;
                let walked =
                    image
                        .analysis
                        .bound_with(root_address, &image.resolutions, keep, control)?;
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
                    name: name.clone(),
                    hart: root.hart,
                    root: stack.root.clone(),
                    storage,
                    minimum_free_bytes: u64::from(stack.minimum_free_bytes),
                    linked: (storage_bounds.capacity, walked),
                    path,
                    unresolved,
                });
            }
            let mut task_bounds = Vec::new();
            if stacks.iter().any(|root| root.hart.is_some()) {
                for &address in image.resolutions.task_roots() {
                    task_bounds.push(image.analysis.bound_with(
                        address,
                        &image.resolutions,
                        keep,
                        control,
                    )?);
                }
            }
            Ok((bounds, task_bounds))
        })?;
        Ok(Self {
            expression_limited_functions: image.analysis.expression_limited_functions(),
            traps: image.traps.clone(),
            coverage,
            stacks: bounds,
            resources: image.resources.clone(),
            witness_memory_peak,
            task_bounds,
            declared_proven: false,
            size_sources: BTreeMap::new(),
            handoff: None,
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
            if (self.declared_proven || stack.hart.is_some())
                && (bound.bytes.is_none() || !stack.unresolved.is_empty())
            {
                return Err(format!("every used hart needs a proven task bound, but the {} has {} unresolved sites:\n{}", stack.name, stack.unresolved.len(), stack.render()).into());
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
        let mut resources = (*self.resources).clone();
        if self.expression_limited_functions != 0 {
            resources
                .limited
                .result(oer_analysis_riscv_model::LimitReason::LocalBudget(
                    oer_analysis_riscv_model::LocalBudgetScope::FunctionExpressions,
                ));
        }
        let mut out = budget::header(&resources, resources.usage.peak_admitted_bytes);
        if self.declared_proven {
            out.push_str(
                "task stacks: the image sizes them from their bound; an unresolved path fails\n",
            );
        }
        if self.expression_limited_functions != 0 {
            let _ = writeln!(
                out,
                "expression limits: {} functions",
                self.expression_limited_functions
            );
        }
        let _ = writeln!(out, "{}", self.traps);
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
        if let Some(handoff) = &self.handoff {
            out.push_str(&handoff.render());
        }
        for stack in &self.stacks {
            if let Some(source) = stack.hart.and_then(|hart| self.size_sources.get(&hart)) {
                let source = match *source {
                    crate::linker::task_stack::SizeSource::Bound => format!(
                        "the {} bound plus the policy's minimum free bytes",
                        stack.state()
                    ),
                    other => other.to_string(),
                };
                let _ = writeln!(
                    out,
                    "{}: {} bytes allocated from {source}; minimum free {} bytes",
                    stack.name, stack.linked.0, stack.minimum_free_bytes
                );
            }
        }
        out
    }
}

impl StackBound {
    /// The bound's state: `proven`, `conditional (…)` or `partial`.
    pub fn state(&self) -> String {
        let bound = &self.linked.1;
        bound_state(bound.bytes, &bound.assumptions, false)
    }

    /// This stack's section of the report.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let (capacity, bound) = &self.linked;
        let budget = capacity.saturating_sub(self.minimum_free_bytes);
        let bytes = bound.bytes.map_or_else(
            || format!("{} + ?", bound.partial),
            |bytes| bytes.to_string(),
        );
        let state = self.state();
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
pub(crate) mod tests;

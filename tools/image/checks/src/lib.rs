//! The checks of an image: the stack check ([`stack`]), the
//! interrupt-stack check ([`interrupts`]), the placement check
//! ([`placement`]), and the post-link input check ([`linker`]) the image
//! linker (`oer-image-linker`) runs in place of `rust-lld`.
//!
//! The checks an image build runs when its caller requests them are
//! assembled here.
//!
//! [`Gates`] are the checks an image build runs: `cargo fw build` enforces
//! interrupt bounds and function frames, stand experiments report them, and
//! HIL enforces them all.
//! Every check reads what it needs from the chip profile and the stack
//! policy, whatever the chip's boot kind. A requested check whose data is
//! absent is a [`CheckResult::MissingContract`], which fails an enforced check: a
//! missing table never proves that a property does not apply.

#![forbid(unsafe_code)]

pub mod codegen;
pub mod critical_section;
pub mod interrupts;
pub mod linker;
pub mod panics;
pub mod placement;
pub mod sizes;
pub mod stack;

use std::{collections::BTreeSet, path::Path};

use serde::{Deserialize, Serialize};

use crate::interrupts::Required;
use oer_chip_profile::{Profile, policy::StackPolicy};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Which ELF of an image a check examined.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckedElf {
    /// The runtime (application) ELF.
    Runtime,
    /// A staged boot's bootstrap ELF.
    Bootstrap,
}

/// The result of one requested check. Only [`CheckResult::Passed`] and
/// [`CheckResult::NotApplicable`] pass it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "result", content = "reason")]
pub enum CheckResult {
    Passed,
    /// The check ran and the image does not hold the property.
    Failed(String),
    /// The chip data or policy the check needs is absent: the property was
    /// not shown, whether or not it applies. Fails a requested check.
    MissingContract(String),
    /// The property does not apply to this image, for the explicit reason
    /// given (never because data is missing).
    NotApplicable(String),
}

impl CheckResult {
    /// Whether the check passed.
    pub fn passes(&self) -> bool {
        matches!(self, Self::Passed | Self::NotApplicable(_))
    }
}

impl std::fmt::Display for CheckResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(formatter, "passed"),
            Self::Failed(reason) => write!(formatter, "failed: {reason}"),
            Self::MissingContract(what) => write!(formatter, "missing contract: {what}"),
            Self::NotApplicable(reason) => write!(formatter, "not applicable: {reason}"),
        }
    }
}

/// One requested check of one ELF and its result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRecord {
    pub elf: CheckedElf,
    pub check: String,
    pub result: CheckResult,
}

/// What a check of one of an image's ELF files reads.
pub struct CheckInput<'a> {
    /// The tree the image is built from.
    pub root: &'a Path,
    pub profile: &'a Profile,
    pub policy: &'a StackPolicy,
    pub elf: &'a Path,
    /// The staged runtime's flattened image, beside its ELF.
    pub flat: Option<&'a Path>,
    /// The bundle's directory, where reports go.
    pub output: &'a Path,
    /// The image declares its task stacks proven: an unresolved path in one
    /// fails the stack check.
    pub task_stack_proven: bool,
    /// The only hart whose reachable code may enter the critical section.
    pub critical_section_hart: u32,
    /// The stage-two ELF the bootstrap enters, for the bootstrap's `handoff`.
    pub stage_two_elf: Option<&'a Path>,
    /// The crate layers and the image's panic budget, for the panic check.
    pub panics: Option<&'a panics::Contract>,
}

/// What the checks passed with.
#[derive(Clone, Debug, Default)]
pub struct CheckOutcome {
    /// Measurements of the same analysis the checks used.
    pub sizes: Option<sizes::Measurements>,
    /// Every requested check, by name, with its result.
    pub results: Vec<(String, CheckResult)>,
    /// The reports they wrote into the bundle, by file name.
    pub reports: Vec<String>,
    /// The reviewed ROM summaries an analysis applied.
    pub rom_summaries: BTreeSet<String>,
    /// The frame coverage reviews (`stack-coverage.toml` symbols) a stack
    /// check applied.
    pub coverage_reviews: BTreeSet<String>,
    /// Partial or conditional results they passed with.
    pub warnings: Vec<String>,
}

/// The ROM ranges of `profile`'s chip no image may call into.
fn forbidden_rom(profile: &oer_chip_profile::Profile) -> &[oer_espressif_facts::RomRange] {
    profile.facts.rom.as_ref().map_or(&[], |rom| &rom.forbidden)
}

/// The placement report in an image bundle.
pub const PLACEMENT_REPORT: &str = "placement.txt";

/// An audit of the runtime ELF that the caller adds to the checks.
pub type Audit = Box<dyn Fn(&oer_analysis_elf::Elf<'_>) -> Result<()> + Send + Sync>;

/// One check a developer can request.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Check {
    /// Every recorded function frame in runtime and bootstrap.
    Frames,
    /// The task and bootstrap stacks and the frame coverage.
    Stack,
    /// The image's placement in the chip's memory map.
    Placement,
    /// Each hart's interrupt stack, proven.
    Interrupts,
    /// The panic sites each root reaches, by layer, against the image's reporting budget.
    Panics,
}

/// Which requested checks must pass for the build to succeed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Enforced {
    /// Every requested check, including the caller's audit.
    All,
    /// Only the named built-in checks; the caller's audit remains a report.
    Only(BTreeSet<Check>),
    /// Every check is reported; failures become warnings.
    #[default]
    Reported,
}

impl Enforced {
    /// Whether the requested check `name` must pass. An audit is enforced
    /// only by [`Enforced::All`].
    pub fn requires(&self, name: &str) -> bool {
        match self {
            Self::All => true,
            Self::Only(checks) => {
                let check = match name {
                    FRAMES => Check::Frames,
                    STACK => Check::Stack,
                    INTERRUPTS => Check::Interrupts,
                    PLACEMENT => Check::Placement,
                    PANICS => Check::Panics,
                    _ => return false,
                };
                checks.contains(&check)
            }
            Self::Reported => false,
        }
    }
}

/// The checks of one image build.
#[derive(Default)]
pub struct Gates {
    /// Which requested checks fail the build when they do not pass;
    /// the others are reported as warnings.
    pub enforced: Enforced,
    /// Every recorded frame, reporting move-limit and exception-ceiling excesses.
    pub frames: bool,
    /// The task and bootstrap stacks.
    pub stack: bool,
    /// The image's placement in the chip's memory map.
    pub placement: bool,
    /// The interrupt code placement and stack policy, when checked.
    pub interrupts: Option<Required>,
    /// The panic sites of the stack and interrupt roots, by layer, compared
    /// with the image's reporting budget ([`panics`]).
    pub panics: bool,
    /// The caller's own audit of the runtime ELF.
    pub audit: Option<Audit>,
}

impl Gates {
    /// Every check, with interrupt code and stacks held to `interrupts`.
    pub fn all(interrupts: Required) -> Self {
        Self {
            enforced: Enforced::All,
            frames: true,
            stack: true,
            placement: true,
            interrupts: Some(interrupts),
            panics: true,
            audit: None,
        }
    }

    /// The checks `checks` names, with the product interrupt policy.
    pub fn of(checks: &[Check]) -> Self {
        Self {
            enforced: Enforced::All,
            frames: checks.contains(&Check::Frames),
            stack: checks.contains(&Check::Stack),
            placement: checks.contains(&Check::Placement),
            interrupts: checks
                .contains(&Check::Interrupts)
                .then_some(Required::Product),
            panics: checks.contains(&Check::Panics),
            audit: None,
        }
    }
}

/// The names of the checks.
pub const STACK: &str = "stack";
const FRAMES: &str = "frames";
const INTERRUPTS: &str = "interrupts";
const PLACEMENT: &str = "placement";
const PANICS: &str = "panics";
const AUDIT: &str = "audit";

/// The code sections of the mask ROM, from `profile`'s pinned `[rom]` ELF at
/// `root`; none for a chip without one.
fn rom_code(root: &Path, profile: &Profile) -> Result<Vec<oer_espressif_facts::Region>> {
    let Some(rom) = &profile.facts.rom else {
        return Ok(Vec::new());
    };
    let bytes = std::fs::read(oer_tool_pins::artifacts::ensure(
        root,
        &profile.facts.id,
        &rom.elf,
    )?)?;
    let elf = oer_analysis_elf::Elf::parse(&bytes)?;
    elf.sections()
        .filter(|section| section.allocated && section.executable && section.size > 0)
        .map(|section| {
            Ok(oer_espressif_facts::Region::new(
                u32::try_from(section.address)?,
                u32::try_from(section.size)?,
            )?)
        })
        .collect()
}

/// The result of an analysis that ran: passed, or failed with its error.
fn ran(analysis: Result<()>) -> CheckResult {
    match analysis {
        Ok(()) => CheckResult::Passed,
        Err(error) => CheckResult::Failed(error.to_string()),
    }
}

impl Gates {
    /// The names of the checks requested of `elf`; each gets a result in
    /// the outcome of [`Gates::runtime`] or [`Gates::bootstrap`].
    pub fn requested(&self, elf: CheckedElf) -> Vec<String> {
        match elf {
            CheckedElf::Runtime => [
                (self.frames, FRAMES),
                (self.stack, STACK),
                (self.interrupts.is_some(), INTERRUPTS),
                (self.placement, PLACEMENT),
                (self.panics, PANICS),
                (self.audit.is_some(), AUDIT),
            ]
            .into_iter()
            .filter(|(requested, _)| *requested)
            .map(|(_, name)| name.to_owned())
            .collect(),
            // A staged boot's bootstrap has its own stack.
            CheckedElf::Bootstrap => [
                (self.frames, FRAMES),
                (self.stack, STACK),
                (self.panics, PANICS),
            ]
            .into_iter()
            .filter(|(requested, _)| *requested)
            .map(|(_, name)| name.to_owned())
            .collect(),
        }
    }

    /// Every requested check gets a result: an analysis that runs passes or
    /// fails; one whose chip data or policy is absent is a missing
    /// contract, never "not applicable".
    pub fn runtime(&self, input: &CheckInput<'_>) -> Result<CheckOutcome> {
        let mut outcome = CheckOutcome::default();
        let mut image_profile = input.policy.runtime_profile(input.profile)?;
        // The image linker sized a proven task stack; its range is exactly
        // that size.
        let linked_task_stack = linker::strict::Placement::read(input.output)?.task_stacks;
        for allocation in &linked_task_stack {
            StackPolicy::set_task_stack_bytes(
                &mut image_profile,
                allocation.hart,
                allocation.bytes,
            )?;
        }
        let profile = &image_profile;
        let chip = |table: &str| {
            format!(
                "chips/{}/platform/chip.toml names no {table}",
                profile.facts.id
            )
        };
        let missing = |what: String| CheckResult::MissingContract(what);
        // What each analysis needs.
        let rom = profile.facts.rom.is_some();
        let stacks = input.policy.stacks().is_ok();
        let interrupts = profile.facts.memory.is_some() && profile.facts.interrupts.is_some();
        let stack_runs = self.stack && rom && stacks;
        let frames_run = self.frames && rom;
        let interrupts_run = self.interrupts.is_some() && rom && interrupts;
        if self.stack && !stack_runs {
            outcome.results.push((
                STACK.to_owned(),
                missing(if rom {
                    "the image's stack policy names no stacks".to_owned()
                } else {
                    chip("[rom] ELF")
                }),
            ));
        }
        if self.interrupts.is_some() && !interrupts_run {
            outcome.results.push((
                INTERRUPTS.to_owned(),
                missing(if rom {
                    chip("[memory] map or [interrupts] contract")
                } else {
                    chip("[rom] ELF")
                }),
            ));
        }
        if self.frames && !frames_run {
            outcome
                .results
                .push((FRAMES.to_owned(), missing(chip("[rom] ELF"))));
        }
        {
            // One analysis serves the frame, task-stack and interrupt checks.
            match crate::stack::Image::read(input.root, profile, input.elf) {
                Ok(image) => {
                    // These are memory-safety prerequisites, independent of optional
                    // product/diagnostic checks and their reporting policy.
                    let cores = crate::stack::cores::audit_runtime(&image, input.policy, profile)?;
                    let interrupt_stacks = crate::interrupts::interrupt_stacks_of(&image)?;
                    let critical_section = crate::critical_section::audit_runtime(
                        &image,
                        input.policy,
                        &interrupt_stacks,
                        input.critical_section_hart,
                    )?;
                    std::fs::write(
                        input.output.join(crate::critical_section::REPORT),
                        critical_section,
                    )?;
                    std::fs::write(input.output.join(crate::stack::cores::REPORT), cores)?;
                    outcome.reports.extend(
                        [crate::critical_section::REPORT, crate::stack::cores::REPORT]
                            .map(str::to_owned),
                    );
                    outcome.sizes = Some(sizes::Measurements::of(&image, profile)?);
                    outcome
                        .rom_summaries
                        .extend(image.analysis.summaries.iter().cloned());
                    if frames_run {
                        self.frames_of(&image, input, CheckedElf::Runtime, &mut outcome);
                    }
                    let mut sites = crate::stack::sites::Report::default();
                    let mut interrupt_roots = None;
                    let mut stack_roots = None;
                    if let Some(required) = self.interrupts.filter(|_| interrupts_run) {
                        let result = ran((|| -> Result<()> {
                            let stacks = interrupt_stacks;
                            interrupt_roots = Some(panics::interrupt_roots(&image, &stacks));
                            if let Some(sizes) = &mut outcome.sizes {
                                sizes.interrupts(&image, &stacks)?;
                            }

                            let report =
                                crate::stack::reachability::Report::new(&image, input.policy);
                            let mut reachability = report.header(stacks.witness_memory_peak);
                            for hart in &stacks.harts {
                                for level in &hart.levels {
                                    reachability.push_str(&report.root(
                                        &format!("hart{}-P{}", hart.core, level.level),
                                        &level.bound,
                                    ));
                                }
                                reachability.push_str(&report.root(
                                    &format!("hart{}-exception", hart.core),
                                    &hart.exception.bound,
                                ));
                            }
                            for hart in &stacks.harts {
                                for level in &hart.levels {
                                    sites.add(
                                        &image,
                                        &format!("hart{}-P{}", hart.core, level.level),
                                        &level.bound,
                                    );
                                }
                                sites.add(
                                    &image,
                                    &format!("hart{}-exception", hart.core),
                                    &hart.exception.bound,
                                );
                            }
                            crate::stack::rom::audit_forbidden(
                                &image,
                                forbidden_rom(profile),
                                stacks.harts.iter().flat_map(|hart| {
                                    hart.levels
                                        .iter()
                                        .map(|level| {
                                            (
                                                format!("hart{}-P{}", hart.core, level.level),
                                                &level.bound,
                                            )
                                        })
                                        .chain([(
                                            format!("hart{}-exception", hart.core),
                                            &hart.exception.bound,
                                        )])
                                }),
                            )?;
                            std::fs::write(
                                input
                                    .output
                                    .join(crate::stack::reachability::INTERRUPT_REPORT),
                                reachability,
                            )?;
                            outcome
                                .reports
                                .push(crate::stack::reachability::INTERRUPT_REPORT.to_owned());
                            let placement = crate::interrupts::InterruptPlacement::of(
                                &image, &stacks, profile,
                            )?;
                            let linked = crate::linker::strict::Placement::read(input.output)?;
                            std::fs::write(
                                input.output.join(crate::interrupts::INTERRUPT_REPORT),
                                format!(
                                    "{}{}",
                                    stacks.render_with_placement(&placement),
                                    linked.summary()
                                ),
                            )?;
                            outcome
                                .reports
                                .push(crate::interrupts::INTERRUPT_REPORT.to_owned());
                            std::fs::write(
                                input
                                    .output
                                    .join(crate::interrupts::INTERRUPT_PLACEMENT_REPORT),
                                format!("{}\n{}", placement.render(), linked.render()),
                            )?;
                            outcome
                                .reports
                                .push(crate::interrupts::INTERRUPT_PLACEMENT_REPORT.to_owned());
                            outcome.warnings.extend(stacks.check(required)?);
                            Ok(())
                        })());
                        outcome.results.push((INTERRUPTS.to_owned(), result));
                    }
                    if stack_runs {
                        let result = ran((|| -> Result<()> {
                            let warnings = (|| -> Result<Vec<String>> {
                                let mut audit = crate::stack::StackAudit::of(
                                    &image,
                                    input.policy,
                                    &input.policy.runtime_task_stacks(),
                                )?;
                                outcome
                                    .coverage_reviews
                                    .extend(audit.coverage.applied.iter().cloned());
                                stack_roots =
                                    Some(panics::stack_roots(&image, &audit, input.policy));
                                crate::stack::rom::audit_forbidden(
                                    &image,
                                    forbidden_rom(input.profile),
                                    audit.roots(input.policy),
                                )?;
                                audit.declared_proven = input.task_stack_proven;
                                for root in input.policy.runtime_task_stacks() {
                                    if let Some(hart) = root.hart {
                                        audit.size_sources.insert(
                                            hart,
                                            linker::task_stack::SizeSource::of(
                                                input.policy,
                                                hart,
                                                linked_task_stack
                                                    .iter()
                                                    .find(|allocation| allocation.hart == hart),
                                            ),
                                        );
                                    }
                                }
                                if let Some(sizes) = &mut outcome.sizes {
                                    for stack in &audit.stacks {
                                        sizes.task(&image, stack)?;
                                    }
                                }
                                for (label, bound) in audit.roots(input.policy) {
                                    sites.add(&image, &label, bound);
                                }
                                std::fs::write(
                                    input.output.join(crate::stack::RUNTIME_REPORT),
                                    audit.render(),
                                )?;
                                std::fs::write(
                                    input
                                        .output
                                        .join(crate::stack::reachability::RUNTIME_REPORT),
                                    audit.render_reachability(&image, input.policy),
                                )?;
                                audit.check()
                            })();
                            outcome
                                .reports
                                .push(crate::stack::RUNTIME_REPORT.to_owned());
                            outcome
                                .reports
                                .push(crate::stack::reachability::RUNTIME_REPORT.to_owned());
                            outcome.warnings.extend(warnings?);
                            Ok(())
                        })());
                        outcome.results.push((STACK.to_owned(), result));
                    }
                    if !sites.roots.is_empty() {
                        sites.write(
                            input.output,
                            crate::stack::sites::RUNTIME_REPORT,
                            crate::stack::sites::RUNTIME_JSON,
                        )?;
                        outcome.reports.extend(
                            [
                                crate::stack::sites::RUNTIME_REPORT,
                                crate::stack::sites::RUNTIME_JSON,
                            ]
                            .map(str::to_owned),
                        );
                    }
                    if self.panics {
                        let roots =
                            match (stack_runs && interrupts_run, stack_roots, interrupt_roots) {
                                (false, _, _) => Err(missing(
                                    "the panic check counts the roots of the stack and interrupts \
                                 checks, which do not both run"
                                        .to_owned(),
                                )),
                                (true, Some(mut stack), Some(interrupts)) => {
                                    stack.extend(interrupts);
                                    Ok(stack)
                                }
                                (true, _, _) => Err(CheckResult::Failed(
                                    "the stack or interrupts analysis did not reach its roots"
                                        .to_owned(),
                                )),
                            };
                        self.panics_of(&image, input, CheckedElf::Runtime, roots, &mut outcome);
                    }
                }
                Err(error) => {
                    return Err(format!(
                        "critical-section prerequisite: image analysis failed: {error}"
                    )
                    .into());
                }
            }
        }
        if self.panics && !outcome.results.iter().any(|(name, _)| name == PANICS) {
            outcome.results.push((
                PANICS.to_owned(),
                missing(
                    "the panic check counts the roots of the stack and interrupts checks, \
                     which do not run"
                        .to_owned(),
                ),
            ));
        }
        if self.placement {
            let result = match (&profile.facts.memory, &profile.facts.placement) {
                (Some(memory), Some(placement)) => ran((|| -> Result<()> {
                    let rom = if placement
                        .interrupt_handlers
                        .iter()
                        .any(|region| region == crate::placement::ROM)
                    {
                        rom_code(input.root, profile)?
                    } else {
                        Vec::new()
                    };
                    let contract = crate::placement::Contract {
                        memory,
                        placement,
                        interrupts: profile.facts.interrupts.as_ref(),
                        rom: &rom,
                    };
                    let report = crate::placement::audit(&contract, input.elf, input.flat)?;
                    std::fs::write(input.output.join(PLACEMENT_REPORT), report)?;
                    outcome.reports.push(PLACEMENT_REPORT.to_owned());
                    Ok(())
                })()),
                _ => missing(chip("[memory] map or [placement] contract")),
            };
            outcome.results.push((PLACEMENT.to_owned(), result));
        }
        if let Some(audit) = &self.audit {
            let result = ran((|| -> Result<()> {
                audit(&oer_analysis_elf::Elf::parse(&std::fs::read(input.elf)?)?)
            })());
            outcome.results.push((AUDIT.to_owned(), result));
        }
        Ok(outcome)
    }

    /// Check a staged boot's bootstrap ELF.
    pub fn bootstrap(&self, input: &CheckInput<'_>) -> Result<CheckOutcome> {
        let mut outcome = CheckOutcome::default();
        if !self.stack && !self.frames && !self.panics {
            return Ok(outcome);
        }
        if input.profile.facts.rom.is_none() {
            let missing = CheckResult::MissingContract(format!(
                "chips/{}/platform names no ROM layer with a [rom] ELF",
                input.profile.facts.id
            ));
            for name in self.requested(CheckedElf::Bootstrap) {
                outcome.results.push((name, missing.clone()));
            }
            return Ok(outcome);
        }
        let image = match crate::stack::Image::read(input.root, input.profile, input.elf) {
            Ok(image) => image,
            Err(error) => {
                for name in self.requested(CheckedElf::Bootstrap) {
                    outcome.results.push((
                        name,
                        CheckResult::Failed(format!("the image analysis failed: {error}")),
                    ));
                }
                return Ok(outcome);
            }
        };
        self.bootstrap_image(input, image)
    }

    fn bootstrap_image(
        &self,
        input: &CheckInput<'_>,
        mut image: crate::stack::Image,
    ) -> Result<CheckOutcome> {
        let mut outcome = CheckOutcome {
            sizes: Some(sizes::Measurements::of(&image, input.profile)?),
            rom_summaries: image.analysis.summaries.iter().cloned().collect(),
            ..CheckOutcome::default()
        };
        if self.frames {
            self.frames_of(&image, input, CheckedElf::Bootstrap, &mut outcome);
        }
        if !self.stack {
            if self.panics {
                outcome.results.push((
                    PANICS.to_owned(),
                    CheckResult::MissingContract(
                        "the panic check counts the roots of the stack check, which does not run"
                            .to_owned(),
                    ),
                ));
            }
            return Ok(outcome);
        }
        let mut stack_roots = None;
        let result = if input.policy.stacks().is_err() {
            CheckResult::MissingContract("the image's stack policy names no stacks".to_owned())
        } else {
            match (|| -> Result<Option<Vec<String>>> {
                let Some(stack) = input.policy.stacks()?.bootstrap_stack else {
                    return Ok(None);
                };
                crate::stack::cores::audit_bootstrap(&image, input.profile)?;
                let handoff = match &stack.handoff {
                    Some(symbol) => {
                        let stage_two = input.stage_two_elf.ok_or(
                            "the bootstrap policy names a handoff, but no stage-two ELF is given",
                        )?;
                        let top = input
                            .policy
                            .task_stack(
                                input
                                    .profile
                                    .facts
                                    .interrupts
                                    .as_ref()
                                    .ok_or("handoff needs hart geometry")?
                                    .boot_hart,
                            )
                            .ok_or("boot task stack is missing")?
                            .top_symbol
                            .as_deref()
                            .ok_or(
                                "the boot hart task stack names no top symbol for the handoff",
                            )?;
                        Some(image.end_path_at_stage_two(symbol, stage_two, top)?)
                    }
                    None => None,
                };
                let mut audit = crate::stack::StackAudit::of(
                    &image,
                    input.policy,
                    &[oer_chip_profile::policy::StackRoot::bootstrap(stack)],
                )?;
                outcome
                    .coverage_reviews
                    .extend(audit.coverage.applied.iter().cloned());
                crate::stack::rom::audit_forbidden(
                    &image,
                    forbidden_rom(input.profile),
                    audit.roots(input.policy),
                )?;
                audit.handoff = handoff;
                stack_roots = Some(crate::panics::stack_roots(&image, &audit, input.policy));
                if let Some(sizes) = &mut outcome.sizes {
                    for stack in &audit.stacks {
                        sizes.task(&image, stack)?;
                    }
                }
                let mut sites = crate::stack::sites::Report::default();
                for (label, bound) in audit.roots(input.policy) {
                    sites.add(&image, &label, bound);
                }
                sites.write(
                    input.output,
                    crate::stack::sites::BOOTSTRAP_REPORT,
                    crate::stack::sites::BOOTSTRAP_JSON,
                )?;
                std::fs::write(
                    input.output.join(crate::stack::BOOTSTRAP_REPORT),
                    audit.render(),
                )?;
                std::fs::write(
                    input
                        .output
                        .join(crate::stack::reachability::BOOTSTRAP_REPORT),
                    audit.render_reachability(&image, input.policy),
                )?;
                audit.check().map(Some)
            })() {
                Ok(Some(warnings)) => {
                    outcome.warnings.extend(warnings);
                    outcome
                        .reports
                        .push(crate::stack::BOOTSTRAP_REPORT.to_owned());
                    outcome
                        .reports
                        .push(crate::stack::reachability::BOOTSTRAP_REPORT.to_owned());
                    outcome.reports.extend(
                        [
                            crate::stack::sites::BOOTSTRAP_REPORT,
                            crate::stack::sites::BOOTSTRAP_JSON,
                        ]
                        .map(str::to_owned),
                    );
                    CheckResult::Passed
                }
                Ok(None) => CheckResult::MissingContract(
                    "the image's stack policy names no bootstrap stack".to_owned(),
                ),
                Err(error) => CheckResult::Failed(error.to_string()),
            }
        };
        outcome.results.push((STACK.to_owned(), result));
        if self.panics {
            let roots = stack_roots.ok_or_else(|| {
                CheckResult::Failed(
                    "the bootstrap stack analysis did not reach its root".to_owned(),
                )
            });
            self.panics_of(&image, input, CheckedElf::Bootstrap, roots, &mut outcome);
        }
        Ok(outcome)
    }

    /// The panic check of `elf` over `roots`, or the result that stands in
    /// for it when the roots are missing.
    fn panics_of(
        &self,
        image: &crate::stack::Image,
        input: &CheckInput<'_>,
        elf: CheckedElf,
        roots: std::result::Result<Vec<panics::Root>, CheckResult>,
        outcome: &mut CheckOutcome,
    ) {
        let result = match (roots, input.panics) {
            (Err(result), _) => result,
            (Ok(_), None) => CheckResult::MissingContract(
                "the image build supplies no panic contract (crate layers and budget)".to_owned(),
            ),
            (Ok(roots), Some(contract)) => ran((|| -> Result<()> {
                let report = panics::Report {
                    elf,
                    roots,
                    names: &image.names,
                };
                let name = match elf {
                    CheckedElf::Runtime => panics::RUNTIME_REPORT,
                    CheckedElf::Bootstrap => panics::BOOTSTRAP_REPORT,
                };
                std::fs::write(input.output.join(name), report.render(contract))?;
                outcome.reports.push(name.to_owned());
                outcome.warnings.extend(report.warnings(contract));
                Ok(())
            })()),
        };
        outcome.results.push((PANICS.to_owned(), result));
    }

    fn frames_of(
        &self,
        image: &crate::stack::Image,
        input: &CheckInput<'_>,
        elf: CheckedElf,
        outcome: &mut CheckOutcome,
    ) {
        let result = ran(
            crate::stack::frames::audit(image, input.policy, elf, input.output)
                .map(|warnings| outcome.warnings.extend(warnings)),
        );
        outcome.results.push((FRAMES.to_owned(), result));
        outcome.reports.push(
            match elf {
                CheckedElf::Runtime => crate::stack::frames::RUNTIME_REPORT,
                CheckedElf::Bootstrap => crate::stack::frames::BOOTSTRAP_REPORT,
            }
            .to_owned(),
        );
    }
}

#[cfg(test)]
mod enforcement_tests {
    use super::*;

    #[test]
    fn constructors_enforce_every_requested_check_and_default_reports() {
        for gates in [
            Gates::all(Required::Product),
            Gates::of(&[Check::Interrupts]),
        ] {
            assert_eq!(gates.enforced, Enforced::All);
        }
        let gates = Gates::default();
        assert_eq!(gates.enforced, Enforced::Reported);
        assert!(gates.requested(CheckedElf::Runtime).is_empty());
        let frames = Gates::of(&[Check::Frames]);
        for elf in [CheckedElf::Runtime, CheckedElf::Bootstrap] {
            assert_eq!(frames.requested(elf), [FRAMES]);
            assert!(
                Gates::all(Required::Diagnostic)
                    .requested(elf)
                    .contains(&FRAMES.to_owned())
            );
        }
    }

    #[test]
    fn only_enforces_the_named_checks_and_never_the_audit() {
        for (check, name) in [
            (Check::Frames, FRAMES),
            (Check::Stack, STACK),
            (Check::Interrupts, INTERRUPTS),
            (Check::Placement, PLACEMENT),
            (Check::Panics, PANICS),
        ] {
            let enforced = Enforced::Only([check].into());
            for requested in [FRAMES, STACK, INTERRUPTS, PLACEMENT, PANICS, AUDIT] {
                assert_eq!(enforced.requires(requested), requested == name);
                assert!(Enforced::All.requires(requested));
                assert!(!Enforced::Reported.requires(requested));
            }
        }
        assert!(!Enforced::Only(BTreeSet::new()).requires(AUDIT));
    }
}

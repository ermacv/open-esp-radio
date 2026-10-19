//! The checks of an image: the stack check ([`stack`]), the
//! interrupt-stack check ([`interrupts`]), the placement check
//! ([`placement`]), and the post-link input check ([`linker`]) the image
//! linker (`oer-image-linker`) runs in place of `rust-lld`.
//!
//! The checks an image build runs when its caller requests them are
//! assembled here.
//!
//! [`Gates`] are the checks an image build runs: `cargo fw` reports them,
//! HIL images must pass them all.
//! Every check reads what it needs from the chip profile and the stack
//! policy, whatever the chip's boot kind. A requested check whose data is
//! absent is a [`CheckResult::MissingContract`], which fails the build: a
//! missing table never proves that a property does not apply.

#![forbid(unsafe_code)]

pub mod interrupts;
pub mod linker;
pub mod placement;
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
}

/// What the checks passed with.
#[derive(Clone, Debug, Default)]
pub struct CheckOutcome {
    /// Every requested check, by name, with its result.
    pub results: Vec<(String, CheckResult)>,
    /// The reports they wrote into the bundle, by file name.
    pub reports: Vec<String>,
    /// The reviewed ROM summaries an analysis applied.
    pub rom_summaries: BTreeSet<String>,
    /// Partial or conditional results they passed with.
    pub warnings: Vec<String>,
}

/// The placement report in an image bundle.
pub const PLACEMENT_REPORT: &str = "placement.txt";

/// An audit of the runtime ELF that the caller adds to the checks.
pub type Audit = Box<dyn Fn(&oer_analysis_elf::Elf<'_>) -> Result<()> + Send + Sync>;

/// One check a developer can request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Check {
    /// The task and bootstrap stacks and the frame coverage.
    Stack,
    /// The image's placement in the chip's memory map.
    Placement,
    /// Each hart's interrupt stack, proven.
    Interrupts,
}

/// The checks of one image build.
#[derive(Default)]
pub struct Gates {
    /// Whether a requested check that does not pass fails the build;
    /// otherwise the build reports it as a warning.
    pub enforced: bool,
    /// The task and bootstrap stacks.
    pub stack: bool,
    /// The image's placement in the chip's memory map.
    pub placement: bool,
    /// What the interrupt stacks must reach, when they are checked.
    pub interrupts: Option<Required>,
    /// The caller's own audit of the runtime ELF.
    pub audit: Option<Audit>,
}

impl Gates {
    /// Every check, the interrupt stacks held to `interrupts`.
    pub fn all(interrupts: Required) -> Self {
        Self {
            enforced: true,
            stack: true,
            placement: true,
            interrupts: Some(interrupts),
            audit: None,
        }
    }

    /// The checks `checks` names, the interrupt stacks proven.
    pub fn of(checks: &[Check]) -> Self {
        Self {
            enforced: true,
            stack: checks.contains(&Check::Stack),
            placement: checks.contains(&Check::Placement),
            interrupts: checks
                .contains(&Check::Interrupts)
                .then_some(Required::Proven),
            audit: None,
        }
    }
}

/// The names of the checks.
const STACK: &str = "stack";
const INTERRUPTS: &str = "interrupts";
const PLACEMENT: &str = "placement";
const AUDIT: &str = "audit";

/// The code sections of the mask ROM, from `profile`'s pinned `[rom]` ELF at
/// `root`; none for a chip without one.
fn rom_code(root: &Path, profile: &Profile) -> Result<Vec<oer_chip_profile::Region>> {
    let Some(rom) = &profile.rom else {
        return Ok(Vec::new());
    };
    let bytes = std::fs::read(oer_tool_pins::artifacts::ensure(
        root,
        &profile.id,
        &rom.elf,
    )?)?;
    let elf = oer_analysis_elf::Elf::parse(&bytes)?;
    elf.sections()
        .filter(|section| section.allocated && section.executable && section.size > 0)
        .map(|section| {
            Ok(oer_chip_profile::Region {
                origin: u32::try_from(section.address)?,
                length: u32::try_from(section.size)?,
            })
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
                (self.stack, STACK),
                (self.interrupts.is_some(), INTERRUPTS),
                (self.placement, PLACEMENT),
                (self.audit.is_some(), AUDIT),
            ]
            .into_iter()
            .filter(|(requested, _)| *requested)
            .map(|(_, name)| name.to_owned())
            .collect(),
            // A staged boot's bootstrap has its own stack.
            CheckedElf::Bootstrap => [(self.stack, STACK)]
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
        let profile = input.profile;
        let chip = |table: &str| format!("chips/{}/platform/chip.toml names no {table}", profile.id);
        let missing = |what: String| CheckResult::MissingContract(what);
        // What each analysis needs.
        let rom = profile.rom.is_some();
        let stacks = input.policy.stacks().is_ok();
        let interrupts = profile.memory.is_some() && profile.interrupts.is_some();
        let stack_runs = self.stack && rom && stacks;
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
        if stack_runs || interrupts_run {
            // One analysis of the image serves both stack checks.
            match crate::stack::Image::read(input.root, profile, input.elf) {
                Ok(image) => {
                    outcome
                        .rom_summaries
                        .extend(image.analysis.summaries.iter().cloned());
                    if let Some(required) = self.interrupts.filter(|_| interrupts_run) {
                        let result = ran((|| -> Result<()> {
                            let stacks = crate::interrupts::interrupt_stacks_of(&image)?;
                            std::fs::write(
                                input.output.join(crate::interrupts::INTERRUPT_REPORT),
                                stacks.render(),
                            )?;
                            outcome
                                .reports
                                .push(crate::interrupts::INTERRUPT_REPORT.to_owned());
                            outcome.warnings.extend(stacks.check(required)?);
                            Ok(())
                        })());
                        outcome.results.push((INTERRUPTS.to_owned(), result));
                    }
                    if stack_runs {
                        let result = ran((|| -> Result<()> {
                            let warnings = crate::stack::audit_runtime_stacks(
                                &image,
                                input.policy,
                                input.output,
                            );
                            outcome
                                .reports
                                .push(crate::stack::RUNTIME_REPORT.to_owned());
                            outcome.warnings.extend(warnings?);
                            Ok(())
                        })());
                        outcome.results.push((STACK.to_owned(), result));
                    }
                }
                Err(error) => {
                    // The analysis both checks share did not run.
                    let failed = CheckResult::Failed(format!("the image analysis failed: {error}"));
                    if stack_runs {
                        outcome.results.push((STACK.to_owned(), failed.clone()));
                    }
                    if interrupts_run {
                        outcome.results.push((INTERRUPTS.to_owned(), failed));
                    }
                }
            }
        }
        if self.placement {
            let result = match (&profile.memory, &profile.placement) {
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
                        interrupts: profile.interrupts.as_ref(),
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
        if !self.stack {
            return Ok(outcome);
        }
        let result = if input.profile.rom.is_none() {
            CheckResult::MissingContract(format!(
                "chips/{}/platform/chip.toml names no [rom] ELF",
                input.profile.id
            ))
        } else if input.policy.stacks().is_err() {
            CheckResult::MissingContract("the image's stack policy names no stacks".to_owned())
        } else {
            match crate::stack::audit_bootstrap_stack(
                input.root,
                input.profile,
                input.elf,
                input.policy,
                input.output,
            ) {
                Ok(Some(warnings)) => {
                    outcome.warnings = warnings;
                    outcome
                        .reports
                        .push(crate::stack::BOOTSTRAP_REPORT.to_owned());
                    CheckResult::Passed
                }
                Ok(None) => CheckResult::MissingContract(
                    "the image's stack policy names no bootstrap stack".to_owned(),
                ),
                Err(error) => CheckResult::Failed(error.to_string()),
            }
        };
        outcome.results.push((STACK.to_owned(), result));
        Ok(outcome)
    }
}

//! The checks an image build runs when its caller requests them, assembled
//! from the check crates: the stack check (`oer-image-check-stack`), the
//! interrupt-stack check (`oer-image-check-interrupts`) and the placement
//! check (`oer-image-check-placement`).
//!
//! [`Gates`] is the [`oer_image::Checks`] of an image build: the dev kit
//! passes the checks a developer asks for, HIL images always pass them all.
//! Every check reads what it needs from the chip profile and the stack
//! policy, whatever the chip's boot kind. A requested check whose data is
//! absent is a [`CheckResult::MissingContract`], which fails the build: a
//! missing table never proves that a property does not apply.

use oer_image::{CheckInput, CheckOutcome, CheckResult, CheckedElf};
use oer_image_check_interrupts::Required;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The placement report in an image bundle.
pub const PLACEMENT_REPORT: &str = "placement.txt";

/// An audit of the runtime ELF that the caller adds to the checks.
pub type Audit = Box<dyn Fn(&oer_elf::Elf<'_>) -> Result<()> + Send + Sync>;

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
            stack: true,
            placement: true,
            interrupts: Some(interrupts),
            audit: None,
        }
    }

    /// The checks `checks` names, the interrupt stacks proven.
    pub fn of(checks: &[Check]) -> Self {
        Self {
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

/// The result of an analysis that ran: passed, or failed with its error.
fn ran(analysis: Result<()>) -> CheckResult {
    match analysis {
        Ok(()) => CheckResult::Passed,
        Err(error) => CheckResult::Failed(error.to_string()),
    }
}

impl oer_image::Checks for Gates {
    fn requested(&self, elf: CheckedElf) -> Vec<String> {
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
    fn runtime(&self, input: &CheckInput<'_>) -> oer_image::Result<CheckOutcome> {
        let mut outcome = CheckOutcome::default();
        let profile = input.profile;
        let chip = |table: &str| format!("platform/{}/chip.toml names no {table}", profile.id);
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
            match oer_image_check_stack::Image::read(input.root, profile, input.elf) {
                Ok(image) => {
                    outcome
                        .rom_summaries
                        .extend(image.analysis.summaries.iter().cloned());
                    if let Some(required) = self.interrupts.filter(|_| interrupts_run) {
                        let result = ran((|| -> Result<()> {
                            let stacks = oer_image_check_interrupts::interrupt_stacks_of(&image)?;
                            std::fs::write(
                                input
                                    .output
                                    .join(oer_image_check_interrupts::INTERRUPT_REPORT),
                                stacks.render(),
                            )?;
                            outcome
                                .reports
                                .push(oer_image_check_interrupts::INTERRUPT_REPORT.to_owned());
                            outcome.warnings.extend(stacks.check(required)?);
                            Ok(())
                        })());
                        outcome.results.push((INTERRUPTS.to_owned(), result));
                    }
                    if stack_runs {
                        let result = ran((|| -> Result<()> {
                            let warnings = oer_image_check_stack::audit_runtime_stacks(
                                &image,
                                input.policy,
                                input.output,
                            );
                            outcome
                                .reports
                                .push(oer_image_check_stack::RUNTIME_REPORT.to_owned());
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
                    let report =
                        oer_image_check_placement::audit(memory, placement, input.elf, input.flat)?;
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
                audit(&oer_elf::Elf::parse(&std::fs::read(input.elf)?)?)
            })());
            outcome.results.push((AUDIT.to_owned(), result));
        }
        Ok(outcome)
    }

    fn bootstrap(&self, input: &CheckInput<'_>) -> oer_image::Result<CheckOutcome> {
        let mut outcome = CheckOutcome::default();
        if !self.stack {
            return Ok(outcome);
        }
        let result = if input.profile.rom.is_none() {
            CheckResult::MissingContract(format!(
                "platform/{}/chip.toml names no [rom] ELF",
                input.profile.id
            ))
        } else if input.policy.stacks().is_err() {
            CheckResult::MissingContract("the image's stack policy names no stacks".to_owned())
        } else {
            match oer_image_check_stack::audit_bootstrap_stack(
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
                        .push(oer_image_check_stack::BOOTSTRAP_REPORT.to_owned());
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

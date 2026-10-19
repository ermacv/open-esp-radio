//! The stack checks of an image build: the runtime's task and interrupt
//! stacks and the bootstrap's stack, under the chip's stack policy
//! (`firmware/platform/<chip>/stack.toml`), from one analysis of each ELF
//! with the pinned ROM. They report and never fail the build yet: a failed
//! check is a warning, as is a check that passes with one (a partial bound,
//! an admitted assumption). The reports lie beside the image.

use std::path::Path;

use oer_image_checks::interrupts::{INTERRUPT_REPORT, Required, interrupt_stacks_of};
use oer_image_checks::stack;
use oer_image_profile::Profile;
use oer_image_profile::policy::StackPolicy;

use crate::Result;

/// The chip's stack policy.
pub fn policy(root: &Path, profile: &Profile) -> Result<StackPolicy> {
    StackPolicy::load(&root.join(profile.stack_policy()))
}

/// The warnings of a check: its own, or its failure as one.
fn reported(check: &str, result: Result<Vec<String>>) -> Vec<String> {
    result.unwrap_or_else(|error| vec![format!("{check} check failed (reported only): {error}")])
}

/// Bound the runtime ELF `elf`'s task and interrupt stacks, write their
/// reports into `output` and check them, every interrupt stack proven;
/// returns the warnings, failures among them.
pub fn runtime(
    root: &Path,
    profile: &Profile,
    policy: &StackPolicy,
    elf: &Path,
    output: &Path,
) -> Vec<String> {
    let image = match stack::Image::read(root, profile, elf) {
        Ok(image) => image,
        Err(error) => return reported("runtime stack", Err(error)),
    };
    let mut warnings = reported("interrupt stack", interrupts(&image, output));
    warnings.extend(reported(
        "runtime stack",
        stack::audit_runtime_stacks(&image, policy, output),
    ));
    warnings
}

fn interrupts(image: &stack::Image, output: &Path) -> Result<Vec<String>> {
    let interrupts = interrupt_stacks_of(image)?;
    std::fs::write(output.join(INTERRUPT_REPORT), interrupts.render())?;
    interrupts.check(Required::Proven)
}

/// Bound the bootstrap ELF `elf`'s stack, write its report into `output` and
/// check it; returns the warnings, a failure among them.
pub fn bootstrap(
    root: &Path,
    profile: &Profile,
    policy: &StackPolicy,
    elf: &Path,
    output: &Path,
) -> Vec<String> {
    reported(
        "bootstrap stack",
        bootstrap_stack(root, profile, policy, elf, output),
    )
}

/// The bootstrap's check. A policy without a bootstrap stack fails: a
/// missing table never proves that a stack needs no bound.
fn bootstrap_stack(
    root: &Path,
    profile: &Profile,
    policy: &StackPolicy,
    elf: &Path,
    output: &Path,
) -> Result<Vec<String>> {
    stack::audit_bootstrap_stack(root, profile, elf, policy, output)?.ok_or_else(|| {
        format!(
            "{} names no bootstrap stack",
            profile.stack_policy().display()
        )
        .into()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_failed_check_is_reported_and_a_passed_one_keeps_its_warnings() {
        assert_eq!(
            super::reported("stack", Err("no bound".into())),
            ["stack check failed (reported only): no bound"]
        );
        assert_eq!(
            super::reported("stack", Ok(vec!["partial".to_owned()])),
            ["partial"]
        );
    }
}

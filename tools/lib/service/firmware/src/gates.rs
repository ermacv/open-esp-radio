//! The gates of an image build: the stack check of the runtime's task and
//! interrupt stacks and of the bootstrap's stack, under the chip's stack
//! policy (`firmware/platform/<chip>/stack.toml`), from one analysis of each
//! ELF with the pinned ROM. A failed gate fails the build; a gate that passes
//! with a warning (a partial bound, an admitted assumption) reports it.

use std::path::Path;

use oer_tool_data_chip_profile::Profile;
use oer_tool_data_image_policy::StackPolicy;
use oer_tool_service_image_check_interrupts::{INTERRUPT_REPORT, Required, interrupt_stacks_of};
use oer_tool_service_image_check_stack as stack;

use crate::Result;

/// The chip's stack policy.
pub fn policy(root: &Path, profile: &Profile) -> Result<StackPolicy> {
    StackPolicy::load(&root.join(profile.stack_policy()))
}

/// Bound the runtime ELF `elf`'s task and interrupt stacks, write their
/// reports into `output` and check them: every interrupt stack is proven.
pub fn runtime(
    root: &Path,
    profile: &Profile,
    policy: &StackPolicy,
    elf: &Path,
    output: &Path,
) -> Result<Vec<String>> {
    let image = stack::Image::read(root, profile, elf)?;
    let interrupts = interrupt_stacks_of(&image)?;
    std::fs::write(output.join(INTERRUPT_REPORT), interrupts.render())?;
    let mut warnings = interrupts.check(Required::Proven)?;
    warnings.extend(stack::audit_runtime_stacks(&image, policy, output)?);
    Ok(warnings)
}

/// Bound the bootstrap ELF `elf`'s stack, write its report into `output` and
/// check it. A policy without a bootstrap stack fails: a missing table never
/// proves that a stack needs no bound.
pub fn bootstrap(
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

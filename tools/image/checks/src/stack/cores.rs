//! The cores of a chip and what an image does with them. The stack policy
//! names every core's task stack and interrupt stack, each proven from its
//! root or `unused = true`; this proves the unused ones from the linked image:
//!
//! - no function that releases a core from reset or stall
//!   (`[core-start]` of the chip profile) is linked unless a core other than
//!   core 0 runs a task of its own, with the root the policy names;
//! - an unused stack has no storage: its linked range is empty, or absent;
//! - an interrupt-table entry routes to a core with an interrupt stack only
//!   (checked where the table is read, [`crate::interrupts`]).

use std::fmt::Write as _;

use oer_chip_profile::{Profile, policy::StackPolicy};

use super::{Image, Result, reachability::matches};

/// The cores report of the runtime image.
pub const REPORT: &str = "cores.txt";

/// The linked functions that match a starter pattern of the chip.
fn linked_starters(image: &Image, profile: &Profile) -> Vec<String> {
    let Some(core_start) = &profile.core_start else {
        return Vec::new();
    };
    let mut linked = Vec::new();
    for function in &image.functions {
        if let Some(name) = image.names.get(&function.address)
            && core_start
                .starters
                .iter()
                .any(|pattern| matches(pattern, name))
        {
            linked.push(name.clone());
        }
    }
    linked.sort();
    linked.dedup();
    linked
}

/// The linked size of the range `bottom..top`, `None` when the image links
/// neither symbol.
fn linked_bytes(image: &Image, bottom: &str, top: &str) -> Result<Option<u64>> {
    let file = oer_analysis_elf::Elf::parse(&image.elf)?;
    Ok(oer_analysis_elf::StorageBounds::range(&file, bottom, top)?.map(|range| range.capacity))
}

/// Prove the runtime `image`'s unused cores and return the report: every
/// core with its two stacks, used or proven unused.
pub fn audit_runtime(image: &Image, policy: &StackPolicy, profile: &Profile) -> Result<String> {
    policy.check_cores(profile.properties.cores)?;
    let unused_tasks = policy.unused_task_cores();
    let unused_interrupts = policy.unused_interrupt_cores();
    let starters = linked_starters(image, profile);
    let mut report = String::from("core\ttask stack\tinterrupt stack\n");
    let started: Vec<_> = unused_tasks.iter().filter(|core| **core != 0).collect();
    if !starters.is_empty() && !started.is_empty() {
        return Err(format!(
            "the image links code that starts a core, but its stack policy declares the task \
             stack of core {} unused: declare the core used with a proven root, or link no \
             starter:\n  {}",
            started
                .iter()
                .map(|core| core.to_string())
                .collect::<Vec<_>>()
                .join(", "),
            starters.join("\n  ")
        )
        .into());
    }
    for core in policy.core.keys() {
        for (kind, unused) in [
            ("task", unused_tasks.contains(core)),
            ("irq", unused_interrupts.contains(core)),
        ] {
            let (bottom, top) = (
                format!("__runtime_cpu{core}_{kind}_stack_bottom"),
                format!("__runtime_cpu{core}_{kind}_stack_top"),
            );
            let linked = linked_bytes(image, &bottom, &top)?;
            if unused && linked.is_some_and(|bytes| bytes != 0) {
                return Err(format!(
                    "the stack policy declares core {core}'s {kind} stack unused, but the image \
                     links {} bytes for it (`{bottom}`..`{top}`)",
                    linked.unwrap_or_default()
                )
                .into());
            }
        }
        let describe = |unused: bool, kind: &str| -> Result<String> {
            Ok(if unused {
                "unused: 0 bytes, no starter linked".to_owned()
            } else {
                match linked_bytes(
                    image,
                    &format!("__runtime_cpu{core}_{kind}_stack_bottom"),
                    &format!("__runtime_cpu{core}_{kind}_stack_top"),
                )? {
                    Some(bytes) => format!("used: {bytes} bytes"),
                    None => "used: not linked".to_owned(),
                }
            })
        };
        writeln!(
            report,
            "{core}\t{}\t{}",
            describe(unused_tasks.contains(core), "task")?,
            describe(unused_interrupts.contains(core), "irq")?
        )?;
    }
    if !starters.is_empty() {
        writeln!(report, "linked starters:\n  {}", starters.join("\n  "))?;
    }
    Ok(report)
}

/// The bootstrap runs on core 0 alone: it links no starter.
pub fn audit_bootstrap(image: &Image, profile: &Profile) -> Result<()> {
    let starters = linked_starters(image, profile);
    if starters.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the bootstrap links code that starts a core; it runs on core 0 alone:\n  {}",
        starters.join("\n  ")
    )
    .into())
}

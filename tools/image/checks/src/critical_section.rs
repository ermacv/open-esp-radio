//! A hart-local mask is a critical section only when one hart can acquire it.
//! Read per-hart roots through the existing resolved stack walk.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use oer_chip_profile::policy::StackPolicy;

use crate::{Result, interrupts::InterruptStacks, stack::Image};

pub const REPORT: &str = "critical-section.txt";

// Include the macro's qualified DWARF paths: LTO may inline the exported shim.
const ENTRIES: &[&str] = &[
    "_critical_section_1_0_acquire",
    "_critical_section_1_0_release",
    "oer_espressif_staged_runtime::critical::hart::_critical_section_1_0_acquire",
    "oer_espressif_staged_runtime::critical::hart::_critical_section_1_0_release",
];

/// Functions containing an out-of-line or inlined critical-section entry.
fn callers(image: &Image) -> BTreeSet<u32> {
    let mut owners: BTreeSet<_> = image
        .analysis
        .functions
        .iter()
        .filter(|(_, facts)| {
            facts
                .function
                .names
                .iter()
                .any(|name| ENTRIES.contains(&name.as_str()))
        })
        .map(|(&address, _)| address)
        .collect();
    for start in image.dwarf.instances(ENTRIES).values().flatten() {
        if let Some((&address, facts)) = image.analysis.functions.range(..=*start).next_back()
            && start.saturating_sub(address) < facts.function.size
        {
            owners.insert(address);
        }
    }
    owners
}

/// Every entry, every handler (including shorter handlers at the same level),
/// and every hart's exception. Task polls and launch payloads are followed by
/// the same resolutions as the stack check, from their owning entry/handler.
pub(crate) fn roots(
    image: &Image,
    policy: &StackPolicy,
    table: &[oer_image_stack::TableEntry],
    harts: &[oer_image_stack::HartStack],
) -> Result<BTreeMap<(u32, u32), String>> {
    let mut roots = BTreeMap::new();
    for root in policy.runtime_task_stacks() {
        let hart = root.hart.ok_or("runtime task root has no hart")?;
        let address = image
            .symbol(&root.stack.root)
            .ok_or_else(|| format!("hart {hart} has no task entry `{}`", root.stack.root))?;
        roots.insert(
            (hart, address),
            format!("hart{hart}-entry:{}", root.stack.root),
        );
    }
    for entry in table {
        if let Some(handler) = entry.handler {
            roots.insert(
                (entry.core, handler),
                format!(
                    "hart{}-P{}-handler:{handler:#010x}",
                    entry.core, entry.level
                ),
            );
        }
    }
    for hart in harts {
        roots.insert(
            (hart.core, hart.exception.bound.root),
            format!("hart{}-exception", hart.core),
        );
    }
    Ok(roots)
}

/// This safety prerequisite is enforced even for diagnostic builds.
pub fn audit_runtime(
    image: &Image,
    policy: &StackPolicy,
    interrupts: &InterruptStacks,
    declared: u32,
) -> Result<String> {
    if policy.task_stack(declared).is_none() {
        return Err(format!("critical-section-hart {declared} has no used task stack").into());
    }
    audit_roots(
        image,
        &roots(image, policy, &interrupts.table, &interrupts.harts)?,
        declared,
    )
}

fn audit_roots(
    image: &Image,
    roots: &BTreeMap<(u32, u32), String>,
    declared: u32,
) -> Result<String> {
    let owners = callers(image);
    let (report, _) = image.with_bounds(|keep, control| {
        let mut report = format!("critical-section-hart={declared}\nroot\tverdict\n");
        for (&(hart, address), label) in roots {
            if hart == declared {
                writeln!(report, "{label}\tdeclared hart")?;
                continue;
            }
            let bound = image.analysis.bound_with(address, &image.resolutions, keep, control)?;
            if let Some(caller) = bound.reached.intersection(&owners).next() {
                return Err(format!("critical section confined to hart {declared}: {label} on hart {hart} reaches {} ({caller:#010x})", image.names.get(caller).map_or("unnamed", String::as_str)).into());
            }
            if let Some((site, reason)) = bound.unresolved.first() {
                return Err(format!("critical section confined to hart {declared}: cannot prove {label} on hart {hart} excludes it; unresolved {site:#010x}: {reason}").into());
            }
            writeln!(report, "{label}\tproven unreachable")?;
        }
        Ok(report)
    })?;
    Ok(report)
}

#[cfg(test)]
mod tests;

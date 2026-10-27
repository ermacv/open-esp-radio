//! The per-hart task stacks of an image that declares it proven, sized once from
//! the first ELF's bound and verified on the final ELF without retries.

use crate::{Result, stack::StackAudit};
use oer_chip_profile::policy::{StackPolicy, TaskStack};
use serde::{Deserialize, Serialize};

/// The largest change of the bound the relink may bring, as for the IRQ stacks.
const BOUND_CHANGE_LIMIT_BYTES: u64 = 64;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub hart: u32,
    pub bytes: u32,
    /// The first ELF's proven bound.
    pub first_bound: u64,
    pub minimum_free_bytes: u32,
}

/// The allocation for the bound of the first ELF's task stack. The bound must
/// be complete and rest only on admitted assumptions.
pub(super) fn select(audit: &StackAudit, policy: &StackPolicy) -> Result<Vec<Allocation>> {
    policy
        .proven_task_stacks()?
        .into_iter()
        .map(|root| {
            let hart = root.hart.ok_or("runtime task root has no hart")?;
            let first_bound = proven_bound(audit, hart, "first")?;
            Ok(Allocation {
                hart,
                bytes: root.stack.proven_allocation_bytes(first_bound)?,
                first_bound,
                minimum_free_bytes: root.stack.minimum_free_bytes,
            })
        })
        .collect()
}

pub(super) fn verify(audit: &StackAudit, planned: &[Allocation]) -> Result<()> {
    if audit.stacks.len() != planned.len() {
        return Err("task hart inventory changed after relinking".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for allocation in planned {
        if !seen.insert(allocation.hart) {
            return Err("duplicate task allocation hart".into());
        }
        let stack = stack_of(audit, allocation.hart)?;
        if stack.linked.0 != u64::from(allocation.bytes) {
            return Err(format!(
                "hart {} final task allocation {} differs from planned {}",
                allocation.hart, stack.linked.0, allocation.bytes
            )
            .into());
        }
        verify_bound(allocation, proven_bound(audit, allocation.hart, "final")?)?;
    }
    Ok(())
}

fn stack_of(audit: &StackAudit, hart: u32) -> Result<&crate::stack::StackBound> {
    let mut stacks = audit.stacks.iter().filter(|stack| stack.hart == Some(hart));
    let stack = stacks
        .next()
        .ok_or_else(|| format!("hart {hart} task root missing"))?;
    if stacks.next().is_some() {
        return Err(format!("duplicate task root for hart {hart}").into());
    }
    Ok(stack)
}

fn proven_bound(audit: &StackAudit, hart: u32, which: &str) -> Result<u64> {
    let stack = stack_of(audit, hart)?;
    let bound = &stack.linked.1;
    let Some(bytes) = bound.bytes.filter(|_| stack.unresolved.is_empty()) else {
        return Err(format!("hart {hart} {which} task bound from `{}` is {} + ? bytes with {} unresolved sites; no further link attempted", stack.root, bound.partial, stack.unresolved.len()).into());
    };
    if let Some(assumption) = bound
        .assumptions
        .iter()
        .find(|assumption| !crate::stack::ADMITTED.contains(assumption))
    {
        return Err(format!(
            "hart {hart} {which} task bound assumes {assumption}, which the gate does not admit"
        )
        .into());
    }
    Ok(bytes)
}

fn verify_bound(planned: &Allocation, bound: u64) -> Result<()> {
    if planned.first_bound.abs_diff(bound) > BOUND_CHANGE_LIMIT_BYTES {
        return Err(format!(
            "the task stack bound changed after relinking: {} -> {bound} bytes (difference {}, limit {BOUND_CHANGE_LIMIT_BYTES}); allocated {} bytes; no further link attempted",
            planned.first_bound,
            planned.first_bound.abs_diff(bound),
            planned.bytes
        )
        .into());
    }
    let needed = bound + u64::from(planned.minimum_free_bytes);
    if needed > u64::from(planned.bytes) {
        return Err(format!(
            "the task stack after relinking: first bound {}, final bound {bound} + {} free bytes requires {needed} bytes; allocated {} bytes; no further link attempted",
            planned.first_bound, planned.minimum_free_bytes, planned.bytes
        )
        .into());
    }
    Ok(())
}

/// Where a task stack's size came from, for the report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SizeSource {
    /// The image declares the stack sized from its bound: the bound plus
    /// the reserve. The report states whether that bound is proven.
    Bound,
    /// The policy's `allocation_bytes`.
    PolicyAllocation,
    /// This hart's policy/layout default.
    PerHartDefault,
}

impl SizeSource {
    pub fn of(policy: &StackPolicy, hart: u32, linked: Option<&Allocation>) -> Self {
        if linked.is_some() {
            Self::Bound
        } else if policy
            .task_stack(hart)
            .is_some_and(|stack: &TaskStack| stack.allocation_bytes.is_some())
        {
            Self::PolicyAllocation
        } else {
            Self::PerHartDefault
        }
    }
}

impl std::fmt::Display for SizeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Bound => "the bound plus the policy's minimum free bytes",
            Self::PolicyAllocation => "the policy's allocation_bytes",
            Self::PerHartDefault => "the per-hart policy/layout default",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned() -> Allocation {
        Allocation {
            hart: 0,
            bytes: 17264,
            first_bound: 880,
            minimum_free_bytes: 16384,
        }
    }

    #[test]
    fn a_bound_that_still_fits_passes_and_a_larger_change_stops_with_figures() {
        verify_bound(&planned(), 880).unwrap();
        verify_bound(&planned(), 880 - 64).unwrap();
        // 896 + 16384 = 17280 > 17264, within the 64-byte change limit.
        let error = verify_bound(&planned(), 896).unwrap_err().to_string();
        for figure in ["880", "896", "16384", "17280", "17264", "no further link"] {
            assert!(error.contains(figure), "{error}");
        }
        for bound in [880 + 65, 880 - 65] {
            let error = verify_bound(&planned(), bound).unwrap_err().to_string();
            assert!(error.contains("difference 65, limit 64"), "{error}");
        }
    }

    #[test]
    fn the_size_source_names_where_the_size_came_from() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let policy = StackPolicy::load(&root.join("chips/esp32s31/platform/stack.toml")).unwrap();
        assert_eq!(SizeSource::of(&policy, 0, None), SizeSource::PerHartDefault);
        assert_eq!(
            SizeSource::of(&policy, 0, Some(&planned())),
            SizeSource::Bound
        );
        let mut explicit = policy.clone();
        explicit.task_stack_mut(0).unwrap().allocation_bytes = Some(1 << 20);
        assert_eq!(
            SizeSource::of(&explicit, 0, None),
            SizeSource::PolicyAllocation
        );
    }
}

#[cfg(test)]
mod hart_tests {
    use super::*;
    use crate::stack::tests::{image, policy_with, program};

    #[test]
    fn relink_selection_and_verification_are_keyed_by_hart() {
        for harts in [3, 7] {
            let last = harts - 1;
            let mut source =
                String::from("schema=6\ncoverage_policy='coverage.toml'\nmax_move_bytes=4096\n");
            for hart in 0..harts {
                if hart == 0 || hart == last {
                    let root = if hart == 0 { "runtime_main" } else { "leaf" };
                    source.push_str(&format!("[core.{hart}.task_stack]\nroot='{root}'\nstorage_symbol='task_{hart}'\nminimum_free_bytes=16\n[core.{hart}.interrupt_stack]\n"));
                } else {
                    source.push_str(&format!("[core.{hart}.task_stack]\nunused=true\n[core.{hart}.interrupt_stack]\nunused=true\n"));
                }
            }
            let (_directory, policy) = policy_with(&source, "");
            let first_name = format!("task_{}", 0);
            let last_name = format!("task_{last}");
            let first = image(
                &program(true),
                &[
                    (first_name.as_str(), 0, 1024),
                    (last_name.as_str(), 1024, 1024),
                ],
            );
            let audit = StackAudit::of(&first, &policy, &policy.runtime_task_stacks()).unwrap();
            let planned = select(&audit, &policy).unwrap();
            assert_eq!(
                planned
                    .iter()
                    .map(|a| (a.hart, a.bytes))
                    .collect::<Vec<_>>(),
                [(0, 64), (last, 48)]
            );
            let final_image = image(
                &program(true),
                &[(first_name.as_str(), 0, 64), (last_name.as_str(), 128, 48)],
            );
            let mut audit =
                StackAudit::of(&final_image, &policy, &policy.runtime_task_stacks()).unwrap();
            audit.stacks.reverse();
            verify(&audit, &planned).unwrap();
            let mut duplicate = planned.clone();
            duplicate[1].hart = duplicate[0].hart;
            assert!(verify(&audit, &duplicate).is_err());
            let mut partial = policy.clone();
            partial.task_stack_mut(last).unwrap().root = "indirect".into();
            let partial =
                StackAudit::of(&final_image, &partial, &partial.runtime_task_stacks()).unwrap();
            assert!(verify(&partial, &planned).is_err());
            audit.stacks.pop();
            assert!(verify(&audit, &planned).is_err());
        }
    }
}

//! The CPU0 task stack of an image that declares it proven, sized once from
//! the first ELF's bound and verified on the final ELF without retries.

use crate::{Result, stack::StackAudit};
use oer_chip_profile::policy::{StackPolicy, TaskStack};
use serde::{Deserialize, Serialize};

/// The largest change of the bound the relink may bring, as for the IRQ stacks.
const BOUND_CHANGE_LIMIT_BYTES: u64 = 64;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub bytes: u32,
    /// The first ELF's proven bound.
    pub first_bound: u64,
    pub minimum_free_bytes: u32,
}

/// The allocation for the bound of the first ELF's task stack. The bound must
/// be complete and rest only on admitted assumptions.
pub(super) fn select(audit: &StackAudit, policy: &StackPolicy) -> Result<Allocation> {
    let stack = policy.proven_task_stack()?;
    let first_bound = proven_bound(audit, "first")?;
    Ok(Allocation {
        bytes: stack.proven_allocation_bytes(first_bound)?,
        first_bound,
        minimum_free_bytes: stack.minimum_free_bytes,
    })
}

/// The final ELF keeps the planned size and a bound that still fits it.
pub(super) fn verify(audit: &StackAudit, planned: &Allocation) -> Result<()> {
    let linked = audit
        .stacks
        .first()
        .ok_or("the final ELF has no task stack to verify")?
        .linked
        .0;
    if linked != u64::from(planned.bytes) {
        return Err(format!(
            "the final task stack is {linked} bytes, expected {} from the first bound {} + {} free bytes",
            planned.bytes, planned.first_bound, planned.minimum_free_bytes
        )
        .into());
    }
    let bound = proven_bound(audit, "final")?;
    verify_bound(planned, bound)
}

fn proven_bound(audit: &StackAudit, which: &str) -> Result<u64> {
    let [stack] = audit
        .stacks
        .iter()
        .filter(|stack| stack.name == "CPU0 task stack")
        .collect::<Vec<_>>()[..]
    else {
        return Err("the image declares one task stack proven".into());
    };
    let bound = &stack.linked.1;
    let Some(bytes) = bound.bytes else {
        return Err(format!(
            "the image declares its task stack proven, but the {which} link's bound from `{}` is {} + ? bytes with {} unresolved sites; no further link attempted",
            stack.root,
            bound.partial,
            stack.unresolved.len()
        )
        .into());
    };
    if let Some(assumption) = bound
        .assumptions
        .iter()
        .find(|assumption| !crate::stack::ADMITTED.contains(assumption))
    {
        return Err(format!(
            "the {which} link's task stack bound assumes {assumption}, which the gate does not admit"
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
    /// The chip layout's default.
    ChipDefault,
}

impl SizeSource {
    pub fn of(policy: &StackPolicy, linked: Option<&Allocation>) -> Self {
        if linked.is_some() {
            Self::Bound
        } else if policy
            .cpu0_task_stack()
            .is_some_and(|stack: &TaskStack| stack.allocation_bytes.is_some())
        {
            Self::PolicyAllocation
        } else {
            Self::ChipDefault
        }
    }
}

impl std::fmt::Display for SizeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Bound => "the bound plus the policy's minimum free bytes",
            Self::PolicyAllocation => "the policy's allocation_bytes",
            Self::ChipDefault => "the chip layout's default",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned() -> Allocation {
        Allocation {
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
        assert_eq!(SizeSource::of(&policy, None), SizeSource::ChipDefault);
        assert_eq!(SizeSource::of(&policy, Some(&planned())), SizeSource::Bound);
        let mut explicit = policy.clone();
        explicit.cpu0_task_stack_mut().unwrap().allocation_bytes = Some(1 << 20);
        assert_eq!(
            SizeSource::of(&explicit, None),
            SizeSource::PolicyAllocation
        );
    }
}

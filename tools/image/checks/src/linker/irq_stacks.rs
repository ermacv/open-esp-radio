//! Select once from the first ELF, then verify the final ELF without retries.

use crate::{Result, interrupts::InterruptStacks};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub hart: u32,
    pub bytes: u32,
    /// The first-pass bound, including diagnostic bounds retained at maximum.
    pub first_bound: Option<u64>,
    pub from_bound: bool,
}

pub(crate) fn select(stacks: &InterruptStacks, diagnostic: bool) -> Result<Vec<Allocation>> {
    stacks
        .harts
        .iter()
        .map(|hart| {
            let from_bound = !diagnostic
                && hart.bytes.is_some()
                && hart.level_drops().is_empty()
                && hart
                    .assumptions()
                    .iter()
                    .all(|assumption| crate::stack::ADMITTED.contains(assumption));
            let bound = hart.bytes.filter(|_| from_bound);
            Ok(Allocation {
                hart: hart.core,
                bytes: stacks
                    .contract
                    .interrupts
                    .irq_stack_size(hart.core, bound)?,
                first_bound: hart.bytes,
                from_bound,
            })
        })
        .collect()
}

pub(super) fn verify(stacks: &InterruptStacks, planned: &[Allocation]) -> Result<()> {
    if stacks.harts.len() != planned.len() {
        return Err("the IRQ hart inventory changed after relinking".into());
    }
    for allocation in planned {
        let hart = stacks
            .harts
            .iter()
            .find(|hart| hart.core == allocation.hart)
            .ok_or("an IRQ hart disappeared after relinking")?;
        let linked = stacks
            .allocations
            .get(&hart.core)
            .ok_or("the final ELF lacks an IRQ stack allocation")?;
        if linked.bytes != allocation.bytes || linked.from_bound != allocation.from_bound {
            return Err(format!("hart {}'s final IRQ stack is {} bytes (from_bound={}), expected {} (from_bound={})", hart.core, linked.bytes, linked.from_bound, allocation.bytes, allocation.from_bound).into());
        }
        verify_bound(
            allocation,
            hart.bytes,
            hart.partial,
            &stacks.contract.interrupts,
        )?;
        if allocation.from_bound
            && let Some(problem) = stacks.guard_problem(hart)
        {
            return Err(format!("after relinking: {problem}").into());
        }
    }
    Ok(())
}

fn verify_bound(
    allocation: &Allocation,
    bound: Option<u64>,
    partial: u64,
    contract: &oer_chip_profile::InterruptContract,
) -> Result<()> {
    let hart = allocation.hart;
    if let (Some(first), Some(final_bound)) = (allocation.first_bound, bound)
        && first.abs_diff(final_bound) > 64
    {
        return Err(format!("hart {hart}'s IRQ stack bound changed after relinking: {first} -> {final_bound} bytes (difference {}, limit 64); allocated {} bytes; no further link attempted", first.abs_diff(final_bound), allocation.bytes).into());
    }
    if allocation.from_bound && bound.is_none() {
        return Err(format!("hart {hart}'s IRQ stack lost its proof after relinking (first bound {:?}, final {partial} + ?, allocated {} bytes); no further link attempted", allocation.first_bound, allocation.bytes).into());
    }
    let bytes = bound.unwrap_or(partial);
    let needed = contract.irq_stack_with_margin(bytes)?;
    let guard = contract.irq_stack_guard_bytes[&hart];
    let usable = allocation.bytes - guard;
    if needed > u64::from(usable) {
        return Err(format!("hart {hart}'s IRQ stack after relinking: first bound {:?}, final bound {bytes}{} + {}% margin requires {needed} usable bytes; allocated {} - guard {guard} = {usable} usable bytes; no further link attempted", allocation.first_bound, if bound.is_none() { " + ?" } else { "" }, contract.irq_stack_margin_percent, allocation.bytes).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> oer_chip_profile::InterruptContract {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        oer_chip_profile::Profile::load(&root, "esp32c5")
            .unwrap()
            .interrupts
            .unwrap()
    }

    #[test]
    fn a_small_bound_increase_that_exceeds_the_final_capacity_is_an_error_with_figures() {
        let contract = contract();
        let allocation = Allocation {
            hart: 0,
            bytes: contract.irq_stack_size(0, Some(1000)).unwrap(),
            first_bound: Some(1000),
            from_bound: true,
        };
        verify_bound(&allocation, Some(1000), 1000, &contract).unwrap();
        let error = verify_bound(&allocation, Some(1010), 1010, &contract)
            .unwrap_err()
            .to_string();
        for figure in [
            "1000",
            "1010",
            "1111",
            "5200",
            "4096",
            "1104",
            "no further link",
        ] {
            assert!(error.contains(figure), "{error}");
        }
        assert!(verify_bound(&allocation, None, 1000, &contract).is_err());
    }

    #[test]
    fn a_change_over_sixty_four_bytes_stops_even_if_the_allocation_has_room() {
        let contract = contract();
        let allocation = Allocation {
            hart: 0,
            bytes: contract.irq_stack_bytes,
            first_bound: Some(1000),
            from_bound: false,
        };
        verify_bound(&allocation, Some(1064), 1064, &contract).unwrap();
        for bound in [1065, 935] {
            let error = verify_bound(&allocation, Some(bound), bound, &contract)
                .unwrap_err()
                .to_string();
            assert!(error.contains("difference 65, limit 64"), "{error}");
        }
        verify_bound(&allocation, None, 1000, &contract).unwrap();
    }
}

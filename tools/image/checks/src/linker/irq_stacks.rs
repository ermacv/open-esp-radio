//! Select once from the first ELF, then verify the final ELF without retries.

use crate::{Result, interrupts::InterruptStacks};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub hart: u32,
    pub bytes: u32,
    pub guard_bytes: u32,
    /// The first-pass bound, including diagnostic bounds retained at maximum.
    pub first_bound: Option<u64>,
    pub from_bound: bool,
}

pub(crate) fn select(stacks: &InterruptStacks, diagnostic: bool) -> Result<Vec<Allocation>> {
    let mut bottom = u64::from(stacks.contract.memory.sram.origin());
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
            let physical = stacks
                .contract
                .interrupts
                .irq_stack_allocation(hart.core, Some(hart.bytes.unwrap_or(hart.partial)))?;
            let bytes = if from_bound {
                physical.bytes
            } else {
                stacks.contract.interrupts.irq_stack_bytes
            };
            let guard_bytes = if physical.guard_bytes != 0
                && bottom.is_multiple_of(u64::from(physical.guard_bytes))
            {
                physical.guard_bytes
            } else {
                0
            };
            bottom = bottom
                .checked_add(u64::from(bytes))
                .ok_or("IRQ stack placement overflows u64")?;
            Ok(Allocation {
                hart: hart.core,
                bytes,
                guard_bytes,
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
        if linked.guard_bytes != allocation.guard_bytes {
            return Err(format!(
                "hart {}'s final IRQ guard is {} bytes, expected {}",
                hart.core, linked.guard_bytes, allocation.guard_bytes
            )
            .into());
        }
        if linked.bytes != allocation.bytes || linked.from_bound != allocation.from_bound {
            return Err(format!("hart {}'s final IRQ stack is {} bytes (from_bound={}), expected {} (from_bound={})", hart.core, linked.bytes, linked.from_bound, allocation.bytes, allocation.from_bound).into());
        }
        verify_bound(
            allocation,
            hart.bytes,
            hart.partial,
            &stacks.contract.interrupts,
        )?;
    }
    Ok(())
}

fn verify_bound(
    allocation: &Allocation,
    bound: Option<u64>,
    partial: u64,
    contract: &oer_espressif_facts::InterruptContract,
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
    let guard = allocation.guard_bytes;
    let usable = allocation.bytes - guard;
    if needed > u64::from(usable) {
        return Err(format!("hart {hart}'s IRQ stack after relinking: first bound {:?}, final bound {bytes}{} + {}% margin requires {needed} usable bytes; allocated {} - guard {guard} = {usable} usable bytes; no further link attempted", allocation.first_bound, if bound.is_none() { " + ?" } else { "" }, contract.irq_stack_margin_percent, allocation.bytes).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> oer_espressif_facts::InterruptContract {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        oer_chip_profile::Profile::load(&root, "esp32s31")
            .unwrap()
            .facts
            .interrupts
            .unwrap()
    }

    #[test]
    fn guard_overflow_and_omission_pass_selection_and_final_verification() -> Result<()> {
        use crate::interrupts::{LinkedStack, Required, tests::partial};
        for diagnostic in [false, true] {
            for bound in [1000, 29000, 29789] {
                let mut stacks = partial(bound);
                let hart = stacks.harts.first_mut().ok_or("missing test hart")?;
                hart.bytes = Some(bound);
                hart.exception.bound.reached.insert(0x1000);
                stacks.frames.insert(0x1000, Some(5000));
                let planned = select(&stacks, diagnostic)?;
                let allocation = planned.first().ok_or("missing test allocation")?;
                stacks.allocations.insert(
                    0,
                    LinkedStack {
                        bottom: u64::from(stacks.contract.memory.sram.origin()),
                        bytes: allocation.bytes,
                        guard_bytes: allocation.guard_bytes,
                        from_bound: allocation.from_bound,
                    },
                );
                verify(&stacks, &planned)?;
                for policy in [Required::Product, Required::Diagnostic] {
                    assert!(
                        stacks
                            .check(policy)?
                            .iter()
                            .any(|warning| warning.contains("reported only"))
                    );
                }
                if bound >= 29000 {
                    assert_eq!(allocation.guard_bytes, 0);
                    assert_eq!(allocation.bytes, stacks.contract.interrupts.irq_stack_bytes);
                    assert!(stacks.render().contains("guard omitted (reported only)"));
                }
                assert!(stacks.render().contains("Interrupt guards"));
            }
            let stacks = partial(29790);
            assert!(select(&stacks, diagnostic).is_err());
        }
        Ok(())
    }

    #[test]
    fn partial_bounds_and_contiguous_alignment_can_omit_guards() -> Result<()> {
        use crate::interrupts::{Required, tests::partial};
        let stacks = partial(29789);
        for diagnostic in [false, true] {
            let planned = select(&stacks, diagnostic)?;
            assert!(planned.iter().all(|allocation| !allocation.from_bound
                && allocation.guard_bytes == 0
                && allocation.bytes == 32768));
        }
        // The bound remains unresolved; only Diagnostic may accept it.
        let mut stacks = stacks;
        let planned = select(&stacks, true)?;
        for allocation in &planned {
            stacks.allocations.insert(
                allocation.hart,
                crate::interrupts::LinkedStack {
                    bottom: u64::from(stacks.contract.memory.sram.origin()),
                    bytes: allocation.bytes,
                    guard_bytes: allocation.guard_bytes,
                    from_bound: allocation.from_bound,
                },
            );
        }
        verify(&stacks, &planned)?;
        assert!(!stacks.check(Required::Diagnostic)?.is_empty());
        assert!(stacks.check(Required::Product).is_err());

        let mut stacks = partial(1000);
        stacks.contract.interrupts.irq_stack_bytes = 4112;
        let first = stacks.harts.first_mut().ok_or("missing test hart")?;
        first.bytes = Some(1000);
        let mut second = first.clone();
        second.core = 1;
        stacks.harts.push(second);
        let planned = select(&stacks, false)?;
        assert_eq!(planned.len(), 2);
        assert!(planned.iter().all(|allocation| allocation.guard_bytes == 0));
        Ok(())
    }

    #[test]
    fn a_small_bound_increase_that_exceeds_the_final_capacity_is_an_error_with_figures() {
        let contract = contract();
        let allocation = Allocation {
            hart: 1,
            bytes: contract.irq_stack_allocation(1, Some(1000)).unwrap().bytes,
            guard_bytes: 1024,
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
            "2128",
            "1024",
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
            guard_bytes: 4096,
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

//! Size every used hart's interrupt stack once from the first ELF, then
//! verify the final ELF without retries.
//!
//! A hart whose bound is proven takes `align16(bound with the policy's
//! margin) + guard`; a hart whose bound is not proven takes the size its
//! image declares (`[package.metadata.open-radio] irq-stack`) plus its guard.
//! A size has one source: a declaration where the bound is proven, or neither
//! a proof nor a declaration, fails the build. The guard is always placed,
//! and the stack is rounded so that the next hart's guard is aligned; what
//! limits the allocation is the SRAM it is placed in.

use crate::{Result, interrupts::InterruptStacks};
use serde::{Deserialize, Serialize};

/// The key an image declares its unproven interrupt stacks' sizes under.
pub const DECLARATION: &str = "irq-stack";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub hart: u32,
    pub bytes: u32,
    pub guard_bytes: u32,
    /// The first link's bound; the final link's must equal it.
    pub first_bound: Option<u64>,
    pub from_bound: bool,
}

/// The size an image declares for the interrupt stack of a hart whose bound
/// is not proven: the bytes the stack holds above its guard.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredIrqStack {
    pub hart: u32,
    pub bytes: u32,
}

/// How the image names `hart` in its declarations: `boot` or its number.
fn hart_name(stacks: &InterruptStacks, hart: u32) -> String {
    if hart == stacks.contract.interrupts.boot_hart {
        "boot".to_owned()
    } else {
        hart.to_string()
    }
}

pub(crate) fn select(
    stacks: &InterruptStacks,
    declared: &[DeclaredIrqStack],
) -> Result<Vec<Allocation>> {
    for declaration in declared {
        if !stacks
            .harts
            .iter()
            .any(|hart| hart.core == declaration.hart)
        {
            return Err(format!(
                "`{DECLARATION}` declares hart {}'s interrupt stack, which the image does not use",
                hart_name(stacks, declaration.hart)
            )
            .into());
        }
    }
    stacks
        .harts
        .iter()
        .map(|hart| {
            let proven = hart.bytes.filter(|_| {
                hart.level_drops().is_empty()
                    && hart
                        .assumptions()
                        .iter()
                        .all(|assumption| crate::stack::ADMITTED.contains(assumption))
            });
            let name = hart_name(stacks, hart.core);
            let declaration = declared
                .iter()
                .find(|declaration| declaration.hart == hart.core);
            let usable = match (proven, declaration) {
                (Some(bound), Some(declaration)) => {
                    return Err(format!(
                        "hart {name}'s interrupt stack has a proven bound of {bound} bytes, which \
                         sizes it; its declared size {} in `[package.metadata.open-radio] \
                         {DECLARATION}` is a second source: remove `{name}` from it",
                        declaration.bytes
                    )
                    .into());
                }
                (Some(bound), None) => stacks.contract.irq_stack_with_margin(bound)?,
                (None, Some(declaration)) => u64::from(declaration.bytes),
                (None, None) => {
                    return Err(format!(
                        "hart {name}'s interrupt stack has no proven bound ({} + ? bytes): \
                         declare its size as `{DECLARATION} = {{ {name} = <bytes> }}` under \
                         `[package.metadata.open-radio]`",
                        hart.bytes.unwrap_or(hart.partial)
                    )
                    .into());
                }
            };
            let physical = stacks
                .contract
                .interrupts
                .irq_stack_allocation(hart.core, usable)?;
            Ok(Allocation {
                hart: hart.core,
                bytes: physical.bytes,
                guard_bytes: physical.guard_bytes,
                first_bound: hart.bytes,
                from_bound: proven.is_some(),
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
        verify_bound(allocation, hart.bytes, &stacks.contract)?;
    }
    Ok(())
}

/// The final link's bound equals the first's (nothing the second link does
/// changes a frame or an edge, so a difference is a fault of the analysis),
/// and a stack sized from its bound holds it with the margin.
fn verify_bound(
    allocation: &Allocation,
    bound: Option<u64>,
    contract: &crate::stack::contract::Contract,
) -> Result<()> {
    let hart = allocation.hart;
    let figure = |bound: Option<u64>| bound.map_or("no proven".to_owned(), |b| b.to_string());
    if allocation.first_bound != bound {
        return Err(format!(
            "hart {hart}'s IRQ stack bound changed after relinking: first link {} bytes, final \
             link {} bytes; the two links' bounds must be equal; allocated {} bytes; no further \
             link attempted",
            figure(allocation.first_bound),
            figure(bound),
            allocation.bytes
        )
        .into());
    }
    let Some(bound) = bound.filter(|_| allocation.from_bound) else {
        return Ok(());
    };
    let needed = contract.irq_stack_with_margin(bound)?;
    let guard = allocation.guard_bytes;
    let usable = allocation.bytes - guard;
    if needed > u64::from(usable) {
        return Err(format!("hart {hart}'s IRQ stack after relinking: bound {bound} + {}% margin requires {needed} usable bytes; allocated {} - guard {guard} = {usable} usable bytes; no further link attempted", contract.irq_margin_percent, allocation.bytes).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interrupts::{LinkedStack, tests::partial};

    fn link(stacks: &mut InterruptStacks, planned: &[Allocation]) {
        let mut bottom = u64::from(stacks.contract.memory.sram.origin());
        for allocation in planned {
            stacks.allocations.insert(
                allocation.hart,
                LinkedStack {
                    bottom,
                    bytes: allocation.bytes,
                    guard_bytes: allocation.guard_bytes,
                    from_bound: allocation.from_bound,
                },
            );
            bottom += u64::from(allocation.bytes);
        }
    }

    #[test]
    fn a_proven_bound_sizes_the_stack_with_its_guard_and_no_maximum() -> Result<()> {
        for bound in [1000, 29000, 29789, 40000] {
            let mut stacks = partial(bound);
            stacks.harts[0].bytes = Some(bound);
            let planned = select(&stacks, &[])?;
            let allocation = &planned[0];
            assert!(allocation.from_bound);
            assert_eq!(allocation.guard_bytes, 4096);
            let usable = stacks.contract.irq_stack_with_margin(bound)?;
            assert_eq!(
                allocation.bytes,
                stacks
                    .contract
                    .interrupts
                    .irq_stack_allocation(0, usable)?
                    .bytes
            );
            link(&mut stacks, &planned);
            verify(&stacks, &planned)?;
        }
        Ok(())
    }

    #[test]
    fn a_hart_without_a_proof_takes_its_declared_size_and_without_one_fails_naming_the_key() {
        let stacks = partial(1000);
        let error = select(&stacks, &[]).unwrap_err().to_string();
        for named in ["hart boot", "irq-stack = { boot = <bytes> }", "1000 + ?"] {
            assert!(error.contains(named), "{error}");
        }
        let planned = select(
            &stacks,
            &[DeclaredIrqStack {
                hart: 0,
                bytes: 0x8000,
            }],
        )
        .unwrap();
        assert!(!planned[0].from_bound);
        assert_eq!(planned[0].guard_bytes, 4096);
        assert_eq!(planned[0].bytes, 0x8000 + 4096);
        let mut stacks = stacks;
        link(&mut stacks, &planned);
        verify(&stacks, &planned).unwrap();
    }

    #[test]
    fn a_declared_size_where_the_bound_is_proven_fails_naming_the_proof() {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        let error = select(
            &stacks,
            &[DeclaredIrqStack {
                hart: 0,
                bytes: 0x8000,
            }],
        )
        .unwrap_err()
        .to_string();
        for named in [
            "hart boot",
            "proven bound of 1000 bytes",
            "second source",
            "remove `boot`",
        ] {
            assert!(error.contains(named), "{error}");
        }
        let error = select(
            &stacks,
            &[DeclaredIrqStack {
                hart: 1,
                bytes: 0x8000,
            }],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("does not use"), "{error}");
    }

    #[test]
    fn a_final_bound_one_byte_apart_fails_with_both_bounds() {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        let planned = select(&stacks, &[]).unwrap();
        link(&mut stacks, &planned);
        verify(&stacks, &planned).unwrap();
        for changed in [999, 1001] {
            stacks.harts[0].bytes = Some(changed);
            let error = verify(&stacks, &planned).unwrap_err().to_string();
            for figure in [
                "first link 1000 bytes",
                &format!("final link {changed} bytes"),
                "must be equal",
            ] {
                assert!(error.contains(figure), "{error}");
            }
        }
        stacks.harts[0].bytes = None;
        let error = verify(&stacks, &planned).unwrap_err().to_string();
        assert!(error.contains("final link no proven bytes"), "{error}");
    }

    #[test]
    fn hart_zero_is_rounded_to_hart_one_s_guard_and_both_guards_are_placed() -> Result<()> {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        let mut second = stacks.harts[0].clone();
        second.core = 1;
        stacks.harts.push(second);
        let planned = select(&stacks, &[])?;
        assert_eq!(planned.len(), 2);
        assert_eq!(planned[0].guard_bytes, 4096);
        assert_eq!(planned[1].guard_bytes, 1024);
        assert_eq!(planned[0].bytes % 1024, 0);
        Ok(())
    }

    /// The first link's placeholder does not change the second link's
    /// sizes or the SRAM left for strict code: two placeholders give the
    /// same selection and the same free SRAM.
    #[test]
    fn two_placeholders_give_the_same_selection_and_the_same_free_sram() -> Result<()> {
        const SRAM_FOR_STACKS_AND_CODE: u64 = 0x40000;
        let mut outcomes = Vec::new();
        for placeholder in [0x8000_u32, 0x2000] {
            let mut stacks = partial(1000);
            stacks.harts[0].bytes = Some(1000);
            let mut second = stacks.harts[0].clone();
            second.core = 1;
            second.bytes = Some(700);
            stacks.harts.push(second);
            // The first link: every used stack at the placeholder.
            let first: Vec<Allocation> = [0, 1]
                .map(|hart| Allocation {
                    hart,
                    bytes: placeholder,
                    guard_bytes: 0,
                    first_bound: None,
                    from_bound: false,
                })
                .into();
            link(&mut stacks, &first);
            let free_sram = SRAM_FOR_STACKS_AND_CODE - 2 * u64::from(placeholder);
            let selected = select(&stacks, &[])?;
            let free = super::super::strict::free_after_stacks(&stacks, free_sram, &selected)?;
            outcomes.push((
                selected
                    .iter()
                    .map(|allocation| (allocation.bytes, allocation.guard_bytes))
                    .collect::<Vec<_>>(),
                free,
            ));
        }
        assert_eq!(outcomes[0], outcomes[1]);
        Ok(())
    }

    #[test]
    fn stacks_beyond_the_sram_fail_with_their_figures() {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        let first = [Allocation {
            hart: 0,
            bytes: 0x100,
            guard_bytes: 0,
            first_bound: None,
            from_bound: false,
        }];
        link(&mut stacks, &first);
        let selected = select(&stacks, &[]).unwrap();
        let error = super::super::strict::free_after_stacks(&stacks, 0, &selected)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(&format!("need {} bytes", selected[0].bytes)),
            "{error}"
        );
    }

    /// The image's margin, zero included, sizes the stack and holds the
    /// second link's bound; a declared size takes no margin.
    #[test]
    fn the_image_s_margin_sizes_the_stack_and_a_declared_size_takes_none() -> Result<()> {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        stacks.contract.irq_margin_percent = 0;
        let planned = select(&stacks, &[])?;
        assert_eq!(
            planned[0].bytes,
            stacks
                .contract
                .interrupts
                .irq_stack_allocation(0, 1000)?
                .bytes
        );
        link(&mut stacks, &planned);
        verify(&stacks, &planned)?;
        stacks.contract.irq_margin_percent = 50;
        let wider = select(&stacks, &[])?;
        assert_eq!(
            wider[0].bytes,
            stacks
                .contract
                .interrupts
                .irq_stack_allocation(0, 1500)?
                .bytes
        );
        let mut declared = partial(1000);
        declared.contract.irq_margin_percent = 50;
        let planned = select(
            &declared,
            &[DeclaredIrqStack {
                hart: 0,
                bytes: 2048,
            }],
        )?;
        assert_eq!(
            planned[0].bytes,
            declared
                .contract
                .interrupts
                .irq_stack_allocation(0, 2048)?
                .bytes
        );
        Ok(())
    }

    /// The final check holds a proven bound with the margin against the
    /// linked allocation on its own: a plan too small for its bound fails
    /// with the figures, whatever `select` would have planned.
    #[test]
    fn a_plan_too_small_for_its_bound_fails_the_final_check_with_figures() {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        let small = [Allocation {
            hart: 0,
            bytes: 4096 + 512,
            guard_bytes: 4096,
            first_bound: Some(1000),
            from_bound: true,
        }];
        link(&mut stacks, &small);
        let error = verify(&stacks, &small).unwrap_err().to_string();
        for figure in [
            "bound 1000",
            "10% margin",
            "requires 1100 usable bytes",
            "4608 - guard 4096 = 512 usable bytes",
        ] {
            assert!(error.contains(figure), "{error}");
        }
    }
}

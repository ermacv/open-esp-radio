//! Link-time allocation of a hart's interrupt stack: what it must hold, its
//! guard below it, and the alignment of the next hart's guard above it.

use alloc::format;

use crate::{InterruptContract, Result};

/// Physical IRQ storage and its diagnostic guard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IrqStackAllocation {
    pub bytes: u32,
    pub guard_bytes: u32,
}

/// A proven bound plus `margin_percent`, rounding fractional bytes upwards:
/// what the stack must hold above its guard.
pub fn irq_stack_with_margin(bound: u64, margin_percent: u32) -> Result<u64> {
    let bytes = u128::from(margin_percent)
        .checked_add(100)
        .and_then(|factor| u128::from(bound).checked_mul(factor))
        .and_then(|bytes| bytes.checked_add(99))
        .ok_or("IRQ stack margin calculation overflows u128")?
        / 100;
    Ok(u64::try_from(bytes).map_err(|_| "IRQ stack bound with margin overflows u64")?)
}

impl InterruptContract {
    /// A hart's allocation holding `usable` bytes above its guard:
    /// `align16(usable) + guard`, so that the following hart's bottom, which
    /// is this stack's top, is aligned to that hart's NAPOT guard. The guard
    /// is always placed; what limits the allocation is the SRAM it is placed
    /// in, which the link checks.
    pub fn irq_stack_allocation(&self, hart: u32, usable: u64) -> Result<IrqStackAllocation> {
        let mut harts = self.harts.iter().skip_while(|&&core| core != hart);
        harts
            .next()
            .ok_or_else(|| format!("no interrupt stack for hart {hart}"))?;
        let guard_bytes = self.irq_stack_guard(hart)?;
        let bytes = usable
            .checked_next_multiple_of(16)
            .and_then(|bytes| bytes.checked_add(u64::from(guard_bytes)))
            .ok_or("IRQ stack allocation overflows u64")?;
        let alignment = match harts.next() {
            Some(&next) => self.irq_stack_guard(next)?,
            None => 16,
        };
        let bytes = bytes
            .checked_next_multiple_of(u64::from(alignment))
            .ok_or_else(|| {
                format!(
                    "IRQ stack alignment {alignment} is zero or overflows u64 for {bytes} bytes"
                )
            })?;
        let bytes = u32::try_from(bytes).map_err(|_| {
            format!("hart {hart}'s IRQ stack of {bytes} bytes does not fit a u32 allocation")
        })?;
        Ok(IrqStackAllocation { bytes, guard_bytes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn contract(chip: &str) -> InterruptContract {
        crate::tests::tracked(chip)
            .interrupts
            .expect("a staged chip has an interrupt contract")
    }

    #[test]
    fn per_hart_bounds_round_percentage_then_sixteen_then_the_next_guard() {
        let s31 = contract("esp32s31");
        // ceil(1001 * 1.1) = 1102; align16 = 1104.
        assert_eq!(irq_stack_with_margin(1001, 10).unwrap(), 1102);
        assert_eq!(
            s31.irq_stack_allocation(1, 1102).unwrap().bytes,
            1104 + 1024
        );
        // Hart 0's top is hart 1's bottom, aligned to 1024.
        assert_eq!(s31.irq_stack_allocation(0, 1102).unwrap().bytes, 6144);
        assert_eq!(s31.irq_stack_allocation(0, 17812).unwrap().bytes, 22528);
    }

    #[test]
    fn no_maximum_limits_a_stack_and_the_guard_is_always_placed() {
        let contract = contract("esp32s31");
        for &hart in &contract.harts {
            let guard = contract.irq_stack_guard(hart).unwrap();
            for usable in [16, 32768, 40000, 0x40000] {
                let allocation = contract.irq_stack_allocation(hart, usable).unwrap();
                assert_eq!(allocation.guard_bytes, guard);
                assert!(u64::from(allocation.bytes) >= usable + u64::from(guard));
            }
        }
        assert!(irq_stack_with_margin(u64::MAX, 10).is_err());
        assert!(contract.irq_stack_allocation(0, u64::MAX).is_err());
        assert!(contract.irq_stack_allocation(7, 16).is_err());
    }

    #[test]
    fn missing_current_or_following_guard_returns_an_error() {
        let mut missing = contract("esp32s31");
        missing.irq_stack_guard_bytes.remove(&0);
        assert!(
            missing
                .irq_stack_allocation(0, 1)
                .unwrap_err()
                .to_string()
                .contains("hart 0")
        );
        let mut missing = contract("esp32s31");
        missing.irq_stack_guard_bytes.remove(&1);
        assert!(
            missing
                .irq_stack_allocation(0, 1)
                .unwrap_err()
                .to_string()
                .contains("hart 1")
        );
    }

    #[test]
    fn zero_alignment_returns_an_error() {
        let mut invalid = contract("esp32s31");
        invalid.irq_stack_guard_bytes.insert(1, 0);
        assert!(
            invalid
                .irq_stack_allocation(0, 1)
                .unwrap_err()
                .to_string()
                .contains("alignment 0")
        );
    }
}

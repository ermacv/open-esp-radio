//! Link-time allocation within the interrupt contract's maximum.

use alloc::format;

use crate::{InterruptContract, Result};

/// Physical IRQ storage and its optional diagnostic guard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IrqStackAllocation {
    pub bytes: u32,
    pub guard_bytes: u32,
}

impl InterruptContract {
    /// Bound plus the contract margin, rounding fractional bytes upwards.
    pub fn irq_stack_with_margin(&self, bound: u64) -> Result<u64> {
        let bytes = u128::from(self.irq_stack_margin_percent)
            .checked_add(100)
            .and_then(|factor| u128::from(bound).checked_mul(factor))
            .and_then(|bytes| bytes.checked_add(99))
            .ok_or("IRQ stack margin calculation overflows u128")?
            / 100;
        Ok(u64::try_from(bytes).map_err(|_| "IRQ stack bound with margin overflows u64")?)
    }

    /// A hart's allocation, including its guard when it fits. The first stack must also
    /// leave the following hart's bottom aligned to that hart's NAPOT guard.
    /// `None` deliberately keeps the maximum; partial bounds are not proofs.
    pub fn irq_stack_allocation(
        &self,
        hart: u32,
        bound: Option<u64>,
    ) -> Result<IrqStackAllocation> {
        let mut harts = self.harts.iter().skip_while(|&&core| core != hart);
        harts
            .next()
            .ok_or_else(|| format!("no interrupt stack for hart {hart}"))?;
        let guard_bytes = self.irq_stack_guard(hart)?;
        let Some(bound) = bound else {
            return Ok(IrqStackAllocation {
                bytes: self.irq_stack_bytes,
                guard_bytes,
            });
        };
        let usable = self.irq_stack_with_margin(bound)?;
        if usable > u64::from(self.irq_stack_bytes) {
            return Err(format!("hart {hart}'s IRQ stack: bound {bound} + {}% margin requires {usable} usable bytes, contract maximum {} bytes", self.irq_stack_margin_percent, self.irq_stack_bytes).into());
        }
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
        if bytes > u64::from(self.irq_stack_bytes) {
            // The owner gates the bound plus margin. Guard storage is optional:
            // keep the aligned contract maximum and omit the whole NAPOT region.
            return Ok(IrqStackAllocation {
                bytes: self.irq_stack_bytes,
                guard_bytes: 0,
            });
        }
        Ok(IrqStackAllocation {
            bytes: bytes as u32,
            guard_bytes,
        })
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
        assert_eq!(s31.irq_stack_with_margin(1001).unwrap(), 1102);
        assert_eq!(
            s31.irq_stack_allocation(1, Some(1001)).unwrap().bytes,
            1104 + 1024
        );
        // Hart 0's top is hart 1's bottom, aligned to 1024.
        assert_eq!(s31.irq_stack_allocation(0, Some(1001)).unwrap().bytes, 6144);
        assert_eq!(
            s31.irq_stack_allocation(0, Some(16192)).unwrap().bytes,
            22528
        );
    }

    #[test]
    fn absent_proof_keeps_the_maximum_and_oversized_proof_fails_with_figures() {
        let contract = contract("esp32s31");
        for &hart in &contract.harts {
            assert_eq!(
                contract.irq_stack_allocation(hart, None).unwrap().bytes,
                32768
            );
        }
        let error = contract
            .irq_stack_allocation(0, Some(30000))
            .unwrap_err()
            .to_string();
        for figure in ["30000", "10%", "33000", "32768"] {
            assert!(error.contains(figure), "{error}");
        }
        assert!(contract.irq_stack_with_margin(u64::MAX).is_err());
        assert!(contract.irq_stack_allocation(0, Some(u64::MAX)).is_err());
        assert!(contract.irq_stack_allocation(7, None).is_err());
    }

    #[test]
    fn missing_current_or_following_guard_returns_an_error() {
        let mut missing = contract("esp32s31");
        missing.irq_stack_guard_bytes.remove(&0);
        assert!(
            missing
                .irq_stack_allocation(0, Some(1))
                .unwrap_err()
                .to_string()
                .contains("hart 0")
        );
        assert!(
            missing
                .irq_stack_usable_bytes(0)
                .unwrap_err()
                .to_string()
                .contains("hart 0")
        );
        let mut missing = contract("esp32s31");
        missing.irq_stack_guard_bytes.remove(&1);
        assert!(
            missing
                .irq_stack_allocation(0, Some(1))
                .unwrap_err()
                .to_string()
                .contains("hart 1")
        );
    }

    #[test]
    fn zero_alignment_and_guard_larger_than_stack_return_errors() {
        let mut invalid = contract("esp32s31");
        invalid.irq_stack_guard_bytes.insert(1, 0);
        assert!(
            invalid
                .irq_stack_allocation(0, Some(1))
                .unwrap_err()
                .to_string()
                .contains("alignment 0")
        );
        invalid.irq_stack_guard_bytes.insert(0, u32::MAX);
        assert!(
            invalid
                .irq_stack_usable_bytes(0)
                .unwrap_err()
                .to_string()
                .contains("exceeds its stack")
        );
    }

    #[test]
    fn guard_storage_does_not_gate_a_bound_with_margin_that_fits() {
        let contract = contract("esp32s31");
        for hart in [0, 1] {
            let allocation = contract.irq_stack_allocation(hart, Some(29789)).unwrap();
            assert_eq!(contract.irq_stack_with_margin(29789).unwrap(), 32768);
            assert_eq!(
                allocation,
                IrqStackAllocation {
                    bytes: 32768,
                    guard_bytes: 0
                }
            );
            assert!(contract.irq_stack_allocation(hart, Some(29790)).is_err());
        }
    }
}

//! Link-time allocation within the interrupt contract's maximum.

use alloc::format;

use crate::{InterruptContract, Result};

impl InterruptContract {
    /// Bound plus the contract margin, rounding fractional bytes upwards.
    pub fn irq_stack_with_margin(&self, bound: u64) -> Result<u64> {
        let bytes =
            (u128::from(bound) * (100 + u128::from(self.irq_stack_margin_percent))).div_ceil(100);
        Ok(u64::try_from(bytes).map_err(|_| "IRQ stack bound with margin overflows u64")?)
    }

    /// A hart's allocation, including its guard. The first stack must also
    /// leave the following hart's bottom aligned to that hart's NAPOT guard.
    /// `None` deliberately keeps the maximum; partial bounds are not proofs.
    pub fn irq_stack_size(&self, hart: u32, bound: Option<u64>) -> Result<u32> {
        let index = self
            .harts
            .iter()
            .position(|&core| core == hart)
            .ok_or_else(|| format!("no interrupt stack for hart {hart}"))?;
        let Some(bound) = bound else {
            return Ok(self.irq_stack_bytes);
        };
        let guard = u64::from(self.irq_stack_guard_bytes[&hart]);
        let usable = self.irq_stack_with_margin(bound)?;
        let bytes = usable
            .checked_add(15)
            .map(|bytes| bytes / 16 * 16)
            .and_then(|bytes| bytes.checked_add(guard))
            .ok_or("IRQ stack allocation overflows u64")?;
        let alignment = self
            .harts
            .get(index + 1)
            .map_or(16, |next| self.irq_stack_guard_bytes[next]);
        let alignment = u64::from(alignment);
        let bytes = bytes
            .checked_add(alignment - 1)
            .map(|bytes| bytes / alignment * alignment)
            .ok_or("IRQ stack alignment overflows u64")?;
        if bytes > u64::from(self.irq_stack_bytes) {
            return Err(format!("hart {hart}'s IRQ stack: bound {bound} + {}% margin requires {usable} usable bytes, guard {guard}, aligned allocation {bytes}, contract maximum {} bytes", self.irq_stack_margin_percent, self.irq_stack_bytes).into());
        }
        Ok(bytes as u32)
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
        assert_eq!(s31.irq_stack_size(1, Some(1001)).unwrap(), 1104 + 1024);
        // Hart 0's top is hart 1's bottom, aligned to 1024.
        assert_eq!(s31.irq_stack_size(0, Some(1001)).unwrap(), 6144);
        assert_eq!(s31.irq_stack_size(0, Some(16192)).unwrap(), 22528);
    }

    #[test]
    fn absent_proof_keeps_the_maximum_and_oversized_proof_fails_with_figures() {
        let contract = contract("esp32s31");
        for &hart in &contract.harts {
            assert_eq!(contract.irq_stack_size(hart, None).unwrap(), 32768);
        }
        let error = contract
            .irq_stack_size(0, Some(30000))
            .unwrap_err()
            .to_string();
        for figure in ["30000", "10%", "33000", "4096", "32768"] {
            assert!(error.contains(figure), "{error}");
        }
        assert!(contract.irq_stack_with_margin(u64::MAX).is_err());
        assert!(contract.irq_stack_size(0, Some(u64::MAX)).is_err());
        assert!(contract.irq_stack_size(7, None).is_err());
    }
}

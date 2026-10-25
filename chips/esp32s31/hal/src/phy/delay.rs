//! Short machine-cycle settles inside exclusive PHY transactions.

/// The production PHY short-settle implementation.
///
/// The clock owner captures the stable CPU rate at runtime handover. Interrupts
/// stay enabled: preemption can only lengthen the minimum settle. The public
/// type name is retained by the firmware's delay-provider contract.
#[derive(Clone, Copy, Debug, Default)]
pub struct RomShortDelay;

impl RomShortDelay {
    /// Maximum admitted blocking settle.
    /// SOURCE: `docs/architecture.md`, Execution: named settles up to 20 µs.
    pub const MAX_MICROS: u32 = 20;

    /// Complete a named minimum settle; refuse zero, long or unclocked waits.
    pub fn settle_micros(micros: u32) -> bool {
        if micros == 0 || micros > Self::MAX_MICROS {
            return false;
        }
        #[cfg(target_arch = "riscv32")]
        {
            let Some(cycles) = crate::boot::ClockRegisters::short_settle_cycles(micros) else {
                return false;
            };
            settle(cycles, oer_riscv::cycle32);
            true
        }
        #[cfg(not(target_arch = "riscv32"))]
        false
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn settle(cycles: u32, mut counter: impl FnMut() -> u32) {
    let start = counter();
    while counter().wrapping_sub(start) < cycles {
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_settle_handles_counter_wrap_and_preemption() {
        let mut readings = [u32::MAX - 2, u32::MAX, 1, 18].into_iter();
        settle(20, || {
            readings
                .next()
                .expect("settle must finish at the elapsed bound")
        });
        assert_eq!(readings.next(), None);
        let mut readings = [100, 1000].into_iter();
        settle(20, || {
            readings
                .next()
                .expect("a preemption already covers the minimum settle")
        });
        assert_eq!(readings.next(), None);
    }

    #[test]
    fn zero_and_long_waits_are_refused_before_reading_hardware() {
        assert!(!RomShortDelay::settle_micros(0));
        assert!(!RomShortDelay::settle_micros(RomShortDelay::MAX_MICROS + 1));
    }
}

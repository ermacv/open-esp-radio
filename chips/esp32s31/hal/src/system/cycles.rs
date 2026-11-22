//! The settle counter: the CPU's cycle counter (`mcycle`) at the rate the
//! runtime's adoption gave.
#![forbid(unsafe_code)]

use oer_chips_family_boot::adoption::CpuHz;
use oer_portable_time::settle::{SettleCounter, SettleLimit};

/// The cycles of a microsecond at most: any rate a `u32` holds, rounded up.
const MOST_CYCLES_PER_MICRO: u32 = u32::MAX.div_ceil(1_000_000);

// A settle's longest limit counted at the fastest rate fits a `u32`: the
// product in `cycles` cannot overflow.
const _: () = {
    let longest = SettleLimit::MAX.as_micros();
    if (MOST_CYCLES_PER_MICRO as u64) * longest > u32::MAX as u64 {
        panic!("a settle's cycles fit a u32 at any rate");
    }
};

/// The CPU's cycle counter at the adopted rate: a copy for each driver
/// that settles. It holds the rate the image runs at, and a copy does not
/// learn of a change: were the rate lowered, a settle would wait longer
/// than its limit, never shorter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CycleCounter {
    /// The cycles of one microsecond, rounded up: at most
    /// [`MOST_CYCLES_PER_MICRO`].
    per_micro: u32,
}

impl CycleCounter {
    /// The counter at the adopted rate `cpu`.
    pub const fn new(cpu: CpuHz) -> Self {
        Self {
            per_micro: cpu.get().div_ceil(1_000_000),
        }
    }
}

impl SettleCounter for CycleCounter {
    fn cycles(&self, limit: SettleLimit) -> u32 {
        // At most `MOST_CYCLES_PER_MICRO` × 20, which the const above bounds.
        self.per_micro.wrapping_mul(limit.micros())
    }

    fn sample(&self) -> u32 {
        #[cfg(target_arch = "riscv32")]
        {
            oer_portable_riscv::cycle32()
        }
        // No counter is made on the host: its adoption samples no rate. The
        // arm only lets host builds type-check the drivers over it.
        #[cfg(not(target_arch = "riscv32"))]
        {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limit_s_cycles_round_up_and_fit_at_every_rate() {
        let limit = |micros| {
            SettleLimit::new(oer_portable_time::Duration::from_micros(micros))
                .expect("a test limit")
        };
        let counter = |hz| CycleCounter::new(CpuHz::new(hz).expect("a test rate"));
        assert_eq!(counter(320_000_000).cycles(limit(20)), 6_400);
        assert_eq!(counter(320_300_000).cycles(limit(1)), 321);
        assert_eq!(counter(40_000_000).cycles(limit(2)), 80);
        assert_eq!(counter(1).cycles(limit(20)), 20);
        assert_eq!(counter(u32::MAX).cycles(limit(20)), 4_295 * 20);
    }
}

use crate::CoexError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexTimerClock {
    selector: CoexClockSelector,
    /// Hardware divisor minus one, already decoded and bounded by the PAC.
    divider_minus_one: u16,
    xtal_mhz: u32,
    real_chip: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexClockSelector {
    Selector1,
    Selector2,
    Selector4,
    Selector8,
}

impl CoexClockSelector {
    /// Validate the public divisor used by `coex_hw_timer_freq_set`. The
    /// vendor bounds it only from below; the register keeps the low twelve
    /// bits of the divisor minus one.
    pub const fn accepts_divisor(self, divisor: u32) -> bool {
        match self {
            Self::Selector8 => divisor == 1,
            Self::Selector4 => divisor >= 50,
            Self::Selector2 => divisor >= 40,
            Self::Selector1 => divisor >= 3,
        }
    }
}

impl CoexTimerClock {
    /// Construct a timer clock from fields already decoded by the chip PAC.
    pub const fn from_hardware_fields(
        selector: CoexClockSelector,
        divider_minus_one: u16,
        xtal_mhz: u32,
        real_chip: bool,
    ) -> Self {
        Self {
            selector,
            divider_minus_one,
            xtal_mhz,
            real_chip,
        }
    }

    /// Reproduce `coex_hw_timer_tick_get`, including its two-stage integer
    /// division for clock sources two and four.
    pub fn tick_image(self, value: u32) -> Result<u32, CoexError> {
        let numerator = u64::from(value) << 19;
        let divider = u64::from(self.divider_minus_one) + 1;
        let scale = match self.selector {
            CoexClockSelector::Selector8 => {
                if self.real_chip {
                    16_000_000_u64
                } else {
                    16_384_000_u64
                }
            }
            CoexClockSelector::Selector4 => {
                if self.xtal_mhz == 0 {
                    return Err(CoexError::UnsupportedClock);
                }
                (524_288_u64 * 1_000_000 * divider) / (u64::from(self.xtal_mhz) * 1_000_000)
            }
            CoexClockSelector::Selector2 => (524_288_u64 * 1_000_000 * divider) / 20_000_000,
            CoexClockSelector::Selector1 => 1,
        };
        if scale == 0 {
            return Err(CoexError::UnsupportedClock);
        }
        Ok((numerator / scale) as u32)
    }
}

/// Platform-owned access to the shared low-power clock configuration.
///
/// Sampling is deliberately mutable: each call represents the two fresh MMIO
/// reads performed by one vendor `coex_hw_timer_tick_get` invocation.
pub trait CoexClockHardware {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError>;
}

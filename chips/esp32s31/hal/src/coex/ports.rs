//! The coexistence ports of one radio domain.
//!
//! The radio domain owns the timer bank, the event priority table and the shared
//! modem clock fields. [`CoexDomainPorts`] lends them to
//! [`oer_espressif_coex::CoexCore`] for one transaction: the timer port
//! programs the policy timers and reads the domain's current event
//! priorities, the clock port samples the bank's low-power clock selection.

use oer_espressif_coex::{
    CoexClient, CoexClockHardware, CoexClockSelector, CoexError, CoexEventId, CoexPti,
    CoexTimerClock, CoexTimerHardware, CoexTimerIndex,
};

use super::{CoexClockPort, CoexPolicyTimer, CoexPolicyTimerBank, timer_clock};
use crate::{
    domain::RadioDomain,
    power::PlatformClockBackend,
    types::{
        CoexTimerClientValue, CoexTimerClockDividerMinusOne, CoexTimerClockSource,
        CoexTimerPtiValue,
    },
};

const fn bank_timer(index: CoexTimerIndex) -> CoexPolicyTimer {
    match index {
        CoexTimerIndex::Timer0 => CoexPolicyTimer::Timer0,
        CoexTimerIndex::Timer1 => CoexPolicyTimer::Timer1,
        CoexTimerIndex::Timer2 => CoexPolicyTimer::Timer2,
        CoexTimerIndex::Timer3 => CoexPolicyTimer::Timer3,
        CoexTimerIndex::Timer4 => CoexPolicyTimer::Timer4,
    }
}

/// The disjoint timer and clock owners of one radio-domain loan.
pub struct CoexDomainPorts<'radio> {
    timer: CoexDomainTimer<'radio>,
    clock: CoexDomainClock<'radio>,
}

/// Exclusive access to the policy timers and the borrowed priority table.
pub struct CoexDomainTimer<'radio> {
    bank: CoexPolicyTimerBank<'radio>,
}

/// Exclusive clock access; each sample reads the current hardware fields.
pub struct CoexDomainClock<'radio> {
    port: CoexClockPort<'radio>,
    real_chip: bool,
}

/// A public clock selection refused before any register write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerClockError {
    InvalidDivisor {
        selector: CoexClockSelector,
        divisor: u32,
    },
}

struct TimerClockConfig {
    source: CoexTimerClockSource,
    divider: CoexTimerClockDividerMinusOne,
}

impl TimerClockConfig {
    const fn new(selector: CoexClockSelector, divisor: u32) -> Result<Self, TimerClockError> {
        if !selector.accepts_divisor(divisor) {
            return Err(TimerClockError::InvalidDivisor { selector, divisor });
        }
        let source = match selector {
            CoexClockSelector::Selector1 => CoexTimerClockSource::Selector1,
            CoexClockSelector::Selector2 => CoexTimerClockSource::Selector2,
            CoexClockSelector::Selector4 => CoexTimerClockSource::Selector4,
            CoexClockSelector::Selector8 => CoexTimerClockSource::Selector8,
        };
        // SOURCE: complete pinned coex_hw_timer_freq_set keeps the field's low bits.
        // The selector check above rejects zero; wrapping_sub names this encoding arithmetic.
        let divider = CoexTimerClockDividerMinusOne::from_bits(oer_mmio::Bits::truncate(
            divisor.wrapping_sub(1),
        ));
        Ok(Self { source, divider })
    }
}

impl<'radio> CoexDomainPorts<'radio> {
    /// Lend the domain's coexistence resources for one transaction.
    pub fn new<B: PlatformClockBackend, T>(radio: &'radio mut RadioDomain<B, T>) -> Self {
        let (bank, port) = radio.coex_ports();
        Self {
            timer: CoexDomainTimer { bank },
            clock: CoexDomainClock {
                port,
                real_chip: true,
            },
        }
    }

    /// Borrow both disjoint ports exclusively; no runtime borrow guard is needed.
    pub fn ports(&mut self) -> (&mut CoexDomainTimer<'radio>, &mut CoexDomainClock<'radio>) {
        (&mut self.timer, &mut self.clock)
    }

    /// Select the reviewed initial clock before policy requests.
    ///
    /// SOURCE: complete pinned libcoexist.a[coexist_core.o]::coex_core_pre_init.
    pub fn configure_timer_clock(&mut self) {
        let config = if self.clock.real_chip {
            const {
                match TimerClockConfig::new(CoexClockSelector::Selector4, 50) {
                    Ok(config) => config,
                    Err(_) => panic!("the silicon clock selection is valid"),
                }
            }
        } else {
            const {
                match TimerClockConfig::new(CoexClockSelector::Selector8, 1) {
                    Ok(config) => config,
                    Err(_) => panic!("the FPGA clock selection is valid"),
                }
            }
        };
        self.clock
            .port
            .configure_timer_clock(config.source, config.divider);
    }

    /// Program the selected clock and the low field bits of divisor minus one.
    /// Invalid input is returned before any write.
    ///
    /// SOURCE: complete pinned libcoexist.a[coexist_hw.o]::coex_hw_timer_freq_set.
    pub fn set_timer_clock(
        &mut self,
        selector: CoexClockSelector,
        divisor: u32,
    ) -> Result<(), TimerClockError> {
        let config = TimerClockConfig::new(selector, divisor)?;
        self.clock
            .port
            .configure_timer_clock(config.source, config.divider);
        Ok(())
    }
}

impl CoexClockHardware for CoexDomainClock<'_> {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
        timer_clock(self.port.sample_low_power_clock(), self.real_chip)
    }
}

impl CoexTimerHardware for CoexDomainTimer<'_> {
    fn pti(&mut self, event: CoexEventId) -> CoexPti {
        self.bank.pti(event)
    }

    fn configure_request(
        &mut self,
        index: CoexTimerIndex,
        client: CoexClient,
        pti: CoexPti,
    ) -> Result<(), CoexError> {
        let client = CoexTimerClientValue::new(u32::from(client.timer_client_value()))
            .ok_or(CoexError::Hardware)?;
        let pti = CoexTimerPtiValue::new(u32::from(pti.value())).ok_or(CoexError::Hardware)?;
        self.bank.configure(bank_timer(index), client, pti);
        Ok(())
    }

    fn set_primary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.bank.set_primary_target(bank_timer(index), tick_image);
        Ok(())
    }

    fn set_secondary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.bank
            .set_secondary_target(bank_timer(index), tick_image);
        Ok(())
    }

    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.bank.enable(bank_timer(index));
        Ok(())
    }

    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.bank.disable(bank_timer(index));
        Ok(())
    }

    fn force(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.bank.force(bank_timer(index));
        Ok(())
    }

    fn unforce(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.bank.unforce(bank_timer(index));
        Ok(())
    }
}

#[cfg(test)]
mod tests;

//! The coexistence ports of one radio arbiter lease.
//!
//! The arbiter owns the timer bank, the event priority table and the shared
//! modem clock fields. [`CoexArbiterPorts`] lends them to [`crate::CoexCore`]
//! for one transaction: the timer port programs the policy timers and reads
//! the arbiter's current event priorities, the clock port samples the bank's
//! low-power clock selection.

use core::cell::RefCell;

use oer_esp32s31_hal::{
    coex::{CoexPolicyTimer, timer_clock},
    shared_radio::SharedRadioLease,
    types::{
        CoexTimerClientValue, CoexTimerClockDividerMinusOne, CoexTimerClockSource,
        CoexTimerPtiValue,
    },
};

use crate::{
    CoexClient, CoexClockHardware, CoexClockSelector, CoexError, CoexEventId, CoexPti,
    CoexTimerClock, CoexTimerHardware, CoexTimerIndex,
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

/// The timer and clock ports of one arbiter lease, shared by the two port
/// handles of a single transaction.
pub struct CoexArbiterPorts<'lease, 'radio, T> {
    lease: RefCell<&'lease mut SharedRadioLease<'radio, T>>,
    real_chip: bool,
}

/// The timer port of [`CoexArbiterPorts`].
pub struct CoexArbiterTimer<'ports, 'lease, 'radio, T> {
    ports: &'ports CoexArbiterPorts<'lease, 'radio, T>,
}

/// The clock port of [`CoexArbiterPorts`].
pub struct CoexArbiterClock<'ports, 'lease, 'radio, T> {
    ports: &'ports CoexArbiterPorts<'lease, 'radio, T>,
}

impl<'lease, 'radio, T> CoexArbiterPorts<'lease, 'radio, T> {
    /// Lend the lease's coexistence resources for one transaction.
    pub fn new(lease: &'lease mut SharedRadioLease<'radio, T>) -> Self {
        Self::with_real_chip(lease, true)
    }

    /// As [`Self::new`], with the vendor's FPGA clock constant selectable for
    /// compiled comparison.
    pub(crate) fn with_real_chip(
        lease: &'lease mut SharedRadioLease<'radio, T>,
        real_chip: bool,
    ) -> Self {
        Self {
            lease: RefCell::new(lease),
            real_chip,
        }
    }

    /// The timer and clock port handles.
    pub fn ports(
        &self,
    ) -> (
        CoexArbiterTimer<'_, 'lease, 'radio, T>,
        CoexArbiterClock<'_, 'lease, 'radio, T>,
    ) {
        (
            CoexArbiterTimer { ports: self },
            CoexArbiterClock { ports: self },
        )
    }

    /// Select the clock the policy timers count, as `coex_core_pre_init`
    /// does before any request: the crystal divided by 50 on silicon, and
    /// selector 8 undivided otherwise. Without a selected clock no request
    /// can convert its duration.
    ///
    /// SOURCE: complete pinned `libcoexist.a[coexist_core.o]::
    /// coex_core_pre_init`.
    pub fn configure_timer_clock(&self) {
        let (selector, divisor) = if self.real_chip {
            (CoexClockSelector::Selector4, 50)
        } else {
            (CoexClockSelector::Selector8, 1)
        };
        assert!(
            self.set_timer_clock(selector, divisor),
            "the vendor timer clock is an accepted selection"
        );
    }

    /// Program the timer clock `selector` divided by `divisor`, as
    /// `coex_hw_timer_freq_set` does: a divisor the selector does not accept
    /// changes nothing and returns `false`; otherwise the selector replaces
    /// the low nibble, then the divisor minus one the divider field.
    ///
    /// SOURCE: complete pinned `libcoexist.a[coexist_hw.o]::
    /// coex_hw_timer_freq_set`.
    pub fn set_timer_clock(&self, selector: CoexClockSelector, divisor: u32) -> bool {
        if !selector.accepts_divisor(divisor) {
            return false;
        }
        let source = match selector {
            CoexClockSelector::Selector1 => CoexTimerClockSource::Selector1,
            CoexClockSelector::Selector2 => CoexTimerClockSource::Selector2,
            CoexClockSelector::Selector4 => CoexTimerClockSource::Selector4,
            CoexClockSelector::Selector8 => CoexTimerClockSource::Selector8,
        };
        // The divider field keeps the low twelve bits of the divisor minus one.
        let divider_minus_one = CoexTimerClockDividerMinusOne::new((divisor - 1) & 0x0fff)
            .expect("twelve bits fit the divider field");
        self.with_bank(|bank| bank.configure_timer_clock(source, divider_minus_one));
        true
    }

    fn with_bank(&self, operation: impl FnOnce(&mut oer_esp32s31_hal::coex::CoexTimerBank<'_>)) {
        operation(&mut self.lease.borrow_mut().coex_timer_bank());
    }
}

impl<T> CoexClockHardware for CoexArbiterClock<'_, '_, '_, T> {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
        let observation = self
            .ports
            .lease
            .borrow_mut()
            .coex_timer_bank()
            .sample_low_power_clock();
        timer_clock(observation, self.ports.real_chip)
    }
}

impl<T> CoexTimerHardware for CoexArbiterTimer<'_, '_, '_, T> {
    fn pti(&mut self, event: CoexEventId) -> CoexPti {
        self.ports.lease.borrow().coex_pti(event)
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
        self.ports
            .with_bank(|bank| bank.configure(bank_timer(index), client, pti));
        Ok(())
    }

    fn set_primary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.ports
            .with_bank(|bank| bank.set_primary_target(bank_timer(index), tick_image));
        Ok(())
    }

    fn set_secondary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError> {
        self.ports
            .with_bank(|bank| bank.set_secondary_target(bank_timer(index), tick_image));
        Ok(())
    }

    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.ports.with_bank(|bank| bank.enable(bank_timer(index)));
        Ok(())
    }

    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.ports.with_bank(|bank| bank.disable(bank_timer(index)));
        Ok(())
    }

    fn force(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.ports.with_bank(|bank| bank.force(bank_timer(index)));
        Ok(())
    }

    fn unforce(&mut self, index: CoexTimerIndex) -> Result<(), CoexError> {
        self.ports.with_bank(|bank| bank.unforce(bank_timer(index)));
        Ok(())
    }
}

//! Exact generated-PAC transactions for the internal coexistence timer bank.

#![forbid(unsafe_code)]

use crate::{CoexTimerClientValue, CoexTimerPtiValue, CoexTimerTickInput, SharedRadioRegisters};

pub const COEX_TIMER_COUNT: u8 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CoexTimerRegister {
    Timer0 = 0,
    Timer1 = 1,
    Timer2 = 2,
    Timer3 = 3,
    Timer4 = 4,
    Timer5 = 5,
}

impl CoexTimerRegister {
    pub const ALL: [Self; COEX_TIMER_COUNT as usize] = [
        Self::Timer0,
        Self::Timer1,
        Self::Timer2,
        Self::Timer3,
        Self::Timer4,
        Self::Timer5,
    ];

    pub const fn new(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Timer0),
            1 => Some(Self::Timer1),
            2 => Some(Self::Timer2),
            3 => Some(Self::Timer3),
            4 => Some(Self::Timer4),
            5 => Some(Self::Timer5),
            _ => None,
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// The coexistence timer bank, borrowed apart from the PHY registers.
///
/// Every method is the exact generated transaction of one vendor timer leaf.
pub struct CoexTimerBankRegisters<'registers> {
    timers: &'registers crate::svd::CoexHwTimer,
}

impl SharedRadioRegisters {
    /// Borrow the coexistence timer bank.
    pub fn coex_timers_mut(&mut self) -> CoexTimerBankRegisters<'_> {
        CoexTimerBankRegisters {
            timers: &self.coexistence.coex_hw_timer,
        }
    }

    /// Borrow the PHY registers and the coexistence timer bank together.
    pub fn radio_phy_and_coex_timers_mut(
        &mut self,
    ) -> (&mut crate::RadioPhyRegisters, CoexTimerBankRegisters<'_>) {
        (
            &mut self.radio_phy,
            CoexTimerBankRegisters {
                timers: &self.coexistence.coex_hw_timer,
            },
        )
    }

    /// Enable one timer in the exact disable-clear/enable-set order.
    pub fn enable_coex_timer(&mut self, timer: CoexTimerRegister) {
        self.coex_timers_mut().enable(timer);
    }

    /// Disable one timer in the exact enable-clear/disable-set order.
    pub fn disable_coex_timer(&mut self, timer: CoexTimerRegister) {
        self.coex_timers_mut().disable(timer);
    }

    /// Force one timer by clearing only its low 24-bit tick image.
    pub fn force_coex_timer(&mut self, timer: CoexTimerRegister) {
        self.coex_timers_mut().force(timer);
    }

    /// Remove the force condition using the vendor's exact value of 1000.
    pub fn unforce_coex_timer(&mut self, timer: CoexTimerRegister) {
        self.coex_timers_mut().unforce(timer);
    }

    /// Program the first two fresh-read RMW edges of `coex_hw_timer_set`.
    pub fn configure_coex_timer(
        &mut self,
        timer: CoexTimerRegister,
        parameter_1: CoexTimerClientValue,
        parameter_2: CoexTimerPtiValue,
    ) {
        self.coex_timers_mut()
            .configure(timer, parameter_1, parameter_2);
    }

    /// Publish the converted primary target of `coex_hw_timer_set`.
    pub fn set_coex_timer_primary_target(
        &mut self,
        timer: CoexTimerRegister,
        primary_tick_image: u32,
    ) {
        self.coex_timers_mut()
            .set_primary_target(timer, primary_tick_image);
    }

    /// Publish the converted secondary target of `coex_hw_timer_set`.
    pub fn set_coex_timer_secondary_target(
        &mut self,
        timer: CoexTimerRegister,
        secondary_tick_image: u32,
    ) {
        self.coex_timers_mut()
            .set_secondary_target(timer, secondary_tick_image);
    }
}

impl CoexTimerBankRegisters<'_> {
    /// Enable one timer in the exact disable-clear/enable-set order.
    pub fn enable(&mut self, timer: CoexTimerRegister) {
        let index = timer.index();
        let timers = self.timers;
        crate::generated::clear_coex_timer_disable(timers, index);
        crate::generated::set_coex_timer_enable(timers, index);
    }

    /// Disable one timer in the exact enable-clear/disable-set order.
    pub fn disable(&mut self, timer: CoexTimerRegister) {
        let index = timer.index();
        let timers = self.timers;
        crate::generated::clear_coex_timer_enable(timers, index);
        crate::generated::set_coex_timer_disable(timers, index);
    }

    /// Force one timer by clearing only its low 24-bit tick image.
    pub fn force(&mut self, timer: CoexTimerRegister) {
        let index = timer.index();
        crate::generated::force_coex_timer(self.timers, index);
    }

    /// Remove the force condition using the vendor's exact value of 1000.
    pub fn unforce(&mut self, timer: CoexTimerRegister) {
        let index = timer.index();
        crate::generated::unforce_coex_timer(self.timers, index);
    }

    /// Program the first two fresh-read RMW edges of `coex_hw_timer_set`.
    ///
    /// Tick conversion deliberately does not happen in this method. The
    /// vendor samples the platform clock after these two writes and again
    /// between the primary and secondary target writes.
    pub fn configure(
        &mut self,
        timer: CoexTimerRegister,
        parameter_1: CoexTimerClientValue,
        parameter_2: CoexTimerPtiValue,
    ) {
        let index = timer.index();
        let timers = self.timers;
        crate::generated::configure_coex_timer_client(timers, index, parameter_1);
        crate::generated::configure_coex_timer_pti(timers, index, parameter_2);
    }

    /// Publish the converted primary target in the third fresh-read RMW edge
    /// of `coex_hw_timer_set`.
    pub fn set_primary_target(&mut self, timer: CoexTimerRegister, primary_tick_image: u32) {
        let index = timer.index();
        let primary_tick_input = CoexTimerTickInput::new(primary_tick_image)
            .expect("every u32 belongs to the reviewed COEX timer input domain");
        crate::generated::configure_coex_timer_primary_target(
            self.timers,
            index,
            primary_tick_input,
        );
    }

    /// Publish the converted secondary target in the final fresh-read RMW
    /// edge of `coex_hw_timer_set`.
    pub fn set_secondary_target(&mut self, timer: CoexTimerRegister, secondary_tick_image: u32) {
        let index = timer.index();
        let secondary_tick_input = CoexTimerTickInput::new(secondary_tick_image)
            .expect("every u32 belongs to the reviewed COEX timer input domain");
        crate::generated::configure_coex_timer_secondary_target(
            self.timers,
            index,
            secondary_tick_input,
        );
    }
}

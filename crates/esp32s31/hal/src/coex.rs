//! Shared coexistence mechanism owned by the radio arbiter.
//!
//! The arbiter keeps the coexistence priority table ([`CoexPtiTable`]) and
//! lends the timer bank ([`CoexTimerBank`]) through its lease. Both are
//! mechanism: which event requests which timer, and when, is the coexistence
//! driver's policy, and every protocol programs its own MAC PTI registers from
//! the values read here. A timer write or a PTI value does not report RF
//! admission or grant revocation; the hardware arbitrates by priority and
//! gives the CPU no grant acknowledgement.
//!
//! Timer 5 is not lent: the arbiter programs it for the PHY grant-protect
//! request (`SharedRadioLease::acquire_phy_grant_protect`).

use oer_esp32s31_pac::{
    CoexTimerBankRegisters, CoexTimerClientValue, CoexTimerPtiValue, CoexTimerRegister,
    CoexistenceLowPowerClockObservation, SharedRadioRegisters,
};

/// The event priority table and IEEE 802.15.4 levels every Espressif chip
/// shares; the arbiter keeps one table as shared state.
pub use oer_espressif_coex::{
    COEX_EVENT_COUNT, CoexEventId, CoexPti, CoexPtiTable, Ieee802154CoexLevel,
};

mod clock;

pub use clock::timer_clock;

/// Why the PHY grant-protect request cannot change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyGrantProtectError {
    /// The request is already programmed; the vendor event has no nesting.
    AlreadyProtected,
    /// No request is programmed.
    NotProtected,
}

/// The coexistence request kind of `phy_acquire_grant_protect` (`2`), as
/// the timer client field value: `coex_core_request` maps request kinds
/// 0 through 4 through its five-byte table `02 01 00 03 04`.
const PHY_GRANT_PROTECT_CLIENT: u32 = 0;

/// The vendor PHY grant-protect event.
pub(crate) const PHY_GRANT_PROTECT_EVENT: CoexEventId = match CoexEventId::new(48) {
    Some(event) => event,
    None => unreachable!(),
};

/// The PHY grant-protect request of one radio owner.
///
/// This is the pinned archive's `phy_acquire_grant_protect` /
/// `phy_release_grant_protect` pair: `coex_core_request(2, 48, 0, 0)` and
/// `coex_core_release(2, 48)` on timer 5. The owner keeps one flag, so the
/// request is serialized as the vendor event has no nesting. It is a priority
/// request to the hardware arbiter, never a grant acknowledgement: it does not
/// prove that any other radio stopped.
pub struct PhyGrantProtect<'owner> {
    timers: CoexTimerBankRegisters<'owner>,
    pti: CoexPti,
    protected: &'owner mut bool,
}

impl<'owner> PhyGrantProtect<'owner> {
    pub(crate) fn new(
        timers: CoexTimerBankRegisters<'owner>,
        pti: CoexPti,
        protected: &'owner mut bool,
    ) -> Self {
        Self {
            timers,
            pti,
            protected,
        }
    }

    /// Program the request: timer 5 receives the request-kind client field,
    /// the event-48 priority and zero primary and secondary targets, then is
    /// enabled.
    ///
    /// Zero timing arguments convert to zero tick images for every clock
    /// selection, so no clock sample is taken.
    ///
    /// # Errors
    ///
    /// The request is already programmed; nothing is written.
    pub fn acquire(&mut self) -> Result<(), PhyGrantProtectError> {
        if *self.protected {
            return Err(PhyGrantProtectError::AlreadyProtected);
        }
        let (Some(client), Some(pti)) = (
            CoexTimerClientValue::new(PHY_GRANT_PROTECT_CLIENT),
            CoexTimerPtiValue::new(u32::from(self.pti.value())),
        ) else {
            unreachable!("the client and four-bit priority are in their domains");
        };
        let timer = CoexTimerRegister::Timer5;
        self.timers.configure(timer, client, pti);
        self.timers.set_primary_target(timer, 0);
        self.timers.set_secondary_target(timer, 0);
        self.timers.enable(timer);
        *self.protected = true;
        Ok(())
    }

    /// Withdraw the request by disabling timer 5.
    ///
    /// # Errors
    ///
    /// No request is programmed; nothing is written.
    pub fn release(&mut self) -> Result<(), PhyGrantProtectError> {
        if !*self.protected {
            return Err(PhyGrantProtectError::NotProtected);
        }
        self.timers.disable(CoexTimerRegister::Timer5);
        *self.protected = false;
        Ok(())
    }

    /// Whether the request is programmed.
    pub fn is_protected(&self) -> bool {
        *self.protected
    }
}

/// One coexistence timer the coexistence policy may program.
///
/// Timer 5 is not among them: the arbiter reserves it for the PHY
/// grant-protect request (vendor event 48), so no policy request can
/// withdraw or overwrite that protection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CoexPolicyTimer {
    Timer0 = 0,
    Timer1 = 1,
    Timer2 = 2,
    Timer3 = 3,
    Timer4 = 4,
}

impl CoexPolicyTimer {
    pub const ALL: [Self; 5] = [
        Self::Timer0,
        Self::Timer1,
        Self::Timer2,
        Self::Timer3,
        Self::Timer4,
    ];

    /// A policy timer, or `None` for the reserved timer or beyond the bank.
    pub const fn new(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Timer0),
            1 => Some(Self::Timer1),
            2 => Some(Self::Timer2),
            3 => Some(Self::Timer3),
            4 => Some(Self::Timer4),
            _ => None,
        }
    }

    pub const fn value(self) -> u8 {
        self as u8
    }

    const fn register(self) -> CoexTimerRegister {
        match self {
            Self::Timer0 => CoexTimerRegister::Timer0,
            Self::Timer1 => CoexTimerRegister::Timer1,
            Self::Timer2 => CoexTimerRegister::Timer2,
            Self::Timer3 => CoexTimerRegister::Timer3,
            Self::Timer4 => CoexTimerRegister::Timer4,
        }
    }
}

/// Borrowed authority over the policy timers of the coexistence bank.
///
/// Every method is one finite register transaction. A write does not report
/// RF admission or grant revocation.
pub struct CoexTimerBank<'registers> {
    registers: &'registers mut SharedRadioRegisters,
}

impl<'registers> CoexTimerBank<'registers> {
    pub(crate) fn from_owned(registers: &'registers mut SharedRadioRegisters) -> Self {
        Self { registers }
    }

    /// Publish the client and PTI request fields of one timer.
    pub fn configure(
        &mut self,
        timer: CoexPolicyTimer,
        client: CoexTimerClientValue,
        pti: CoexTimerPtiValue,
    ) {
        self.registers
            .configure_coex_timer(timer.register(), client, pti);
    }

    /// Publish one already converted primary target tick image.
    pub fn set_primary_target(&mut self, timer: CoexPolicyTimer, tick_image: u32) {
        self.registers
            .set_coex_timer_primary_target(timer.register(), tick_image);
    }

    /// Publish one already converted secondary target tick image.
    pub fn set_secondary_target(&mut self, timer: CoexPolicyTimer, tick_image: u32) {
        self.registers
            .set_coex_timer_secondary_target(timer.register(), tick_image);
    }

    pub fn enable(&mut self, timer: CoexPolicyTimer) {
        self.registers.enable_coex_timer(timer.register());
    }

    pub fn disable(&mut self, timer: CoexPolicyTimer) {
        self.registers.disable_coex_timer(timer.register());
    }

    pub fn force(&mut self, timer: CoexPolicyTimer) {
        self.registers.force_coex_timer(timer.register());
    }

    pub fn unforce(&mut self, timer: CoexPolicyTimer) {
        self.registers.unforce_coex_timer(timer.register());
    }

    /// Select the clock the coexistence timers count, as complete
    /// `coex_hw_timer_freq_set` does.
    pub fn configure_timer_clock(
        &mut self,
        source: crate::types::CoexTimerClockSource,
        divider_minus_one: crate::types::CoexTimerClockDividerMinusOne,
    ) {
        self.registers
            .configure_coexistence_timer_clock(source, divider_minus_one);
    }

    /// Sample the shared coexistence low-power clock selection once.
    ///
    /// Each call performs fresh reads; `None` reports an unreviewed encoding.
    pub fn sample_low_power_clock(&mut self) -> Option<CoexistenceLowPowerClockObservation> {
        self.registers.sample_coexistence_low_power_clock()
    }
}

pub use oer_esp32s31_pac::{ExternalCoexRole, ExternalCoexWires};

/// An external priority level (`esp_coex_pti_level_t`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalCoexLevel {
    Mid,
    High,
}

impl ExternalCoexLevel {
    /// The priority `ic_set_extern_coex` publishes for this level.
    const fn priority(self) -> u8 {
        match self {
            Self::Mid => 0x8,
            Self::High => 0xc,
        }
    }
}

/// The priority `ic_set_extern_coex` publishes first, whatever the levels.
const EXTERNAL_COEX_BASE_PRIORITY: u8 = 0x3;

/// External coexistence, as ESP-IDF's `esp_enable_extern_coex_gpio_pin`
/// configures it after routing the signals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExternalCoexConfig {
    pub role: ExternalCoexRole,
    pub wires: ExternalCoexWires,
    /// Grant delay in microseconds; the hardware keeps the low four bits.
    pub grant_delay_us: u8,
    /// Whether the external grant is valid high.
    pub validate_high: bool,
    /// The two levels `esp_coex_external_set` receives last; ESP-IDF passes
    /// `MID` and `HIGH`.
    pub levels: [ExternalCoexLevel; 2],
}

impl ExternalCoexConfig {
    /// ESP-IDF's defaults for `role` and `wires`: no grant delay, valid high,
    /// and the `MID`, `HIGH` levels.
    pub const fn vendor(role: ExternalCoexRole, wires: ExternalCoexWires) -> Self {
        Self {
            role,
            wires,
            grant_delay_us: 0,
            validate_high: true,
            levels: [ExternalCoexLevel::Mid, ExternalCoexLevel::High],
        }
    }

    pub(crate) fn priorities(self) -> [oer_esp32s31_pac::ExternalCoexPriority; 3] {
        let priority = |value| {
            oer_esp32s31_pac::ExternalCoexPriority::new(value)
                .unwrap_or_else(|| unreachable!("the reviewed priorities fit four bits"))
        };
        [
            priority(EXTERNAL_COEX_BASE_PRIORITY),
            priority(self.levels[0].priority()),
            priority(self.levels[1].priority()),
        ]
    }
}

/// Why external coexistence cannot change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalCoexError {
    /// External coexistence already runs.
    AlreadyActive,
    /// The coexistence module clock could not change.
    Clock(crate::shared_radio::ModemClockError),
}

#[cfg(test)]
mod tests;

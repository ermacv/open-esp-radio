//! Wi-Fi as a client of the shared radio domain.
//!
//! The concurrent split hands Wi-Fi its MAC partition and interrupt setup;
//! common radio power, the modem clocks and every shared register stay with
//! the radio domain. These owners follow ESP-IDF's bring-up: common power, then
//! `modem_clock_module_enable(PERIPH_WIFI_MODULE)`, then the Wi-Fi
//! initialized status that keeps the Wi-Fi clocks enabled while Wi-Fi is
//! initialized. Each stage proves only its own step; MAC, PHY and RF
//! readiness are later stages.
//!
//! A clocked client borrows its capabilities with the radio domain: the
//! cold MAC HAL and the channel HAL take the shared registers from the radio domain
//! for one transaction, the MAC reset pulses through it, and the runtime MAC
//! HAL needs no radio domain because the Wi-Fi hot path touches Wi-Fi registers
//! only. After cold MAC setup [`WifiClocked::into_running`] splits the client
//! into the task-side [`RadioRuntimeOwner`] and the MAC interrupt setup;
//! [`WifiClocked::from_running`] reunites a stopped runtime epoch.

use core::fmt;

use oer_esp32s31_pac::{MacInterruptSetup, WifiRadioRegisters};

use crate::types::MacInterruptEnableState;

use crate::{
    domain::{
        CoexAbsent, CommonPower, CommonPowerExitFailure, CommonRadioPowerError, ModemClockError,
        PlatformClockProvider, RadioDomain, WifiClient,
    },
    ieee80211::{
        channel::RadioChannelHal,
        mac::{WifiMacColdHal, WifiMacHal},
        station_wake::StationWakeState,
    },
    owner::RadioRuntimeOwner,
    root::WifiPartition,
};

/// The Wi-Fi MAC register set and its interrupt setup.
struct WifiClientRegisters {
    mac: WifiRadioRegisters,
    interrupts: MacInterruptSetup,
}

/// Unpowered Wi-Fi client holding its partition.
///
/// Construction performs no MMIO and proves neither power nor clocks.
#[must_use = "the Wi-Fi client retains its radio partition"]
pub struct WifiCold {
    registers: WifiClientRegisters,
}

/// Wi-Fi client inside common radio power; its module clocks are off.
#[must_use = "the powered Wi-Fi client must leave common power"]
pub struct WifiPowered {
    registers: WifiClientRegisters,
}

/// Wi-Fi client with its module clocks enabled.
#[must_use = "the clocked Wi-Fi client must disable its clocks"]
pub struct WifiClocked {
    registers: WifiClientRegisters,
    initialized: bool,
    station_wake: StationWakeState,
    clocks: WifiClocksOn,
}

/// Proof that the Wi-Fi module clocks are enabled.
///
/// A [`WifiClocked`] client holds it, and hands it out beside its running
/// epoch in [`WifiClocked::into_running`]; the clocks can be disabled only
/// after [`WifiClocked::from_running`] took it back. A running role
/// therefore proves its clocks to the shared PHY while its registers and
/// interrupt setup belong to its datapath.
///
/// ```compile_fail
/// use oer_esp32s31_hal::ieee80211::client::WifiClocksOn;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<WifiClocksOn>();
/// ```
#[must_use = "the proof returns to its clocked Wi-Fi client"]
pub struct WifiClocksOn {
    _private: (),
}

/// Failed Wi-Fi lifecycle transition retaining the unchanged owner.
#[must_use = "a failed Wi-Fi transition still owns the partition"]
pub struct WifiTransitionFailure<Owner, Error> {
    owner: Owner,
    error: Error,
}

impl<Owner, Error: Copy> WifiTransitionFailure<Owner, Error> {
    pub const fn error(&self) -> Error {
        self.error
    }

    /// Recover the owner. After [`ModemClockError::Poisoned`] no further
    /// modem clock change succeeds until reset.
    pub fn into_owner(self) -> Owner {
        self.owner
    }
}

impl<Owner, Error: fmt::Debug> fmt::Debug for WifiTransitionFailure<Owner, Error> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WifiTransitionFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl WifiCold {
    /// Take the partition of a concurrent split. This performs no MMIO.
    pub fn from_partition(partition: WifiPartition) -> Self {
        let (mac, interrupts) = partition.into_parts();
        Self {
            registers: WifiClientRegisters {
                mac: WifiRadioRegisters::new(mac),
                interrupts,
            },
        }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_partition(self) -> WifiPartition {
        let WifiClientRegisters { mac, interrupts } = self.registers;
        WifiPartition::from_parts(mac.into_partition(), interrupts)
    }

    /// Enter common radio power as the Wi-Fi client. Only the first client
    /// runs the modem/PHY power sequence.
    ///
    /// # Errors
    ///
    /// The client already holds power, or the first client's sequence failed
    /// a read-back checkpoint; the unchanged owner is returned.
    pub fn power_up<T>(
        self,
        radio: &mut RadioDomain<T>,
        clocks: &impl PlatformClockProvider,
    ) -> Result<
        (WifiPowered, CommonPower<WifiClient>),
        WifiTransitionFailure<Self, CommonRadioPowerError>,
    > {
        match radio.enter_common_power(&self.registers.mac, clocks) {
            Ok(power) => Ok((
                WifiPowered {
                    registers: self.registers,
                },
                power,
            )),
            Err(error) => Err(WifiTransitionFailure { owner: self, error }),
        }
    }
}

impl WifiPowered {
    /// Leave common radio power, outside coexistence (`coex`); the last
    /// client restores the cold baseline.
    ///
    /// # Errors
    ///
    /// The baseline did not read back; the client stays entered and keeps
    /// its owner and proof.
    #[allow(
        clippy::type_complexity,
        reason = "the failure returns the owner beside its power proof"
    )]
    pub fn power_down<T>(
        self,
        radio: &mut RadioDomain<T>,
        power: CommonPower<WifiClient>,
        coex: &CoexAbsent<WifiClient>,
    ) -> Result<
        WifiCold,
        WifiTransitionFailure<(Self, CommonPower<WifiClient>), CommonRadioPowerError>,
    > {
        match radio.exit_common_power(&self.registers.mac, power, coex) {
            Ok(()) => Ok(WifiCold {
                registers: self.registers,
            }),
            Err(CommonPowerExitFailure { power, error }) => Err(WifiTransitionFailure {
                owner: (self, power),
                error,
            }),
        }
    }

    /// Enable the Wi-Fi module clocks through the shared planner:
    /// `modem_clock_module_enable(PERIPH_WIFI_MODULE)`. Only dependencies no
    /// other client holds are switched on.
    ///
    /// # Errors
    ///
    /// The planner rejected the request before any access, or a platform
    /// request failed and poisoned the modem clocks.
    pub fn enable_clocks<T>(
        self,
        radio: &mut RadioDomain<T>,
        platform: &impl PlatformClockProvider,
    ) -> Result<WifiClocked, WifiTransitionFailure<Self, ModemClockError>> {
        match radio.enable_modem_clocks(&self.registers.mac, platform) {
            Ok(()) => Ok(WifiClocked {
                registers: self.registers,
                initialized: false,
                station_wake: StationWakeState::default(),
                clocks: WifiClocksOn { _private: () },
            }),
            Err(error) => Err(WifiTransitionFailure { owner: self, error }),
        }
    }
}

impl WifiClocked {
    /// Mark Wi-Fi initialized (`esp_wifi_init` success) or no longer
    /// initialized (`esp_wifi_deinit`), as `modem_clock_configure_wifi_status`
    /// does.
    ///
    /// # Errors
    ///
    /// The modem clocks are poisoned.
    pub fn set_initialized<T>(
        &mut self,
        radio: &mut RadioDomain<T>,
        initialized: bool,
    ) -> Result<(), ModemClockError> {
        radio.set_wifi_initialized(&self.registers.mac, initialized)?;
        self.initialized = initialized;
        Ok(())
    }

    /// A clocked owner over a host-test partition, without MMIO and without
    /// claiming any clock state.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn for_host_test(cold: WifiCold) -> Self {
        Self {
            registers: cold.registers,
            initialized: false,
            station_wake: StationWakeState::default(),
            clocks: WifiClocksOn { _private: () },
        }
    }

    /// The proof that this client's clocks are enabled.
    pub const fn clocks_on(&self) -> &WifiClocksOn {
        &self.clocks
    }

    /// Whether Wi-Fi is marked initialized.
    pub const fn initialized(&self) -> bool {
        self.initialized
    }

    /// Pulse the Wi-Fi MAC reset, as
    /// `modem_clock_module_mac_reset(PERIPH_WIFI_MODULE)` does through
    /// `modem_syscon_ll_reset_wifimac`.
    pub fn reset_mac<T>(&mut self, radio: &mut RadioDomain<T>) {
        let phy = radio.registers_mut().radio_phy_mut();
        phy.set_wifi_mac_reset(true);
        phy.set_wifi_mac_reset(false);
    }

    /// Borrow the cold MAC HAL for one transaction, with the shared radio
    /// owner borrowed from the radio domain.
    ///
    /// Clock and modem-source configuration of the exclusive route are
    /// no-ops here: the radio domain enabled the Wi-Fi module clocks already.
    pub fn cold_mac_hal<'hal, T>(
        &'hal mut self,
        radio: &'hal mut RadioDomain<T>,
    ) -> WifiMacColdHal<'hal> {
        let WifiClientRegisters { mac, interrupts } = &mut self.registers;
        WifiMacColdHal::from_leased(mac, interrupts, radio.registers_mut())
    }

    /// Borrow the runtime MAC HAL over the MAC alone.
    pub fn wifi_mac_hal(&mut self) -> WifiMacHal<'_> {
        WifiMacHal::from_mac(&mut self.registers.mac)
    }

    /// Borrow the channel HAL for one channel transaction, with the shared
    /// PHY borrowed from the radio domain.
    pub fn channel_hal<'hal, P, T>(
        &'hal mut self,
        platform: &'hal mut P,
        radio: &'hal mut RadioDomain<T>,
    ) -> RadioChannelHal<'hal, P> {
        let (phy, restore) = radio.phy_parts_mut();
        RadioChannelHal::from_leased(platform, &mut self.registers.mac, phy, restore)
    }

    /// Borrow the channel HAL together with the domain's attachment, for a
    /// PHY channel operation that updates the shared domain's state.
    pub fn channel_hal_with_attachment<'hal, P, T>(
        &'hal mut self,
        platform: &'hal mut P,
        radio: &'hal mut RadioDomain<T>,
    ) -> (RadioChannelHal<'hal, P>, &'hal mut T) {
        let (phy, restore, attachment) = radio.phy_parts_with_attachment();
        (
            RadioChannelHal::from_leased(platform, &mut self.registers.mac, phy, restore),
            attachment,
        )
    }

    /// Close the cold polling interrupt phase before the task/ISR split:
    /// mask and clear every MAC interrupt and return the previous enables.
    pub fn close_cold_interrupt_phase(&mut self) -> MacInterruptEnableState {
        let interrupts = &mut self.registers.interrupts;
        let mask = interrupts.mac_interrupt_enable();
        interrupts.mask_and_clear_all_mac_interrupts();
        mask
    }

    /// Complete the one-way transition after cold MAC setup: the task-side
    /// runtime owner, the MAC interrupt setup, which keeps MAC interrupts
    /// masked until its activation, and the proof that the clocks stay on.
    /// This performs no MMIO.
    pub fn into_running(
        self,
    ) -> (
        RadioRuntimeOwner,
        crate::owner::MacInterruptSetup,
        WifiClocksOn,
    ) {
        let WifiClientRegisters { mac, interrupts } = self.registers;
        (
            RadioRuntimeOwner::from_clocked(mac, self.station_wake, self.initialized),
            crate::owner::MacInterruptSetup { inner: interrupts },
            self.clocks,
        )
    }

    /// Reunite a stopped runtime epoch with its inactive interrupt setup.
    ///
    /// The caller must have completed its protocol stop: no DMA owner or
    /// installed interrupt route may remain outside these values. This
    /// performs no MMIO; the Wi-Fi clocks stay enabled.
    pub fn from_running(
        runtime: RadioRuntimeOwner,
        interrupts: crate::owner::MacInterruptSetup,
        clocks: WifiClocksOn,
    ) -> Self {
        let (mac, station_wake, initialized) = runtime.into_clocked_parts();
        Self {
            registers: WifiClientRegisters {
                mac,
                interrupts: interrupts.inner,
            },
            initialized,
            station_wake,
            clocks,
        }
    }

    /// End this stopped installation's clock obligation exactly once.
    ///
    /// The caller has recovered both the task owner and inactive IRQ setup
    /// into this value. Any failed clock release, whether a physical failure
    /// or the shared modem planner's internal refusal (`Rejected`,
    /// `NotEnabled`), returns stopped custody and an unreleased, poisoned
    /// obligation; neither can restart or retry it.
    /// The shared modem planner's recovery policy remains separate, unfinished
    /// work. Its internal refusal is never exposed as a retryable Wi-Fi state.
    #[allow(
        clippy::result_large_err,
        reason = "failure must retain both stopped registers and the clock obligation"
    )]
    pub fn release_clocks<T>(
        mut self,
        radio: &mut RadioDomain<T>,
    ) -> Result<ClockReleased, (StoppedInstallation, ClockPoisoned)> {
        let release = self
            .set_initialized(radio, false)
            .and_then(|()| radio.disable_modem_clocks(&self.registers.mac));
        self.finish_clock_release(release)
    }

    fn finish_clock_release(
        self,
        release: Result<(), ModemClockError>,
    ) -> Result<ClockReleased, (StoppedInstallation, ClockPoisoned)> {
        match release {
            Ok(()) => {
                let WifiClocksOn { _private: () } = self.clocks;
                Ok(ClockReleased {
                    powered: WifiPowered {
                        registers: self.registers,
                    },
                })
            }
            Err(_release_failure) => Err((
                StoppedInstallation {
                    _registers: self.registers,
                    _station_wake: self.station_wake,
                },
                ClockPoisoned {
                    _obligation: self.clocks,
                },
            )),
        }
    }
}

/// Successful consumption of one Wi-Fi module-clock obligation.
#[must_use = "the powered owner must still leave common power"]
pub struct ClockReleased {
    powered: WifiPowered,
}
impl ClockReleased {
    pub fn into_powered(self) -> WifiPowered {
        self.powered
    }
}

/// Register custody retained after a failed release; it cannot become running.
#[must_use = "stopped custody remains installed until reset"]
pub struct StoppedInstallation {
    _registers: WifiClientRegisters,
    _station_wake: StationWakeState,
}

/// The exact unresolved obligation, retained by value after any failed clock
/// release, physical or a planner refusal.
/// It has no release/retry method and cannot manufacture a fresh ClocksOn.
#[must_use = "poison does not discharge the clock obligation"]
pub struct ClockPoisoned {
    _obligation: WifiClocksOn,
}

#[cfg(test)]
mod tests;

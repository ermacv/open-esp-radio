//! Common radio power, its platform clock references and the reversible
//! cold-power baseline.
//!
//! The restricted PAC exposes straight-line configuration and baseline
//! transactions. This module decides when common power takes and gives back the
//! platform-owned 160 MHz and analog-I2C holds, which state it restores
//! on release and when the captured cold-power baseline may be committed.
//! Every value here is route-scoped: a route gives back its references and
//! restores the baseline before the neutral root is reconstructed.

use crate::{radio::modem::WifiPowerBaseline, radio::phy::RadioPhyRegisters};

use crate::power::platform::{PlatformClock, PlatformGates, PlatformHold, WifiPowerGate};
use crate::system::root::WifiPowerRestoreCheckpoint;
use oer_chips_family_radio_power::baseline::PowerRestoreCheckpoint;

/// Register transactions consumed by route clock policy.
pub(crate) trait ClockPort {
    fn select_wifi_low_power_clock(&mut self);
    fn capture_power_baseline(&self) -> WifiPowerBaseline;
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), PowerRestoreCheckpoint>;
    /// Run the common modem/PHY power sequence, taking the platform clock
    /// holds into `refs` at their exact edges.
    fn run_common_power_sequence(
        &mut self,
        refs: &mut PlatformClockRefs,
        platform: &mut impl PlatformGates,
    ) -> Result<(), oer_chips_family_radio_power::power::PowerError>;
}

impl ClockPort for RadioPhyRegisters {
    fn select_wifi_low_power_clock(&mut self) {
        oer_chips_family_radio_power::clocks::select_wifi_low_power_clock(self);
    }
    fn capture_power_baseline(&self) -> WifiPowerBaseline {
        self.capture_wifi_power_baseline()
    }
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), PowerRestoreCheckpoint> {
        oer_chips_family_radio_power::baseline::restore(self, baseline)
    }
    fn run_common_power_sequence(
        &mut self,
        refs: &mut PlatformClockRefs,
        platform: &mut impl PlatformGates,
    ) -> Result<(), oer_chips_family_radio_power::power::PowerError> {
        oer_chips_family_radio_power::power::execute_owned(&mut crate::power::RoutePower {
            phy: self,
            refs,
            platform,
        })
    }
}

/// The platform clock holds common radio power keeps, one of each while
/// any client holds it; a retried power-up takes only the holds it does not
/// keep yet.
#[derive(Debug, Default)]
pub(crate) struct PlatformClockRefs {
    pll_f160m: Option<PlatformHold>,
    analog_i2c: Option<PlatformHold>,
}

impl PlatformClockRefs {
    fn slot(
        &mut self,
        clock: oer_chips_family_radio_power::membership::Clock,
    ) -> &mut Option<PlatformHold> {
        use oer_chips_family_radio_power::membership::Clock;
        match clock {
            Clock::Pll160m => &mut self.pll_f160m,
            Clock::AnalogI2c => &mut self.analog_i2c,
        }
    }

    pub(crate) fn acquire(
        &mut self,
        clock: oer_chips_family_radio_power::power::PowerClock,
        platform: &mut impl PlatformGates,
    ) {
        use oer_chips_family_radio_power::{membership::Clock, power::PowerClock};
        let (held, clock) = match clock {
            PowerClock::Pll160m => (Clock::Pll160m, PlatformClock::Pll160m),
            PowerClock::AnalogI2cMaster => (Clock::AnalogI2c, PlatformClock::AnalogI2cMaster),
        };
        let slot = self.slot(held);
        if slot.is_none() {
            *slot = Some(platform.acquire(clock));
        }
    }

    /// Give back the hold of `clock`, if kept.
    fn release(
        &mut self,
        clock: oer_chips_family_radio_power::membership::Clock,
        platform: &mut impl PlatformGates,
    ) {
        if let Some(hold) = self.slot(clock).take() {
            platform.release(hold);
        }
    }
}

/// Protocol clients that can share the powered radio.
pub use oer_protocols_radio_coex::RadioClient;

mod sealed {
    pub trait RadioClientKind {}
}

/// One protocol client of the shared radio, as a type.
pub trait RadioClientKind: sealed::RadioClientKind + 'static {
    const CLIENT: RadioClient;
}

/// The IEEE 802.11 client.
#[derive(Debug)]
pub enum WifiClient {}

impl sealed::RadioClientKind for WifiClient {}
impl RadioClientKind for WifiClient {
    const CLIENT: RadioClient = RadioClient::Wifi;
}

/// Proof that client `K` holds common radio power.
///
/// Only entering common power creates it, and leaving consumes it, so a
/// client leaves only power it holds, and only outside coexistence
/// ([`crate::radio::domain::CoexAbsent`]). Dropping it keeps the client in
/// common power until a chip reset.
///
/// A client leaves with its own proof:
///
/// ```
/// use oer_chips_esp32s31_hal::radio::domain::{CoexAbsent, CommonPower, RadioDomain, WifiClient};
/// use oer_chips_esp32s31_hal::radio::ieee80211::registers::WifiRadioRegisters;
///
/// fn leave(
///     radio: &mut RadioDomain,
///     wifi: &WifiRadioRegisters,
///     power: CommonPower<WifiClient>,
///     coex: &CoexAbsent<WifiClient>,
/// ) {
///     let _ = radio.exit_common_power(wifi, power, coex);
/// }
/// ```
///
/// And a proof cannot be made without entering power:
///
/// ```compile_fail,E0451
/// use oer_chips_esp32s31_hal::radio::domain::{CommonPower, WifiClient};
///
/// let _power: CommonPower<WifiClient> = CommonPower { _client: core::marker::PhantomData };
/// ```
#[must_use = "common power is left through the client's power_down"]
#[derive(Debug)]
pub struct CommonPower<K: RadioClientKind> {
    _client: core::marker::PhantomData<fn() -> K>,
}

impl<K: RadioClientKind> CommonPower<K> {
    const fn entered() -> Self {
        Self {
            _client: core::marker::PhantomData,
        }
    }

    /// The client that holds power.
    pub const fn client(&self) -> RadioClient {
        K::CLIENT
    }

    /// A proof without entering power, for host tests of the later stages.
    /// It proves no power, so only test builds have it.
    #[cfg(test)]
    pub const fn for_host_test() -> Self {
        Self::entered()
    }
}

/// Leaving common power failed; the client stays entered and keeps its
/// proof for a retry.
#[derive(Debug)]
pub struct CommonPowerExitFailure<K: RadioClientKind> {
    /// The proof of the client, which still holds power.
    pub power: CommonPower<K>,
    /// Why the client could not leave.
    pub error: CommonRadioPowerError,
}

/// Why a client cannot enter or leave the common radio power.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommonRadioPowerError {
    /// The client already holds common power.
    AlreadyEntered,
    /// The first client's power sequence failed a read-back checkpoint.
    Power(oer_chips_family_radio_power::power::PowerError),
    /// The last client's cold-power baseline did not read back.
    Restore(WifiPowerRestoreCheckpoint),
}

/// Common modem/PHY power shared by concurrently running clients.
///
/// The power sequence runs once, for the first client; a later client must
/// not repeat it while another protocol runs. It never resets the Wi-Fi MAC;
/// the retained PHY configuration in that window survives every entry. The
/// cold-power clock baseline is captured before the first edge and restored
/// after the last client leaves; the reset lines are not part of it. Refcounted modem
/// clock dependencies such as coexistence belong to the shared modem clock
/// planner, not to this membership.
pub(crate) struct CommonRadioPower {
    membership: oer_chips_family_radio_power::membership::PowerMembership<WifiPowerBaseline>,
    refs: PlatformClockRefs,
}
impl Default for CommonRadioPower {
    fn default() -> Self {
        Self {
            membership: Default::default(),
            refs: Default::default(),
        }
    }
}

impl CommonRadioPower {
    pub(crate) fn holds(&self, client: RadioClient) -> bool {
        self.membership.holds(client)
    }
    pub(crate) fn any_client(&self) -> bool {
        self.membership.any_client()
    }
    #[cfg(test)]
    pub(crate) fn hold_for_test(&mut self, client: RadioClient) {
        self.membership.hold_for_test(client);
    }
    pub(crate) fn enter<K: RadioClientKind>(
        &mut self,
        port: &mut impl ClockPort,
        platform: &mut (impl PlatformGates + WifiPowerGate),
    ) -> Result<CommonPower<K>, CommonRadioPowerError> {
        // The gate opens at Wi-Fi's acceptance, never on its refusal.
        if K::CLIENT == RadioClient::Wifi && !self.membership.holds(K::CLIENT) {
            platform.open_wifi_power();
        }
        self.membership
            .enter(
                K::CLIENT,
                &mut PowerBinding {
                    port,
                    refs: &mut self.refs,
                    platform,
                },
            )
            .map_err(power_error)?;
        Ok(CommonPower::entered())
    }
    /// Wi-Fi's entry: its power gate opens first, then common power.
    pub(crate) fn enter_wifi(
        &mut self,
        port: &mut impl crate::radio::ieee80211::client::WifiPowerPort,
        platform: &mut (impl PlatformGates + WifiPowerGate),
    ) -> Result<CommonPower<WifiClient>, CommonRadioPowerError> {
        // The gate opens at the entry's acceptance, never on its refusal.
        if !self.membership.holds(RadioClient::Wifi) {
            platform.open_wifi_power();
        }
        self.membership
            .enter_wifi(&mut PowerBinding {
                port,
                refs: &mut self.refs,
                platform,
            })
            .map_err(power_error)?;
        Ok(CommonPower::entered())
    }
    /// Leave common power; Wi-Fi's leave closes its power gate last.
    pub(crate) fn exit<K: RadioClientKind>(
        &mut self,
        port: &mut impl ClockPort,
        platform: &mut (impl PlatformGates + WifiPowerGate),
        power: CommonPower<K>,
    ) -> Result<(), CommonPowerExitFailure<K>> {
        match self.membership.exit(
            K::CLIENT,
            &mut PowerBinding {
                port,
                refs: &mut self.refs,
                platform,
            },
        ) {
            Ok(()) => {
                if K::CLIENT == RadioClient::Wifi {
                    platform.close_wifi_power();
                }
                Ok(())
            }
            Err(error) => Err(CommonPowerExitFailure {
                power,
                error: power_error(error),
            }),
        }
    }
}
struct PowerBinding<'a, H, G> {
    port: &'a mut H,
    refs: &'a mut PlatformClockRefs,
    platform: &'a mut G,
}
impl<H: ClockPort, G: PlatformGates> oer_chips_family_radio_power::membership::PowerPort
    for PowerBinding<'_, H, G>
{
    type Baseline = WifiPowerBaseline;
    type RestoreError = PowerRestoreCheckpoint;
    fn capture(&self) -> Self::Baseline {
        self.port.capture_power_baseline()
    }
    fn restore(&mut self, baseline: Self::Baseline) -> Result<(), Self::RestoreError> {
        self.port.restore_power_baseline(baseline)
    }
    fn select_wifi_low_power_clock(&mut self) {
        self.port.select_wifi_low_power_clock();
    }
    fn release(&mut self, clock: oer_chips_family_radio_power::membership::Clock) {
        self.refs.release(clock, self.platform);
    }
}
impl<H: ClockPort, G: PlatformGates> oer_chips_family_radio_power::membership::PowerEntryPort
    for PowerBinding<'_, H, G>
{
    fn power_up(&mut self) -> Result<(), oer_chips_family_radio_power::power::PowerError> {
        self.port
            .run_common_power_sequence(self.refs, self.platform)
    }
}
impl<H: crate::radio::ieee80211::client::WifiPowerPort, G: PlatformGates>
    oer_chips_family_radio_power::membership::WifiPowerPort for PowerBinding<'_, H, G>
{
    fn reset_wifi(&mut self) {
        self.port.pulse_wifi_baseband_and_mac_reset();
    }
}
fn power_error(
    error: oer_chips_family_radio_power::membership::PowerError<PowerRestoreCheckpoint>,
) -> CommonRadioPowerError {
    use oer_chips_family_radio_power::membership::PowerError;
    match error {
        PowerError::AlreadyEntered => CommonRadioPowerError::AlreadyEntered,
        PowerError::Power(error) => CommonRadioPowerError::Power(error),
        PowerError::Restore(checkpoint) => CommonRadioPowerError::Restore(match checkpoint {
            PowerRestoreCheckpoint::ModemSyscon => WifiPowerRestoreCheckpoint::ModemSyscon,
            PowerRestoreCheckpoint::ModemSourceClocks => {
                WifiPowerRestoreCheckpoint::ModemSourceClocks
            }
            PowerRestoreCheckpoint::ModemRegisterBusClock => {
                WifiPowerRestoreCheckpoint::ModemRegisterBusClock
            }
        }),
    }
}

#[cfg(test)]
mod tests;

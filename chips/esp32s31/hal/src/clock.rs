//! Common radio power, its platform clock references and the reversible
//! cold-power baseline.
//!
//! The restricted PAC exposes straight-line configuration and baseline
//! transactions. This module decides when common power takes and gives back the
//! platform-owned 160 MHz, analog-I2C and MPLL references, which state it restores
//! on release and when the captured cold-power baseline may be committed.
//! Every value here is route-scoped: a route gives back its references and
//! restores the baseline before the neutral root is reconstructed.

use crate::{modem::WifiPowerBaseline, phy::RadioPhyRegisters};

use crate::power::{
    PlatformClock, PlatformClockBackend, PlatformClockError, PlatformClockGuard,
    PlatformClockHolds, PlatformClockProvider,
};
use crate::root::WifiPowerRestoreCheckpoint;
use oer_espressif_radio::baseline::PowerRestoreCheckpoint;

/// Register transactions consumed by route clock policy.
pub(crate) trait ClockPort {
    fn select_wifi_low_power_clock(&mut self);
    fn capture_power_baseline(&self) -> WifiPowerBaseline;
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), PowerRestoreCheckpoint>;
    /// Run the common modem/PHY power sequence, taking the platform clock
    /// references into `refs` at their exact edges.
    fn run_common_power_sequence<B: PlatformClockBackend>(
        &mut self,
        refs: &mut PlatformClockRefs<B>,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(), oer_espressif_radio::power::PowerError>;
}

impl ClockPort for RadioPhyRegisters {
    fn select_wifi_low_power_clock(&mut self) {
        oer_espressif_radio::clocks::select_wifi_low_power_clock(self);
    }
    fn capture_power_baseline(&self) -> WifiPowerBaseline {
        self.capture_wifi_power_baseline()
    }
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), PowerRestoreCheckpoint> {
        oer_espressif_radio::baseline::restore(self, baseline)
    }
    fn run_common_power_sequence<B: PlatformClockBackend>(
        &mut self,
        refs: &mut PlatformClockRefs<B>,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(), oer_espressif_radio::power::PowerError> {
        oer_espressif_radio::power::execute_owned(&mut crate::power::RoutePower {
            phy: self,
            refs,
            platform,
        })
    }
}

/// The platform clock references common radio power holds.
///
/// The 160 MHz reference, the analog-I2C master clock and the MPLL are shared
/// with other SoC users, so their platform owner
/// ([`ChipClocks`](crate::power::ChipClocks)) counts references and alone
/// writes their gates. Common power holds one guard for each for as
/// long as any client holds it; a retried power-up takes only the references
/// it does not hold yet.
#[derive(Debug)]
pub(crate) struct PlatformClockRefs<B: PlatformClockBackend> {
    pll_f160m: Option<PlatformClockGuard<B>>,
    analog_i2c: Option<PlatformClockGuard<B>>,
    mpll: Option<PlatformClockGuard<B>>,
}

impl<B: PlatformClockBackend> Default for PlatformClockRefs<B> {
    fn default() -> Self {
        Self {
            pll_f160m: None,
            analog_i2c: None,
            mpll: None,
        }
    }
}

impl<B: PlatformClockBackend> PlatformClockRefs<B> {
    fn slot(&mut self, clock: PlatformClock) -> &mut Option<PlatformClockGuard<B>> {
        match clock {
            PlatformClock::Pll160m => &mut self.pll_f160m,
            PlatformClock::AnalogI2cMaster => &mut self.analog_i2c,
            PlatformClock::Mpll => &mut self.mpll,
            PlatformClock::ModemCoexistence | PlatformClock::ModemLowPowerTimer => {
                unreachable!("common power holds no modem module gate")
            }
        }
    }

    pub(crate) fn acquire(
        &mut self,
        clock: PlatformClock,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<(), PlatformClockError> {
        let slot = self.slot(clock);
        if slot.is_none() {
            *slot = Some(platform.acquire(clock)?);
        }
        Ok(())
    }

    /// Give back the reference to `clock`, if held.
    ///
    /// # Errors
    ///
    /// The platform refused the release: the guard stays held here.
    fn release(&mut self, clock: PlatformClock) -> Result<(), PlatformClockError> {
        let slot = self.slot(clock);
        if let Some(guard) = slot.take()
            && let Err(refused) = guard.release()
        {
            let error = refused.error();
            *slot = Some(refused.into_guard());
            return Err(error);
        }
        Ok(())
    }

    fn count_into(&self, holds: &mut PlatformClockHolds) {
        for guard in [&self.pll_f160m, &self.analog_i2c, &self.mpll] {
            holds.count(guard.as_ref());
        }
    }

    /// Keep every reference still held on, and forget it.
    #[cfg(test)]
    fn abandon(self) {
        for guard in [self.pll_f160m, self.analog_i2c, self.mpll]
            .into_iter()
            .flatten()
        {
            guard.abandon();
        }
    }
}

/// Protocol clients that can share the powered radio.
pub use oer_radio_coex::RadioClient;

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
/// The Bluetooth LE client.
#[derive(Debug)]
pub enum BluetoothClient {}
/// The IEEE 802.15.4 client.
#[derive(Debug)]
pub enum Ieee802154Client {}

impl sealed::RadioClientKind for WifiClient {}
impl RadioClientKind for WifiClient {
    const CLIENT: RadioClient = RadioClient::Wifi;
}
impl sealed::RadioClientKind for BluetoothClient {}
impl RadioClientKind for BluetoothClient {
    const CLIENT: RadioClient = RadioClient::Bluetooth;
}
impl sealed::RadioClientKind for Ieee802154Client {}
impl RadioClientKind for Ieee802154Client {
    const CLIENT: RadioClient = RadioClient::Ieee802154;
}

/// Proof that client `K` holds common radio power.
///
/// Only entering common power creates it, and leaving consumes it, so a
/// client leaves only power it holds, and only outside coexistence
/// ([`crate::domain::CoexAbsent`]). Dropping it keeps the client in
/// common power until a chip reset.
///
/// A client leaves with its own proof:
///
/// ```
/// use oer_esp32s31_hal::domain::{CoexAbsent, CommonPower, RadioDomain, WifiClient};
/// use oer_esp32s31_hal::ieee80211::registers::WifiRadioRegisters;
///
/// fn leave<B: oer_esp32s31_hal::power::PlatformClockBackend>(
///     radio: &mut RadioDomain<B>,
///     wifi: &WifiRadioRegisters,
///     power: CommonPower<WifiClient>,
///     coex: &CoexAbsent<WifiClient>,
/// ) {
///     let _ = radio.exit_common_power(wifi, power, coex);
/// }
/// ```
///
/// Not with another client's:
///
/// ```compile_fail,E0308
/// use oer_esp32s31_hal::domain::{CoexAbsent, CommonPower, RadioDomain, WifiClient};
/// use oer_esp32s31_hal::ieee802154::mac::Ieee802154TaskOwner;
///
/// fn leave<B: oer_esp32s31_hal::power::PlatformClockBackend>(
///     radio: &mut RadioDomain<B>,
///     ieee802154: &Ieee802154TaskOwner,
///     power: CommonPower<WifiClient>,
///     coex: &CoexAbsent<WifiClient>,
/// ) {
///     let _ = radio.exit_common_power(ieee802154, power, coex);
/// }
/// ```
///
/// And a proof cannot be made without entering power:
///
/// ```compile_fail,E0451
/// use oer_esp32s31_hal::domain::{CommonPower, WifiClient};
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
    Power(oer_espressif_radio::power::PowerError),
    /// The last client's cold-power baseline did not read back.
    Restore(WifiPowerRestoreCheckpoint),
    /// The platform refused to take back the last client's reference to
    /// this clock, which stays held.
    Release(PlatformClock),
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
pub(crate) struct CommonRadioPower<B: PlatformClockBackend> {
    membership: oer_espressif_radio::membership::PowerMembership<WifiPowerBaseline>,
    refs: PlatformClockRefs<B>,
}
impl<B: PlatformClockBackend> Default for CommonRadioPower<B> {
    fn default() -> Self {
        Self {
            membership: Default::default(),
            refs: Default::default(),
        }
    }
}

impl<B: PlatformClockBackend> CommonRadioPower<B> {
    /// End a test's owner, keeping every reference it still holds on.
    #[cfg(test)]
    pub(crate) fn abandon_for_test(mut self) {
        self.abandon_platform_clocks();
    }

    /// Keep every platform reference still held on until a chip reset, and
    /// forget it: common power is fail-stop from here.
    #[cfg(test)]
    pub(crate) fn abandon_platform_clocks(&mut self) {
        core::mem::take(&mut self.refs).abandon();
    }

    /// Take a platform reference as a failed power sequence leaves it.
    #[cfg(test)]
    pub(crate) fn hold_platform_clock_for_test(
        &mut self,
        clock: PlatformClock,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) {
        self.refs
            .acquire(clock, platform)
            .unwrap_or_else(|_| panic!("granted"));
    }
}

impl<B: PlatformClockBackend> CommonRadioPower<B> {
    pub(crate) fn holds(&self, client: RadioClient) -> bool {
        self.membership.holds(client)
    }
    pub(crate) fn any_client(&self) -> bool {
        self.membership.any_client()
    }
    pub(crate) fn count_platform_clocks(&self, holds: &mut PlatformClockHolds) {
        self.refs.count_into(holds);
    }
    #[cfg(test)]
    pub(crate) fn hold_for_test(&mut self, client: RadioClient) {
        self.membership.hold_for_test(client);
    }
    pub(crate) fn enter<K: RadioClientKind>(
        &mut self,
        port: &mut impl ClockPort,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<CommonPower<K>, CommonRadioPowerError> {
        self.membership
            .enter(
                K::CLIENT,
                &mut PowerEntryBinding {
                    binding: PowerBinding {
                        port,
                        refs: &mut self.refs,
                    },
                    platform,
                },
            )
            .map_err(power_error)?;
        Ok(CommonPower::entered())
    }
    pub(crate) fn enter_wifi(
        &mut self,
        port: &mut impl crate::ieee80211::client::WifiPowerPort,
        platform: &impl PlatformClockProvider<Backend = B>,
    ) -> Result<CommonPower<WifiClient>, CommonRadioPowerError> {
        self.membership
            .enter_wifi(&mut PowerEntryBinding {
                binding: PowerBinding {
                    port,
                    refs: &mut self.refs,
                },
                platform,
            })
            .map_err(power_error)?;
        Ok(CommonPower::entered())
    }
    pub(crate) fn exit<K: RadioClientKind>(
        &mut self,
        port: &mut impl ClockPort,
        power: CommonPower<K>,
    ) -> Result<(), CommonPowerExitFailure<K>> {
        // The release binding requires no provider; the exact guards own release.
        self.membership
            .exit(
                K::CLIENT,
                &mut PowerBinding {
                    port,
                    refs: &mut self.refs,
                },
            )
            .map_err(|error| CommonPowerExitFailure {
                power,
                error: power_error(error),
            })
    }
}
struct PowerBinding<'a, H, B: PlatformClockBackend> {
    port: &'a mut H,
    refs: &'a mut PlatformClockRefs<B>,
}
impl<H: ClockPort, B: PlatformClockBackend> oer_espressif_radio::membership::PowerPort
    for PowerBinding<'_, H, B>
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
    fn release(
        &mut self,
        clock: oer_espressif_radio::membership::Clock,
    ) -> Result<(), oer_espressif_radio::membership::ReleaseRefused> {
        self.refs
            .release(platform_clock(clock))
            .map_err(|_| oer_espressif_radio::membership::ReleaseRefused)
    }
}
struct PowerEntryBinding<'a, H, B: PlatformClockBackend, P> {
    binding: PowerBinding<'a, H, B>,
    platform: &'a P,
}
impl<H: ClockPort, B: PlatformClockBackend, P> oer_espressif_radio::membership::PowerPort
    for PowerEntryBinding<'_, H, B, P>
{
    type Baseline = WifiPowerBaseline;
    type RestoreError = PowerRestoreCheckpoint;
    fn capture(&self) -> Self::Baseline {
        self.binding.capture()
    }
    fn restore(&mut self, baseline: Self::Baseline) -> Result<(), Self::RestoreError> {
        self.binding.restore(baseline)
    }
    fn select_wifi_low_power_clock(&mut self) {
        self.binding.select_wifi_low_power_clock();
    }
    fn release(
        &mut self,
        clock: oer_espressif_radio::membership::Clock,
    ) -> Result<(), oer_espressif_radio::membership::ReleaseRefused> {
        self.binding.release(clock)
    }
}
impl<H: ClockPort, B: PlatformClockBackend, P: PlatformClockProvider<Backend = B>>
    oer_espressif_radio::membership::PowerEntryPort for PowerEntryBinding<'_, H, B, P>
{
    fn power_up(&mut self) -> Result<(), oer_espressif_radio::power::PowerError> {
        self.binding
            .port
            .run_common_power_sequence(self.binding.refs, self.platform)
    }
}
impl<
    H: crate::ieee80211::client::WifiPowerPort,
    B: PlatformClockBackend,
    P: PlatformClockProvider<Backend = B>,
> oer_espressif_radio::membership::WifiPowerPort for PowerEntryBinding<'_, H, B, P>
{
    fn reset_wifi(&mut self) {
        self.binding.port.pulse_wifi_baseband_and_mac_reset();
    }
}
fn platform_clock(clock: oer_espressif_radio::membership::Clock) -> PlatformClock {
    use oer_espressif_radio::membership::Clock;
    match clock {
        Clock::AnalogI2c => PlatformClock::AnalogI2cMaster,
        Clock::Pll160m => PlatformClock::Pll160m,
        Clock::Mpll => PlatformClock::Mpll,
    }
}
fn power_error(
    error: oer_espressif_radio::membership::PowerError<PowerRestoreCheckpoint>,
) -> CommonRadioPowerError {
    use oer_espressif_radio::membership::PowerError;
    match error {
        PowerError::AlreadyEntered => CommonRadioPowerError::AlreadyEntered,
        PowerError::Power(error) => CommonRadioPowerError::Power(error),
        PowerError::Release(clock) => CommonRadioPowerError::Release(platform_clock(clock)),
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

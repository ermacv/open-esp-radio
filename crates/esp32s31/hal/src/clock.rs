//! Common radio power, its platform clock references and the reversible
//! cold-power baseline.
//!
//! The restricted PAC exposes straight-line configuration and baseline
//! transactions. This module decides when common power takes and gives back the
//! platform-owned 160 MHz, analog-I2C and MPLL references, which state it restores
//! on release and when the captured cold-power baseline may be committed.
//! Every value here is route-scoped: a route gives back its references and
//! restores the baseline before the neutral root is reconstructed.

use oer_esp32s31_pac::{RadioPhyRegisters, WifiPowerBaseline, WifiPowerRestoreReadback};

use crate::power::{
    PlatformClock, PlatformClockError, PlatformClockGuard, PlatformClockHolds,
    PlatformClockProvider,
};
use crate::root::WifiPowerRestoreCheckpoint;

/// Register transactions consumed by route clock policy.
pub(crate) trait ClockPort {
    fn capture_power_baseline(&self) -> WifiPowerBaseline;
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), WifiPowerRestoreReadback>;
    /// Run the common modem/PHY power sequence, taking the platform clock
    /// references into `refs` at their exact edges.
    fn run_common_power_sequence(
        &mut self,
        refs: &mut PlatformClockRefs,
        platform: &impl PlatformClockProvider,
        entry: crate::power::PowerEntry,
    ) -> Result<(), crate::power::PowerError>;
}

impl ClockPort for RadioPhyRegisters {
    fn capture_power_baseline(&self) -> WifiPowerBaseline {
        self.capture_wifi_power_baseline()
    }
    fn restore_power_baseline(
        &mut self,
        baseline: WifiPowerBaseline,
    ) -> Result<(), WifiPowerRestoreReadback> {
        self.restore_wifi_power_baseline(baseline)
    }
    fn run_common_power_sequence(
        &mut self,
        refs: &mut PlatformClockRefs,
        platform: &impl PlatformClockProvider,
        entry: crate::power::PowerEntry,
    ) -> Result<(), crate::power::PowerError> {
        crate::power::execute_owned(
            &mut crate::power::RoutePower {
                phy: self,
                refs,
                platform,
            },
            entry,
        )
    }
}

/// The platform clock references common radio power holds.
///
/// The 160 MHz reference, the analog-I2C master clock and the MPLL are shared
/// with other SoC users, so their platform owner (ESP-HAL) counts references
/// and alone writes their gates. Common power holds one guard for each for as
/// long as any client holds it; a retried power-up takes only the references
/// it does not hold yet.
#[derive(Debug, Default)]
pub(crate) struct PlatformClockRefs {
    pll_f160m: Option<PlatformClockGuard>,
    analog_i2c: Option<PlatformClockGuard>,
    mpll: Option<PlatformClockGuard>,
}

impl PlatformClockRefs {
    fn slot(&mut self, clock: PlatformClock) -> &mut Option<PlatformClockGuard> {
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
        platform: &impl PlatformClockProvider,
    ) -> Result<(), PlatformClockError> {
        let slot = self.slot(clock);
        if slot.is_none() {
            *slot = Some(platform.acquire(clock)?);
        }
        Ok(())
    }

    fn release(&mut self, clock: PlatformClock) {
        if let Some(guard) = self.slot(clock).take() {
            guard.release();
        }
    }

    fn count_into(&self, holds: &mut PlatformClockHolds) {
        for guard in [&self.pll_f160m, &self.analog_i2c, &self.mpll] {
            holds.count(guard.as_ref());
        }
    }

    /// Keep every reference still held on, and forget it.
    #[cfg(any(test, feature = "validation-probes"))]
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
/// ([`crate::shared_radio::CoexAbsent`]). Dropping it keeps the client in
/// common power until a chip reset.
///
/// A client leaves with its own proof:
///
/// ```
/// use oer_esp32s31_hal::shared_radio::{CoexAbsent, CommonPower, SharedRadioLease, WifiClient};
/// use oer_esp32s31_pac::WifiRadioRegisters;
///
/// fn leave(
///     lease: &mut SharedRadioLease<'_>,
///     wifi: &WifiRadioRegisters,
///     power: CommonPower<WifiClient>,
///     coex: &CoexAbsent<WifiClient>,
/// ) {
///     let _ = lease.exit_common_power(wifi, power, coex);
/// }
/// ```
///
/// Not with another client's:
///
/// ```compile_fail,E0308
/// use oer_esp32s31_hal::shared_radio::{CoexAbsent, CommonPower, SharedRadioLease, WifiClient};
/// use oer_esp32s31_pac::BluetoothTaskRegisters;
///
/// fn leave(
///     lease: &mut SharedRadioLease<'_>,
///     bluetooth: &BluetoothTaskRegisters,
///     power: CommonPower<WifiClient>,
///     coex: &CoexAbsent<WifiClient>,
/// ) {
///     let _ = lease.exit_common_power(bluetooth, power, coex);
/// }
/// ```
///
/// And a proof cannot be made without entering power:
///
/// ```compile_fail,E0451
/// use oer_esp32s31_hal::shared_radio::{CommonPower, WifiClient};
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
    /// It proves no power, so only test and validation builds have it.
    #[cfg(any(test, feature = "validation-probes"))]
    pub const fn for_validation() -> Self {
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

/// The membership bit of `client` in [`CommonRadioPower`].
const fn client_bit(client: RadioClient) -> u8 {
    match client {
        RadioClient::Wifi => 1,
        RadioClient::Bluetooth => 1 << 1,
        RadioClient::Ieee802154 => 1 << 2,
    }
}

/// Why a client cannot enter or leave the common radio power.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommonRadioPowerError {
    /// The client already holds common power.
    AlreadyEntered,
    /// The first client's power sequence failed a read-back checkpoint.
    Power(crate::power::PowerError),
    /// The last client's cold-power baseline did not read back.
    Restore(WifiPowerRestoreCheckpoint),
}

/// Common modem/PHY power shared by concurrently running clients.
///
/// The power sequence runs once, for the first client; a later client must
/// not repeat it while another protocol runs. Only the first power-up since
/// boot pulses the Wi-Fi baseband and MAC resets: after every client left,
/// a new first client runs the sequence without that pulse, so the retained
/// PHY configuration in the Wi-Fi MAC register window survives. The
/// cold-power clock baseline is captured before the first edge and restored
/// after the last client leaves; the reset lines are not part of it. Refcounted modem
/// clock dependencies such as coexistence belong to the shared modem clock
/// planner, not to this membership.
#[derive(Default)]
pub(crate) struct CommonRadioPower {
    clients: u8,
    refs: PlatformClockRefs,
    power: PowerEpoch,
    /// The Wi-Fi baseband and MAC resets were pulsed by a successful
    /// power-up since boot.
    wifi_resets_pulsed: bool,
}

impl CommonRadioPower {
    /// End a test's owner, keeping every reference it still holds on.
    #[cfg(test)]
    pub(crate) fn abandon_for_test(mut self) {
        self.abandon_platform_clocks();
    }

    /// Keep every platform reference still held on until a chip reset, and
    /// forget it: common power is fail-stop from here.
    #[cfg(any(test, feature = "validation-probes"))]
    pub(crate) fn abandon_platform_clocks(&mut self) {
        core::mem::take(&mut self.refs).abandon();
    }

    /// Take a platform reference as a failed power sequence leaves it.
    #[cfg(test)]
    pub(crate) fn hold_platform_clock_for_test(
        &mut self,
        clock: PlatformClock,
        platform: &impl PlatformClockProvider,
    ) {
        self.refs
            .acquire(clock, platform)
            .unwrap_or_else(|_| panic!("granted"));
    }
}

impl CommonRadioPower {
    /// Whether `client` currently holds common power.
    pub(crate) const fn holds(&self, client: RadioClient) -> bool {
        self.clients & client_bit(client) != 0
    }

    /// Whether any client holds common power.
    pub(crate) const fn any_client(&self) -> bool {
        self.clients != 0
    }

    /// Count the platform references common power holds into `holds`.
    pub(crate) fn count_platform_clocks(&self, holds: &mut PlatformClockHolds) {
        self.refs.count_into(holds);
    }

    #[cfg(test)]
    pub(crate) fn hold_for_test(&mut self, client: RadioClient) {
        self.clients |= client_bit(client);
    }

    /// Enter common power; only the first client runs the power sequence.
    ///
    /// A failed sequence admits no client. It keeps the original baseline
    /// and any platform reference already taken, so a retry restarts from
    /// the same cold state without taking a reference twice.
    pub(crate) fn enter<K: RadioClientKind>(
        &mut self,
        port: &mut impl ClockPort,
        platform: &impl PlatformClockProvider,
    ) -> Result<CommonPower<K>, CommonRadioPowerError> {
        let client = K::CLIENT;
        if self.holds(client) {
            return Err(CommonRadioPowerError::AlreadyEntered);
        }
        if self.clients == 0 {
            self.power.prepare(port);
            let entry = if self.wifi_resets_pulsed {
                crate::power::PowerEntry::Repeated
            } else {
                crate::power::PowerEntry::FirstSinceBoot
            };
            port.run_common_power_sequence(&mut self.refs, platform, entry)
                .map_err(CommonRadioPowerError::Power)?;
            self.wifi_resets_pulsed = true;
        }
        self.clients |= client_bit(client);
        Ok(CommonPower::entered())
    }

    /// Leave common power; the last client gives back the analog-I2C
    /// reference, restores the cold baseline and then gives back the 160 MHz
    /// and MPLL references, as the vendor closes a gate before releasing its source.
    ///
    /// A failed step keeps the client entered and returns its proof so the
    /// exit can be retried.
    pub(crate) fn exit<K: RadioClientKind>(
        &mut self,
        port: &mut impl ClockPort,
        power: CommonPower<K>,
    ) -> Result<(), CommonPowerExitFailure<K>> {
        let client = power.client();
        if self.clients == client_bit(client) {
            self.refs.release(PlatformClock::AnalogI2cMaster);
            if let Err(checkpoint) = self.power.restore(port) {
                return Err(CommonPowerExitFailure {
                    power,
                    error: CommonRadioPowerError::Restore(checkpoint),
                });
            }
            self.refs.release(PlatformClock::Pll160m);
            self.refs.release(PlatformClock::Mpll);
        }
        self.clients &= !client_bit(client);
        Ok(())
    }
}

/// Cold-power baseline captured before the first route power edge.
#[derive(Default)]
pub(crate) struct PowerEpoch {
    baseline: Option<WifiPowerBaseline>,
}

impl PowerEpoch {
    /// Capture the reversible baseline before the first mutation.
    ///
    /// A retry of the same partially executed power-up keeps the original
    /// baseline instead of sampling its own mutations as a new cold state.
    pub(crate) fn prepare(&mut self, port: &impl ClockPort) {
        if self.baseline.is_none() {
            self.baseline = Some(port.capture_power_baseline());
        }
    }

    /// Restore every non-monotonic field changed by the cold-power path.
    ///
    /// The analog-I2C reference must be released first. The baseline is committed
    /// only after every readback matches.
    pub(crate) fn restore(
        &mut self,
        port: &mut impl ClockPort,
    ) -> Result<(), WifiPowerRestoreCheckpoint> {
        let Some(baseline) = self.baseline else {
            return Ok(());
        };
        port.restore_power_baseline(baseline)
            .map_err(|readback| match readback {
                WifiPowerRestoreReadback::ModemSyscon => WifiPowerRestoreCheckpoint::ModemSyscon,
                WifiPowerRestoreReadback::ModemSourceClocks => {
                    WifiPowerRestoreCheckpoint::ModemSourceClocks
                }
                WifiPowerRestoreReadback::ModemRegisterBusClock => {
                    WifiPowerRestoreCheckpoint::ModemRegisterBusClock
                }
            })?;
        self.baseline = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

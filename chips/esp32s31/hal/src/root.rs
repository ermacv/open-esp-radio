//! Protocol-neutral radio root and its split.
//!
//! The restricted PAC supplies opaque register partitions and register sets
//! without any route policy. This module owns the complete neutral root.
//! [`RadioHardware::split`] hands every protocol partition out at once and
//! places the shared partitions in one [`RadioDomain`];
//! [`RadioHardware::reunite`] reunites them only after the shared restore
//! obligations are complete.

/// The read-only Wi-Fi MAC local-time capability a [`WifiPartition`]
/// lends.
pub use oer_esp32s31_pac::MacLocalTime;
/// The platform's proof that it consumed every radio singleton, which
/// [`RadioHardware::from_handoff`] takes.
pub use oer_esp32s31_pac::RadioHandoff;
/// The bootstrap's owner of the PMU's radio view, its handoff and the proof
/// of its release, which [`RadioHardware::from_handoff`] takes.
pub use oer_esp32s31_pac::pll_power::{
    BootstrapPllPower, PllPower, PllPowerHandoff, PllPowerReleased,
};

use crate::power::PlatformClockBackend;
use oer_esp32s31_pac::{
    BluetoothControllerPartition, BluetoothInterruptPeripherals, BluetoothModemLpTimerPartition,
    Ieee802154Partition, MacInterruptSetup, RadioPartitions, SharedRadioParts,
    SharedRadioRegisters, WifiMacPartition, WifiRxDmaRegisters,
};

use crate::{
    domain::{
        BluetoothClient, CoexAbsent, Ieee802154Client, RadioDomain, RadioDomainReleaseError,
        WifiClient,
    },
    phy::{registration::PhyRegistration, restore::PhyRouteState},
};

/// Proof that its holder owns the chip's analog-I2C master (regi2c) for the
/// rest of the program, as [`RadioHardware::from_handoff`] requires.
///
/// The platform that owns the master hands it out once: on ESP-HAL,
/// `oer-espressif-esp-hal-radio`'s `take_radio`, which consumes the
/// `I2C_ANA_MST` singleton. Without it there is no radio root:
///
/// ```compile_fail,E0061
/// use oer_esp32s31_hal::root::{PllPowerReleased, RadioHandoff, RadioHardware};
///
/// fn root(handoff: RadioHandoff, pll_power: PllPowerReleased) -> RadioHardware {
///     RadioHardware::from_handoff(handoff, pll_power)
/// }
/// ```
#[derive(Debug)]
#[must_use = "the radio root needs the analog bus ownership"]
pub struct AnalogBusOwnership(());

impl AnalogBusOwnership {
    /// Assert ownership of the analog-I2C master.
    ///
    /// # Safety
    ///
    /// Call at most once per program. The caller owns the analog-I2C master
    /// exclusively from now on and never releases it: no other code writes
    /// analog registers.
    #[allow(
        unsafe_code,
        reason = "exclusive ownership of the analog bus is a platform guarantee the type cannot check"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// Unique protocol-neutral owner of every reviewed ESP32-S31 radio region.
///
/// This is the sole production acquisition root. It can be consumed by
/// exactly one concurrent split, which cannot manufacture a second owner. The
/// split returns the complete root after every partition and the domain
/// have been reunited. Protocol-specific cached state is
/// deliberately not retained in this neutral owner.
///
/// ```compile_fail
/// use oer_esp32s31_hal::root::RadioHardware;
///
/// # fn split_twice<B: oer_esp32s31_hal::power::PlatformClockBackend>(hardware: RadioHardware) {
/// let _first = hardware.split::<B, _>(());
/// let _second = hardware.split::<B, _>(());
/// # }
/// ```
#[must_use = "dropping the radio root permanently loses the unique hardware capability"]
pub struct RadioHardware {
    partitions: RadioPartitions,
    phy_registration: PhyRegistration,
}

impl RadioHardware {
    /// Reassemble the neutral root from a returning route.
    ///
    /// Leaving the route retires the current PHY registration, so a
    /// registration result held apart from its hardware cannot describe a
    /// later route.
    fn returned(partitions: RadioPartitions, phy: PhyRouteState) -> Self {
        Self {
            partitions,
            phy_registration: phy.into_registration(),
        }
    }

    /// Adopt the radio registers from the platform handoff.
    ///
    /// `analog_bus` proves the caller owns the analog-I2C master: the PHY
    /// writes analog registers through transactions split across executor
    /// polls, which no other writer may interleave. `pll_power` proves the
    /// bootstrap's owner of the PMU's radio view was consumed.
    pub fn from_handoff(
        handoff: RadioHandoff,
        _analog_bus: AnalogBusOwnership,
        pll_power: PllPowerReleased,
    ) -> Self {
        Self {
            partitions: RadioPartitions::from_handoff(handoff, pll_power),
            phy_registration: PhyRegistration::new(),
        }
    }

    /// The complete root of an isolated probe image (`cfg(oer_probe)`).
    #[cfg(oer_probe)]
    #[doc(hidden)]
    pub fn for_probe() -> Self {
        Self {
            partitions: RadioPartitions::for_probe(),
            phy_registration: PhyRegistration::new(),
        }
    }

    /// Construct a fictitious complete root for one host test.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn for_host_test() -> Self {
        Self {
            partitions: RadioPartitions::for_host_test(),
            phy_registration: PhyRegistration::new(),
        }
    }
}

/// Wi-Fi MAC partition and its interrupt setup, taken from a concurrent split.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiPartition {
    mac: WifiMacPartition,
    rx_dma: WifiRxDmaRegisters,
    interrupts: MacInterruptSetup,
}

impl WifiPartition {
    /// The Wi-Fi MAC local-time counter: the clock of receive timestamps.
    pub const fn mac_local_time(&self) -> MacLocalTime {
        self.mac.mac_local_time()
    }

    pub(crate) fn into_parts(self) -> (WifiMacPartition, WifiRxDmaRegisters, MacInterruptSetup) {
        (self.mac, self.rx_dma, self.interrupts)
    }

    pub(crate) const fn from_parts(
        mac: WifiMacPartition,
        rx_dma: WifiRxDmaRegisters,
        interrupts: MacInterruptSetup,
    ) -> Self {
        Self {
            mac,
            rx_dma,
            interrupts,
        }
    }
}

/// Bluetooth controller, modem low-power timer and interrupt partitions,
/// taken from a concurrent split.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct BluetoothPartition {
    controller: BluetoothControllerPartition,
    modem_lp_timer: BluetoothModemLpTimerPartition,
    interrupts: BluetoothInterruptPeripherals,
}

impl BluetoothPartition {
    pub(crate) fn into_parts(
        self,
    ) -> (
        BluetoothControllerPartition,
        BluetoothModemLpTimerPartition,
        BluetoothInterruptPeripherals,
    ) {
        (self.controller, self.modem_lp_timer, self.interrupts)
    }

    pub(crate) const fn from_parts(
        controller: BluetoothControllerPartition,
        modem_lp_timer: BluetoothModemLpTimerPartition,
        interrupts: BluetoothInterruptPeripherals,
    ) -> Self {
        Self {
            controller,
            modem_lp_timer,
            interrupts,
        }
    }
}

/// IEEE 802.15.4 MAC partition, taken from a concurrent split.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct Ieee802154RadioPartition {
    mac: Ieee802154Partition,
}

impl Ieee802154RadioPartition {
    pub(crate) fn into_mac(self) -> Ieee802154Partition {
        self.mac
    }

    pub(crate) const fn from_mac(mac: Ieee802154Partition) -> Self {
        Self { mac }
    }
}

/// Protocol partitions of a split radio root.
///
/// Each field can move to its own protocol driver; the shared radio stays in
/// the [`RadioDomain`] returned beside it.
/// Each protocol also receives its coexistence state, outside coexistence;
/// it returns with the partition when the split reunites.
pub struct ProtocolPartitions {
    pub wifi: WifiPartition,
    pub wifi_coex: CoexAbsent<WifiClient>,
    pub bluetooth: BluetoothPartition,
    pub bluetooth_coex: CoexAbsent<BluetoothClient>,
    pub ieee802154: Ieee802154RadioPartition,
    pub ieee802154_coex: CoexAbsent<Ieee802154Client>,
}

/// Why a split cannot become the neutral root again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReunionError {
    Shared(RadioDomainReleaseError),
}

/// Rejected reunion returning both unchanged owners.
#[must_use = "a rejected reunion still owns the domain and every partition"]
pub struct ReunionFailure<B: PlatformClockBackend, T = ()> {
    shared: RadioDomain<B, T>,
    partitions: ProtocolPartitions,
    error: ReunionError,
}

impl<B: PlatformClockBackend, T> ReunionFailure<B, T> {
    pub const fn error(&self) -> ReunionError {
        self.error
    }

    pub fn into_parts(self) -> (RadioDomain<B, T>, ProtocolPartitions) {
        (self.shared, self.partitions)
    }
}

impl RadioHardware {
    /// Split the root for concurrently running protocols. This performs no
    /// MMIO.
    ///
    /// The shared radio partitions and the shared PHY state go to the
    /// returned [`RadioDomain`], and every protocol partition to
    /// [`ProtocolPartitions`]. The two are separate owners, so a protocol
    /// driver takes its partition while the radio system owns the domain.
    ///
    /// `B` is the platform clock backend shared by every provider used with
    /// this domain.
    ///
    /// `attachment` is the upper layer's state kept with the domain, such
    /// as the PHY layer's registered domain slot.
    pub fn split<B: PlatformClockBackend, T>(
        self,
        attachment: T,
    ) -> (RadioDomain<B, T>, ProtocolPartitions) {
        let RadioPartitions {
            wifi_mac,
            wifi_rx_dma,
            wifi_interrupts,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_modem_lp_timer,
            bluetooth_interrupts,
            shared_radio,
            ieee802154,
        } = self.partitions;
        (
            RadioDomain::new(
                SharedRadioRegisters::new(SharedRadioParts {
                    radio_phy,
                    coexistence,
                    shared_radio,
                }),
                PhyRouteState::new(self.phy_registration),
                attachment,
            ),
            ProtocolPartitions {
                wifi: WifiPartition {
                    mac: wifi_mac,
                    rx_dma: wifi_rx_dma,
                    interrupts: wifi_interrupts,
                },
                bluetooth: BluetoothPartition {
                    controller: bluetooth,
                    modem_lp_timer: bluetooth_modem_lp_timer,
                    interrupts: bluetooth_interrupts,
                },
                ieee802154: Ieee802154RadioPartition { mac: ieee802154 },
                wifi_coex: CoexAbsent::issued(),
                bluetooth_coex: CoexAbsent::issued(),
                ieee802154_coex: CoexAbsent::issued(),
            },
        )
    }

    /// Reunite a split into the neutral root. This performs no MMIO.
    ///
    /// Leaving retires the PHY registration, as every route return does.
    ///
    /// # Errors
    ///
    /// Returns both owners while a client holds common power or a PHY
    /// calibration still owns a restore obligation.
    #[allow(
        clippy::result_large_err,
        reason = "rejection returns the domain and every partition without allocation"
    )]
    pub fn reunite<B: PlatformClockBackend, T>(
        shared: RadioDomain<B, T>,
        partitions: ProtocolPartitions,
    ) -> Result<(Self, T), ReunionFailure<B, T>> {
        let (registers, phy, attachment) = match shared.into_parts() {
            Ok(parts) => parts,
            Err((shared, error)) => {
                return Err(ReunionFailure {
                    shared,
                    partitions,
                    error: ReunionError::Shared(error),
                });
            }
        };
        let SharedRadioParts {
            radio_phy,
            coexistence,
            shared_radio,
        } = registers.into_parts();
        let ProtocolPartitions {
            wifi,
            wifi_coex: _,
            bluetooth,
            bluetooth_coex: _,
            ieee802154,
            ieee802154_coex: _,
        } = partitions;
        Ok((
            Self::returned(
                RadioPartitions {
                    wifi_mac: wifi.mac,
                    wifi_rx_dma: wifi.rx_dma,
                    wifi_interrupts: wifi.interrupts,
                    radio_phy,
                    coexistence,
                    bluetooth: bluetooth.controller,
                    bluetooth_modem_lp_timer: bluetooth.modem_lp_timer,
                    bluetooth_interrupts: bluetooth.interrupts,
                    shared_radio,
                    ieee802154: ieee802154.mac,
                },
                phy,
            ),
            attachment,
        ))
    }
}

/// Why a cold protocol route cannot release the neutral radio root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioPhyReleaseError {
    /// TX-DC PWDET still owns fields that must be restored in this route.
    TxDcPwdetRestorePending,
    /// TX-IQ still owns a tone-control image that must be restored.
    TxIqToneControlRestorePending,
    /// RX-DCO still owns one or both nested control-field snapshots.
    RxDcoControlRestorePending,
    /// Bluetooth TX-power calibration still owns analog-control snapshots.
    BluetoothTxPowerControlRestorePending,
    /// The route still owns TSENS/SAR-I2C, or an acquisition failed mid-reset.
    TemperatureResourcesHeld,
    /// A route-owned cold-power field did not return to its captured baseline.
    WifiPowerRestore(WifiPowerRestoreCheckpoint),
}

/// Exact stage whose route-owned cold-power baseline could not be restored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiPowerRestoreCheckpoint {
    /// Modem syscon clocks and resets did not read back.
    ModemSyscon,
    /// The upstream PLL-source fields did not read back.
    ModemSourceClocks,
    /// The modem register bus clock did not read back.
    ModemRegisterBusClock,
}

/// Reject release while a PHY calibration still owns a restore obligation.
pub(crate) fn check_phy_restore_complete(
    restore: &PhyRouteState,
) -> Result<(), RadioPhyReleaseError> {
    if restore.txdc_pending() {
        return Err(RadioPhyReleaseError::TxDcPwdetRestorePending);
    }
    if restore.txiq_pending() {
        return Err(RadioPhyReleaseError::TxIqToneControlRestorePending);
    }
    if restore.rx_dco_pending() {
        return Err(RadioPhyReleaseError::RxDcoControlRestorePending);
    }
    if restore.bluetooth_tx_power_control_pending() {
        return Err(RadioPhyReleaseError::BluetoothTxPowerControlRestorePending);
    }
    if restore.temperature_blocks_release() {
        return Err(RadioPhyReleaseError::TemperatureResourcesHeld);
    }
    Ok(())
}

#[cfg(test)]
mod tests;

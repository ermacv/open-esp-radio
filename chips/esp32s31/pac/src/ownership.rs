//! Hardware ownership, capabilities, and affine lifecycle transitions.

use super::*;
use crate::generated::{self, peripheral_ownership::Claims};

/// Reviewed Bluetooth low-power timer divider for the main crystal source.
pub const BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER: ModemLowPowerClockDivider =
    match ModemLowPowerClockDivider::new(399) {
        Some(divider) => divider,
        None => panic!("reviewed Bluetooth low-power divider exceeds its PAC field"),
    };

/// Physical owners used by one IEEE 802.15.4 task register set.
///
/// The MAC, its ETM channels and the read of the BTBB receive information
/// are the whole IEEE 802.15.4 authority; its interrupt route words are the
/// interrupt matrix's. Clocks, resets, the common PHY and the rest of the
/// shared BTBB baseband it depends on belong to the shared radio partition.
pub(crate) struct Ieee802154TaskPeripheralOwners {
    pub(crate) ieee802154_mac: crate::ieee802154::ownership::TaskRegisters,
    pub(crate) etm: crate::modem::etm::Ieee802154EtmChannels,
    pub(crate) rx_info: crate::ieee802154::baseband::Ieee802154BasebandRxInfo,
}

/// Unique restricted owner of the shared radio-PHY register partition.
///
/// The type exposes only reviewed, named PHY transactions. It contains no
/// Wi-Fi MAC, Bluetooth controller, coexistence, or shared-baseband owner.
/// The HAL decides which protocol route currently holds it.
#[must_use = "the shared PHY owner must remain inside its active radio route"]
pub struct RadioPhyRegisters {
    /// The partition's generated blocks.
    pub peripherals: generated::peripheral_ownership::RadioPhyPeripherals,
    /// The PHY's copy of the MAC local-time read, for its SDM deadline.
    pub(crate) mac_local_time: generated::wifi_mac_local_time::WifiMacLocalTimeCounter,
}

/// Unique owner of every radio register partition shared by more than one
/// protocol: the radio PHY, coexistence arbitration and shared baseband.
///
/// Protocol register sets contain only their own partitions. A transaction
/// that touches both protocol and shared registers borrows this owner
/// explicitly, so the HAL decides whether one exclusive route holds it or an
/// arbiter serializes it between concurrently running routes.
#[must_use = "the shared radio owner must remain inside its radio domain"]
pub struct SharedRadioRegisters {
    pub(crate) radio_phy: RadioPhyRegisters,
    pub(crate) coexistence: generated::peripheral_ownership::CoexistencePeripherals,
    pub(crate) shared_radio: SharedRadioBlocks,
}

/// The shared radio partition's blocks, with the shared baseband less the
/// receive-information read IEEE 802.15.4 holds
/// ([`crate::ieee802154::baseband`]).
pub(crate) struct SharedRadioBlocks {
    pub(crate) bt_v3_2_baseband: generated::bt_v3_2_baseband::BtV3_2BasebandRest,
    pub(crate) shared_radio_init_control:
        generated::shared_radio_init_control::SharedRadioInitControl,
    #[expect(
        dead_code,
        reason = "the partition holds the shared reset word; no reviewed operation reaches it"
    )]
    pub(crate) shared_baseband_reset_idle_status:
        generated::shared_baseband_reset_idle_status::SharedBasebandResetIdleStatus,
    pub(crate) shared_baseband_tx_timing:
        generated::shared_baseband_tx_timing::SharedBasebandTxTiming,
    pub(crate) zbbb_radio_control: generated::zbbb_radio_control::ZbbbRadioControl,
}

/// Partitions consumed by the shared radio owner.
pub struct SharedRadioParts {
    pub radio_phy: RadioPhyRegisters,
    pub coexistence: CoexistencePartition,
    pub shared_radio: SharedRadioPartition,
}

impl SharedRadioRegisters {
    /// Assemble the shared radio owner. This performs no MMIO.
    pub fn new(parts: SharedRadioParts) -> Self {
        let SharedRadioParts {
            radio_phy,
            coexistence: CoexistencePartition(coexistence),
            shared_radio: SharedRadioPartition(shared_radio),
        } = parts;
        Self {
            radio_phy,
            coexistence,
            shared_radio,
        }
    }

    /// Return the partitions. This performs no MMIO.
    pub fn into_parts(self) -> SharedRadioParts {
        SharedRadioParts {
            radio_phy: self.radio_phy,
            coexistence: CoexistencePartition(self.coexistence),
            shared_radio: SharedRadioPartition(self.shared_radio),
        }
    }

    /// Program the coexistence timer clock as `coex_hw_timer_freq_set` does.
    #[doc(hidden)]
    pub fn configure_coexistence_timer_clock(
        &mut self,
        source: crate::CoexTimerClockSource,
        divider_minus_one: crate::CoexTimerClockDividerMinusOne,
    ) {
        self.radio_phy
            .configure_coexistence_timer_clock(source, divider_minus_one);
    }

    /// Preserve the vendor two-read coexistence clock sampling rule.
    #[doc(hidden)]
    pub fn sample_coexistence_low_power_clock(
        &self,
    ) -> Option<CoexistenceLowPowerClockObservation> {
        self.radio_phy.sample_coexistence_low_power_clock()
    }

    /// Borrow the shared PHY component without exposing another owner.
    #[doc(hidden)]
    pub const fn radio_phy(&self) -> &RadioPhyRegisters {
        &self.radio_phy
    }

    /// Mutably borrow the shared PHY component without exposing another owner.
    #[doc(hidden)]
    pub fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
        &mut self.radio_phy
    }

    /// The shared radio-initialization control word, whose low field the
    /// Wi-Fi MAC's `mac_txrx_init` suffix also writes.
    #[doc(hidden)]
    pub fn shared_radio_init_control_mut(
        &mut self,
    ) -> &mut generated::shared_radio_init_control::SharedRadioInitControl {
        &mut self.shared_radio.shared_radio_init_control
    }
}

/// The Wi-Fi MAC register partition: the generated blocks its HAL owns, with
/// a copy of the read-only MAC local time.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiMacPartition {
    /// The Wi-Fi MAC blocks.
    pub blocks: generated::peripheral_ownership::WifiMacPeripherals,
    /// The receive-interface address pairs, the shared library's typed block.
    pub interface: generated::wifi_mac_interface_address::WifiMacInterfaceAddress,
    /// The read of the free-running MAC local-time counter, which the radio
    /// PHY holds a copy of.
    pub local_time: generated::wifi_mac_local_time::WifiMacLocalTimeCounter,
}

/// The Wi-Fi interrupt banks, moved from cold setup to the hard interrupt
/// handlers.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiInterruptPartition {
    /// The MAC interrupt bank.
    pub mac: generated::wifi_mac_interrupt::WifiMacInterrupt,
    /// The MAC power (`WDEVPWR`) interrupt bank.
    pub power: generated::wifi_mac_power_interrupt::WifiMacPowerInterrupt,
}

/// Opaque coexistence register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct CoexistencePartition(generated::peripheral_ownership::CoexistencePeripherals);

/// Opaque Bluetooth controller register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct BluetoothControllerPartition {
    bluetooth: generated::peripheral_ownership::BluetoothControllerPeripherals,
    etm: crate::modem::etm::BluetoothPhyEtmChannel,
}

/// Opaque shared-baseband register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct SharedRadioPartition(SharedRadioBlocks);

/// Opaque IEEE 802.15.4 MAC register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct Ieee802154Partition {
    peripherals: generated::peripheral_ownership::Ieee802154Peripherals,
    etm: crate::modem::etm::Ieee802154EtmChannels,
    rx_info: crate::ieee802154::baseband::Ieee802154BasebandRxInfo,
}

/// Proof that the platform transferred every ESP32-S31 radio singleton.
#[must_use = "the radio root needs the consumed platform ownership"]
pub struct RadioHandoff(());

impl RadioHandoff {
    /// Assert the platform's unique, permanent radio ownership handoff.
    ///
    /// # Safety
    /// Call at most once. The caller consumed the platform's analog-I2C,
    /// modem clock, Wi-Fi, Bluetooth and IEEE 802.15.4 singletons, and no
    /// other code may access their radio register partitions thereafter.
    #[allow(
        unsafe_code,
        reason = "the platform proves exclusive radio MMIO ownership"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// Every reviewed ESP32-S31 radio register partition, from the platform
/// handoff.
///
/// This is the sole production acquisition point. The partitions carry no
/// protocol route or lifecycle policy: the HAL composes them into register
/// sets and returns them here only as an ownership bundle.
///
/// ```compile_fail,E0061
/// use oer_esp32s31_pac::RadioPartitions;
/// let partitions = RadioPartitions::from_handoff();
/// ```
///
/// ```compile_fail,E0061
/// use oer_esp32s31_pac::{RadioHandoff, RadioPartitions};
/// fn without_released_pmu(handoff: RadioHandoff) {
///     let partitions = RadioPartitions::from_handoff(handoff);
/// }
/// ```
///
/// ```compile_fail,E0382
/// use oer_esp32s31_pac::{RadioHandoff, RadioPartitions, pll_power::PllPowerReleased};
/// fn duplicate(handoff: RadioHandoff, first: PllPowerReleased, second: PllPowerReleased) {
///     let first = RadioPartitions::from_handoff(handoff, first);
///     let second = RadioPartitions::from_handoff(handoff, second);
/// }
/// ```
///
/// ```compile_fail,E0382
/// use oer_esp32s31_pac::{RadioHandoff, RadioPartitions, pll_power::PllPowerReleased};
/// fn duplicate(handoff: RadioHandoff, pmu: PllPowerReleased) {
///     let partitions = RadioPartitions::from_handoff(handoff, pmu);
///     let _first = partitions.wifi_mac;
///     let _second = partitions.wifi_mac;
/// }
/// ```
#[must_use = "dropping the radio partitions permanently loses the unique hardware capability"]
pub struct RadioPartitions {
    pub wifi_mac: WifiMacPartition,
    /// The Wi-Fi MAC RX descriptor walker's registers, owned by the HAL's RX
    /// DMA engine alone.
    pub wifi_rx_dma: generated::wifi_mac_rx_dma::WifiMacRxDma,
    pub wifi_interrupts: WifiInterruptPartition,
    pub radio_phy: RadioPhyRegisters,
    pub coexistence: CoexistencePartition,
    pub bluetooth: BluetoothControllerPartition,
    pub bluetooth_modem_lp_timer: BluetoothModemLpTimerRegisters,
    pub bluetooth_interrupts: BluetoothInterruptSetup,
    pub shared_radio: SharedRadioPartition,
    pub ieee802154: Ieee802154Partition,
}

impl RadioPartitions {
    /// Adopt the platform's consumed radio owners without a second
    /// singleton, and claim the typed blocks once. `pll_power` proves the
    /// bootstrap's owner of the PMU's radio view was consumed.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(
        _handoff: RadioHandoff,
        _pll_power: crate::pll_power::PllPowerReleased,
    ) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the radio root claims the typed blocks once, from the handoff"
        )]
        // SAFETY: `RadioHandoff`'s constructor guarantees exclusive, permanent
        // ownership of every radio register partition, and it is consumed
        // here, so this root is built once; the consumed `PllPowerReleased`
        // proves the bootstrap's owner of the PMU's radio view no longer
        // exists: no other owner of the typed blocks' words exists. The
        // generated `const` assertion beside `Claims` checks that no two
        // typed blocks list the same word; words esp-hal also writes are
        // reviewed with their coordination in `registers/shared-words.toml`.
        let claims = unsafe { oer_mmio::assume_owned::<Claims>() };
        Self::from_claims(claims)
    }

    /// Bind the claimed typed blocks to the partition owners.
    pub(crate) fn from_claims(claims: Claims) -> Self {
        let generated::peripheral_ownership::PeripheralPartitions {
            wifi_mac,
            wifi_mac_interface,
            wifi_rx_dma,
            wifi_interrupts,
            mac_local_time,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_modem_lp_timer,
            bluetooth_interrupts,
            shared_radio,
            modem_etm,
            ieee802154,
        } = generated::peripheral_ownership::partition(claims);
        let mac_local_time = mac_local_time.wifi_mac_local_time.into_counter();
        let (ieee802154_etm, bluetooth_phy_etm, bluetooth_etm) =
            crate::modem::etm::split(modem_etm);
        let generated::peripheral_ownership::SharedRadioPeripherals {
            bt_v3_2_baseband,
            shared_radio_init_control,
            shared_baseband_reset_idle_status,
            shared_baseband_tx_timing,
            zbbb_radio_control,
        } = shared_radio;
        let (bt_v3_2_baseband, ieee802154_rx_info) =
            crate::ieee802154::baseband::split(bt_v3_2_baseband);
        let shared_radio = SharedRadioBlocks {
            bt_v3_2_baseband,
            shared_radio_init_control,
            shared_baseband_reset_idle_status,
            shared_baseband_tx_timing,
            zbbb_radio_control,
        };
        Self {
            wifi_mac: WifiMacPartition {
                blocks: wifi_mac,
                interface: wifi_mac_interface.wifi_mac_interface_address,
                local_time: mac_local_time,
            },
            wifi_rx_dma: wifi_rx_dma.wifi_mac_rx_dma,
            wifi_interrupts: WifiInterruptPartition {
                mac: wifi_interrupts.wifi_mac_interrupt,
                power: wifi_interrupts.wifi_mac_power_interrupt,
            },
            radio_phy: RadioPhyRegisters {
                peripherals: radio_phy,
                mac_local_time,
            },
            coexistence: CoexistencePartition(coexistence),
            bluetooth: BluetoothControllerPartition {
                bluetooth,
                etm: bluetooth_phy_etm,
            },
            bluetooth_modem_lp_timer: BluetoothModemLpTimerRegisters::new(
                bluetooth_modem_lp_timer,
                bluetooth_etm,
            ),
            bluetooth_interrupts: BluetoothInterruptSetup {
                peripherals: bluetooth_interrupts,
            },
            shared_radio: SharedRadioPartition(shared_radio),
            ieee802154: Ieee802154Partition {
                peripherals: ieee802154,
                etm: ieee802154_etm,
                rx_info: ieee802154_rx_info,
            },
        }
    }

    /// Construct every partition as a fictitious root for one host test.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "a host-test root is fictitious; each test needs its own fresh singleton"
    )]
    pub fn for_host_test() -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "a host-test root claims the typed blocks of its fictitious root"
        )]
        // SAFETY: only host tests build this root. It is fictitious: no real
        // MMIO exists behind it and no test accesses one, so the claim of the
        // typed blocks need not be unique.
        let claims = unsafe { oer_mmio::assume_owned::<Claims>() };
        Self::from_claims(claims)
    }

    /// Construct every partition in an isolated probe image.
    #[cfg(oer_probe)]
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "a probe image runs one closed comparison and no runtime driver"
    )]
    pub fn for_probe() -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "a probe root claims the typed blocks once"
        )]
        // SAFETY: a probe image (`cfg(oer_probe)`, built by `cargo fw` from
        // `firmware/probes/` alone) contains one closed comparison and
        // no runtime driver, so no other owner of these registers runs; it
        // claims the typed blocks once.
        let claims = unsafe { oer_mmio::assume_owned::<Claims>() };
        Self::from_claims(claims)
    }
}

#[inline]
pub(crate) fn device_fence() {
    oer_mmio::fence();
}

/// Ordinary task-side owner of the IEEE 802.15.4 MAC partition.
///
/// It contains no shared radio partition: the modem clocks and resets, the
/// common PHY and the shared BTBB baseband the MAC depends on are reached
/// through [`SharedRadioRegisters`]. Possessing this owner does not itself
/// prove that any of those prerequisites have run.
#[must_use = "the IEEE 802.15.4 task owner must be reunited before release"]
pub struct Ieee802154TaskRegisters {
    pub(crate) peripherals: Ieee802154TaskPeripheralOwners,
}

impl Ieee802154TaskRegisters {
    /// Assemble the task register set and split the MAC block into its
    /// disjoint task and interrupt owners. This performs no MMIO.
    pub fn new(partition: Ieee802154Partition) -> (Self, Ieee802154InterruptSetup) {
        let Ieee802154Partition {
            peripherals: generated::peripheral_ownership::Ieee802154Peripherals { ieee802154_mac },
            etm,
            rx_info,
        } = partition;
        let (task_mac, interrupt_mac) = crate::ieee802154::ownership::split(ieee802154_mac);
        (
            Self {
                peripherals: Ieee802154TaskPeripheralOwners {
                    ieee802154_mac: task_mac,
                    etm,
                    rx_info,
                },
            },
            Ieee802154InterruptSetup {
                registers: interrupt_mac,
            },
        )
    }

    /// Reunite the MAC block with its inactive interrupt owner and return the
    /// partition. This performs no MMIO.
    pub fn into_partition(self, interrupts: Ieee802154InterruptSetup) -> Ieee802154Partition {
        let Ieee802154TaskPeripheralOwners {
            ieee802154_mac: task_mac,
            etm,
            rx_info,
        } = self.peripherals;
        let ieee802154_mac = crate::ieee802154::ownership::reunite(task_mac, interrupts.registers);
        Ieee802154Partition {
            peripherals: generated::peripheral_ownership::Ieee802154Peripherals { ieee802154_mac },
            etm,
            rx_info,
        }
    }

    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        device_fence();
    }
}

/// Task-side setup token before one IEEE 802.15.4 hard-IRQ epoch.
///
/// The raw interrupt handle is already disjoint from
/// [`Ieee802154TaskRegisters`], but remains inactive until the reviewed setup
/// transaction consumes this value.
#[must_use = "the IEEE 802.15.4 interrupt setup must remain paired with its task owner"]
pub struct Ieee802154InterruptSetup {
    pub(crate) registers: crate::ieee802154::ownership::InterruptRegisters,
}

/// Disjoint IEEE 802.15.4 event/status capability for the hard ISR.
///
/// Task command, DMA, policy, and event-enable operations are absent from this
/// type. It must be deactivated after the platform CPU route is disabled and
/// reunited with the task owner before the whole radio can be released.
#[must_use = "the IEEE 802.15.4 interrupt owner must be deactivated and reunited"]
pub struct Ieee802154InterruptRegisters {
    pub(crate) registers: crate::ieee802154::ownership::InterruptRegisters,
}

/// Ordinary task-side owner of the Bluetooth controller partition.
///
/// It contains no shared radio partition: a transaction that also touches the
/// radio PHY, coexistence or shared baseband borrows [`SharedRadioRegisters`]
/// explicitly. Methods on this owner are individually reviewed register
/// transactions; possessing it does not itself prove that common PHY, BTBB or
/// controller lifecycle prerequisites have run.
#[must_use = "the Bluetooth task owner must be reunited before release"]
pub struct BluetoothTaskRegisters {
    pub(crate) bluetooth: generated::peripheral_ownership::BluetoothControllerPeripherals,
    /// Modem ETM channel two, which BLE PHY initialization programs.
    pub(crate) etm: crate::modem::etm::BluetoothPhyEtmChannel,
}

impl BluetoothTaskRegisters {
    /// Assemble the Bluetooth task register set. This performs no MMIO.
    pub fn new(bluetooth: BluetoothControllerPartition) -> Self {
        let BluetoothControllerPartition { bluetooth, etm } = bluetooth;
        Self { bluetooth, etm }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_partition(self) -> BluetoothControllerPartition {
        BluetoothControllerPartition {
            bluetooth: self.bluetooth,
            etm: self.etm,
        }
    }

    #[doc(hidden)]
    pub fn bluetooth_shared_clock_observation(
        &self,
        shared: &SharedRadioRegisters,
    ) -> (
        SharedModemClockObservation,
        BluetoothLowPowerClockObservation,
    ) {
        (
            shared.radio_phy.shared_modem_clock_observation(),
            shared.radio_phy.bluetooth_low_power_clock_observation(),
        )
    }

    /// Reset the Bluetooth controller domains and fence the reset edge.
    #[doc(hidden)]
    pub fn reset_controller_domains(&mut self, shared: &mut SharedRadioRegisters) {
        shared.radio_phy.reset_bluetooth_controller_domains();
        device_fence();
    }

    /// Whether the Bluetooth controller domain resets read back released.
    pub fn controller_resets_released(&self, shared: &SharedRadioRegisters) -> bool {
        self.controller_clock_observation(shared)
            .controller_resets_released
    }

    /// Read back the Bluetooth controller and APB clock sets and resets.
    pub fn controller_clock_observation(
        &self,
        shared: &SharedRadioRegisters,
    ) -> ModemSysconBluetoothObservation {
        shared.radio_phy.bluetooth_clock_observation()
    }
}

/// Inactive owner of the reviewed Bluetooth interrupt partition.
#[must_use = "the Bluetooth interrupt setup must remain paired with its task owner"]
pub struct BluetoothInterruptSetup {
    pub(crate) peripherals: generated::peripheral_ownership::BluetoothInterruptPeripherals,
}

/// Bluetooth interrupt-bank capability staged for a future powered ISR epoch.
///
/// [`BluetoothInterruptOutputPrepared::stage_for_cpu_routes`] constructs this
/// value only after the reviewed baseline masks and controller output have
/// been prepared. The platform must retain it in stable storage shared by the
/// primary and NRT handlers before enabling either CPU route.
#[must_use = "the Bluetooth interrupt owner must be deactivated and reunited"]
pub struct BluetoothInterruptRegisters {
    pub(crate) peripherals: generated::peripheral_ownership::BluetoothInterruptPeripherals,
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;

//! Hardware ownership, capabilities, and affine lifecycle transitions.

use super::*;

/// Reviewed Bluetooth low-power timer divider for the main crystal source.
pub const BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER: ModemLowPowerClockDivider =
    match ModemLowPowerClockDivider::new(399) {
        Some(divider) => divider,
        None => panic!("reviewed Bluetooth low-power divider exceeds its PAC field"),
    };

/// Private Wi-Fi MAC owners used by one Wi-Fi register set.
pub(crate) struct WifiRadioPeripheralOwners {
    pub(crate) wifi_mac: svd::peripheral_ownership::WifiMacPeripherals,
}

/// Physical owners used by one IEEE 802.15.4 task register set.
///
/// The MAC, its interrupt route, its ETM channels and the read of the BTBB
/// receive information are the whole IEEE 802.15.4 authority. Clocks,
/// resets, the common PHY and the rest of the shared BTBB baseband it
/// depends on belong to the shared radio partition.
pub(crate) struct Ieee802154TaskPeripheralOwners {
    pub(crate) ieee802154_mac: crate::ieee802154::ownership::TaskRegisters,
    pub(crate) ieee802154_interrupt_route: svd::Ieee802154InterruptRoute,
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
    pub(crate) peripherals: svd::peripheral_ownership::RadioPhyPeripherals,
    /// The PHY's copy of the MAC local-time read, for its SDM deadline.
    pub(crate) mac_local_time: MacLocalTime,
}

/// Read-only capability for the free-running Wi-Fi MAC local-time counter.
///
/// The radio PHY (its SDM-stability deadline) and the Wi-Fi MAC (the time
/// its receive timestamps count in) both read this counter, and reading it
/// has no side effect, so the capability is copyable rather than owned by
/// either.
///
/// Invariant: [`RadioPartitions`] consumes the unique generated
/// `MacLocalTimePeripherals` owner when it mints this capability, no
/// partition or transaction writes the counter, and the only access any copy
/// performs is the single read of [`Self::now`].
#[derive(Clone, Copy, Debug)]
pub struct MacLocalTime(());

impl MacLocalTime {
    /// Mint the capability from the consumed partition owner.
    pub(crate) fn mint(owner: svd::peripheral_ownership::MacLocalTimePeripherals) -> Self {
        let svd::peripheral_ownership::MacLocalTimePeripherals {
            wifi_mac_local_time: _,
        } = owner;
        Self(())
    }

    /// The counter now: a full-width wrapping value in microseconds, at the
    /// system timer's rate (observed on hardware).
    #[allow(
        unsafe_code,
        reason = "every copy of the read-only capability reads the counter the consumed owner held"
    )]
    pub fn now(self) -> u32 {
        // SAFETY: the owner was consumed by `mint` (see the invariant), and
        // this handle only performs one side-effect-free read.
        let registers = unsafe { svd::WifiMacLocalTime::steal() };
        svd::field_read::read_mac_local_time(&registers)
    }
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
    pub(crate) coexistence: svd::peripheral_ownership::CoexistencePeripherals,
    pub(crate) shared_radio: svd::peripheral_ownership::SharedRadioPeripherals,
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
}

/// Opaque Wi-Fi MAC register partition, with a copy of the read-only MAC
/// local time.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiMacPartition {
    peripherals: svd::peripheral_ownership::WifiMacPeripherals,
    mac_local_time: MacLocalTime,
}

impl WifiMacPartition {
    /// The MAC local-time read this partition holds a copy of.
    pub const fn mac_local_time(&self) -> MacLocalTime {
        self.mac_local_time
    }
}

/// Opaque coexistence register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct CoexistencePartition(svd::peripheral_ownership::CoexistencePeripherals);

/// Opaque Bluetooth controller register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct BluetoothControllerPartition {
    bluetooth: svd::peripheral_ownership::BluetoothControllerPeripherals,
    etm: crate::modem::etm::BluetoothPhyEtmChannel,
}

/// Opaque shared-baseband register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct SharedRadioPartition(svd::peripheral_ownership::SharedRadioPeripherals);

/// Opaque IEEE 802.15.4 MAC and interrupt-route register partition.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct Ieee802154Partition {
    peripherals: svd::peripheral_ownership::Ieee802154Peripherals,
    etm: crate::modem::etm::Ieee802154EtmChannels,
    rx_info: crate::ieee802154::baseband::Ieee802154BasebandRxInfo,
}

/// Every reviewed ESP32-S31 radio register partition.
///
/// This is the sole production acquisition point. The partitions carry no
/// protocol route or lifecycle policy: the HAL composes them into register
/// sets and returns them here only as an ownership bundle.
///
/// ```compile_fail
/// use oer_esp32s31_pac::RadioPartitions;
///
/// let partitions = RadioPartitions::take().unwrap();
/// let _first = partitions.wifi_mac;
/// let _second = partitions.wifi_mac;
/// ```
#[must_use = "dropping the radio partitions permanently loses the unique hardware capability"]
pub struct RadioPartitions {
    pub wifi_mac: WifiMacPartition,
    pub wifi_interrupts: MacInterruptSetup,
    pub radio_phy: RadioPhyRegisters,
    pub coexistence: CoexistencePartition,
    pub bluetooth: BluetoothControllerPartition,
    pub bluetooth_modem_lp_timer: BluetoothModemLpTimerRegisters,
    pub bluetooth_interrupts: BluetoothInterruptSetup,
    pub shared_radio: SharedRadioPartition,
    pub ieee802154: Ieee802154Partition,
}

impl RadioPartitions {
    /// Acquire the generated radio singleton once.
    pub fn take() -> Option<Self> {
        svd::Peripherals::take().map(Self::from_peripherals)
    }

    /// Bind the generated singleton to the opaque partition owners.
    pub(crate) fn from_peripherals(peripherals: svd::Peripherals) -> Self {
        let svd::peripheral_ownership::PeripheralPartitions {
            wifi_mac,
            wifi_interrupts,
            radio_phy,
            coexistence,
            bluetooth,
            bluetooth_modem_lp_timer,
            bluetooth_interrupts,
            shared_radio,
            ieee802154,
            modem_etm,
            mac_local_time,
        } = svd::peripheral_ownership::partition(peripherals);
        let mac_local_time = MacLocalTime::mint(mac_local_time);
        let (ieee802154_etm, bluetooth_phy_etm, bluetooth_etm) =
            crate::modem::etm::split(modem_etm);
        let (shared_radio, ieee802154_rx_info) = crate::ieee802154::baseband::split(shared_radio);
        Self {
            wifi_mac: WifiMacPartition {
                peripherals: wifi_mac,
                mac_local_time,
            },
            wifi_interrupts: MacInterruptSetup::from_peripherals(wifi_interrupts),
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

    /// Construct every partition inside one isolated validation image.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn for_validation() -> Self {
        Self::from_peripherals(svd::peripheral_ownership::peripherals_for_validation())
    }
}

/// Semantic MAC work causes recovered from reviewed vendor transactions.
///
/// This type is deliberately not a register image. The generated PAC owns
/// STATUS field geometry; higher layers can only combine named causes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacInterruptEvents {
    pub(crate) tx_complete: bool,
    pub(crate) collision: bool,
    pub(crate) rx_success: bool,
    pub(crate) tx_timeout: bool,
}

impl MacInterruptEvents {
    pub const TX_COMPLETE: Self = Self::from_causes(true, false, false, false);
    pub const COLLISION: Self = Self::from_causes(false, true, false, false);
    pub const RX_SUCCESS: Self = Self::from_causes(false, false, true, false);
    pub const TX_TIMEOUT: Self = Self::from_causes(false, false, false, true);

    pub const fn empty() -> Self {
        Self::from_causes(false, false, false, false)
    }

    pub const fn from_causes(
        tx_complete: bool,
        collision: bool,
        rx_success: bool,
        tx_timeout: bool,
    ) -> Self {
        Self {
            tx_complete,
            collision,
            rx_success,
            tx_timeout,
        }
    }

    pub const fn union(self, other: Self) -> Self {
        Self::from_causes(
            self.tx_complete || other.tx_complete,
            self.collision || other.collision,
            self.rx_success || other.rx_success,
            self.tx_timeout || other.tx_timeout,
        )
    }

    pub const fn contains(self, other: Self) -> bool {
        (!other.tx_complete || self.tx_complete)
            && (!other.collision || self.collision)
            && (!other.rx_success || self.rx_success)
            && (!other.tx_timeout || self.tx_timeout)
    }

    pub const fn is_empty(self) -> bool {
        !self.tx_complete && !self.collision && !self.rx_success && !self.tx_timeout
    }

    pub const fn tx_complete(self) -> bool {
        self.tx_complete
    }

    pub const fn collision(self) -> bool {
        self.collision
    }

    pub const fn rx_success(self) -> bool {
        self.rx_success
    }

    pub const fn tx_timeout(self) -> bool {
        self.tx_timeout
    }
}

impl core::ops::BitOr for MacInterruptEvents {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

/// PAC-owned semantic partition of one sampled MAC interrupt image.
///
/// The raw W1C geometry stays private to this crate. Higher layers can route
/// qualified work, acknowledge-only auxiliary events, and opaque evidence
/// without applying masks to the sampled register themselves.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacInterruptObservation {
    pub(crate) work_events: MacInterruptEvents,
    pub(crate) auxiliary_event: bool,
    pub(crate) unhandled_event: bool,
}

impl MacInterruptObservation {
    /// Construct semantic evidence for a host-side interrupt implementation.
    ///
    /// No register image enters this API: test doubles name the causes they
    /// simulate in the same vocabulary consumed by the MAC driver.
    pub const fn from_semantic_events(
        work_events: MacInterruptEvents,
        auxiliary_event: bool,
        unhandled_event: bool,
    ) -> Self {
        Self {
            work_events,
            auxiliary_event,
            unhandled_event,
        }
    }

    pub const fn is_empty(self) -> bool {
        self.work_events.is_empty() && !self.auxiliary_event && !self.unhandled_event
    }

    pub const fn work_events(self) -> MacInterruptEvents {
        self.work_events
    }

    pub const fn has_auxiliary_event(self) -> bool {
        self.auxiliary_event
    }

    pub const fn has_unhandled_event(self) -> bool {
        self.unhandled_event
    }
}

/// One sampled MAC interrupt image which can be acknowledged exactly once.
pub struct MacInterruptSnapshot(pub(crate) svd::interrupt_snapshot::MacInterruptSnapshot);

impl MacInterruptSnapshot {
    pub fn observation(&self) -> MacInterruptObservation {
        let work_events = MacInterruptEvents::from_causes(
            self.0.tx_complete(),
            self.0.bss_color_collision(),
            self.0.rx_success(),
            self.0.tx_timeout(),
        );
        let auxiliary_event =
            self.0.rx_associated_auxiliary_5() || self.0.rx_associated_auxiliary_24();
        let unhandled_event = self.0.bits_0_4_opaque() != 0
            || self.0.bit_6_opaque()
            || self.0.bits_9_10_opaque() != 0
            || self.0.watchdog()
            || self.0.bit_12_opaque()
            || self.0.bit_13_opaque()
            || self.0.sta_beacon_filter()
            || self.0.bits_16_18_opaque() != 0
            || self.0.bit_20_opaque()
            || self.0.bit_21_opaque()
            || self.0.bit_22_opaque()
            || self.0.bit_23_opaque()
            || self.0.bits_25_26_opaque() != 0
            || self.0.bit_27_opaque()
            || self.0.bit_28_opaque()
            || self.0.bits_29_31_opaque() != 0;
        MacInterruptObservation::from_semantic_events(work_events, auxiliary_event, unhandled_event)
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_bits(&self) -> u32 {
        self.0.bits()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn for_validation(bits: u32) -> Self {
        Self(svd::interrupt_snapshot::mac_interrupt_for_validation(bits))
    }
}

/// One sampled power-interrupt image with intentionally opaque bit semantics.
pub struct MacPowerInterruptSnapshot(pub(crate) svd::interrupt_snapshot::MacPowerInterruptSnapshot);

impl MacPowerInterruptSnapshot {
    pub fn observation(&self) -> MacPowerInterruptObservation {
        MacPowerInterruptObservation::from_semantic_events(
            self.0.tsf_timer_0(),
            self.0.tsf_timer_1(),
            self.0.tsf_timer_2(),
            self.0.tsf_timer_3(),
            self.0.tbtt_0(),
            // Only the station TSF's TBTT is ever enabled.
            self.0.tbtt_1() || self.0.tbtt_2() || self.0.tbtt_3() || self.0.bits_8_31_opaque() != 0,
        )
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_bits(&self) -> u32 {
        self.0.bits()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn for_validation(bits: u32) -> Self {
        Self(svd::interrupt_snapshot::mac_power_interrupt_for_validation(
            bits,
        ))
    }
}

/// Semantic WDEVPWR causes sampled through generated PAC field accessors.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacPowerInterruptObservation(pub(crate) u8);

impl MacPowerInterruptObservation {
    // Driver-local semantic flags. Their positions intentionally do not match
    // WDEVPWR register geometry, which remains behind generated accessors.
    pub(crate) const TSF_TIMER_0: u8 = 0x01;
    pub(crate) const TSF_TIMER_1: u8 = 0x02;
    pub(crate) const TSF_TIMER_2: u8 = 0x04;
    pub(crate) const TSF_TIMER_3: u8 = 0x08;
    pub(crate) const UNHANDLED_EVENT: u8 = 0x10;
    pub(crate) const STA_TBTT: u8 = 0x20;

    pub const fn from_semantic_events(
        tsf_timer_0: bool,
        tsf_timer_1: bool,
        tsf_timer_2: bool,
        tsf_timer_3: bool,
        sta_tbtt: bool,
        unhandled_event: bool,
    ) -> Self {
        Self(
            (if tsf_timer_0 { Self::TSF_TIMER_0 } else { 0 })
                | (if tsf_timer_1 { Self::TSF_TIMER_1 } else { 0 })
                | (if tsf_timer_2 { Self::TSF_TIMER_2 } else { 0 })
                | (if tsf_timer_3 { Self::TSF_TIMER_3 } else { 0 })
                | (if sta_tbtt { Self::STA_TBTT } else { 0 })
                | (if unhandled_event {
                    Self::UNHANDLED_EVENT
                } else {
                    0
                }),
        )
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn tsf_timer_0(self) -> bool {
        self.0 & Self::TSF_TIMER_0 != 0
    }

    pub const fn tsf_timer_1(self) -> bool {
        self.0 & Self::TSF_TIMER_1 != 0
    }

    pub const fn tsf_timer_2(self) -> bool {
        self.0 & Self::TSF_TIMER_2 != 0
    }

    pub const fn tsf_timer_3(self) -> bool {
        self.0 & Self::TSF_TIMER_3 != 0
    }

    /// The station TSF reached its target beacon transmission time.
    pub const fn sta_tbtt(self) -> bool {
        self.0 & Self::STA_TBTT != 0
    }

    pub const fn has_unhandled_event(self) -> bool {
        self.0 & Self::UNHANDLED_EVENT != 0
    }
}

#[inline]
pub(crate) fn device_fence() {
    svd::device_access::fence();
}

/// Unique logical owner of the ESP32-S31 radio register regions after cold
/// MAC initialization has completed.
///
/// The generated [`svd::Peripherals`] singleton is kept private. This running
/// owner deliberately has no typed access to the MAC interrupt enable/clear or
/// WDEVPWR status/clear transactions. Those disjoint banks belong to
/// [`MacInterruptSetup`] and then to [`MacInterruptRegisters`] plus
/// [`MacPowerInterruptRegisters`].
///
/// Raw PAC types are deliberately not part of this crate's public API:
///
/// ```compile_fail
/// use oer_esp32s31_pac::svd;
/// ```
///
/// No address-bearing register catalog is exposed:
///
/// ```compile_fail
/// use oer_esp32s31_pac::Register32;
///
/// let forged = Register32::new(0x2010_4000);
/// ```
///
/// Finally, the owner has no generic address/value escape hatch. Every
/// writable transaction must be an explicitly reviewed capability:
///
/// ```compile_fail
/// use oer_esp32s31_pac::WifiRadioRegisters;
///
/// let unreviewed_write = WifiRadioRegisters::write_register;
/// ```
pub struct WifiRadioRegisters {
    pub(crate) peripherals: WifiRadioPeripheralOwners,
    mac_local_time: MacLocalTime,
}

impl WifiRadioRegisters {
    /// Assemble the Wi-Fi register set. This performs no MMIO.
    pub fn new(wifi_mac: WifiMacPartition) -> Self {
        let WifiMacPartition {
            peripherals: wifi_mac,
            mac_local_time,
        } = wifi_mac;
        Self {
            peripherals: WifiRadioPeripheralOwners { wifi_mac },
            mac_local_time,
        }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_partition(self) -> WifiMacPartition {
        WifiMacPartition {
            peripherals: self.peripherals.wifi_mac,
            mac_local_time: self.mac_local_time,
        }
    }

    /// The MAC local-time read the Wi-Fi registers hold a copy of.
    pub const fn mac_local_time(&self) -> MacLocalTime {
        self.mac_local_time
    }

    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        device_fence();
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) const fn contains(address: usize) -> bool {
        // The host-only catalog is limited to the custom modem/radio aperture.
        matches!(address, 0x2010_0000..=0x2010_ffff)
    }
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
            peripherals:
                svd::peripheral_ownership::Ieee802154Peripherals {
                    ieee802154_mac,
                    ieee802154_interrupt_route,
                },
            etm,
            rx_info,
        } = partition;
        let (task_mac, interrupt_mac) = crate::ieee802154::ownership::split(ieee802154_mac);
        (
            Self {
                peripherals: Ieee802154TaskPeripheralOwners {
                    ieee802154_mac: task_mac,
                    ieee802154_interrupt_route,
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
            ieee802154_interrupt_route,
            etm,
            rx_info,
        } = self.peripherals;
        let ieee802154_mac = crate::ieee802154::ownership::reunite(task_mac, interrupts.registers);
        Ieee802154Partition {
            peripherals: svd::peripheral_ownership::Ieee802154Peripherals {
                ieee802154_mac,
                ieee802154_interrupt_route,
            },
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
    pub(crate) bluetooth: svd::peripheral_ownership::BluetoothControllerPeripherals,
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
    pub(crate) peripherals: svd::peripheral_ownership::BluetoothInterruptPeripherals,
}

/// Bluetooth interrupt-bank capability staged for a future powered ISR epoch.
///
/// [`BluetoothInterruptOutputPrepared::stage_for_cpu_routes`] constructs this
/// value only after the reviewed baseline masks and controller output have
/// been prepared. The platform must retain it in stable storage shared by the
/// primary and NRT handlers before enabling either CPU route.
#[must_use = "the Bluetooth interrupt owner must be deactivated and reunited"]
pub struct BluetoothInterruptRegisters {
    pub(crate) peripherals: svd::peripheral_ownership::BluetoothInterruptPeripherals,
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;

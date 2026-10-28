//! Hardware ownership, capabilities, and affine lifecycle transitions.

use crate::power::PlatformClockBackend;
#[cfg(test)]
use crate::{domain::RadioDomain, ieee80211::channel};

use crate::{
    ieee80211::{dma::rx_dma::WifiRxDmaEngine, mac as wifi_mac, station_wake::StationWakeState},
    phy::i2c::PhyAdcRate,
    types::{
        MacInterruptEnableState, MacInterruptMask, MacInterruptRawSnapshot, MacInterruptSnapshot,
        MacPowerInterruptSnapshot,
    },
};

use super::*;

use crate::ieee80211::registers::{WifiRadioRegisters, WifiRxDmaRegisters};
use crate::phy::registers::RadioPhyParts;
use crate::phy::{baseband::ops as bb_ops, restore::PhyRouteState};

pub mod maintenance;

pub use crate::phy::registration::{PhyRegistrationEpoch, PhyRegistrationGrant};

/// One PAC-observed image of the Wi-Fi baseband enable condition.
///
/// The shared PBus work-mode leaf uses this condition only to decide whether
/// its caller must execute the recovered settle pulse. Keeping it separate
/// prevents the protocol-neutral PHY owner from retaining Wi-Fi state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiBasebandEnableObservation(pub(crate) bool);

impl WifiBasebandEnableObservation {
    /// Record one semantic PAC readback made by the lifecycle owner.
    pub const fn from_pac_readback(enabled: bool) -> Self {
        Self(enabled)
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn is_enabled(self) -> bool {
        self.0
    }
}

use crate::domain::transaction::PhyHardware;

pub(crate) mod sealed {

    use crate::{owner::WifiBasebandEnableObservation, phy::restore::PhyRouteState};

    use super::RadioPhyRegisters;

    pub trait SharedPhyAccess {
        fn pac(&self) -> &RadioPhyRegisters;
        /// The route PHY software state lent together with the registers.
        fn route_state(&self) -> &PhyRouteState;
        /// Borrow the shared PHY registers together with the route restore
        /// slot that serializes PHY calibrations.
        fn parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState);
    }

    pub trait SharedPhyContext {
        fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation;
    }

    pub trait PhyInitializationAccess {}
}

/// Sealed protocol-neutral port accepted by named PHY HAL operations.
///
/// External crates can borrow an owned [`PhyHardware`] or channel transaction but cannot implement this trait for an arbitrary owner or recover
/// the underlying PAC.
pub trait SharedPhyAccess: sealed::SharedPhyAccess {
    /// The registration that currently describes this PHY partition.
    ///
    /// A registration result held apart from its hardware is valid for this
    /// borrow only while its recorded epoch equals this value.
    fn registration_epoch(&self) -> Option<PhyRegistrationEpoch> {
        sealed::SharedPhyAccess::route_state(self).registration_epoch()
    }

    fn set_phy_calibration_clock(&mut self, enabled: bool) {
        crate::phy::clock::ops::set_calibration_clock(
            &mut phy_blocks_mut(self).phy_clock_oracle,
            enabled,
        );
    }

    fn set_rx_gain_dc_calibration(&mut self, enabled: bool) {
        bb_ops::set_rx_gain_dc_calibration(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
            enabled,
        );
    }

    fn configure_power_control_tone(&mut self, selector: u16, step: u8) {
        bb_ops::configure_power_control_tone(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
            selector,
            step,
        );
    }

    fn configure_calibration_tone(&mut self, enabled: bool, selector: u16, step: u8) {
        bb_ops::configure_calibration_tone(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
            enabled,
            selector,
            step,
        );
    }

    fn configure_tx_iq_correction(&mut self, begin: bool) {
        bb_ops::configure_tx_iq_correction(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
            begin,
        );
    }

    /// Capture the first-path TX-IQ tone-control state into the route slot.
    ///
    /// A second caller is rejected before reading the register and therefore
    /// cannot replace another calibration's restore authority.
    fn prepare_txiq_tone_control_restore(
        &mut self,
    ) -> Result<(), crate::phy::restore::TxIqToneControlPrepareError> {
        let (phy, restore) = sealed::SharedPhyAccess::parts_mut(self);
        restore.prepare_txiq_with(|| {
            crate::phy::baseband::ops::TxIqToneControlFields::capture(
                &phy.peripherals.phy_baseband_config_oracle,
            )
        })
    }

    /// Restore and consume the saved TX-IQ tone-control state.
    ///
    /// A caller without a successful prepare operation is rejected before
    /// MMIO. The slot is cleared only after the complete accessor write.
    fn restore_txiq_tone_control(
        &mut self,
    ) -> Result<(), crate::phy::restore::TxIqToneControlRestoreError> {
        let (phy, restore) = sealed::SharedPhyAccess::parts_mut(self);
        restore.restore_txiq_with(|fields| {
            fields.restore(&mut phy.peripherals.phy_baseband_config_oracle)
        })
    }

    fn configure_txiq_mismatch_power(
        &mut self,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    ) {
        bb_ops::configure_txiq_mismatch_power(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
            first,
            polarity,
            attenuation,
            selector,
        );
    }

    fn set_tx_iq_gain_coefficient(&mut self, coefficient: i8) {
        bb_ops::set_tx_iq_gain_coefficient(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
            coefficient,
        );
    }

    fn set_tx_iq_phase_coefficient(&mut self, coefficient: i8) {
        bb_ops::set_tx_iq_phase_coefficient(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
            coefficient,
        );
    }

    fn set_rx_iq_gain_coefficient(&mut self, coefficient: i8) {
        bb_ops::set_rx_iq_gain_coefficient(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
            coefficient,
        );
    }

    fn set_rx_iq_phase_coefficient(&mut self, coefficient: i8) {
        bb_ops::set_rx_iq_phase_coefficient(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
            coefficient,
        );
    }

    fn configure_rx_iq_calibration_mode(&mut self) {
        bb_ops::configure_rx_iq_calibration_mode(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
        );
    }

    fn configure_adc_rate(&mut self, rate: PhyAdcRate) {
        bb_ops::configure_adc_rate(&mut phy_blocks_mut(self).phy_baseband_config_oracle, rate);
    }

    fn set_power_detector_tone_armed(&mut self, armed: bool) {
        bb_ops::set_power_detector_tone_armed(
            &mut phy_blocks_mut(self).phy_baseband_config_oracle,
            armed,
        );
    }

    fn stop_power_detector_tone(&mut self) {
        bb_ops::stop_power_detector_tone(&mut phy_blocks_mut(self).phy_baseband_config_oracle);
    }

    fn trigger_tx_dc_measurement(&mut self) {
        bb_ops::trigger_tx_dc_measurement(&mut phy_blocks_mut(self).phy_baseband_config_oracle);
    }

    fn tx_dc_measurement_is_ready(&mut self) -> bool {
        bb_ops::tx_dc_measurement_is_ready(&phy_blocks(self).phy_baseband_config_oracle)
    }

    fn sample_tx_dc_comparators(&mut self) -> [bool; 2] {
        bb_ops::sample_tx_dc_comparators(&phy_blocks(self).phy_baseband_config_oracle)
    }

    fn clear_tx_dc_measurement(&mut self) {
        bb_ops::clear_tx_dc_measurement(&mut phy_blocks_mut(self).phy_baseband_config_oracle);
    }

    fn open_frontend_baseband_internal_clocks(&mut self) {
        crate::phy::clock::ops::open_frontend_baseband_internal_clocks(
            &mut phy_blocks_mut(self).phy_clock_oracle,
        );
    }

    fn set_rf_circuit_power(&mut self, enabled: bool) {
        phy_pac_mut(self).set_rf_circuit_power(enabled);
    }

    fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        phy_pac_mut(self).set_bb_i2c_power_tie(enabled);
    }

    fn analog_i2c_is_powered(&self) -> bool {
        sealed::SharedPhyAccess::pac(self).analog_i2c_is_powered()
    }

    fn set_analog_i2c_power(&mut self, enabled: bool) {
        phy_pac_mut(self).set_analog_i2c_power(enabled);
    }

    fn analog_i2c_reset_is_released(&self) -> bool {
        sealed::SharedPhyAccess::pac(self).analog_i2c_reset_is_released()
    }

    fn set_analog_i2c_reset_released(&mut self, released: bool) {
        phy_pac_mut(self).set_analog_i2c_reset_released(released);
    }

    fn enable_frontend_baseband_power(&mut self) {
        phy_pac_mut(self).enable_frontend_baseband_power();
    }
}

/// Shared PHY access paired with one explicit Wi-Fi-baseband observation.
///
/// Only the PBus work-mode settle decision needs this additional context.
/// Ordinary shared-PHY leaves require [`SharedPhyAccess`] alone.
pub trait SharedPhyContext: SharedPhyAccess + sealed::SharedPhyContext {
    fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation {
        sealed::SharedPhyContext::wifi_baseband_enable_observation(self)
    }
}

/// Common PHY-initialization port that tracks temporary Wi-Fi-BB edges.
///
/// `register_chipv7_phy` temporarily drives the physical Wi-Fi-BB enable state
/// even when entered by a concurrent Bluetooth or IEEE 802.15.4 client. Implementations
/// sample it through the retained PAC owner; this capability conveys no Wi-Fi
/// MAC or protocol-role ownership.
pub trait PhyInitializationAccess: SharedPhyContext + sealed::PhyInitializationAccess {
    /// Begin a registration, retire every earlier epoch of this partition and
    /// give the registration's one grant.
    ///
    /// Registration calls this before its first hardware edge. Any other call
    /// only invalidates existing registration results; it cannot mint one.
    fn begin_registration(&mut self) -> PhyRegistrationGrant {
        PhyRegistrationGrant::new(
            sealed::SharedPhyAccess::parts_mut(self)
                .1
                .begin_registration_epoch(),
        )
    }
}

impl sealed::SharedPhyAccess for PhyHardware {
    fn pac(&self) -> &RadioPhyRegisters {
        &self.registers
    }

    fn route_state(&self) -> &PhyRouteState {
        &self.restore
    }

    fn parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
        (&mut self.registers, &mut self.restore)
    }
}

impl sealed::SharedPhyContext for PhyHardware {
    fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation {
        WifiBasebandEnableObservation::from_pac_readback(self.registers.wifi_baseband_is_enabled())
    }
}

impl SharedPhyAccess for PhyHardware {}
impl SharedPhyContext for PhyHardware {}

impl sealed::PhyInitializationAccess for PhyHardware {}

impl PhyInitializationAccess for PhyHardware {}

pub(crate) fn phy_pac(access: &(impl SharedPhyAccess + ?Sized)) -> &RadioPhyRegisters {
    sealed::SharedPhyAccess::pac(access)
}

pub(crate) fn phy_pac_mut(access: &mut (impl SharedPhyAccess + ?Sized)) -> &mut RadioPhyRegisters {
    sealed::SharedPhyAccess::parts_mut(access).0
}

/// The generated blocks of the radio-PHY partition the access holds.
pub(crate) fn phy_blocks(access: &(impl SharedPhyAccess + ?Sized)) -> &RadioPhyParts {
    &phy_pac(access).peripherals
}

/// The generated blocks of the radio-PHY partition the access holds.
pub(crate) fn phy_blocks_mut(access: &mut (impl SharedPhyAccess + ?Sized)) -> &mut RadioPhyParts {
    &mut phy_pac_mut(access).peripherals
}

pub(crate) fn phy_parts_mut(
    access: &mut (impl SharedPhyAccess + ?Sized),
) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
    sealed::SharedPhyAccess::parts_mut(access)
}

/// Opaque task-side owner of the running Wi-Fi MAC partition.
///
/// A clocked Wi-Fi client becomes this owner after cold MAC setup
/// ([`WifiClocked::into_running`](crate::ieee80211::client::WifiClocked::into_running)). It holds the Wi-Fi MAC registers only:
/// the Wi-Fi hot path needs no radio domain, and every operation that
/// touches shared radio registers borrows them from the radio domain. It can move
/// between lifecycle owners and the driver, but exposes no PAC owner,
/// dereference operation, or generic register callback.
///
/// Moving the owner transfers its authority rather than copying access:
///
/// ```compile_fail,E0382
/// use oer_esp32s31_hal::owner::RadioAccess;
/// fn duplicate(owner: RadioAccess) -> (RadioAccess, RadioAccess) {
///     (owner, owner)
/// }
/// ```
///
/// A live MAC transaction excludes a second mutable transaction:
///
/// ```compile_fail,E0499
/// use oer_esp32s31_hal::owner::RadioAccess;
/// fn overlap(owner: &mut RadioAccess) {
///     let first = owner.wifi_mac_hal();
///     let second = owner.wifi_mac_hal();
///     drop((first, second));
/// }
/// ```
pub struct RadioAccess {
    pub(crate) registers: WifiRadioRegisters,
    rx_dma: WifiRxDmaEngine,
    station_wake: StationWakeState,
    initialized: bool,
}

impl RadioAccess {
    /// Construct the running register owner inside an isolated validation
    /// image without exposing the underlying PAC owner.
    #[cfg(test)]
    #[doc(hidden)]
    pub fn claim_for_host_test() -> Self {
        let (_shared, partitions) = crate::root::RadioHardware::for_host_test()
            .split::<crate::power::test_clocks::CountingBackend, _>(
                (),
                crate::domain::transaction::PhyIdentity::for_host_test(),
            )
            .expect("identity capacity");
        let clocked = crate::ieee80211::client::WifiClocked::for_host_test(
            crate::ieee80211::client::WifiCold::from_partition(partitions.wifi),
        );
        clocked.into_running().0
    }

    pub(crate) fn from_clocked(
        registers: WifiRadioRegisters,
        rx_dma: WifiRxDmaRegisters,
        station_wake: StationWakeState,
        initialized: bool,
    ) -> Self {
        Self {
            registers,
            rx_dma: WifiRxDmaEngine::new(rx_dma),
            station_wake,
            initialized,
        }
    }

    pub(crate) fn into_clocked_parts(
        self,
    ) -> (
        WifiRadioRegisters,
        WifiRxDmaRegisters,
        StationWakeState,
        bool,
    ) {
        (
            self.registers,
            self.rx_dma.into_registers(),
            self.station_wake,
            self.initialized,
        )
    }

    pub fn wifi_mac_hal(&mut self) -> wifi_mac::WifiMacHal<'_> {
        wifi_mac::WifiMacHal::from_mac(&mut self.registers)
    }

    /// Replace the station TSF through this unique owner. A live HAL borrow
    /// prevents this transaction at compile time.
    pub fn set_station_tsf(&mut self, value: u64) {
        self.wifi_mac_hal().set_station_tsf(value);
    }

    /// The RX DMA engine, the only owner of the RX descriptor walker.
    pub fn rx_dma(&mut self) -> &mut WifiRxDmaEngine {
        &mut self.rx_dma
    }

    /// The MAC HAL and the RX DMA engine at once, for a receive path that
    /// drives both.
    pub fn split_rx_dma(&mut self) -> (wifi_mac::WifiMacHal<'_>, &mut WifiRxDmaEngine) {
        (
            wifi_mac::WifiMacHal::from_mac(&mut self.registers),
            &mut self.rx_dma,
        )
    }

    /// Borrow the channel HAL for one channel transaction, with the shared
    /// PHY borrowed from the radio domain.
    #[cfg(test)]
    pub fn channel_hal<'hal, B: PlatformClockBackend, P, T>(
        &'hal mut self,
        platform: &'hal mut P,
        radio: &'hal mut RadioDomain<B, T>,
    ) -> channel::RadioChannelHal<'hal, P> {
        let (phy, restore) = radio.phy_parts_mut();
        channel::RadioChannelHal::from_leased(platform, &mut self.registers, phy, restore)
    }

    /// Consume PHY custody for one channel operation, borrowing its MAC and platform.
    pub fn channel_transaction<'hal, B: PlatformClockBackend, P, T>(
        &'hal mut self,
        platform: &'hal mut P,
        radio: crate::domain::transaction::RadioTransaction<B, T>,
    ) -> crate::ieee80211::channel::ChannelTransaction<'hal, B, P, T> {
        crate::ieee80211::channel::ChannelTransaction::new(radio, &mut self.registers, platform)
    }

    /// Read the calibrated baseband observation without exposing the PAC
    /// owner or its register encoding.
    pub fn read_noise_floor_dbm(&self) -> i8 {
        self.registers.read_noise_floor_dbm()
    }

    pub fn access_point_receive_policy_snapshot(&self) -> wifi_mac::MacApReceivePolicySnapshot {
        self.registers.ap_receive_policy_snapshot()
    }

    pub fn receive_statistics_snapshot(&self) -> wifi_mac::MacRxStatisticsSnapshot {
        self.registers.rx_statistics_snapshot()
    }

    pub fn transmit_statistics_snapshot(&self) -> wifi_mac::MacTxStatisticsSnapshot {
        self.registers.tx_statistics_snapshot()
    }

    pub fn coex_priority_snapshot(&self) -> wifi_mac::MacCoexPrioritySnapshot {
        self.registers.mac_coex_priority_snapshot()
    }

    pub fn receive_dma_snapshot(&self) -> wifi_mac::MacRxDmaSnapshot {
        self.rx_dma.snapshot()
    }

    /// Latch one indirect SoftAP receive-BA bank through the reviewed HAL
    /// projection without exposing its PAC owner.
    pub fn extra_softap_rx_block_ack_entry_snapshot(
        &mut self,
        index: u8,
    ) -> Option<wifi_mac::ExtraSoftApRxBlockAckEntrySnapshot> {
        let index = types::MacExtraSoftApRxBlockAckEntryIndex::new(u32::from(index))?;
        Some(
            self.wifi_mac_hal()
                .extra_softap_rx_block_ack_entry_snapshot(index),
        )
    }

    /// Sample one ordinary direct receive-BA bank without exposing its PAC
    /// owner.
    pub fn rx_block_ack_entry_snapshot(
        &mut self,
        index: u8,
    ) -> Option<wifi_mac::RxBlockAckEntrySnapshot> {
        let index = types::MacRxBlockAckEntryIndex::new(u32::from(index))?;
        self.wifi_mac_hal().rx_block_ack_entry_snapshot(index)
    }

    /// Copy the reviewed Trigger-queue readback for one reservation.
    pub fn he_trigger_based_queue_snapshot(
        &self,
        reservation: types::MacHeTbLinkReservation,
    ) -> types::MacHeTriggerTxQueueSnapshot {
        self.registers.he_trigger_based_queue_snapshot(reservation)
    }

    pub fn he_trigger_receive_diagnostics(&self) -> types::MacHeTriggerRxDiagnostics {
        self.registers.he_trigger_receive_diagnostics()
    }

    pub(crate) fn pac(&self) -> &WifiRadioRegisters {
        &self.registers
    }

    pub(crate) fn pac_mut(&mut self) -> &mut WifiRadioRegisters {
        &mut self.registers
    }

    /// Borrow the Wi-Fi MAC together with the station wake state.
    pub(crate) fn station_wake_parts_mut(
        &mut self,
    ) -> (&mut WifiRadioRegisters, &mut StationWakeState) {
        (&mut self.registers, &mut self.station_wake)
    }

    /// Borrow the station TBTT and modem-wakeup capability. Both wake banks
    /// are Wi-Fi MAC registers, so this needs no radio domain.
    pub fn station_wake_hal(&mut self) -> crate::ieee80211::station_wake::StationWakeHal<'_> {
        let (registers, state) = self.station_wake_parts_mut();
        crate::ieee80211::station_wake::StationWakeHal::from_owned(registers, state)
    }
}

#[cfg(test)]
mod access_tests;

/// Proof that the connected-STA beacon-filter policy was applied.
pub struct ConnectedStaInterruptPrepared {
    pub(crate) _private: (),
}

impl RadioAccess {
    /// Disable vendor hardware beacon filtering before a connected STA
    /// epoch: CONTROL bits 2:0. The matching interrupt-enable field stays
    /// clear in every image the radio interrupt handler publishes.
    pub fn prepare_connected_sta_without_power_save(&mut self) -> ConnectedStaInterruptPrepared {
        let _ = self.registers.prepare_connected_sta_without_power_save();
        ConnectedStaInterruptPrepared { _private: () }
    }
}

/// The Wi-Fi MAC's interrupt banks, for one binding of the image's
/// interrupt table: the radio interrupt handler holds both for the image's
/// life, and every write of their enable, status and clear words is its.
#[must_use = "the Wi-Fi interrupt banks belong to the radio interrupt handler"]
pub struct WifiInterruptBanks {
    /// The MAC bank (`MODEM_WIFI_MAC`).
    pub mac: MacInterruptRegisters,
    /// The WDEVPWR bank (`MODEM_WIFI_PWR`).
    pub power: MacPowerInterruptRegisters,
}

impl WifiInterruptBanks {
    /// Take both banks from the root's partition. This performs no MMIO.
    pub(crate) fn new(partition: oer_esp32s31_pac::ownership::WifiInterruptPeripherals) -> Self {
        let (mac, power) = crate::ieee80211::registers::interrupt_banks(partition);
        Self {
            mac: MacInterruptRegisters { inner: mac },
            power: MacPowerInterruptRegisters { inner: power },
        }
    }
}

/// The MAC interrupt bank (`MODEM_WIFI_MAC`), which the radio interrupt
/// handler holds.
pub struct MacInterruptRegisters {
    pub(crate) inner: PacMacInterruptRegisters,
}

impl oer_mmio::InterruptSource for MacInterruptRegisters {
    const SOURCE: u16 =
        <oer_esp32s31_pac::wifi_mac::WifiMacInterrupt as oer_mmio::InterruptSource>::SOURCE;
}

impl MacInterruptRegisters {
    /// Observe opaque station delivery lanes on this bank's existing owner.
    pub fn mac_opaque_interrupt_enable(
        &self,
    ) -> crate::ieee80211::registers::MacOpaqueInterruptEnables {
        self.inner.mac_opaque_interrupt_enable()
    }

    /// Read the three RX delivery enables independently.
    pub fn mac_rx_interrupt_enable(&self) -> crate::ieee80211::registers::MacRxInterrupts {
        self.inner.mac_rx_interrupt_enable()
    }

    /// Change only RX delivery enables; preserve other lanes and acknowledge nothing.
    pub fn set_mac_rx_interrupt_enable(
        &mut self,
        enabled: crate::ieee80211::registers::MacRxInterrupts,
    ) {
        self.inner.set_mac_rx_interrupt_enable(enabled);
    }

    /// Mask every MAC source; nothing is acknowledged.
    pub fn mask_all_mac_interrupts(&mut self) {
        self.inner.mask_all_mac_interrupts();
    }

    /// Publish one reviewed MAC interrupt mask.
    pub fn publish_mac_interrupt_mask(&mut self, event_mask: MacInterruptMask) {
        self.inner.publish_mac_interrupt_mask(event_mask);
    }

    /// Read the currently published MAC interrupt mask.
    pub fn mac_interrupt_enable(&self) -> MacInterruptEnableState {
        self.inner.mac_interrupt_enable()
    }

    pub fn mac_interrupt_status(&self) -> MacInterruptSnapshot {
        self.inner.mac_interrupt_status()
    }

    /// Sample unmasked MAC RAW for inspection and one acknowledgement.
    pub fn mac_interrupt_raw(&self) -> MacInterruptRawSnapshot {
        self.inner.mac_interrupt_raw()
    }

    /// Acknowledge only the sampled RAW events. Retain fatal samples instead.
    pub fn acknowledge_mac_interrupt_raw(&mut self, snapshot: MacInterruptRawSnapshot) {
        self.inner.acknowledge_mac_interrupt_raw(snapshot);
    }

    pub fn acknowledge_mac_interrupts(&mut self, snapshot: MacInterruptSnapshot) {
        self.inner.acknowledge_mac_interrupts(snapshot);
    }

    pub fn mask_rx_delivery_interrupts(&mut self) {
        self.inner.mask_rx_delivery_interrupts();
    }

    pub fn unmask_rx_delivery_interrupts(&mut self) {
        self.inner.unmask_rx_delivery_interrupts();
    }
}

/// The WDEVPWR interrupt bank (`MODEM_WIFI_PWR`), which the radio interrupt
/// handler holds.
pub struct MacPowerInterruptRegisters {
    pub(crate) inner: PacMacPowerInterruptRegisters,
}

impl oer_mmio::InterruptSource for MacPowerInterruptRegisters {
    const SOURCE: u16 =
        <oer_esp32s31_pac::modem_pwr::WifiMacPowerInterrupt as oer_mmio::InterruptSource>::SOURCE;
}

impl MacPowerInterruptRegisters {
    /// Mask every WDEVPWR cause; nothing is acknowledged.
    pub fn mask_all_power_interrupts(&mut self) {
        self.inner.mask_all_power_interrupts();
    }

    /// Acknowledge every latched WDEVPWR cause through the generated image,
    /// which masks the bank first.
    pub fn clear_all_power_interrupts(&mut self) {
        self.inner.clear_all_power_interrupts();
    }

    /// Enable the station TBTT event.
    pub fn enable_station_tbtt_interrupt(&mut self) {
        self.inner.enable_station_tbtt_interrupt();
    }

    pub fn power_interrupt_status(&self) -> MacPowerInterruptSnapshot {
        self.inner.power_interrupt_status()
    }

    pub fn acknowledge_power_interrupts(&mut self, snapshot: MacPowerInterruptSnapshot) {
        self.inner.acknowledge_power_interrupts(snapshot);
    }
}

/// Executor-neutral interrupt/event edge.
pub trait AsyncEvent {
    type Event;
    type Error;

    fn wait(&mut self) -> impl Future<Output = Result<Self::Event, Self::Error>> + '_;
}

impl RadioAccess {
    /// Native twelve-bit NF code and decoded quarter-dB observation from one read.
    pub fn noise_floor_snapshot(&self) -> (u16, i32) {
        self.registers.noise_floor_snapshot()
    }
}

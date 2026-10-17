//! Hardware ownership, capabilities, and affine lifecycle transitions.

use crate::{
    ieee80211::{channel, mac as wifi_mac, station_wake::StationWakeState},
    shared_radio::SharedRadioLease,
    types::{
        MacInterruptMask, MacInterruptSnapshot, MacPowerInterruptSnapshot, MacPowerWakeCause,
        PhyAdcRate,
    },
};

use super::*;

use crate::phy::restore::PhyRouteState;
use oer_esp32s31_pac::WifiRadioRegisters;

pub mod maintenance;

pub use crate::phy::registration::PhyRegistrationEpoch;

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

/// Protocol routes that can lend the shared radio PHY.
///
/// The route is part of every [`SharedPhyHal`] type, so an operation that is
/// valid only for one route cannot accept a borrow minted by another.
pub mod route {
    mod sealed {
        pub trait Route {}
    }

    /// Closed set of routes; downstream crates cannot add a lender.
    pub trait Route: sealed::Route {}

    /// Borrowed through a lease of the shared radio arbiter.
    pub enum Shared {}

    impl sealed::Route for Shared {}
    impl Route for Shared {}
}

/// Narrow borrowed HAL capability for the protocol-neutral radio PHY.
///
/// This value cannot acquire, release, or recover the underlying PAC owner.
/// Its lifetime is bounded by the active route `R` that lent it.
///
/// A borrow keeps its route through ordinary moves:
///
/// ```
/// use oer_esp32s31_hal::owner::{SharedPhyHal, route};
/// fn keep(phy: SharedPhyHal<'_, route::Shared>) -> SharedPhyHal<'_, route::Shared> {
///     phy
/// }
/// ```
pub struct SharedPhyHal<'owner, R: route::Route> {
    pub(crate) registers: &'owner mut RadioPhyRegisters,
    pub(crate) restore: &'owner mut PhyRouteState,
    route: core::marker::PhantomData<fn() -> R>,
}

impl<'owner, R: route::Route> SharedPhyHal<'owner, R> {
    pub(crate) fn new(
        registers: &'owner mut RadioPhyRegisters,
        restore: &'owner mut PhyRouteState,
    ) -> Self {
        Self {
            registers,
            restore,
            route: core::marker::PhantomData,
        }
    }
}

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
/// External crates can use an acquired [`SharedPhyHal`] or channel borrow but cannot implement this trait for an arbitrary owner or recover
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
        phy_pac_mut(self).set_phy_calibration_clock(enabled);
    }

    fn set_rx_gain_dc_calibration(&mut self, enabled: bool) {
        phy_pac_mut(self).set_rx_gain_dc_calibration(enabled);
    }

    fn configure_power_control_tone(&mut self, selector: u16, step: u8) {
        phy_pac_mut(self).configure_power_control_tone(selector, step);
    }

    fn configure_calibration_tone(&mut self, enabled: bool, selector: u16, step: u8) {
        phy_pac_mut(self).configure_calibration_tone(enabled, selector, step);
    }

    fn configure_tx_iq_correction(&mut self, begin: bool) {
        phy_pac_mut(self).configure_tx_iq_correction(begin);
    }

    /// Capture the first-path TX-IQ tone-control state into the route slot.
    ///
    /// A second caller is rejected before reading the register and therefore
    /// cannot replace another calibration's restore authority.
    fn prepare_txiq_tone_control_restore(
        &mut self,
    ) -> Result<(), crate::phy::restore::TxIqToneControlPrepareError> {
        let (phy, restore) = sealed::SharedPhyAccess::parts_mut(self);
        restore.prepare_txiq_with(|| phy.capture_txiq_tone_control())
    }

    /// Restore and consume the saved TX-IQ tone-control state.
    ///
    /// A caller without a successful prepare operation is rejected before
    /// MMIO. The slot is cleared only after the complete accessor write.
    fn restore_txiq_tone_control(
        &mut self,
    ) -> Result<(), crate::phy::restore::TxIqToneControlRestoreError> {
        let (phy, restore) = sealed::SharedPhyAccess::parts_mut(self);
        restore.restore_txiq_with(|fields| phy.restore_txiq_tone_control(fields))
    }

    fn configure_txiq_mismatch_power(
        &mut self,
        first: bool,
        polarity: bool,
        attenuation: u8,
        selector: u16,
    ) {
        phy_pac_mut(self).configure_txiq_mismatch_power(first, polarity, attenuation, selector);
    }

    fn set_tx_iq_gain_coefficient(&mut self, coefficient: i8) {
        phy_pac_mut(self).set_tx_iq_gain_coefficient(coefficient);
    }

    fn set_tx_iq_phase_coefficient(&mut self, coefficient: i8) {
        phy_pac_mut(self).set_tx_iq_phase_coefficient(coefficient);
    }

    fn set_rx_iq_gain_coefficient(&mut self, coefficient: i8) {
        phy_pac_mut(self).set_rx_iq_gain_coefficient(coefficient);
    }

    fn set_rx_iq_phase_coefficient(&mut self, coefficient: i8) {
        phy_pac_mut(self).set_rx_iq_phase_coefficient(coefficient);
    }

    fn configure_rx_iq_calibration_mode(&mut self) {
        phy_pac_mut(self).configure_rx_iq_calibration_mode();
    }

    fn configure_adc_rate(&mut self, rate: PhyAdcRate) {
        phy_pac_mut(self).configure_adc_rate(rate);
    }

    fn set_power_detector_tone_armed(&mut self, armed: bool) {
        phy_pac_mut(self).set_power_detector_tone_armed(armed);
    }

    fn stop_power_detector_tone(&mut self) {
        phy_pac_mut(self).stop_power_detector_tone();
    }

    fn trigger_tx_dc_measurement(&mut self) {
        phy_pac_mut(self).trigger_tx_dc_measurement();
    }

    fn tx_dc_measurement_is_ready(&mut self) -> bool {
        phy_pac_mut(self).tx_dc_measurement_is_ready()
    }

    fn sample_tx_dc_comparators(&mut self) -> [bool; 2] {
        phy_pac_mut(self).sample_tx_dc_comparators()
    }

    fn clear_tx_dc_measurement(&mut self) {
        phy_pac_mut(self).clear_tx_dc_measurement();
    }

    fn open_frontend_baseband_internal_clocks(&mut self) {
        phy_pac_mut(self).open_frontend_baseband_internal_clocks();
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
    /// Begin a registration and retire every earlier epoch of this partition.
    ///
    /// Registration calls this before its first hardware edge. Any other call
    /// only invalidates existing registration results; it cannot mint one.
    fn begin_registration_epoch(&mut self) -> PhyRegistrationEpoch {
        sealed::SharedPhyAccess::parts_mut(self)
            .1
            .begin_registration_epoch()
    }
}

impl<R: route::Route> sealed::SharedPhyAccess for SharedPhyHal<'_, R> {
    fn pac(&self) -> &RadioPhyRegisters {
        self.registers
    }

    fn route_state(&self) -> &PhyRouteState {
        self.restore
    }

    fn parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
        (self.registers, self.restore)
    }
}

impl<R: route::Route> sealed::SharedPhyContext for SharedPhyHal<'_, R> {
    fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation {
        WifiBasebandEnableObservation::from_pac_readback(self.registers.wifi_baseband_is_enabled())
    }
}

impl<R: route::Route> SharedPhyAccess for SharedPhyHal<'_, R> {}
impl<R: route::Route> SharedPhyContext for SharedPhyHal<'_, R> {}

impl<R: route::Route> sealed::PhyInitializationAccess for SharedPhyHal<'_, R> {}

impl<R: route::Route> PhyInitializationAccess for SharedPhyHal<'_, R> {}

/// Shared PHY capability over externally supplied registers in an isolated
/// validation image.
///
/// Compiled vendor-comparison entry points receive the register partition by
/// pointer. This wrapper owns a fresh restore slot for one call, so a probe
/// cannot observe or leak another route's calibration restore obligation.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub struct ValidationSharedPhy<'registers> {
    registers: &'registers mut RadioPhyRegisters,
    restore: PhyRouteState,
}

#[cfg(feature = "validation-probes")]
impl<'registers> ValidationSharedPhy<'registers> {
    pub fn new(registers: &'registers mut RadioPhyRegisters) -> Self {
        Self {
            registers,
            restore: PhyRouteState::for_validation(),
        }
    }
}

#[cfg(feature = "validation-probes")]
impl sealed::SharedPhyAccess for ValidationSharedPhy<'_> {
    fn pac(&self) -> &RadioPhyRegisters {
        self.registers
    }

    fn route_state(&self) -> &PhyRouteState {
        &self.restore
    }

    fn parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
        (self.registers, &mut self.restore)
    }
}

#[cfg(feature = "validation-probes")]
impl sealed::SharedPhyContext for ValidationSharedPhy<'_> {
    fn wifi_baseband_enable_observation(&self) -> WifiBasebandEnableObservation {
        WifiBasebandEnableObservation::from_pac_readback(self.registers.wifi_baseband_is_enabled())
    }
}

#[cfg(feature = "validation-probes")]
impl SharedPhyAccess for ValidationSharedPhy<'_> {}

#[cfg(feature = "validation-probes")]
impl SharedPhyContext for ValidationSharedPhy<'_> {}

pub(crate) fn phy_pac(access: &(impl SharedPhyAccess + ?Sized)) -> &RadioPhyRegisters {
    sealed::SharedPhyAccess::pac(access)
}

pub(crate) fn phy_pac_mut(access: &mut (impl SharedPhyAccess + ?Sized)) -> &mut RadioPhyRegisters {
    sealed::SharedPhyAccess::parts_mut(access).0
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
/// the Wi-Fi hot path needs no arbiter lease, and every operation that
/// touches shared radio registers borrows them from the lease. It can move
/// between lifecycle owners and the runtime arena, but exposes no PAC owner,
/// dereference operation, or generic register callback.
pub struct RadioRuntimeOwner {
    pub(crate) registers: WifiRadioRegisters,
    station_wake: StationWakeState,
    initialized: bool,
}

impl RadioRuntimeOwner {
    /// Construct the running register owner inside an isolated validation
    /// image without exposing the underlying PAC owner.
    #[cfg(any(test, feature = "validation-probes"))]
    #[doc(hidden)]
    pub fn claim_for_validation() -> Self {
        let (_shared, partitions) =
            crate::root::RadioHardware::for_validation().into_concurrent(());
        let clocked = crate::ieee80211::client::WifiClocked::for_validation(
            crate::ieee80211::client::WifiCold::from_partition(partitions.wifi),
        );
        clocked.into_running().0
    }

    pub(crate) fn from_clocked(
        registers: WifiRadioRegisters,
        station_wake: StationWakeState,
        initialized: bool,
    ) -> Self {
        Self {
            registers,
            station_wake,
            initialized,
        }
    }

    pub(crate) fn into_clocked_parts(self) -> (WifiRadioRegisters, StationWakeState, bool) {
        (self.registers, self.station_wake, self.initialized)
    }

    pub fn wifi_mac_hal(&mut self) -> wifi_mac::WifiMacHal<'_> {
        wifi_mac::WifiMacHal::from_mac(&mut self.registers)
    }

    /// Borrow the channel HAL for one channel transaction, with the shared
    /// PHY borrowed from the arbiter lease.
    pub fn channel_hal<'hal, P, T>(
        &'hal mut self,
        platform: &'hal mut P,
        lease: &'hal mut SharedRadioLease<'_, T>,
    ) -> channel::RadioChannelHal<'hal, P> {
        let (phy, restore) = lease.phy_parts_mut();
        channel::RadioChannelHal::from_leased(platform, &mut self.registers, phy, restore)
    }

    /// Borrow the channel HAL together with the arbiter's attachment, for a
    /// PHY channel operation that updates the shared domain's state.
    pub fn channel_hal_with_attachment<'hal, P, T>(
        &'hal mut self,
        platform: &'hal mut P,
        lease: &'hal mut SharedRadioLease<'_, T>,
    ) -> (channel::RadioChannelHal<'hal, P>, &'hal mut T) {
        let (phy, restore, attachment) = lease.phy_parts_with_attachment();
        (
            channel::RadioChannelHal::from_leased(platform, &mut self.registers, phy, restore),
            attachment,
        )
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
        self.registers.mac_rx_dma_snapshot()
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
    /// are Wi-Fi MAC registers, so this needs no arbiter lease.
    pub fn station_wake_hal(&mut self) -> crate::ieee80211::station_wake::StationWakeHal<'_> {
        let (registers, state) = self.station_wake_parts_mut();
        crate::ieee80211::station_wake::StationWakeHal::from_owned(registers, state)
    }
}

/// Task-side setup authority for one finite MAC interrupt epoch.
pub struct MacInterruptSetup {
    pub(crate) inner: PacMacInterruptSetup,
}

/// Proof that connected-STA interrupt policy was applied before activation.
pub struct ConnectedStaInterruptPrepared {
    pub(crate) _private: (),
}

/// Disjoint HAL capability installed in the hard MAC interrupt handler.
pub struct MacInterruptRegisters {
    pub(crate) inner: PacMacInterruptRegisters,
}

impl MacInterruptRegisters {
    /// Apply connected-STA interrupt policy while the physical route remains
    /// installed. The platform adapter must exclude the ISR while lending
    /// this capability to task context.
    pub fn prepare_connected_sta_without_power_save(
        &mut self,
        radio: &mut RadioRuntimeOwner,
    ) -> ConnectedStaInterruptPrepared {
        let _ = self
            .inner
            .prepare_connected_sta_without_power_save(&mut radio.registers);
        ConnectedStaInterruptPrepared { _private: () }
    }

    pub(crate) fn prepare_connected_sta_with_pac(
        &mut self,
        registers: &mut WifiRadioRegisters,
    ) -> ConnectedStaInterruptPrepared {
        let _ = self
            .inner
            .prepare_connected_sta_without_power_save(registers);
        ConnectedStaInterruptPrepared { _private: () }
    }

    pub fn mac_interrupt_status(&self) -> MacInterruptSnapshot {
        self.inner.mac_interrupt_status()
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

    pub fn deactivate(self, power: MacPowerInterruptRegisters) -> MacInterruptSetup {
        MacInterruptSetup {
            inner: self.inner.deactivate(power.inner),
        }
    }
}

/// Construct the task-side interrupt setup owner inside an isolated probe.
///
/// The returned value is the same finite capability consumed by production
/// connected-STA setup; only its singleton acquisition is validation-only.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_interrupt_setup() -> MacInterruptSetup {
    MacInterruptSetup {
        inner: oer_esp32s31_pac::validation::mac_interrupt_setup(),
    }
}

/// Construct the hard-MAC interrupt capability inside an isolated validation
/// image without exposing the PAC partition.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_interrupt_registers() -> MacInterruptRegisters {
    MacInterruptRegisters {
        inner: oer_esp32s31_pac::validation::mac_interrupt_registers(),
    }
}

/// Construct the hard power-interrupt capability inside an isolated probe.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub fn validation_mac_power_interrupt_registers() -> MacPowerInterruptRegisters {
    MacPowerInterruptRegisters {
        inner: oer_esp32s31_pac::validation::mac_power_interrupt_registers(),
    }
}

/// Disjoint HAL capability installed in the hard power interrupt handler.
pub struct MacPowerInterruptRegisters {
    pub(crate) inner: PacMacPowerInterruptRegisters,
}

impl MacPowerInterruptRegisters {
    pub fn mask_and_acknowledge_wake_cause(&mut self, cause: MacPowerWakeCause) {
        self.inner.mask_and_acknowledge_wake_cause(cause);
    }

    pub fn acknowledge_wake_cause(&mut self, cause: MacPowerWakeCause) {
        self.inner.acknowledge_wake_cause(cause);
    }

    pub fn power_interrupt_status(&self) -> MacPowerInterruptSnapshot {
        self.inner.power_interrupt_status()
    }

    pub fn acknowledge_power_interrupts(&mut self, snapshot: MacPowerInterruptSnapshot) {
        self.inner.acknowledge_power_interrupts(snapshot);
    }
}

impl MacInterruptSetup {
    pub fn prepare_connected_sta_without_power_save(
        &mut self,
        radio: &mut RadioRuntimeOwner,
    ) -> ConnectedStaInterruptPrepared {
        let _ = self
            .inner
            .prepare_connected_sta_without_power_save(&mut radio.registers);
        ConnectedStaInterruptPrepared { _private: () }
    }

    pub(crate) fn prepare_connected_sta_with_pac(
        &mut self,
        registers: &mut WifiRadioRegisters,
    ) -> ConnectedStaInterruptPrepared {
        let _ = self
            .inner
            .prepare_connected_sta_without_power_save(registers);
        ConnectedStaInterruptPrepared { _private: () }
    }

    pub fn activate(
        self,
        event_mask: MacInterruptMask,
    ) -> (MacInterruptRegisters, MacPowerInterruptRegisters) {
        let (mac, power) = self.inner.activate(event_mask);
        (
            MacInterruptRegisters { inner: mac },
            MacPowerInterruptRegisters { inner: power },
        )
    }
}

/// Executor-neutral interrupt/event edge.
pub trait AsyncEvent {
    type Event;
    type Error;

    fn wait(&mut self) -> impl Future<Output = Result<Self::Event, Self::Error>> + '_;
}

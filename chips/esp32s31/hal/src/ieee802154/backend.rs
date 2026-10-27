//! Executor-neutral lower HAL boundary for ESP32-S31 IEEE 802.15.4.
//!
//! This module deliberately stops below a public radio role or MAC facade.
//! The production backend borrows the task owner and the inactive interrupt
//! owner of [`mac`](crate::ieee802154::mac). Both the backend trait and this
//! module remain crate-private: callers never implement register access or
//! supply addresses, masks, or complete register images.

#![forbid(unsafe_code)]
// Numeric state diagnostics are already typed but remain reserved for the
// later dataplane transition.
#![cfg_attr(not(test), allow(dead_code))]

use oer_espressif_ieee802154_mac_values::{
    Ieee802154CcaMode, Ieee802154EdSampleMode, Ieee802154StateSnapshot,
};
use oer_mmio::Placement;

use crate::ieee802154::{
    lifecycle::Ieee802154Channel,
    mac::{
        EtmPlacement, Ieee802154FoundationSnapshot, Ieee802154InterruptSetupOwner, Ieee802154Pti,
        Ieee802154TaskOwner, MacPlacement, RxInfoPlacement,
    },
    policy::{
        Ieee802154AckTimeout, Ieee802154MacControl, Ieee802154MacPolicySnapshot,
        Ieee802154PanIdentity,
    },
};

/// Typed register operations required by the first IEEE 802.15.4 foundation.
///
/// The trait is crate-private and its production implementation is
/// [`Ieee802154PolledMac`]. A host fake can still prove HAL delegation and
/// state predicates inside this module's unit tests.
pub(crate) trait Ieee802154RegisterBackend {
    /// Mask every MAC event before an ISR owner exists.
    fn mask_all_events(&mut self);

    /// Mask every receive-abort source before an RX dataplane exists.
    fn mask_all_rx_aborts(&mut self);

    /// Mask every transmit-abort source before a TX dataplane exists.
    fn mask_all_tx_aborts(&mut self);

    /// Select average energy-detection sampling.
    fn select_average_ed_sampling(&mut self);

    /// Apply the vendor MAC-initialization delays.
    fn apply_txon_delay(&mut self);

    /// Replace only the MAC frequency-code field with the channel's code.
    fn set_channel(&mut self, channel: Ieee802154Channel);

    /// Replace the CCA-mode field.
    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode);

    /// Replace the signed eight-bit CCA-threshold field.
    fn set_cca_threshold_code(&mut self, threshold: i8);

    /// Apply all six MAC-control fields in their source-confirmed inner order.
    fn set_mac_control(&mut self, control: Ieee802154MacControl);

    /// Replace the complete ACK-timeout field.
    fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout);

    /// Configure and enable the primary PAN identity.
    fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity);

    /// Replace only the TX/RX coexistence priority field.
    fn set_txrx_pti(&mut self, pti: Ieee802154Pti);

    /// Replace only the ACK coexistence priority field.
    fn set_ack_pti(&mut self, pti: Ieee802154Pti);

    /// Sample the fields written by the interrupt-masked foundation.
    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot;

    /// Sample the complete known static-policy subset once per backing word.
    fn mac_policy_snapshot(&mut self) -> Ieee802154MacPolicySnapshot;

    /// Sample the receive and transmit state fields once each.
    fn sample_state(&mut self) -> Ieee802154StateSnapshot;

    /// Fence a completed ownership publication or handoff.
    fn order_device_accesses(&mut self);
}

/// The task owner with the inactive interrupt owner borrowed beside it.
///
/// Borrowing the inactive owner keeps every polled operation away from an
/// active hard-IRQ owner: activation consumes it.
pub(crate) struct Ieee802154PolledMac<
    'registers,
    P: Placement = MacPlacement,
    E: Placement = EtmPlacement,
    B: Placement = RxInfoPlacement,
> {
    task: &'registers mut Ieee802154TaskOwner<P, E, B>,
    interrupts: &'registers Ieee802154InterruptSetupOwner<P>,
}

impl<'registers, P: Placement, E: Placement, B: Placement>
    Ieee802154PolledMac<'registers, P, E, B>
{
    pub(crate) fn new(
        task: &'registers mut Ieee802154TaskOwner<P, E, B>,
        interrupts: &'registers Ieee802154InterruptSetupOwner<P>,
    ) -> Self {
        Self { task, interrupts }
    }
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154RegisterBackend
    for Ieee802154PolledMac<'_, P, E, B>
{
    fn mask_all_events(&mut self) {
        self.task.mask_all_events();
    }

    fn mask_all_rx_aborts(&mut self) {
        self.task.mask_all_rx_aborts();
    }

    fn mask_all_tx_aborts(&mut self) {
        self.task.mask_all_tx_aborts();
    }

    fn select_average_ed_sampling(&mut self) {
        self.task
            .set_ed_sample_mode(Ieee802154EdSampleMode::Average);
    }

    fn apply_txon_delay(&mut self) {
        self.task.apply_txon_delay();
    }

    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.task.set_channel(channel);
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.task.set_cca_mode(mode);
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.task.set_cca_threshold_code(threshold);
    }

    fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.task.set_mac_control(control);
    }

    fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout) {
        self.task.set_ack_timeout(timeout.units());
    }

    fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.task.set_primary_pan_identity(identity);
    }

    fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.task.set_txrx_pti(pti);
    }

    fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.task.set_ack_pti(pti);
    }

    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        self.task.foundation_snapshot()
    }

    fn mac_policy_snapshot(&mut self) -> Ieee802154MacPolicySnapshot {
        self.task.mac_policy_snapshot()
    }

    fn sample_state(&mut self) -> Ieee802154StateSnapshot {
        self.interrupts.state_snapshot()
    }

    fn order_device_accesses(&mut self) {
        self.task.order_device_accesses();
    }
}

/// Temporary, exclusive borrow of one typed IEEE 802.15.4 register backend.
///
/// The wrapper has no raw-register escape hatch and owns no lifecycle state.
/// The whole-radio IEEE 802.15.4 role lends the polled owners here without
/// exposing the complete shared radio owner.
pub(crate) struct Ieee802154Hal<B: Ieee802154RegisterBackend> {
    backend: B,
}

/// Concrete HAL capability over the chip's MAC owners.
pub(crate) type Ieee802154MacHal<'registers> = Ieee802154Hal<Ieee802154PolledMac<'registers>>;

impl<'registers> Ieee802154MacHal<'registers> {
    /// Borrow the task owner and the inactive interrupt owner.
    pub(crate) fn from_owned(
        task: &'registers mut Ieee802154TaskOwner,
        interrupts: &'registers Ieee802154InterruptSetupOwner,
    ) -> Self {
        Self::from_register_backend(Ieee802154PolledMac::new(task, interrupts))
    }
}

impl<B: Ieee802154RegisterBackend> Ieee802154Hal<B> {
    /// Bind one exclusive semantic register backend.
    ///
    /// This constructor is a lower-layer integration hook, not a radio-role
    /// constructor; it neither powers nor enables the MAC.
    #[doc(hidden)]
    pub(crate) fn from_register_backend(backend: B) -> Self {
        Self { backend }
    }

    #[cfg(test)]
    fn into_register_backend(self) -> B {
        self.backend
    }

    pub(crate) fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.backend.set_channel(channel);
    }

    pub(crate) fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.backend.set_cca_mode(mode);
    }

    pub(crate) fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.backend.set_cca_threshold_code(threshold);
    }

    pub(crate) fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.backend.set_mac_control(control);
    }

    pub(crate) fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout) {
        self.backend.set_ack_timeout(timeout);
    }

    pub(crate) fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.backend.set_primary_pan_identity(identity);
    }

    pub(crate) fn mask_all_events(&mut self) {
        self.backend.mask_all_events();
    }

    pub(crate) fn mask_all_rx_aborts(&mut self) {
        self.backend.mask_all_rx_aborts();
    }

    pub(crate) fn mask_all_tx_aborts(&mut self) {
        self.backend.mask_all_tx_aborts();
    }

    pub(crate) fn select_average_ed_sampling(&mut self) {
        self.backend.select_average_ed_sampling();
    }

    pub(crate) fn apply_txon_delay(&mut self) {
        self.backend.apply_txon_delay();
    }

    pub(crate) fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.backend.set_txrx_pti(pti);
    }

    pub(crate) fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.backend.set_ack_pti(pti);
    }

    pub(crate) fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        self.backend.foundation_snapshot()
    }

    pub(crate) fn mac_policy_snapshot(&mut self) -> Ieee802154MacPolicySnapshot {
        self.backend.mac_policy_snapshot()
    }

    /// Sample the typed RX/TX state fields once each.
    pub(crate) fn state_snapshot(&mut self) -> Ieee802154StateSnapshot {
        self.backend.sample_state()
    }

    /// Test only whether both sampled numeric state codes are zero.
    ///
    /// This deliberately does not claim reset readiness or lifecycle
    /// quiescence before those semantics are recovered.
    pub(crate) fn state_codes_are_zero(&mut self) -> bool {
        self.state_snapshot().all_codes_zero()
    }

    /// Publish a device-ordering boundary after a completed typed operation.
    pub(crate) fn order_device_accesses(&mut self) {
        self.backend.order_device_accesses();
    }
}

#[cfg(test)]
mod tests;

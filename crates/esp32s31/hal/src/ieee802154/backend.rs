//! Executor-neutral lower HAL boundary for ESP32-S31 IEEE 802.15.4.
//!
//! This module deliberately stops below a public radio role or MAC facade.
//! The production backend is the narrow PAC lease over the generated
//! `IEEE802154_MAC` peripheral. Both the backend trait and this module remain
//! crate-private: callers never implement register access or supply addresses,
//! masks, or complete register images.

#![forbid(unsafe_code)]
// Frequency programming and numeric state diagnostics are already typed but
// remain reserved for the later PHY/dataplane transition. Keeping them inside
// this closed module avoids reopening raw PAC access in that iteration.
#![cfg_attr(not(test), allow(dead_code))]

use oer_esp32s31_pac::{
    Ieee802154AckTimeoutUnits as PacAckTimeoutUnits, Ieee802154CcaMode as PacCcaMode,
    Ieee802154EdDurationUnits as PacEdDurationUnits, Ieee802154MacControl as PacMacControl,
    Ieee802154MacPolicySnapshot as PacMacPolicySnapshot, Ieee802154PanIdentity as PacPanIdentity,
};

pub(crate) use oer_esp32s31_pac::{
    Ieee802154FoundationSnapshot, Ieee802154FrequencyCode, Ieee802154InterruptSetup,
    Ieee802154PolledRegisterLease, Ieee802154Pti, Ieee802154RegisterLease, Ieee802154StateSnapshot,
    Ieee802154TaskRegisters,
};

use crate::ieee802154::policy::{
    Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154MacControl, Ieee802154MacPolicySnapshot,
    Ieee802154PanIdentity,
};

/// Convert a source-level `uint16_t` ED duration into the register field.
///
/// The reviewed field is sixteen bits wide, so every duration is accepted.
pub(crate) const fn ed_duration_units(units: u16) -> PacEdDurationUnits {
    match PacEdDurationUnits::new(units as u32) {
        Some(duration) => duration,
        None => panic!("the reviewed ED-duration field is sixteen bits wide"),
    }
}

/// Typed register operations required by the first IEEE 802.15.4 foundation.
///
/// The trait is crate-private and its production implementation is sealed to
/// [`Ieee802154PolledRegisterLease`]. A host fake can still prove HAL delegation and
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

    /// Apply the vendor MAC-initialization receive-on delay.
    fn apply_txon_delay(&mut self);

    /// Replace only the opaque eight-bit MAC frequency-code field.
    fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode);

    /// Replace the source-confirmed CCA-mode field.
    fn set_cca_mode(&mut self, mode: PacCcaMode);

    /// Replace the signed eight-bit CCA-threshold field.
    fn set_cca_threshold_code(&mut self, threshold: i8);

    /// Apply all six MAC-control fields in their source-confirmed inner order.
    fn set_mac_control(&mut self, control: PacMacControl);

    /// Replace the complete ACK-timeout field.
    fn set_ack_timeout(&mut self, timeout: PacAckTimeoutUnits);

    /// Configure and enable the primary PAN identity.
    fn set_primary_pan_identity(&mut self, identity: PacPanIdentity);

    /// Replace only the recovered TX/RX coexistence priority field.
    fn set_txrx_pti(&mut self, pti: Ieee802154Pti);

    /// Replace only the recovered ACK coexistence priority field.
    fn set_ack_pti(&mut self, pti: Ieee802154Pti);

    /// Sample the fields written by the interrupt-masked foundation.
    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot;

    /// Sample the complete known static-policy subset once per backing word.
    fn mac_policy_snapshot(&mut self) -> PacMacPolicySnapshot;

    /// Sample the receive and transmit state fields once each.
    fn sample_state(&mut self) -> Ieee802154StateSnapshot;

    /// Fence a completed ownership publication or handoff.
    fn order_device_accesses(&mut self);
}

impl Ieee802154RegisterBackend for Ieee802154PolledRegisterLease<'_> {
    fn mask_all_events(&mut self) {
        Ieee802154PolledRegisterLease::mask_all_events(self);
    }

    fn mask_all_rx_aborts(&mut self) {
        Ieee802154PolledRegisterLease::mask_all_rx_aborts(self);
    }

    fn mask_all_tx_aborts(&mut self) {
        Ieee802154PolledRegisterLease::mask_all_tx_aborts(self);
    }

    fn select_average_ed_sampling(&mut self) {
        Ieee802154PolledRegisterLease::select_average_ed_sampling(self);
    }

    fn apply_txon_delay(&mut self) {
        Ieee802154PolledRegisterLease::apply_txon_delay(self);
    }

    fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
        Ieee802154RegisterLease::set_frequency_code(self, code);
    }

    fn set_cca_mode(&mut self, mode: PacCcaMode) {
        Ieee802154RegisterLease::set_cca_mode(self, mode);
    }

    fn set_cca_threshold_code(&mut self, threshold: i8) {
        Ieee802154RegisterLease::set_cca_threshold_code(self, threshold);
    }

    fn set_mac_control(&mut self, control: PacMacControl) {
        Ieee802154RegisterLease::set_mac_control(self, control);
    }

    fn set_ack_timeout(&mut self, timeout: PacAckTimeoutUnits) {
        Ieee802154RegisterLease::set_ack_timeout(self, timeout);
    }

    fn set_primary_pan_identity(&mut self, identity: PacPanIdentity) {
        Ieee802154RegisterLease::set_primary_pan_identity(self, identity);
    }

    fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        Ieee802154RegisterLease::set_txrx_pti(self, pti);
    }

    fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        Ieee802154RegisterLease::set_ack_pti(self, pti);
    }

    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot {
        Ieee802154PolledRegisterLease::foundation_snapshot(self)
    }

    fn mac_policy_snapshot(&mut self) -> PacMacPolicySnapshot {
        Ieee802154RegisterLease::mac_policy_snapshot(self)
    }

    fn sample_state(&mut self) -> Ieee802154StateSnapshot {
        self.state_snapshot()
    }

    fn order_device_accesses(&mut self) {
        Ieee802154RegisterLease::order_device_accesses(self);
    }
}

/// Temporary, exclusive borrow of one typed IEEE 802.15.4 register backend.
///
/// The wrapper has no raw-register escape hatch and owns no lifecycle state.
/// The whole-radio IEEE 802.15.4 role lends the closed PAC implementation here
/// without exposing the complete shared radio owner.
pub(crate) struct Ieee802154Hal<B: Ieee802154RegisterBackend> {
    backend: B,
}

/// Concrete closed HAL capability backed by the generated ESP32-S31 PAC.
pub(crate) type Ieee802154PacHal<'registers> =
    Ieee802154Hal<Ieee802154PolledRegisterLease<'registers>>;

impl<'registers> Ieee802154PacHal<'registers> {
    /// Borrow the IEEE 802.15.4 leaf from the unique task-side radio owner.
    pub(crate) fn from_owned(
        registers: &'registers mut Ieee802154TaskRegisters,
        interrupts: &'registers mut Ieee802154InterruptSetup,
    ) -> Self {
        Self::from_register_backend(interrupts.polled_register_lease(registers))
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

    pub(crate) fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
        self.backend.set_frequency_code(code);
    }

    pub(crate) fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.backend
            .set_cca_mode(crate::ieee802154::policy::cca_mode_into_pac(mode));
    }

    pub(crate) fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.backend.set_cca_threshold_code(threshold);
    }

    pub(crate) fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.backend.set_mac_control(control.into_pac());
    }

    pub(crate) fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout) {
        self.backend.set_ack_timeout(timeout.into_pac());
    }

    pub(crate) fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.backend.set_primary_pan_identity(identity.into_pac());
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
        Ieee802154MacPolicySnapshot::from_pac(self.backend.mac_policy_snapshot())
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

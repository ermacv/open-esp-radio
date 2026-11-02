// SPDX-License-Identifier: Apache-2.0
//! Hardware-free configuration observations and owned register-port vocabulary.

use crate::{
    Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154Channel, Ieee802154MacControl,
    Ieee802154MultipanEnableState, Ieee802154PanIdentity,
};

/// Readback of the interrupt-masked MAC foundation.
///
/// It excludes `EVENT_STATUS`: the foundation neither owns nor acknowledges
/// pending events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154FoundationSnapshot {
    events_masked: bool,
    rx_aborts_masked: bool,
    tx_aborts_masked: bool,
    ed_uses_average: bool,
    txrx_pti: u8,
    ack_pti: u8,
    txon_delay_applied: bool,
}

impl Ieee802154FoundationSnapshot {
    /// One field-level observation; construction grants no phase authority.
    pub const fn new(
        events_masked: bool,
        rx_aborts_masked: bool,
        tx_aborts_masked: bool,
        ed_uses_average: bool,
        txrx_pti: u8,
        ack_pti: u8,
        txon_delay_applied: bool,
    ) -> Self {
        Self {
            events_masked,
            rx_aborts_masked,
            tx_aborts_masked,
            ed_uses_average,
            txrx_pti,
            ack_pti,
            txon_delay_applied,
        }
    }

    pub const fn events_masked(self) -> bool {
        self.events_masked
    }

    pub const fn rx_aborts_masked(self) -> bool {
        self.rx_aborts_masked
    }

    pub const fn tx_aborts_masked(self) -> bool {
        self.tx_aborts_masked
    }

    pub const fn ed_uses_average(self) -> bool {
        self.ed_uses_average
    }

    pub const fn txrx_pti(self) -> u8 {
        self.txrx_pti
    }

    pub const fn ack_pti(self) -> u8 {
        self.ack_pti
    }

    /// Whether `TXON_DELAY`, `TXOFF_DELAY`, `RXON_DELAY` and
    /// `TXRX_SWITCH_DELAY` hold the vendor MAC-initialization values.
    pub const fn txon_delay_applied(self) -> bool {
        self.txon_delay_applied
    }
}

/// Semantic readback after the two private reset pulses.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154ResetReadback {
    pub mac_reset_released: bool,
    pub apb_reset_released: bool,
}

/// Private MAC reset lines in the shared modem syscon block.
///
/// An implementation borrows the shared radio registers for one reset
/// transition; it never retains them.
pub trait Ieee802154ResetPort {
    fn set_ieee802154_mac_reset(&mut self, asserted: bool);
    fn set_ieee802154_apb_reset(&mut self, asserted: bool);
    fn ieee802154_reset_readback(&self) -> Ieee802154ResetReadback;
}

/// Register atoms for the finite family foundation driver.
///
/// An implementation must retain exclusive access to the IEEE 802.15.4
/// partition for the lifetime of the returned typestate value. In
/// particular, implementations must not reconstruct the IEEE 802.15.4 block
/// from an address or independently claim a second peripheral singleton.
pub trait Ieee802154LifecycleBackend {
    /// The reset loan of this backend, rather than an unrelated caller's port.
    type ResetPort<'a>: Ieee802154ResetPort
    where
        Self: 'a;

    /// Prevent every peripheral event from reaching the future MAC IRQ route.
    fn mask_all_events(&mut self);

    /// Keep every RX-abort source masked until the receive dataplane exists.
    fn mask_all_rx_aborts(&mut self);

    /// Keep every TX-abort source masked until the transmit dataplane exists.
    fn mask_all_tx_aborts(&mut self);

    fn select_average_ed_sampling(&mut self);
    fn disable_txrx_coex(&mut self);
    fn disable_ack_coex(&mut self);

    /// Apply the vendor MAC-initialization receive-on delay.
    fn apply_txon_delay(&mut self);

    /// Order completed foundation writes before publishing the next typestate.
    fn order_device_accesses(&mut self);

    /// Sample the safe, non-operational foundation image.
    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot;
}

/// One field-level image sampled after all deterministic static-policy writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacPolicySnapshot {
    pub frequency_code: u8,
    pub cca_mode: Ieee802154CcaMode,
    pub cca_threshold_code: i8,
    pub ack_timeout: Ieee802154AckTimeout,
    pub control: Ieee802154MacControl,
    pub multipan_enable_state: Ieee802154MultipanEnableState,
    pub identity: Ieee802154PanIdentity,
}

impl Ieee802154MacPolicySnapshot {
    /// One readback, as the registers report it.
    pub const fn new(
        frequency_code: u8,
        cca_mode: Ieee802154CcaMode,
        cca_threshold_code: i8,
        ack_timeout: Ieee802154AckTimeout,
        control: Ieee802154MacControl,
        multipan_enable_state: Ieee802154MultipanEnableState,
        identity: Ieee802154PanIdentity,
    ) -> Self {
        Self {
            frequency_code,
            cca_mode,
            cca_threshold_code,
            ack_timeout,
            control,
            multipan_enable_state,
            identity,
        }
    }
}

impl Ieee802154MacPolicySnapshot {
    pub const fn frequency_code(self) -> u8 {
        self.frequency_code
    }

    pub const fn cca_mode(self) -> Ieee802154CcaMode {
        self.cca_mode
    }

    pub const fn cca_threshold_code(self) -> i8 {
        self.cca_threshold_code
    }

    pub const fn ack_timeout(self) -> Ieee802154AckTimeout {
        self.ack_timeout
    }

    pub const fn control(self) -> Ieee802154MacControl {
        self.control
    }

    pub const fn multipan_enable_state(self) -> Ieee802154MultipanEnableState {
        self.multipan_enable_state
    }

    pub const fn identity(self) -> Ieee802154PanIdentity {
        self.identity
    }
}

/// One composite post-fence readback of the retained foundation and policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacPolicyReadback {
    pub foundation: Ieee802154FoundationSnapshot,
    pub policy: Ieee802154MacPolicySnapshot,
}

impl Ieee802154MacPolicyReadback {
    pub const fn new(
        foundation: Ieee802154FoundationSnapshot,
        policy: Ieee802154MacPolicySnapshot,
    ) -> Self {
        Self { foundation, policy }
    }
}

/// Register atoms for static-policy publication.
///
/// The family driver borrows these atoms only in its proved foundation phase.
/// Its aggregate control and identity operations preserve the source-backed
/// inner order: TX auto ACK, RX auto ACK, enhanced ACK TX, coordinator,
/// promiscuous, enhanced pending; then primary-context enable before each
/// address class.
pub trait Ieee802154MacPolicyWrites {
    fn set_channel(&mut self, channel: Ieee802154Channel);
    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode);
    fn set_cca_threshold_code(&mut self, threshold: i8);
    fn set_mac_control(&mut self, control: Ieee802154MacControl);
    fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeout);
    fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity);
    fn order_device_accesses(&mut self);
}

/// Cold static-policy backend: the foundation owner can still prove that
/// every event and abort source remains masked.
pub trait Ieee802154MacPolicyBackend: Ieee802154MacPolicyWrites {
    /// Sample the foundation invariants and static policy as one semantic
    /// post-fence readback operation.
    fn mac_policy_readback(&mut self) -> Ieee802154MacPolicyReadback;
}

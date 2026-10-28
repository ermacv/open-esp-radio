//! Interrupt-masked static IEEE 802.15.4 MAC policy.
//!
//! This transition deliberately remains below an operational MAC. It neither
//! configures PHY/RF or BTBB, nor routes an IRQ, nor owns DMA buffers. Event
//! and abort enables remain masked by construction: the policy backend has no
//! operation that can change them, and `EVENT_STATUS` is never accessed.
//!
//! The policy is also not a complete vendor PIB update. In particular, the
//! public vendor path programs TX power between channel and CCA mode, but the
//! dBm-to-hardware-code mapping is still opaque and is therefore omitted.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use crate::ieee802154::{lifecycle::Ieee802154ReadbackError, mac::Ieee802154FoundationSnapshot};
use oer_espressif_ieee802154_engine::ll::COEX_DISABLED_PTI;
use oer_espressif_ieee802154_mac_values::{
    Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154Channel, Ieee802154MacControl,
    Ieee802154MultipanEnableState, Ieee802154MultipanIndex, Ieee802154PanIdentity,
};

/// Static, interrupt-masked MAC policy subset whose register semantics are known.
///
/// TX power is intentionally absent because its RF-dependent conversion table
/// has not been recovered from an authoritative public source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacPolicy {
    channel: Ieee802154Channel,
    cca_mode: Ieee802154CcaMode,
    cca_threshold_code: i8,
    ack_timeout: Ieee802154AckTimeout,
    control: Ieee802154MacControl,
    identity: Ieee802154PanIdentity,
}

impl Ieee802154MacPolicy {
    /// Construct the complete source-confirmed static-policy subset.
    ///
    /// `cca_threshold_code` is the signed register code from the public LL;
    /// this type deliberately assigns it no unproved physical unit.
    pub const fn new(
        channel: Ieee802154Channel,
        cca_mode: Ieee802154CcaMode,
        cca_threshold_code: i8,
        ack_timeout: Ieee802154AckTimeout,
        control: Ieee802154MacControl,
        identity: Ieee802154PanIdentity,
    ) -> Self {
        Self {
            channel,
            cca_mode,
            cca_threshold_code,
            ack_timeout,
            control,
            identity,
        }
    }

    /// Return the selected standardized channel.
    pub const fn channel(self) -> Ieee802154Channel {
        self.channel
    }

    /// Return the clear-channel-assessment mode.
    pub const fn cca_mode(self) -> Ieee802154CcaMode {
        self.cca_mode
    }

    /// Return the signed, unitless CCA-threshold register code.
    pub const fn cca_threshold_code(self) -> i8 {
        self.cca_threshold_code
    }

    /// Return the checked ACK timeout.
    pub const fn ack_timeout(self) -> Ieee802154AckTimeout {
        self.ack_timeout
    }

    /// Return the six MAC-control flags.
    pub const fn control(self) -> Ieee802154MacControl {
        self.control
    }

    /// Return the primary address-filter identity.
    pub const fn identity(self) -> Ieee802154PanIdentity {
        self.identity
    }
}

/// First static-policy field that failed semantic readback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154MacPolicyCheckpoint {
    /// Event enables no longer read as fully masked.
    EventsMasked,
    /// RX-abort enables no longer read as fully masked.
    RxAbortsMasked,
    /// TX-abort enables no longer read as fully masked.
    TxAbortsMasked,
    /// ED sampling no longer reads as average mode.
    EdSampleAverage,
    /// TX/RX coexistence PTI no longer reads as disabled.
    TxrxPtiDisabled,
    /// ACK coexistence PTI no longer reads as disabled.
    AckPtiDisabled,
    /// The receive-on delay no longer reads as the MAC-initialization value.
    TxOnDelayApplied,
    /// Channel frequency-code readback mismatched.
    Channel,
    /// CCA-mode readback mismatched.
    CcaMode,
    /// CCA-threshold-code readback mismatched.
    CcaThreshold,
    /// TX auto-ACK readback mismatched.
    TxAutoAck,
    /// RX auto-ACK readback mismatched.
    RxAutoAck,
    /// Enhanced-ACK TX readback mismatched.
    EnhancedAckTx,
    /// Coordinator-mode readback mismatched.
    Coordinator,
    /// Promiscuous-mode readback mismatched.
    Promiscuous,
    /// Enhanced-pending readback mismatched.
    EnhancedPending,
    /// ACK-timeout readback mismatched.
    AckTimeout,
    /// Primary Multi-PAN context zero was not enabled.
    PrimaryPanEnabled,
    /// PAN identifier readback mismatched.
    PanId,
    /// Short-address readback mismatched.
    ShortAddress,
    /// Extended-address readback mismatched.
    ExtendedAddress,
}

impl Ieee802154MacPolicyCheckpoint {
    /// Return whether this mismatch invalidates the preceding foundation
    /// typestate rather than only the attempted policy.
    pub const fn invalidates_foundation(self) -> bool {
        matches!(
            self,
            Self::EventsMasked
                | Self::RxAbortsMasked
                | Self::TxAbortsMasked
                | Self::EdSampleAverage
                | Self::TxrxPtiDisabled
                | Self::AckPtiDisabled
                | Self::TxOnDelayApplied
        )
    }
}

/// One field-level image sampled after all deterministic static-policy writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ieee802154MacPolicySnapshot {
    frequency_code: u8,
    cca_mode: Ieee802154CcaMode,
    cca_threshold_code: i8,
    ack_timeout: Ieee802154AckTimeout,
    control: Ieee802154MacControl,
    multipan_enable_state: Ieee802154MultipanEnableState,
    identity: Ieee802154PanIdentity,
}

impl Ieee802154MacPolicySnapshot {
    /// One readback, as the registers report it.
    pub(crate) const fn new(
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

    #[cfg(test)]
    const fn from_policy(policy: Ieee802154MacPolicy) -> Self {
        Self {
            frequency_code: policy.channel.frequency_code(),
            cca_mode: policy.cca_mode,
            cca_threshold_code: policy.cca_threshold_code,
            ack_timeout: policy.ack_timeout,
            control: policy.control,
            // Context zero must be enabled; unrelated enabled contexts are
            // deliberately retained and accepted by policy verification.
            multipan_enable_state: Ieee802154MultipanEnableState::new(true, false, true, false),
            identity: policy.identity,
        }
    }
}

#[cfg(test)]
impl Ieee802154MacPolicySnapshot {
    pub(crate) const fn frequency_code(self) -> u8 {
        self.frequency_code
    }

    pub(crate) const fn cca_mode(self) -> Ieee802154CcaMode {
        self.cca_mode
    }

    pub(crate) const fn cca_threshold_code(self) -> i8 {
        self.cca_threshold_code
    }

    pub(crate) const fn ack_timeout(self) -> Ieee802154AckTimeout {
        self.ack_timeout
    }

    pub(crate) const fn control(self) -> Ieee802154MacControl {
        self.control
    }

    pub(crate) const fn multipan_enable_state(self) -> Ieee802154MultipanEnableState {
        self.multipan_enable_state
    }

    pub(crate) const fn identity(self) -> Ieee802154PanIdentity {
        self.identity
    }
}

/// One composite post-fence readback of the retained foundation and policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ieee802154MacPolicyReadback {
    foundation: Ieee802154FoundationSnapshot,
    policy: Ieee802154MacPolicySnapshot,
}

impl Ieee802154MacPolicyReadback {
    pub(crate) const fn new(
        foundation: Ieee802154FoundationSnapshot,
        policy: Ieee802154MacPolicySnapshot,
    ) -> Self {
        Self { foundation, policy }
    }
}

/// Closed backend for the static-policy transition.
///
/// The production implementation is the already-proved foundation owner.
/// Its aggregate control and identity operations preserve the source-backed
/// inner order: TX auto ACK, RX auto ACK, enhanced ACK TX, coordinator,
/// promiscuous, enhanced pending; then primary-context enable before each
/// address class.
pub(crate) trait Ieee802154MacPolicyWrites {
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
pub(crate) trait Ieee802154MacPolicyBackend: Ieee802154MacPolicyWrites {
    /// Sample the foundation invariants and static policy as one semantic
    /// post-fence readback operation.
    fn mac_policy_readback(&mut self) -> Ieee802154MacPolicyReadback;
}

/// Publish the deterministic, known static policy and fence it.
///
/// The missing TX-power operation is intentionally visible in this sequence:
/// channel is followed directly by CCA mode, without a placeholder write.
fn write_mac_policy(backend: &mut impl Ieee802154MacPolicyWrites, policy: Ieee802154MacPolicy) {
    backend.set_channel(policy.channel);
    backend.set_cca_mode(policy.cca_mode);
    backend.set_cca_threshold_code(policy.cca_threshold_code);
    backend.set_mac_control(policy.control);
    backend.set_ack_timeout(policy.ack_timeout);
    backend.set_primary_pan_identity(policy.identity);
    backend.order_device_accesses();
}

/// Failed policy transition retaining the exact input owner for retry.
#[derive(Debug)]
pub(crate) struct Ieee802154MacPolicyFailure<Backend> {
    backend: Backend,
    error: Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint>,
}

impl<Backend> Ieee802154MacPolicyFailure<Backend> {
    pub(crate) const fn error(&self) -> Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint> {
        self.error
    }

    pub(crate) fn into_backend(self) -> Backend {
        self.backend
    }
}

/// Apply the static policy on the cold foundation owner and prove one sampled
/// image of the foundation invariants and every policy field.
pub(crate) fn configure_ieee802154_mac_policy<Backend>(
    mut backend: Backend,
    policy: Ieee802154MacPolicy,
) -> Result<Backend, Ieee802154MacPolicyFailure<Backend>>
where
    Backend: Ieee802154MacPolicyBackend,
{
    write_mac_policy(&mut backend, policy);
    verify_ieee802154_mac_policy(backend, policy)
}

/// Prove, without writing, that the foundation invariants and every policy
/// field read back as `policy`.
fn verify_ieee802154_mac_policy<Backend>(
    mut backend: Backend,
    policy: Ieee802154MacPolicy,
) -> Result<Backend, Ieee802154MacPolicyFailure<Backend>>
where
    Backend: Ieee802154MacPolicyBackend,
{
    let readback = backend.mac_policy_readback();
    if let Err(error) = verify_foundation_readback(readback.foundation)
        .and_then(|()| verify_mac_policy_snapshot(readback.policy, policy))
    {
        return Err(Ieee802154MacPolicyFailure { backend, error });
    }

    Ok(backend)
}

fn verify_foundation_readback(
    foundation: Ieee802154FoundationSnapshot,
) -> Result<(), Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint>> {
    verify(
        Ieee802154MacPolicyCheckpoint::EventsMasked,
        foundation.events_masked(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::RxAbortsMasked,
        foundation.rx_aborts_masked(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::TxAbortsMasked,
        foundation.tx_aborts_masked(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::EdSampleAverage,
        foundation.ed_uses_average(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::TxrxPtiDisabled,
        foundation.txrx_pti().value() == COEX_DISABLED_PTI,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::AckPtiDisabled,
        foundation.ack_pti().value() == COEX_DISABLED_PTI,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::TxOnDelayApplied,
        foundation.txon_delay_applied(),
    )?;

    Ok(())
}

fn verify_mac_policy_snapshot(
    snapshot: Ieee802154MacPolicySnapshot,
    expected: Ieee802154MacPolicy,
) -> Result<(), Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint>> {
    verify(
        Ieee802154MacPolicyCheckpoint::Channel,
        snapshot.frequency_code == expected.channel.frequency_code(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::CcaMode,
        snapshot.cca_mode == expected.cca_mode,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::CcaThreshold,
        snapshot.cca_threshold_code == expected.cca_threshold_code,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::TxAutoAck,
        snapshot.control.tx_auto_ack() == expected.control.tx_auto_ack(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::RxAutoAck,
        snapshot.control.rx_auto_ack() == expected.control.rx_auto_ack(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::EnhancedAckTx,
        snapshot.control.enhanced_ack_tx() == expected.control.enhanced_ack_tx(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::Coordinator,
        snapshot.control.coordinator() == expected.control.coordinator(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::Promiscuous,
        snapshot.control.promiscuous() == expected.control.promiscuous(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::EnhancedPending,
        snapshot.control.enhanced_pending() == expected.control.enhanced_pending(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::AckTimeout,
        snapshot.ack_timeout == expected.ack_timeout,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::PrimaryPanEnabled,
        snapshot
            .multipan_enable_state
            .contains(Ieee802154MultipanIndex::CONTEXT0),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::PanId,
        snapshot.identity.pan_id() == expected.identity.pan_id(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::ShortAddress,
        snapshot.identity.short_address() == expected.identity.short_address(),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::ExtendedAddress,
        snapshot.identity.extended_address() == expected.identity.extended_address(),
    )
}

fn verify<Checkpoint: Copy>(
    checkpoint: Checkpoint,
    observed: bool,
) -> Result<(), Ieee802154ReadbackError<Checkpoint>> {
    if observed {
        Ok(())
    } else {
        Err(Ieee802154ReadbackError {
            checkpoint,
            expected: true,
            observed,
        })
    }
}

#[cfg(test)]
mod tests;

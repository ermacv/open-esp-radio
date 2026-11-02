//! Interrupt-masked static IEEE 802.15.4 MAC policy.
//!
//! This transition deliberately remains below an operational MAC. It neither
//! configures PHY/RF or BTBB, nor routes an IRQ, nor owns DMA buffers. Event
//! and abort enables remain masked by construction: the policy backend has no
//! operation that can change them, and `EVENT_STATUS` is never accessed.
//!
//! The policy is also not a complete vendor PIB update. In particular, the
//! public vendor path programs TX power between channel and CCA mode, but the
//! RF-dependent power resolution belongs to the chip adapter and is omitted here.

#![deny(missing_docs)]

use crate::lifecycle::Ieee802154ReadbackError;
use oer_espressif_ieee802154_mac_values::COEX_DISABLED_PTI;
use oer_espressif_ieee802154_mac_values::{
    Ieee802154AckTimeout, Ieee802154CcaMode, Ieee802154Channel, Ieee802154MacControl,
    Ieee802154MultipanIndex, Ieee802154PanIdentity,
};
use oer_espressif_ieee802154_mac_values::{
    Ieee802154FoundationSnapshot, Ieee802154MacPolicyBackend, Ieee802154MacPolicySnapshot,
    Ieee802154MacPolicyWrites,
};

/// Static, interrupt-masked MAC policy subset whose register semantics are known.
///
/// TX power is resolved separately by the chip adapter.
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
pub struct Ieee802154MacPolicyFailure<Backend> {
    backend: Backend,
    error: Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint>,
}

impl<Backend> Ieee802154MacPolicyFailure<Backend> {
    /// Return the first failed foundation or policy checkpoint.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint> {
        self.error
    }

    /// Recover the same owner for typed phase recovery.
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
        foundation.txrx_pti() == COEX_DISABLED_PTI,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::AckPtiDisabled,
        foundation.ack_pti() == COEX_DISABLED_PTI,
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
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]
mod tests;

/// A statically configured MAC retaining its proved foundation.
pub struct Ieee802154MacPolicyConfigured<B> {
    foundation:
        crate::lifecycle::Ieee802154Lifecycle<B, crate::lifecycle::state::FoundationConfigured>,
    policy: Ieee802154MacPolicy,
}
impl<B> Ieee802154MacPolicyConfigured<B> {
    /// Return the proved policy.
    pub const fn policy(&self) -> Ieee802154MacPolicy {
        self.policy
    }
    /// Forget only the policy, retaining the proved foundation.
    pub fn into_foundation(
        self,
    ) -> crate::lifecycle::Ieee802154Lifecycle<B, crate::lifecycle::state::FoundationConfigured>
    {
        self.foundation
    }
}
/// Failed policy configuration, retaining the exact affine phase owner.
pub struct Ieee802154MacPolicyTransitionFailure<B> {
    inner: Ieee802154MacPolicyFailure<
        crate::lifecycle::Ieee802154Lifecycle<B, crate::lifecycle::state::FoundationConfigured>,
    >,
}
/// Strongest phase supported by the failed policy readback.
pub enum Ieee802154MacPolicyRecovery<B> {
    /// Only the policy failed; the foundation may be retried.
    Foundation(
        crate::lifecycle::Ieee802154Lifecycle<B, crate::lifecycle::state::FoundationConfigured>,
    ),
    /// A foundation field failed and must be configured again.
    Reset(crate::lifecycle::Ieee802154Lifecycle<B, crate::lifecycle::state::Reset>),
}
impl<B> Ieee802154MacPolicyTransitionFailure<B> {
    /// Return the first mismatched checkpoint.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154MacPolicyCheckpoint> {
        self.inner.error()
    }
    /// Recover only the strongest phase justified by this readback.
    pub fn into_recovery(self) -> Ieee802154MacPolicyRecovery<B> {
        let invalidates = self.inner.error().checkpoint.invalidates_foundation();
        let foundation = self.inner.into_backend();
        if invalidates {
            Ieee802154MacPolicyRecovery::Reset(foundation.forget_foundation())
        } else {
            Ieee802154MacPolicyRecovery::Foundation(foundation)
        }
    }
}
impl<B: Ieee802154MacPolicyBackend>
    crate::lifecycle::Ieee802154Lifecycle<B, crate::lifecycle::state::FoundationConfigured>
{
    /// Write and verify the static policy without enabling events or touching DMA.
    pub fn configure_mac_policy(
        self,
        policy: Ieee802154MacPolicy,
    ) -> Result<Ieee802154MacPolicyConfigured<B>, Ieee802154MacPolicyTransitionFailure<B>> {
        configure_ieee802154_mac_policy(self, policy)
            .map(|foundation| Ieee802154MacPolicyConfigured { foundation, policy })
            .map_err(|inner| Ieee802154MacPolicyTransitionFailure { inner })
    }
}

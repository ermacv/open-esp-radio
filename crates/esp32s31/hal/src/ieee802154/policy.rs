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

use oer_esp32s31_pac::{
    Ieee802154AckTimeoutUnits as PacAckTimeoutUnits, Ieee802154CcaMode as PacCcaMode,
    Ieee802154FoundationSnapshot, Ieee802154MacControl as PacMacControl,
    Ieee802154MacPolicySnapshot as PacMacPolicySnapshot,
    Ieee802154MultipanEnableState as PacMultipanEnableState,
    Ieee802154MultipanIndex as PacMultipanIndex, Ieee802154PanIdentity as PacPanIdentity,
};

use crate::ieee802154::lifecycle::{COEX_DISABLED_PTI, Ieee802154Channel, Ieee802154ReadbackError};

/// Hardware quantum used by the source-confirmed vendor ACK-timeout conversion.
pub const IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS: u32 = 16;

/// Largest microsecond request that fits the complete sixteen-bit timeout field.
pub const IEEE802154_MAX_ACK_TIMEOUT_MICROSECONDS: u32 =
    u16::MAX as u32 * IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS;

/// Checked ACK timeout represented in complete hardware field units.
///
/// The vendor conversion is `ceil(microseconds / 16)`. Construction from
/// microseconds rejects values that need more than sixteen field bits; it
/// never truncates or wraps. [`Self::from_units`] also represents every field
/// image, including zero, without inventing a narrower semantic domain.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154AckTimeout {
    units: u16,
}

/// A requested ACK timeout that cannot be represented by the hardware field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154AckTimeoutError {
    attempted_microseconds: u32,
}

impl Ieee802154AckTimeoutError {
    /// Return the rejected duration.
    pub const fn attempted_microseconds(self) -> u32 {
        self.attempted_microseconds
    }
}

impl Ieee802154AckTimeout {
    /// Construct any of the complete sixteen-bit hardware field values.
    pub const fn from_units(units: u16) -> Self {
        Self { units }
    }

    /// Convert microseconds with the source-confirmed `(micros + 15) / 16`
    /// rule, expressed without an overflowing addition.
    pub const fn from_microseconds(microseconds: u32) -> Result<Self, Ieee802154AckTimeoutError> {
        let whole_units = microseconds / IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS;
        let rounded_unit =
            if microseconds.is_multiple_of(IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS) {
                0
            } else {
                1
            };
        let units = whole_units + rounded_unit;

        if units <= u16::MAX as u32 {
            Ok(Self {
                units: units as u16,
            })
        } else {
            Err(Ieee802154AckTimeoutError {
                attempted_microseconds: microseconds,
            })
        }
    }

    /// Return the complete sixteen-bit field value.
    pub const fn units(self) -> u16 {
        self.units
    }

    /// Return the effective, upward-quantized timeout in microseconds.
    pub const fn effective_microseconds(self) -> u32 {
        self.units as u32 * IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS
    }

    pub(crate) const fn into_pac(self) -> PacAckTimeoutUnits {
        PacAckTimeoutUnits::new(self.units)
    }
}

pub use oer_espressif_ieee802154_engine::types::Ieee802154CcaMode;

/// The PAC mode of a chip-neutral CCA mode.
pub(crate) const fn cca_mode_into_pac(mode: Ieee802154CcaMode) -> PacCcaMode {
    match mode {
        Ieee802154CcaMode::Carrier => PacCcaMode::Carrier,
        Ieee802154CcaMode::EnergyDetection => PacCcaMode::EnergyDetection,
        Ieee802154CcaMode::CarrierOrEnergyDetection => PacCcaMode::CarrierOrEnergyDetection,
        Ieee802154CcaMode::CarrierAndEnergyDetection => PacCcaMode::CarrierAndEnergyDetection,
    }
}

/// The chip-neutral CCA mode of a PAC mode.
const fn cca_mode_from_pac(value: PacCcaMode) -> Ieee802154CcaMode {
    match value {
        PacCcaMode::Carrier => Ieee802154CcaMode::Carrier,
        PacCcaMode::EnergyDetection => Ieee802154CcaMode::EnergyDetection,
        PacCcaMode::CarrierOrEnergyDetection => Ieee802154CcaMode::CarrierOrEnergyDetection,
        PacCcaMode::CarrierAndEnergyDetection => Ieee802154CcaMode::CarrierAndEnergyDetection,
    }
}

/// Source-confirmed automatic-ACK and address-filter control policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacControl {
    tx_auto_ack: bool,
    rx_auto_ack: bool,
    enhanced_ack_tx: bool,
    coordinator: bool,
    promiscuous: bool,
    enhanced_pending: bool,
}

impl Ieee802154MacControl {
    /// Construct the six independently controlled policy flags in the order
    /// TX auto-ACK, RX auto-ACK, enhanced-ACK TX, coordinator, promiscuous,
    /// and enhanced-pending mode.
    pub const fn new(
        tx_auto_ack: bool,
        rx_auto_ack: bool,
        enhanced_ack_tx: bool,
        coordinator: bool,
        promiscuous: bool,
        enhanced_pending: bool,
    ) -> Self {
        Self {
            tx_auto_ack,
            rx_auto_ack,
            enhanced_ack_tx,
            coordinator,
            promiscuous,
            enhanced_pending,
        }
    }

    /// Return whether hardware automatically transmits an ACK.
    pub const fn tx_auto_ack(self) -> bool {
        self.tx_auto_ack
    }

    /// Return whether hardware automatically waits for a received ACK.
    pub const fn rx_auto_ack(self) -> bool {
        self.rx_auto_ack
    }

    /// Return whether enhanced-ACK transmission is enabled.
    pub const fn enhanced_ack_tx(self) -> bool {
        self.enhanced_ack_tx
    }

    /// Return whether coordinator filtering behavior is enabled.
    pub const fn coordinator(self) -> bool {
        self.coordinator
    }

    /// Return whether promiscuous reception is enabled.
    pub const fn promiscuous(self) -> bool {
        self.promiscuous
    }

    /// Return the one-bit enhanced-pending policy projection.
    pub const fn enhanced_pending(self) -> bool {
        self.enhanced_pending
    }

    pub(crate) const fn into_pac(self) -> PacMacControl {
        PacMacControl::new(
            self.tx_auto_ack,
            self.rx_auto_ack,
            self.enhanced_ack_tx,
            self.coordinator,
            self.promiscuous,
            self.enhanced_pending,
        )
    }

    const fn from_pac(value: PacMacControl) -> Self {
        Self::new(
            value.tx_auto_ack(),
            value.rx_auto_ack(),
            value.enhanced_ack_tx(),
            value.coordinator(),
            value.promiscuous(),
            value.enhanced_pending(),
        )
    }
}

/// Address-filter identity for the primary PAN context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PanIdentity {
    pan_id: u16,
    short_address: u16,
    extended_address: [u8; 8],
}

impl Ieee802154PanIdentity {
    /// Construct the primary PAN identity from host-order short values and the
    /// public eight-byte extended-address order.
    pub const fn new(pan_id: u16, short_address: u16, extended_address: [u8; 8]) -> Self {
        Self {
            pan_id,
            short_address,
            extended_address,
        }
    }

    /// Return the primary PAN identifier.
    pub const fn pan_id(self) -> u16 {
        self.pan_id
    }

    /// Return the primary short address.
    pub const fn short_address(self) -> u16 {
        self.short_address
    }

    /// Return the primary extended address in public byte order.
    pub const fn extended_address(self) -> [u8; 8] {
        self.extended_address
    }

    pub(crate) const fn into_pac(self) -> PacPanIdentity {
        PacPanIdentity::new(self.pan_id, self.short_address, self.extended_address)
    }

    const fn from_pac(value: PacPanIdentity) -> Self {
        Self::new(
            value.pan_id(),
            value.short_address(),
            value.extended_address(),
        )
    }
}

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
    multipan_enable_state: PacMultipanEnableState,
    identity: Ieee802154PanIdentity,
}

impl Ieee802154MacPolicySnapshot {
    pub(crate) const fn from_pac(snapshot: PacMacPolicySnapshot) -> Self {
        Self {
            frequency_code: snapshot.frequency_code().value(),
            cca_mode: cca_mode_from_pac(snapshot.cca_mode()),
            cca_threshold_code: snapshot.cca_threshold_code(),
            ack_timeout: Ieee802154AckTimeout::from_units(snapshot.ack_timeout().value()),
            control: Ieee802154MacControl::from_pac(snapshot.control()),
            multipan_enable_state: snapshot.multipan_enable_state(),
            identity: Ieee802154PanIdentity::from_pac(snapshot.identity()),
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
            multipan_enable_state: PacMultipanEnableState::new(true, false, true, false),
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

    pub(crate) const fn multipan_enable_state(self) -> PacMultipanEnableState {
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
        snapshot.control.tx_auto_ack == expected.control.tx_auto_ack,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::RxAutoAck,
        snapshot.control.rx_auto_ack == expected.control.rx_auto_ack,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::EnhancedAckTx,
        snapshot.control.enhanced_ack_tx == expected.control.enhanced_ack_tx,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::Coordinator,
        snapshot.control.coordinator == expected.control.coordinator,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::Promiscuous,
        snapshot.control.promiscuous == expected.control.promiscuous,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::EnhancedPending,
        snapshot.control.enhanced_pending == expected.control.enhanced_pending,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::AckTimeout,
        snapshot.ack_timeout == expected.ack_timeout,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::PrimaryPanEnabled,
        snapshot
            .multipan_enable_state
            .contains(PacMultipanIndex::CONTEXT0),
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::PanId,
        snapshot.identity.pan_id == expected.identity.pan_id,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::ShortAddress,
        snapshot.identity.short_address == expected.identity.short_address,
    )?;
    verify(
        Ieee802154MacPolicyCheckpoint::ExtendedAddress,
        snapshot.identity.extended_address == expected.identity.extended_address,
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

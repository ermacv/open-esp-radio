//! Finite polled IEEE 802.15.4 energy-detection and CCA transactions.
//!
//! This closed engine owns no DMA buffer and creates no CPU interrupt route. A
//! transaction may temporarily enable exactly `ED_DONE` and `RX_ABORT` only
//! while its semantic backend proves that the CPU route is detached. A stale
//! event-status image fails closed before `ED_START`.
//!
//! Recovery samples and acknowledges the complete pending W1C image in one
//! backend transaction. Reuse is possible only when the image actually
//! consumed by that transaction is exactly lone `ED_DONE`. Abort, timeout,
//! conflicting status, acknowledgement drift, and backend failures mask the
//! window for containment but can never return a reusable owner.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), allow(dead_code))]

use core::convert::Infallible;

#[cfg(test)]
use oer_esp32s31_pac::Ieee802154RxAbortReason;
use oer_esp32s31_pac::{
    Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservationError,
    Ieee802154RxAbortReasonObservation,
};

use crate::{ieee802154::lifecycle::Ieee802154Channel, ieee802154::policy::Ieee802154CcaMode};

/// Source-confirmed ED duration used by the standalone CCA entry path.
pub(crate) const IEEE802154_CCA_ED_DURATION: u16 = 8;

/// One finite operation supported by the interrupt-detached polling boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
// CAPABILITY: i-cca-and-channel-access-energy-detection-command, i-cca-and-channel-access-standalone-cca-command
pub enum Ieee802154PolledOperation {
    /// Sample one uncalibrated signed ED RSS code.
    EnergyDetection {
        /// Checked 2.4 GHz channel selected before the command.
        channel: Ieee802154Channel,
        /// Complete source-level `uint16_t` ED-duration subset.
        duration: u16,
    },
    /// Sample the source-confirmed `CCA_BUSY` result.
    ClearChannelAssessment {
        /// Checked 2.4 GHz channel selected before the command.
        channel: Ieee802154Channel,
        /// Existing reviewed static CCA mode.
        mode: Ieee802154CcaMode,
        /// Signed source-level CCA threshold code.
        threshold_code: i8,
    },
}

impl Ieee802154PolledOperation {
    /// Construct one standalone ED request.
    pub(crate) const fn energy_detection(channel: Ieee802154Channel, duration: u16) -> Self {
        Self::EnergyDetection { channel, duration }
    }

    /// Construct one standalone CCA request.
    pub(crate) const fn clear_channel_assessment(
        channel: Ieee802154Channel,
        mode: Ieee802154CcaMode,
        threshold_code: i8,
    ) -> Self {
        Self::ClearChannelAssessment {
            channel,
            mode,
            threshold_code,
        }
    }

    /// Return the checked 2.4 GHz channel selected by this operation.
    pub const fn channel(self) -> Ieee802154Channel {
        match self {
            Self::EnergyDetection { channel, .. }
            | Self::ClearChannelAssessment { channel, .. } => channel,
        }
    }

    /// Return the configured ED duration, including the fixed reviewed value
    /// used by standalone CCA.
    pub const fn duration(self) -> u16 {
        match self {
            Self::EnergyDetection { duration, .. } => duration,
            Self::ClearChannelAssessment { .. } => IEEE802154_CCA_ED_DURATION,
        }
    }
}

/// Nonzero number of status samples permitted for one command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154OperationPollBudget(u32);

impl Ieee802154OperationPollBudget {
    /// Reject a zero-sample transaction before it owns the backend.
    pub const fn new(samples: u32) -> Option<Self> {
        if samples == 0 {
            None
        } else {
            Some(Self(samples))
        }
    }

    /// Return the exact maximum number of status samples.
    pub const fn samples(self) -> u32 {
        self.0
    }
}

/// Semantic readback of event-enable state, without a raw mask image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154OperationEventMaskState {
    /// Every MAC event remains masked.
    AllMasked,
    /// Exactly `ED_DONE` and `RX_ABORT` are enabled.
    EdDoneAndRxAbortOnly,
    /// At least one other event is enabled or one required event is missing.
    Unexpected,
}

/// Semantic readback of receive-abort-enable state, without a raw mask image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154OperationRxAbortMaskState {
    /// Every receive-abort reason remains masked.
    AllMasked,
    /// Exactly ED abort, ED stop, and ED coexistence reject are enabled.
    ///
    /// The source reason codes are 24, 25, and 26. This semantic type does not
    /// expose or accept their register-positioned bit image.
    EdOperationReasonsOnly,
    /// At least one other reason is enabled or one required reason is missing.
    Unexpected,
}

/// Complete semantic classification of one non-acknowledging `EVENT_STATUS`
/// observation.
///
/// The PAC retains the physical register image. This operation layer accepts
/// either its complete named-event classification or an opaque unclassified
/// result, so an unknown latched event still fails closed without exporting
/// register geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154OperationEventObservation(
    Result<Ieee802154EventMask, Ieee802154EventObservationError>,
);

impl Ieee802154OperationEventObservation {
    /// Retain the PAC-owned complete semantic classification.
    pub(crate) const fn from_classification(
        classification: Result<Ieee802154EventMask, Ieee802154EventObservationError>,
    ) -> Self {
        Self(classification)
    }

    /// Return the complete PAC-owned semantic classification without exposing
    /// the retained physical register image.
    pub const fn classification(
        self,
    ) -> Result<Ieee802154EventMask, Ieee802154EventObservationError> {
        self.0
    }

    #[cfg(test)]
    const fn events(events: Ieee802154EventMask) -> Self {
        Self(Ok(events))
    }

    #[cfg(test)]
    const fn event(event: Ieee802154Event) -> Self {
        Self::events(event.mask())
    }

    #[cfg(test)]
    const fn unclassified() -> Self {
        Self(Err(Ieee802154EventObservationError))
    }

    /// Return whether no event is latched.
    pub const fn is_clear(self) -> bool {
        matches!(self.0, Ok(events) if events.is_empty())
    }

    /// Return the closed semantic status without exposing register geometry.
    pub const fn state(self) -> oer_esp32s31_pac::Ieee802154ObservedEventState {
        match self.0 {
            Ok(events) => events.state(),
            Err(_) => oer_esp32s31_pac::Ieee802154ObservedEventState::Unclassified,
        }
    }

    /// Return whether this is exactly the one recoverable terminal status.
    const fn is_ed_done_only(self) -> bool {
        matches!(self.0, Ok(events) if events.contains(Ieee802154Event::EdDone)
            && events.difference(Ieee802154Event::EdDone.mask()).is_empty())
    }

    /// Return whether `RX_ABORT` is present in a fully classified observation.
    const fn has_rx_abort(self) -> bool {
        matches!(self.0, Ok(events) if events.contains(Ieee802154Event::RxAbort))
    }

    /// Return whether `ED_DONE` is present in a fully classified observation.
    const fn has_ed_done(self) -> bool {
        matches!(self.0, Ok(events) if events.contains(Ieee802154Event::EdDone))
    }
}

impl Default for Ieee802154OperationEventObservation {
    fn default() -> Self {
        Self(Ok(Ieee802154EventMask::NONE))
    }
}

/// Closed semantic backend for one serialized polled operation.
///
/// Implementations must retain exclusive ownership of the event-enable fields,
/// command leaf, ED result fields, and CPU-route attachment decision for the
/// complete transaction. Only `acknowledge_pending_events` may acknowledge
/// event status. It must sample the complete pending image once, consume that
/// exact non-replayable W1C snapshot, and return its semantic observation.
pub(crate) trait Ieee802154PolledOperationBackend: Sized {
    /// Backend-specific failure retained in a terminal quarantine owner.
    type Error;

    /// Select the checked channel through the existing channel policy.
    fn set_channel(&mut self, channel: Ieee802154Channel) -> Result<(), Self::Error>;

    /// Select the reviewed CCA mode.
    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) -> Result<(), Self::Error>;

    /// Select the signed CCA threshold code.
    fn set_cca_threshold_code(&mut self, threshold: i8) -> Result<(), Self::Error>;

    /// Replace the source-level ED duration subset.
    fn set_ed_duration(&mut self, duration: u16) -> Result<(), Self::Error>;

    /// Observe whether the MAC CPU interrupt route is detached.
    fn cpu_interrupt_route_is_detached(&mut self) -> Result<bool, Self::Error>;

    /// Sample the semantic event-enable state.
    fn operation_event_mask_state(
        &mut self,
    ) -> Result<Ieee802154OperationEventMaskState, Self::Error>;

    /// Sample the semantic receive-abort-enable state.
    fn operation_rx_abort_mask_state(
        &mut self,
    ) -> Result<Ieee802154OperationRxAbortMaskState, Self::Error>;

    /// Enable exactly `ED_DONE` and `RX_ABORT`, preserving every other event as masked.
    fn enable_ed_done_and_rx_abort(&mut self) -> Result<(), Self::Error>;

    /// Enable exactly ED abort, ED stop, and ED coexistence reject.
    fn enable_ed_operation_rx_abort_reasons(&mut self) -> Result<(), Self::Error>;

    /// Mask `ED_DONE` and `RX_ABORT` without touching or acknowledging status.
    fn mask_ed_done_and_rx_abort(&mut self) -> Result<(), Self::Error>;

    /// Mask the three ED-operation receive-abort reasons.
    fn mask_ed_operation_rx_abort_reasons(&mut self) -> Result<(), Self::Error>;

    /// Order policy and event-enable changes at the device boundary.
    fn order_device_accesses(&mut self) -> Result<(), Self::Error>;

    /// Issue the source-confirmed `ED_START` command.
    fn request_ed_start(&mut self) -> Result<(), Self::Error>;

    /// Sample the complete event status without acknowledging any bit.
    fn sample_event_status(&mut self) -> Result<Ieee802154OperationEventObservation, Self::Error>;

    /// Sample and acknowledge the complete pending W1C event image.
    fn acknowledge_pending_events(
        &mut self,
    ) -> Result<Ieee802154OperationEventObservation, Self::Error>;

    /// Classify the receive-abort reason after `RX_ABORT`.
    fn sample_rx_abort_reason(&mut self)
    -> Result<Ieee802154RxAbortReasonObservation, Self::Error>;

    /// Sample the signed `ED_RSS` result after `ED_DONE`.
    fn sample_ed_rss_code(&mut self) -> Result<i8, Self::Error>;

    /// Sample `CCA_BUSY` after `ED_DONE`.
    fn sample_cca_busy(&mut self) -> Result<bool, Self::Error>;
}

/// Observable stage at which a one-shot owner entered quarantine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154OperationStage {
    /// Preconditions and static request fields.
    Prepare,
    /// Detached-route event-enable window.
    StartEventWindow,
    /// `ED_START` command publication.
    StartCommand,
    /// Non-acknowledging status poll.
    Poll,
    /// Terminal result sampling.
    TerminalSample,
    /// Snapshot-consuming W1C acknowledgement of the terminal event.
    AcknowledgeTerminalEvent,
    /// Terminal or timeout event remasking.
    Cleanup,
}

/// Why a transaction could not preserve the closed one-shot contract.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum Ieee802154OperationQuarantineReason<Error> {
    /// One semantic backend operation failed.
    Backend {
        /// Stage containing the failed operation.
        stage: Ieee802154OperationStage,
        /// Backend-specific failure.
        error: Error,
    },
    /// The CPU route was not detached at a required checkpoint.
    CpuInterruptRouteAttached {
        /// Stage whose detached-route proof failed.
        stage: Ieee802154OperationStage,
    },
    /// Event enables did not match the exact expected state.
    UnexpectedEventMask {
        /// Stage whose event-mask proof failed.
        stage: Ieee802154OperationStage,
        /// Semantic mask state that was observed.
        observed: Ieee802154OperationEventMaskState,
    },
    /// Receive-abort enables did not match the exact expected state.
    UnexpectedRxAbortMask {
        /// Stage whose receive-abort-mask proof failed.
        stage: Ieee802154OperationStage,
        /// Semantic receive-abort mask state that was observed.
        observed: Ieee802154OperationRxAbortMaskState,
    },
    /// A nonzero event status was already latched before `ED_START`.
    StaleEventStatus {
        /// Complete status retained at the pre-command gate.
        observed: Ieee802154OperationEventObservation,
    },
    /// Nonzero status contained neither `RX_ABORT` nor exactly lone `ED_DONE`.
    UnexpectedTerminalStatus {
        /// Complete terminal status retained for diagnosis.
        observed: Ieee802154OperationEventObservation,
    },
    /// The W1C snapshot actually consumed was not exactly lone `ED_DONE`.
    UnexpectedAcknowledgedEvents {
        /// Complete acknowledged event image retained for diagnosis.
        observed: Ieee802154OperationEventObservation,
    },
    /// `ED_DONE` and `RX_ABORT` were observed together.
    ConflictingTerminalEvents {
        /// Complete conflicting status retained for diagnosis.
        observed: Ieee802154OperationEventObservation,
    },
}

/// Terminal quarantine retaining a backend that must not be reused.
#[must_use = "a quarantined IEEE 802.15.4 operation retains its one-shot backend"]
pub(crate) struct Ieee802154OperationQuarantined<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    backend: Backend,
    reason: Ieee802154OperationQuarantineReason<Backend::Error>,
}

impl<Backend> Ieee802154OperationQuarantined<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// Borrow the reason without exposing the retained backend.
    pub(crate) const fn reason(&self) -> &Ieee802154OperationQuarantineReason<Backend::Error> {
        &self.reason
    }
}

/// Serialized backend before any ED/CCA request has been prepared.
// CAPABILITY: polled-ed-cca
pub(crate) struct Ieee802154PolledOperationOwner<Backend> {
    backend: Backend,
}

impl<Backend> Ieee802154PolledOperationOwner<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// Bind a HAL-owned semantic backend without changing hardware.
    ///
    /// This crate-private hook is not an operational public constructor. The
    /// prepare transition still proves a detached CPU route and all events
    /// masked before any request field is changed.
    pub(crate) const fn from_semantic_backend(backend: Backend) -> Self {
        Self { backend }
    }

    /// Prepare request fields while all events remain masked.
    pub(crate) fn prepare(
        self,
        operation: Ieee802154PolledOperation,
        budget: Ieee802154OperationPollBudget,
    ) -> Result<Ieee802154PolledOperationPrepared<Backend>, Ieee802154OperationQuarantined<Backend>>
    {
        let mut backend = self.backend;
        if let Err(reason) = require_detached_route(&mut backend, Ieee802154OperationStage::Prepare)
        {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::Prepare,
            Ieee802154OperationEventMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::Prepare,
            Ieee802154OperationRxAbortMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }

        if let Err(error) = backend.set_channel(operation.channel()) {
            return Err(quarantined(
                backend,
                backend_error(Ieee802154OperationStage::Prepare, error),
            ));
        }
        if let Ieee802154PolledOperation::ClearChannelAssessment {
            mode,
            threshold_code,
            ..
        } = operation
        {
            if let Err(error) = backend.set_cca_mode(mode) {
                return Err(quarantined(
                    backend,
                    backend_error(Ieee802154OperationStage::Prepare, error),
                ));
            }
            if let Err(error) = backend.set_cca_threshold_code(threshold_code) {
                return Err(quarantined(
                    backend,
                    backend_error(Ieee802154OperationStage::Prepare, error),
                ));
            }
        }
        if let Err(error) = backend.set_ed_duration(operation.duration()) {
            return Err(quarantined(
                backend,
                backend_error(Ieee802154OperationStage::Prepare, error),
            ));
        }
        if let Err(error) = backend.order_device_accesses() {
            return Err(quarantined(
                backend,
                backend_error(Ieee802154OperationStage::Prepare, error),
            ));
        }
        if let Err(reason) = require_detached_route(&mut backend, Ieee802154OperationStage::Prepare)
        {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::Prepare,
            Ieee802154OperationEventMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::Prepare,
            Ieee802154OperationRxAbortMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }

        Ok(Ieee802154PolledOperationPrepared {
            backend,
            operation,
            budget,
        })
    }
}

/// Prepared one-shot operation whose events are still completely masked.
pub(crate) struct Ieee802154PolledOperationPrepared<Backend> {
    backend: Backend,
    operation: Ieee802154PolledOperation,
    budget: Ieee802154OperationPollBudget,
}

impl<Backend> Ieee802154PolledOperationPrepared<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// Open the exact detached-route event window and issue `ED_START`.
    pub(crate) fn start(
        self,
    ) -> Result<Ieee802154PolledOperationActive<Backend>, Ieee802154OperationQuarantined<Backend>>
    {
        let mut backend = self.backend;
        if let Err(reason) =
            require_detached_route(&mut backend, Ieee802154OperationStage::StartEventWindow)
        {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::StartEventWindow,
            Ieee802154OperationEventMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::StartEventWindow,
            Ieee802154OperationRxAbortMaskState::AllMasked,
        ) {
            return Err(quarantined(backend, reason));
        }
        if let Err(error) = backend.enable_ed_done_and_rx_abort() {
            return Err(quarantined_after_cleanup(
                backend,
                backend_error(Ieee802154OperationStage::StartEventWindow, error),
            ));
        }
        if let Err(error) = backend.enable_ed_operation_rx_abort_reasons() {
            return Err(quarantined_after_cleanup(
                backend,
                backend_error(Ieee802154OperationStage::StartEventWindow, error),
            ));
        }
        if let Err(error) = backend.order_device_accesses() {
            return Err(quarantined_after_cleanup(
                backend,
                backend_error(Ieee802154OperationStage::StartEventWindow, error),
            ));
        }
        if let Err(reason) =
            require_detached_route(&mut backend, Ieee802154OperationStage::StartEventWindow)
        {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::StartEventWindow,
            Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly,
        ) {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::StartEventWindow,
            Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly,
        ) {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        let status = match backend.sample_event_status() {
            Ok(status) => status,
            Err(error) => {
                return Err(quarantined_after_cleanup(
                    backend,
                    backend_error(Ieee802154OperationStage::StartEventWindow, error),
                ));
            }
        };
        if !status.is_clear() {
            return Err(quarantined_after_cleanup(
                backend,
                Ieee802154OperationQuarantineReason::StaleEventStatus { observed: status },
            ));
        }
        if let Err(error) = backend.request_ed_start() {
            return Err(quarantined_after_cleanup(
                backend,
                backend_error(Ieee802154OperationStage::StartCommand, error),
            ));
        }

        Ok(Ieee802154PolledOperationActive {
            backend,
            operation: self.operation,
            remaining_polls: self.budget.samples(),
            polls: 0,
        })
    }
}

/// Active detached-route polled command retaining the only backend owner.
pub(crate) struct Ieee802154PolledOperationActive<Backend> {
    backend: Backend,
    operation: Ieee802154PolledOperation,
    remaining_polls: u32,
    polls: u32,
}

/// One accepted result sampled after a lone `ED_DONE`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PolledOperationResult {
    /// Uncalibrated signed `ED_RSS` code.
    EnergyDetection { rss_code: i8 },
    /// Source-confirmed `CCA_BUSY` classification.
    ClearChannelAssessment { busy: bool },
}

/// Evidence preserved for one successfully completed operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PolledOperationEvidence {
    operation: Ieee802154PolledOperation,
    result: Ieee802154PolledOperationResult,
    polls: u32,
}

impl Ieee802154PolledOperationEvidence {
    /// Return the exact prepared operation.
    pub const fn operation(self) -> Ieee802154PolledOperation {
        self.operation
    }

    /// Return the sampled terminal result.
    pub const fn result(self) -> Ieee802154PolledOperationResult {
        self.result
    }

    /// Return the number of non-acknowledging event samples consumed.
    pub const fn polls(self) -> u32 {
        self.polls
    }
}

/// Successful result owner while the detached event window remains active.
#[must_use = "a completed operation must be recovered before its backend can be reused"]
pub(crate) struct Ieee802154PolledOperationCompleted<Backend> {
    backend: Backend,
    evidence: Ieee802154PolledOperationEvidence,
}

impl<Backend> Ieee802154PolledOperationCompleted<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// Borrow the successful evidence before recovery.
    pub(crate) const fn evidence(&self) -> &Ieee802154PolledOperationEvidence {
        &self.evidence
    }

    /// Consume the complete pending W1C snapshot, require exactly `ED_DONE`,
    /// and close the event window before returning a reusable owner.
    pub(crate) fn recover(
        self,
    ) -> Result<Ieee802154PolledOperationRecovered<Backend>, Ieee802154OperationQuarantined<Backend>>
    {
        let mut backend = self.backend;
        if let Err(reason) = require_detached_route(
            &mut backend,
            Ieee802154OperationStage::AcknowledgeTerminalEvent,
        ) {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::AcknowledgeTerminalEvent,
            Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly,
        ) {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::AcknowledgeTerminalEvent,
            Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly,
        ) {
            return Err(quarantined_after_cleanup(backend, reason));
        }
        let acknowledged = match backend.acknowledge_pending_events() {
            Ok(acknowledged) => acknowledged,
            Err(error) => {
                return Err(quarantined_after_cleanup(
                    backend,
                    backend_error(Ieee802154OperationStage::AcknowledgeTerminalEvent, error),
                ));
            }
        };
        if !acknowledged.is_ed_done_only() {
            return Err(quarantined_after_cleanup(
                backend,
                Ieee802154OperationQuarantineReason::UnexpectedAcknowledgedEvents {
                    observed: acknowledged,
                },
            ));
        }

        let backend = cleanup(backend)?;
        Ok(Ieee802154PolledOperationRecovered {
            owner: Ieee802154PolledOperationOwner { backend },
            evidence: self.evidence,
        })
    }
}

/// Successful evidence paired with the only reusable operation owner.
#[must_use = "a recovered operation contains the reusable IEEE 802.15.4 owner"]
pub(crate) struct Ieee802154PolledOperationRecovered<Backend> {
    owner: Ieee802154PolledOperationOwner<Backend>,
    evidence: Ieee802154PolledOperationEvidence,
}

impl<Backend> Ieee802154PolledOperationRecovered<Backend> {
    /// Borrow the successful evidence retained across recovery.
    pub(crate) const fn evidence(&self) -> &Ieee802154PolledOperationEvidence {
        &self.evidence
    }

    /// Consume successful evidence and return the reusable serialized owner.
    pub(crate) fn into_owner(self) -> Ieee802154PolledOperationOwner<Backend> {
        self.owner
    }
}

/// Evidence retained when the operation's `RX_ABORT` event was observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PolledOperationAbortEvidence {
    operation: Ieee802154PolledOperation,
    event_status: Ieee802154OperationEventObservation,
    rx_abort_reason: Ieee802154RxAbortReasonObservation,
    polls: u32,
}

impl Ieee802154PolledOperationAbortEvidence {
    /// Return the operation that aborted.
    pub const fn operation(self) -> Ieee802154PolledOperation {
        self.operation
    }

    /// Return the complete semantic event classification observed at abort.
    pub const fn event_status(self) -> Ieee802154OperationEventObservation {
        self.event_status
    }

    /// Return the classified receive-abort reason sampled before containment.
    pub const fn rx_abort_reason(self) -> Ieee802154RxAbortReasonObservation {
        self.rx_abort_reason
    }

    /// Return the number of completed event-status samples.
    pub const fn polls(self) -> u32 {
        self.polls
    }
}

/// Aborted owner after event and abort masks were closed for containment.
///
/// No event acknowledgement or `STOP` was issued, and no recovery transition
/// is exposed.
#[must_use = "an aborted operation retains a non-reusable backend"]
pub(crate) struct Ieee802154PolledOperationAborted<Backend> {
    backend: Backend,
    evidence: Ieee802154PolledOperationAbortEvidence,
}

impl<Backend> Ieee802154PolledOperationAborted<Backend> {
    /// Borrow the complete abort evidence.
    pub(crate) const fn evidence(&self) -> &Ieee802154PolledOperationAbortEvidence {
        &self.evidence
    }
}

/// Timed-out one-shot owner after operation events were remasked.
///
/// No `STOP` or hardware-idle claim is made, so the backend is intentionally
/// retained without a recovery transition.
#[must_use = "a timed-out operation may still be active in hardware"]
pub(crate) struct Ieee802154PolledOperationTimeout<Backend> {
    backend: Backend,
    operation: Ieee802154PolledOperation,
    polls: u32,
}

impl<Backend> Ieee802154PolledOperationTimeout<Backend> {
    /// Return the operation that exhausted its poll budget.
    pub(crate) const fn operation(&self) -> Ieee802154PolledOperation {
        self.operation
    }

    /// Return the exact number of completed status samples.
    pub(crate) const fn polls(&self) -> u32 {
        self.polls
    }
}

/// Result of one finite status poll.
pub(crate) enum Ieee802154PolledOperationPoll<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// No terminal event was observed and budget remains.
    Pending(Ieee802154PolledOperationActive<Backend>),
    /// Lone `ED_DONE` was observed and result fields were sampled.
    Completed(Ieee802154PolledOperationCompleted<Backend>),
    /// `RX_ABORT` and its complete diagnostic evidence were observed.
    Aborted(Ieee802154PolledOperationAborted<Backend>),
    /// The exact sample budget was exhausted without a terminal event.
    Timeout(Ieee802154PolledOperationTimeout<Backend>),
    /// An invariant or backend operation failed.
    Quarantined(Ieee802154OperationQuarantined<Backend>),
}

/// Terminal reason why a production polled operation cannot return a reusable
/// IEEE 802.15.4 owner.
///
/// Abort and timeout deliberately remain terminal: no source proves that a
/// generic `STOP` makes an in-flight MAC operation synchronously reusable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PolledOperationFailure {
    /// `RX_ABORT` completed the operation; full status evidence is retained.
    Aborted(Ieee802154PolledOperationAbortEvidence),
    /// The exact status-sample budget expired without a terminal event.
    Timeout {
        /// Operation which may still be active in hardware.
        operation: Ieee802154PolledOperation,
        /// Exact number of completed status observations.
        polls: u32,
    },
    /// A CPU interrupt route was not completely at reset.
    CpuInterruptRouteAttached {
        /// Checkpoint which detected the attached or unexpected route image.
        stage: Ieee802154OperationStage,
    },
    /// Event enables differed from the exact state required at a checkpoint.
    UnexpectedEventMask {
        /// Checkpoint whose proof failed.
        stage: Ieee802154OperationStage,
        /// Complete semantic classification observed there.
        observed: Ieee802154OperationEventMaskState,
    },
    /// Receive-abort enables differed from the exact required state.
    UnexpectedRxAbortMask {
        /// Checkpoint whose proof failed.
        stage: Ieee802154OperationStage,
        /// Complete semantic classification observed there.
        observed: Ieee802154OperationRxAbortMaskState,
    },
    /// A nonempty or unclassified event was already pending before `ED_START`.
    StaleEventStatus {
        /// Complete semantic pre-command observation.
        observed: Ieee802154OperationEventObservation,
    },
    /// A nonempty semantic status was neither `RX_ABORT` nor lone `ED_DONE`.
    UnexpectedTerminalStatus {
        /// Complete semantic terminal observation.
        observed: Ieee802154OperationEventObservation,
    },
    /// The W1C snapshot actually consumed was not classified as lone `ED_DONE`.
    UnexpectedAcknowledgedEvents {
        /// Complete semantic acknowledged observation.
        observed: Ieee802154OperationEventObservation,
    },
    /// `ED_DONE` and `RX_ABORT` were observed in the same status sample.
    ConflictingTerminalEvents {
        /// Complete semantic conflicting observation.
        observed: Ieee802154OperationEventObservation,
    },
}

/// Run one infallible semantic backend through prepare, start, finite polling,
/// and exact recovery.
///
/// Concrete MMIO backends are infallible at the Rust call boundary; all
/// hardware divergence is represented by readback-based failure variants.
pub(crate) fn run_ieee802154_polled_operation<Backend>(
    backend: Backend,
    operation: Ieee802154PolledOperation,
    budget: Ieee802154OperationPollBudget,
) -> Result<Ieee802154PolledOperationEvidence, Ieee802154PolledOperationFailure>
where
    Backend: Ieee802154PolledOperationBackend<Error = Infallible>,
{
    let prepared = Ieee802154PolledOperationOwner::from_semantic_backend(backend)
        .prepare(operation, budget)
        .map_err(public_failure_from_quarantine)?;
    let mut active = prepared.start().map_err(public_failure_from_quarantine)?;

    loop {
        match active.poll() {
            Ieee802154PolledOperationPoll::Pending(next) => active = next,
            Ieee802154PolledOperationPoll::Completed(completed) => {
                let recovered = completed
                    .recover()
                    .map_err(public_failure_from_quarantine)?;
                let evidence = *recovered.evidence();
                drop(recovered.into_owner());
                return Ok(evidence);
            }
            Ieee802154PolledOperationPoll::Aborted(aborted) => {
                return Err(Ieee802154PolledOperationFailure::Aborted(
                    *aborted.evidence(),
                ));
            }
            Ieee802154PolledOperationPoll::Timeout(timeout) => {
                return Err(Ieee802154PolledOperationFailure::Timeout {
                    operation: timeout.operation(),
                    polls: timeout.polls(),
                });
            }
            Ieee802154PolledOperationPoll::Quarantined(quarantine) => {
                return Err(public_failure_from_quarantine(quarantine));
            }
        }
    }
}

fn public_failure_from_quarantine<Backend>(
    quarantine: Ieee802154OperationQuarantined<Backend>,
) -> Ieee802154PolledOperationFailure
where
    Backend: Ieee802154PolledOperationBackend<Error = Infallible>,
{
    let Ieee802154OperationQuarantined { backend: _, reason } = quarantine;
    match reason {
        Ieee802154OperationQuarantineReason::Backend { error, .. } => match error {},
        Ieee802154OperationQuarantineReason::CpuInterruptRouteAttached { stage } => {
            Ieee802154PolledOperationFailure::CpuInterruptRouteAttached { stage }
        }
        Ieee802154OperationQuarantineReason::UnexpectedEventMask { stage, observed } => {
            Ieee802154PolledOperationFailure::UnexpectedEventMask { stage, observed }
        }
        Ieee802154OperationQuarantineReason::UnexpectedRxAbortMask { stage, observed } => {
            Ieee802154PolledOperationFailure::UnexpectedRxAbortMask { stage, observed }
        }
        Ieee802154OperationQuarantineReason::StaleEventStatus { observed } => {
            Ieee802154PolledOperationFailure::StaleEventStatus { observed }
        }
        Ieee802154OperationQuarantineReason::UnexpectedTerminalStatus { observed } => {
            Ieee802154PolledOperationFailure::UnexpectedTerminalStatus { observed }
        }
        Ieee802154OperationQuarantineReason::UnexpectedAcknowledgedEvents { observed } => {
            Ieee802154PolledOperationFailure::UnexpectedAcknowledgedEvents { observed }
        }
        Ieee802154OperationQuarantineReason::ConflictingTerminalEvents { observed } => {
            Ieee802154PolledOperationFailure::ConflictingTerminalEvents { observed }
        }
    }
}

impl<Backend> Ieee802154PolledOperationActive<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    /// Sample one finite status observation without acknowledging events.
    pub(crate) fn poll(self) -> Ieee802154PolledOperationPoll<Backend> {
        let mut backend = self.backend;
        if let Err(reason) = require_detached_route(&mut backend, Ieee802154OperationStage::Poll) {
            return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                backend, reason,
            ));
        }
        if let Err(reason) = require_event_mask(
            &mut backend,
            Ieee802154OperationStage::Poll,
            Ieee802154OperationEventMaskState::EdDoneAndRxAbortOnly,
        ) {
            return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                backend, reason,
            ));
        }
        if let Err(reason) = require_rx_abort_mask(
            &mut backend,
            Ieee802154OperationStage::Poll,
            Ieee802154OperationRxAbortMaskState::EdOperationReasonsOnly,
        ) {
            return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                backend, reason,
            ));
        }
        let status = match backend.sample_event_status() {
            Ok(status) => status,
            Err(error) => {
                return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                    backend,
                    backend_error(Ieee802154OperationStage::Poll, error),
                ));
            }
        };
        let polls = self.polls + 1;
        let remaining_polls = self.remaining_polls - 1;

        if status.has_ed_done() && status.has_rx_abort() {
            return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                backend,
                Ieee802154OperationQuarantineReason::ConflictingTerminalEvents { observed: status },
            ));
        }

        if status.has_rx_abort() {
            let rx_abort_reason = match backend.sample_rx_abort_reason() {
                Ok(reason) => reason,
                Err(error) => {
                    return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                        backend,
                        backend_error(Ieee802154OperationStage::TerminalSample, error),
                    ));
                }
            };
            return match cleanup(backend) {
                Ok(backend) => {
                    Ieee802154PolledOperationPoll::Aborted(Ieee802154PolledOperationAborted {
                        backend,
                        evidence: Ieee802154PolledOperationAbortEvidence {
                            operation: self.operation,
                            event_status: status,
                            rx_abort_reason,
                            polls,
                        },
                    })
                }
                Err(quarantine) => Ieee802154PolledOperationPoll::Quarantined(quarantine),
            };
        }

        if !status.is_clear() && !status.is_ed_done_only() {
            return Ieee802154PolledOperationPoll::Quarantined(quarantined_after_cleanup(
                backend,
                Ieee802154OperationQuarantineReason::UnexpectedTerminalStatus { observed: status },
            ));
        }

        if status.is_ed_done_only() {
            let result = match self.operation {
                Ieee802154PolledOperation::EnergyDetection { .. } => {
                    match backend.sample_ed_rss_code() {
                        Ok(rss_code) => {
                            Ieee802154PolledOperationResult::EnergyDetection { rss_code }
                        }
                        Err(error) => {
                            return Ieee802154PolledOperationPoll::Quarantined(
                                quarantined_after_cleanup(
                                    backend,
                                    backend_error(Ieee802154OperationStage::TerminalSample, error),
                                ),
                            );
                        }
                    }
                }
                Ieee802154PolledOperation::ClearChannelAssessment { .. } => {
                    match backend.sample_cca_busy() {
                        Ok(busy) => {
                            Ieee802154PolledOperationResult::ClearChannelAssessment { busy }
                        }
                        Err(error) => {
                            return Ieee802154PolledOperationPoll::Quarantined(
                                quarantined_after_cleanup(
                                    backend,
                                    backend_error(Ieee802154OperationStage::TerminalSample, error),
                                ),
                            );
                        }
                    }
                }
            };
            return Ieee802154PolledOperationPoll::Completed(Ieee802154PolledOperationCompleted {
                backend,
                evidence: Ieee802154PolledOperationEvidence {
                    operation: self.operation,
                    result,
                    polls,
                },
            });
        }

        if remaining_polls == 0 {
            return match cleanup(backend) {
                Ok(backend) => {
                    Ieee802154PolledOperationPoll::Timeout(Ieee802154PolledOperationTimeout {
                        backend,
                        operation: self.operation,
                        polls,
                    })
                }
                Err(quarantine) => Ieee802154PolledOperationPoll::Quarantined(quarantine),
            };
        }

        Ieee802154PolledOperationPoll::Pending(Ieee802154PolledOperationActive {
            backend,
            operation: self.operation,
            remaining_polls,
            polls,
        })
    }
}

fn backend_error<Error>(
    stage: Ieee802154OperationStage,
    error: Error,
) -> Ieee802154OperationQuarantineReason<Error> {
    Ieee802154OperationQuarantineReason::Backend { stage, error }
}

fn quarantined<Backend>(
    backend: Backend,
    reason: Ieee802154OperationQuarantineReason<Backend::Error>,
) -> Ieee802154OperationQuarantined<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    Ieee802154OperationQuarantined { backend, reason }
}

fn require_detached_route<Backend>(
    backend: &mut Backend,
    stage: Ieee802154OperationStage,
) -> Result<(), Ieee802154OperationQuarantineReason<Backend::Error>>
where
    Backend: Ieee802154PolledOperationBackend,
{
    match backend.cpu_interrupt_route_is_detached() {
        Ok(true) => Ok(()),
        Ok(false) => Err(Ieee802154OperationQuarantineReason::CpuInterruptRouteAttached { stage }),
        Err(error) => Err(backend_error(stage, error)),
    }
}

fn require_event_mask<Backend>(
    backend: &mut Backend,
    stage: Ieee802154OperationStage,
    expected: Ieee802154OperationEventMaskState,
) -> Result<(), Ieee802154OperationQuarantineReason<Backend::Error>>
where
    Backend: Ieee802154PolledOperationBackend,
{
    match backend.operation_event_mask_state() {
        Ok(observed) if observed == expected => Ok(()),
        Ok(observed) => {
            Err(Ieee802154OperationQuarantineReason::UnexpectedEventMask { stage, observed })
        }
        Err(error) => Err(backend_error(stage, error)),
    }
}

fn require_rx_abort_mask<Backend>(
    backend: &mut Backend,
    stage: Ieee802154OperationStage,
    expected: Ieee802154OperationRxAbortMaskState,
) -> Result<(), Ieee802154OperationQuarantineReason<Backend::Error>>
where
    Backend: Ieee802154PolledOperationBackend,
{
    match backend.operation_rx_abort_mask_state() {
        Ok(observed) if observed == expected => Ok(()),
        Ok(observed) => {
            Err(Ieee802154OperationQuarantineReason::UnexpectedRxAbortMask { stage, observed })
        }
        Err(error) => Err(backend_error(stage, error)),
    }
}

fn quarantined_after_cleanup<Backend>(
    backend: Backend,
    reason: Ieee802154OperationQuarantineReason<Backend::Error>,
) -> Ieee802154OperationQuarantined<Backend>
where
    Backend: Ieee802154PolledOperationBackend,
{
    match cleanup(backend) {
        Ok(backend) => quarantined(backend, reason),
        Err(cleanup_failure) => cleanup_failure,
    }
}

fn cleanup<Backend>(
    mut backend: Backend,
) -> Result<Backend, Ieee802154OperationQuarantined<Backend>>
where
    Backend: Ieee802154PolledOperationBackend,
{
    if let Err(error) = backend.mask_ed_done_and_rx_abort() {
        return Err(quarantined(
            backend,
            backend_error(Ieee802154OperationStage::Cleanup, error),
        ));
    }
    if let Err(error) = backend.mask_ed_operation_rx_abort_reasons() {
        return Err(quarantined(
            backend,
            backend_error(Ieee802154OperationStage::Cleanup, error),
        ));
    }
    if let Err(error) = backend.order_device_accesses() {
        return Err(quarantined(
            backend,
            backend_error(Ieee802154OperationStage::Cleanup, error),
        ));
    }
    if let Err(reason) = require_event_mask(
        &mut backend,
        Ieee802154OperationStage::Cleanup,
        Ieee802154OperationEventMaskState::AllMasked,
    ) {
        return Err(quarantined(backend, reason));
    }
    if let Err(reason) = require_rx_abort_mask(
        &mut backend,
        Ieee802154OperationStage::Cleanup,
        Ieee802154OperationRxAbortMaskState::AllMasked,
    ) {
        return Err(quarantined(backend, reason));
    }
    if let Err(reason) = require_detached_route(&mut backend, Ieee802154OperationStage::Cleanup) {
        return Err(quarantined(backend, reason));
    }
    Ok(backend)
}

#[cfg(test)]
mod tests;

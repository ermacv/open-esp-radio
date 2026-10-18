//! Validation-only ED-DONE/TIMER0 `EVENT_STATUS` discriminator.
//!
//! The closed transaction keeps source 132 detached on both cores, enables
//! RX-ABORT for diagnosis plus the ED-DONE/TIMER0 discriminator pair, selects
//! exactly the three public ED abort reasons, programs ED duration eight, and
//! starts TIMER0 before ED. A successful trace observes both paired bits,
//! writes only ED-DONE and requires TIMER0 to remain, then writes only TIMER0
//! and requires zero. This is evidence collection, not a production
//! acknowledgement API or a PHY/RF/BTBB readiness transition.

use core::sync::atomic::{AtomicBool, Ordering};

use oer_esp32s31_pac::{
    Ieee802154ObservedEventState, Ieee802154OperationRxAbortEnableObservation,
    Ieee802154RouteState, Ieee802154RxAbortReasonObservation, Ieee802154ValidationEdDurationState,
    Ieee802154ValidationEventEnableState,
};

static RESET_ISOLATION_CLAIMED: AtomicBool = AtomicBool::new(false);

/// Unique process-lifetime claim for the dedicated reset-isolated image.
///
/// The value is not clonable and is never released. The transaction still
/// checks the semantic source-132 reset-detached state before event enable,
/// while the exact mask is active, and after cleanup.
#[must_use = "the reset-isolation claim must be consumed by the terminal validation transaction"]
pub struct Ieee802154EdEventProbeIsolation {
    _private: (),
}

impl Ieee802154EdEventProbeIsolation {
    /// Claim this diagnostic image's single reset-isolation capability.
    #[doc(hidden)]
    pub fn claim_for_reset_isolated_image() -> Option<Self> {
        RESET_ISOLATION_CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { _private: () })
    }
}

/// Finite bounds for one ED-DONE/TIMER0 discriminator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EdEventProbeConfig {
    poll_limit: u32,
    timer_threshold: u32,
}

impl Ieee802154EdEventProbeConfig {
    /// Maximum samples made by the single paired-event wait.
    pub const MAX_POLL_LIMIT: u32 = 1_000_000;
    /// Maximum accepted TIMER0 threshold.
    pub const MAX_TIMER_THRESHOLD: u32 = 1_000;

    /// Construct one nondegenerate bounded probe configuration.
    pub const fn new(poll_limit: u32, timer_threshold: u32) -> Option<Self> {
        if poll_limit == 0
            || poll_limit > Self::MAX_POLL_LIMIT
            || timer_threshold == 0
            || timer_threshold > Self::MAX_TIMER_THRESHOLD
        {
            None
        } else {
            Some(Self {
                poll_limit,
                timer_threshold,
            })
        }
    }

    /// Maximum status samples in the paired-event wait.
    pub const fn poll_limit(self) -> u32 {
        self.poll_limit
    }

    /// Complete TIMER0 threshold written before the stimulus.
    pub const fn timer_threshold(self) -> u32 {
        self.timer_threshold
    }
}

/// Terminal classification for the closed discriminator.
///
/// Even [`Complete`](Self::Complete) is only an observed ED-DONE/TIMER0
/// relation. It does not publish a register-wide or production write class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154EdEventProbeStop {
    /// The paired bits and both exact selected-write relations were observed.
    Complete,
    /// Event or RX-abort delivery was already enabled at entry.
    UnsupportedSetup,
    /// The source-132 route was not semantically reset-detached.
    RouteNotQuiesced,
    /// The entry status was not clear.
    ResetNotClear,
    /// Duration eight did not read back exactly.
    EdDurationReadbackMismatch,
    /// The closed RX-abort/ED-DONE/TIMER0 enable state did not read back.
    EventEnableReadbackMismatch,
    /// The closed ED abort-reason enable state did not read back.
    RxAbortEnableReadbackMismatch,
    /// Enabling the exact mask exposed status before either stimulus started.
    PostEnableStatusNotClear,
    /// TIMER0 did not show bounded activity.
    TimerActivityTimeout,
    /// ED-DONE and TIMER0 did not become simultaneously latched in time.
    PairLatchTimeout,
    /// ED terminated through RX-ABORT; its semantic reason is retained.
    EdAborted,
    /// A bit outside ED-DONE/TIMER0 appeared and was not RX-ABORT alone.
    UnexpectedEvent,
    /// Either selected write did not produce the exact expected semantic state.
    SelectiveWriteMismatch,
    /// A nominally complete trace did not leave both delivery masks, route and
    /// status clear. ED duration deliberately remains at the terminal value.
    /// Failure traces retain their more specific primary stop.
    CleanupNotClear,
}

/// Complete semantic evidence retained by the discriminator.
///
/// Every event field is classified by the PAC without exposing register
/// positions. `observed_events` is the semantic union of all samples. If
/// RX-ABORT appears, its source-confirmed reason is retained and the
/// transaction fails closed without acknowledging the abort event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EdEventProbeEvidence {
    pub stop: Ieee802154EdEventProbeStop,
    pub event_enable_before: Ieee802154ValidationEventEnableState,
    pub event_enable_active: Ieee802154ValidationEventEnableState,
    pub event_enable_after: Ieee802154ValidationEventEnableState,
    pub rx_abort_enable_before: Ieee802154OperationRxAbortEnableObservation,
    pub rx_abort_enable_active: Ieee802154OperationRxAbortEnableObservation,
    pub rx_abort_enable_after: Ieee802154OperationRxAbortEnableObservation,
    pub ed_duration_before: Ieee802154ValidationEdDurationState,
    pub ed_duration_active: Ieee802154ValidationEdDurationState,
    pub ed_duration_after: Ieee802154ValidationEdDurationState,
    pub timer0_value_before_start: u32,
    pub timer0_value_min: u32,
    pub timer0_value_max: u32,
    pub timer0_value_after_stop: u32,
    pub reset_events: Ieee802154ObservedEventState,
    pub post_enable_events: Ieee802154ObservedEventState,
    pub observed_events: Ieee802154ObservedEventState,
    pub terminal_events: Ieee802154ObservedEventState,
    pub after_ed_done_write_events: Ieee802154ObservedEventState,
    pub after_timer0_write_events: Ieee802154ObservedEventState,
    pub cleanup_pending_events: Ieee802154ObservedEventState,
    pub final_events: Ieee802154ObservedEventState,
    pub rx_abort_reason: Option<Ieee802154RxAbortReasonObservation>,
    pub stop_command_issued: bool,
    pub cleanup_clear: bool,
}

impl Ieee802154EdEventProbeEvidence {
    const fn empty() -> Self {
        Self {
            stop: Ieee802154EdEventProbeStop::UnsupportedSetup,
            event_enable_before: Ieee802154ValidationEventEnableState::AllMasked,
            event_enable_active: Ieee802154ValidationEventEnableState::AllMasked,
            event_enable_after: Ieee802154ValidationEventEnableState::AllMasked,
            rx_abort_enable_before: Ieee802154OperationRxAbortEnableObservation::AllMasked,
            rx_abort_enable_active: Ieee802154OperationRxAbortEnableObservation::AllMasked,
            rx_abort_enable_after: Ieee802154OperationRxAbortEnableObservation::AllMasked,
            ed_duration_before: Ieee802154ValidationEdDurationState::Other,
            ed_duration_active: Ieee802154ValidationEdDurationState::Other,
            ed_duration_after: Ieee802154ValidationEdDurationState::Other,
            timer0_value_before_start: 0,
            timer0_value_min: 0,
            timer0_value_max: 0,
            timer0_value_after_stop: 0,
            reset_events: Ieee802154ObservedEventState::Clear,
            post_enable_events: Ieee802154ObservedEventState::Clear,
            observed_events: Ieee802154ObservedEventState::Clear,
            terminal_events: Ieee802154ObservedEventState::Clear,
            after_ed_done_write_events: Ieee802154ObservedEventState::Clear,
            after_timer0_write_events: Ieee802154ObservedEventState::Clear,
            cleanup_pending_events: Ieee802154ObservedEventState::Clear,
            final_events: Ieee802154ObservedEventState::Clear,
            rx_abort_reason: None,
            stop_command_issued: false,
            cleanup_clear: false,
        }
    }
}

/// Closed register vocabulary consumed by the pure validation state machine.
///
/// The target adapter must be the unique PAC lease in a dedicated validation
/// build. Host tests implement the trait without exposing raw addresses or
/// broadening the two selected status writes.
pub(crate) trait Ieee802154EdEventProbeBackend {
    fn event_enable_state(&mut self) -> Ieee802154ValidationEventEnableState;
    fn enable_ed_timer_abort_events(&mut self);
    fn disable_all_events(&mut self);
    fn rx_abort_enable_state(&mut self) -> Ieee802154OperationRxAbortEnableObservation;
    fn enable_ed_abort_reasons(&mut self);
    fn disable_all_rx_abort_reasons(&mut self);
    fn interrupt_route_state(&mut self) -> Ieee802154RouteState;
    fn event_status_state(&mut self) -> Ieee802154ObservedEventState;
    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation;
    fn ed_duration_state(&mut self) -> Ieee802154ValidationEdDurationState;
    fn set_ed_duration_eight(&mut self);
    fn timer0_value(&mut self) -> u32;
    fn set_timer0_threshold(&mut self, threshold: u32);
    fn start_timer0(&mut self);
    fn stop_timer0(&mut self);
    fn start_ed(&mut self);
    fn stop_operation(&mut self);
    fn write_ed_done_event(&mut self);
    fn write_timer0_event(&mut self);
}

#[cfg(feature = "validation-probes")]
impl Ieee802154EdEventProbeBackend for crate::ieee802154::backend::Ieee802154PacHal<'_> {
    fn event_enable_state(&mut self) -> Ieee802154ValidationEventEnableState {
        self.validation_ed_event_enable_state()
    }

    fn enable_ed_timer_abort_events(&mut self) {
        self.validation_enable_ed_timer_abort_events();
    }

    fn disable_all_events(&mut self) {
        self.validation_disable_ed_events();
    }

    fn rx_abort_enable_state(&mut self) -> Ieee802154OperationRxAbortEnableObservation {
        self.validation_ed_rx_abort_enable_state()
    }

    fn enable_ed_abort_reasons(&mut self) {
        self.validation_enable_ed_abort_reasons();
    }

    fn disable_all_rx_abort_reasons(&mut self) {
        self.validation_disable_ed_abort_reasons();
    }

    fn interrupt_route_state(&mut self) -> Ieee802154RouteState {
        self.interrupt_route_state()
    }

    fn event_status_state(&mut self) -> Ieee802154ObservedEventState {
        self.validation_ed_event_status_state()
    }

    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        self.validation_ed_rx_abort_reason()
    }

    fn ed_duration_state(&mut self) -> Ieee802154ValidationEdDurationState {
        self.validation_ed_duration_state()
    }

    fn set_ed_duration_eight(&mut self) {
        self.validation_set_ed_duration_eight();
    }

    fn timer0_value(&mut self) -> u32 {
        self.validation_ed_timer0_value()
    }

    fn set_timer0_threshold(&mut self, threshold: u32) {
        self.validation_set_ed_timer0_threshold(threshold);
    }

    fn start_timer0(&mut self) {
        self.validation_start_ed_timer0();
    }

    fn stop_timer0(&mut self) {
        self.validation_stop_ed_timer0();
    }

    fn start_ed(&mut self) {
        self.validation_start_ed();
    }

    fn stop_operation(&mut self) {
        self.validation_stop_ed_operation();
    }

    fn write_ed_done_event(&mut self) {
        self.validation_write_ed_done_event();
    }

    fn write_timer0_event(&mut self) {
        self.validation_write_ed_timer0_event();
    }
}

fn sample_events<B>(
    backend: &mut B,
    evidence: &mut Ieee802154EdEventProbeEvidence,
) -> Ieee802154ObservedEventState
where
    B: Ieee802154EdEventProbeBackend,
{
    let events = backend.event_status_state();
    evidence.observed_events = evidence.observed_events.union(events);
    if events.has_rx_abort() && evidence.rx_abort_reason.is_none() {
        evidence.rx_abort_reason = Some(backend.rx_abort_reason());
    }
    events
}

fn finish_without_writes<B>(
    backend: &mut B,
    mut evidence: Ieee802154EdEventProbeEvidence,
    stop: Ieee802154EdEventProbeStop,
) -> Ieee802154EdEventProbeEvidence
where
    B: Ieee802154EdEventProbeBackend,
{
    evidence.final_events = sample_events(backend, &mut evidence);
    evidence.event_enable_after = backend.event_enable_state();
    evidence.rx_abort_enable_after = backend.rx_abort_enable_state();
    evidence.ed_duration_after = backend.ed_duration_state();
    evidence.stop = stop;
    evidence
}

fn finish<B>(
    backend: &mut B,
    mut evidence: Ieee802154EdEventProbeEvidence,
    stop: Ieee802154EdEventProbeStop,
    stop_ed_for_cleanup: bool,
) -> Ieee802154EdEventProbeEvidence
where
    B: Ieee802154EdEventProbeBackend,
{
    // Quiesce both delivery gates before issuing any best-effort stop. A route
    // readback is still required below, but no newly completed event should be
    // deliberately exposed during cleanup if external state drifted.
    backend.disable_all_events();
    evidence.event_enable_after = backend.event_enable_state();
    backend.disable_all_rx_abort_reasons();
    evidence.rx_abort_enable_after = backend.rx_abort_enable_state();

    backend.stop_timer0();
    evidence.timer0_value_after_stop = backend.timer0_value();
    if stop_ed_for_cleanup {
        backend.stop_operation();
        evidence.stop_command_issued = true;
    }

    evidence.ed_duration_after = backend.ed_duration_state();

    evidence.cleanup_pending_events = sample_events(backend, &mut evidence);
    let route = backend.interrupt_route_state();

    evidence.final_events = sample_events(backend, &mut evidence);
    evidence.cleanup_clear = evidence.event_enable_after
        == Ieee802154ValidationEventEnableState::AllMasked
        && evidence.rx_abort_enable_after == Ieee802154OperationRxAbortEnableObservation::AllMasked
        && evidence.final_events.is_clear()
        && route.is_reset_detached();
    evidence.stop = if stop == Ieee802154EdEventProbeStop::Complete && !evidence.cleanup_clear {
        Ieee802154EdEventProbeStop::CleanupNotClear
    } else {
        stop
    };
    evidence
}

/// Execute the pure sequence shared by the future PAC adapter and host fake.
pub(crate) fn run_ieee802154_ed_event_probe<B>(
    backend: &mut B,
    config: Ieee802154EdEventProbeConfig,
) -> Ieee802154EdEventProbeEvidence
where
    B: Ieee802154EdEventProbeBackend,
{
    let mut evidence = Ieee802154EdEventProbeEvidence::empty();

    evidence.event_enable_before = backend.event_enable_state();
    evidence.rx_abort_enable_before = backend.rx_abort_enable_state();
    evidence.ed_duration_before = backend.ed_duration_state();
    if evidence.event_enable_before != Ieee802154ValidationEventEnableState::AllMasked
        || evidence.rx_abort_enable_before != Ieee802154OperationRxAbortEnableObservation::AllMasked
    {
        return finish_without_writes(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::UnsupportedSetup,
        );
    }

    let route = backend.interrupt_route_state();
    if !route.is_reset_detached() {
        return finish_without_writes(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::RouteNotQuiesced,
        );
    }

    evidence.reset_events = sample_events(backend, &mut evidence);
    if !evidence.reset_events.is_clear() {
        return finish_without_writes(backend, evidence, Ieee802154EdEventProbeStop::ResetNotClear);
    }

    backend.set_ed_duration_eight();
    evidence.ed_duration_active = backend.ed_duration_state();
    if evidence.ed_duration_active != Ieee802154ValidationEdDurationState::ValidationEight {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::EdDurationReadbackMismatch,
            false,
        );
    }

    backend.enable_ed_abort_reasons();
    evidence.rx_abort_enable_active = backend.rx_abort_enable_state();
    if evidence.rx_abort_enable_active
        != Ieee802154OperationRxAbortEnableObservation::EdOperationReasonsOnly
    {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::RxAbortEnableReadbackMismatch,
            false,
        );
    }

    backend.enable_ed_timer_abort_events();
    evidence.event_enable_active = backend.event_enable_state();
    if evidence.event_enable_active != Ieee802154ValidationEventEnableState::EdDoneTimer0RxAbortOnly
    {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::EventEnableReadbackMismatch,
            false,
        );
    }

    evidence.post_enable_events = sample_events(backend, &mut evidence);
    if !evidence.post_enable_events.is_clear() {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::PostEnableStatusNotClear,
            false,
        );
    }

    let route = backend.interrupt_route_state();
    if !route.is_reset_detached() {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::RouteNotQuiesced,
            false,
        );
    }

    evidence.timer0_value_before_start = backend.timer0_value();
    evidence.timer0_value_min = evidence.timer0_value_before_start;
    evidence.timer0_value_max = evidence.timer0_value_before_start;
    backend.set_timer0_threshold(config.timer_threshold());
    backend.start_timer0();
    backend.start_ed();

    let mut timer_changed = false;
    let mut paired = false;
    for _ in 0..config.poll_limit() {
        let timer = backend.timer0_value();
        evidence.timer0_value_min = evidence.timer0_value_min.min(timer);
        evidence.timer0_value_max = evidence.timer0_value_max.max(timer);
        timer_changed |= timer != evidence.timer0_value_before_start;

        evidence.terminal_events = sample_events(backend, &mut evidence);
        if !matches!(
            evidence.terminal_events,
            Ieee802154ObservedEventState::Clear
                | Ieee802154ObservedEventState::EdDoneOnly
                | Ieee802154ObservedEventState::Timer0Only
                | Ieee802154ObservedEventState::EdDoneAndTimer0
        ) {
            let stop_ed =
                !evidence.terminal_events.has_ed_done() && !evidence.terminal_events.has_rx_abort();
            let stop = if evidence.terminal_events.is_rx_abort_only() {
                Ieee802154EdEventProbeStop::EdAborted
            } else {
                Ieee802154EdEventProbeStop::UnexpectedEvent
            };
            return finish(backend, evidence, stop, stop_ed);
        }
        if evidence.terminal_events == Ieee802154ObservedEventState::EdDoneAndTimer0 {
            paired = true;
            break;
        }
    }

    if !paired {
        let stop = if timer_changed {
            Ieee802154EdEventProbeStop::PairLatchTimeout
        } else {
            Ieee802154EdEventProbeStop::TimerActivityTimeout
        };
        let stop_ed = !evidence.terminal_events.has_ed_done();
        return finish(backend, evidence, stop, stop_ed);
    }

    backend.stop_timer0();
    evidence.timer0_value_after_stop = backend.timer0_value();

    backend.write_ed_done_event();
    evidence.after_ed_done_write_events = sample_events(backend, &mut evidence);
    if evidence.after_ed_done_write_events != Ieee802154ObservedEventState::Timer0Only {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::SelectiveWriteMismatch,
            false,
        );
    }

    backend.write_timer0_event();
    evidence.after_timer0_write_events = sample_events(backend, &mut evidence);
    if !evidence.after_timer0_write_events.is_clear() {
        return finish(
            backend,
            evidence,
            Ieee802154EdEventProbeStop::SelectiveWriteMismatch,
            false,
        );
    }

    finish(
        backend,
        evidence,
        Ieee802154EdEventProbeStop::Complete,
        false,
    )
}

#[cfg(test)]
mod tests;

//! Validation-only discriminator for the unresolved IEEE 802.15.4
//! `EVENT_STATUS` write class.
//!
//! The transaction enables exactly the two validation timer events only after
//! read-only checks prove the source-132 CPU routes are semantically
//! reset-detached on both cores. It never writes a route. This remains evidence
//! collection, not an active IRQ owner, acknowledge capability, or MAC-ready
//! transition.

use core::sync::atomic::{AtomicBool, Ordering};

use oer_esp32s31_pac::{
    Ieee802154ObservedEventState, Ieee802154RouteState, Ieee802154ValidationEventEnableState,
};

static RESET_ISOLATION_CLAIMED: AtomicBool = AtomicBool::new(false);

/// Unique process-lifetime claim for the reset-isolated validation image.
///
/// The capability is available only with `validation-probes`, cannot be
/// constructed or cloned by callers, and is never released. It proves that at
/// most one transaction in the dedicated image may rely on both source-132
/// routes remaining untouched. The transaction still checks the semantic
/// reset-detached state before and during the active phase.
#[must_use = "the reset-isolation claim must be consumed by the terminal validation transaction"]
pub struct Ieee802154EventStatusProbeIsolation {
    _private: (),
}

impl Ieee802154EventStatusProbeIsolation {
    /// Claim the dedicated image's one reset-isolation capability.
    #[doc(hidden)]
    pub fn claim_for_reset_isolated_image() -> Option<Self> {
        RESET_ISOLATION_CLAIMED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { _private: () })
    }
}

/// Bounded inputs for one closed `EVENT_STATUS` validation transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventStatusProbeConfig {
    poll_limit: u32,
    timer_threshold: u32,
}

impl Ieee802154EventStatusProbeConfig {
    /// Wire-compatible upper bound for each individual status polling loop.
    pub const MAX_POLL_LIMIT: u32 = 1_000_000;
    /// Wire-compatible upper bound for the timer stimulus threshold.
    pub const MAX_TIMER_THRESHOLD: u32 = 1_000;

    /// Construct one finite probe configuration.
    ///
    /// Zero is rejected for both values: it would either perform no bounded
    /// observation or make the timer stimulus degenerate.  The upper bounds
    /// match the HIL wire contract and keep every busy wait tightly finite.
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

    /// Maximum status samples made by each individual wait.
    pub const fn poll_limit(self) -> u32 {
        self.poll_limit
    }

    /// Complete field value written to each validation timer threshold.
    pub const fn timer_threshold(self) -> u32 {
        self.timer_threshold
    }
}

/// Terminal classification for one closed validation transaction.
///
/// These variants report bounded experimental control flow only. Even
/// [`Complete`](Self::Complete) publishes validation evidence, not a production
/// acknowledgement capability. Production uses the separate generated affine
/// W1C snapshot transaction; this probe never mints that owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154EventStatusProbeStop {
    /// Every expected timer-bit relation was observed and cleanup read zero.
    Complete,
    /// MAC event delivery was not masked at entry.
    UnsupportedSetup,
    /// A source-132 route was not semantically reset-detached.
    RouteNotQuiesced,
    /// The post-reset entry sample was not clear.
    ResetNotClear,
    /// The closed timer-pair enable state did not read back.
    EventEnableReadbackMismatch,
    /// Enabling delivery revealed a pre-existing status before timer start.
    PostEnableStatusNotClear,
    /// One or both timer counters did not change while their start commands
    /// were active and no dual event latch was observed.
    TimerActivityTimeout,
    /// Both independently started timers did not latch within the bound.
    DualLatchTimeout,
    /// A selected write did not clear only the selected timer event.
    SelectiveAcknowledgeMismatch,
    /// Timer zero did not latch alone within the bound.
    DistinctFirstLatchTimeout,
    /// Timer one did not join the retained timer-zero snapshot in time.
    DistinctSecondLatchTimeout,
    /// Best-effort stop and timer-bit cleanup did not leave the delivery mask,
    /// masked status observation, and both route controls clear.
    CleanupNotClear,
}

/// Semantic trace from one closed validation transaction.
///
/// Every event observation is classified by the PAC without exposing register
/// positions. `dual_observed_events` is the semantic union of every sample in the first
/// bounded wait, while `dual_latched_events` is its last sample and therefore
/// preserves the simultaneous two-bit condition.
/// `cleanup_pending_events` is sampled after delivery is masked and may hide a
/// retained latch; cleanup selection also retains every pre-mask timer sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventStatusProbeEvidence {
    pub stop: Ieee802154EventStatusProbeStop,
    pub event_enable_before: Ieee802154ValidationEventEnableState,
    pub event_enable_active: Ieee802154ValidationEventEnableState,
    pub event_enable_after: Ieee802154ValidationEventEnableState,
    pub post_enable_events: Ieee802154ObservedEventState,
    pub timer0_value_before_start: u32,
    pub timer1_value_before_start: u32,
    pub timer0_value_min: u32,
    pub timer0_value_max: u32,
    pub timer1_value_min: u32,
    pub timer1_value_max: u32,
    pub timer0_value_after_stop: u32,
    pub timer1_value_after_stop: u32,
    pub reset_events: Ieee802154ObservedEventState,
    pub dual_observed_events: Ieee802154ObservedEventState,
    pub dual_latched_events: Ieee802154ObservedEventState,
    pub after_timer0_ack_events: Ieee802154ObservedEventState,
    pub after_timer1_ack_events: Ieee802154ObservedEventState,
    pub distinct_snapshot_events: Ieee802154ObservedEventState,
    pub distinct_before_ack_events: Ieee802154ObservedEventState,
    pub distinct_after_ack_events: Ieee802154ObservedEventState,
    pub cleanup_pending_events: Ieee802154ObservedEventState,
    pub final_events: Ieee802154ObservedEventState,
}

impl Ieee802154EventStatusProbeEvidence {
    const fn empty() -> Self {
        Self {
            stop: Ieee802154EventStatusProbeStop::UnsupportedSetup,
            event_enable_before: Ieee802154ValidationEventEnableState::AllMasked,
            event_enable_active: Ieee802154ValidationEventEnableState::AllMasked,
            event_enable_after: Ieee802154ValidationEventEnableState::AllMasked,
            post_enable_events: Ieee802154ObservedEventState::Clear,
            timer0_value_before_start: 0,
            timer1_value_before_start: 0,
            timer0_value_min: 0,
            timer0_value_max: 0,
            timer1_value_min: 0,
            timer1_value_max: 0,
            timer0_value_after_stop: 0,
            timer1_value_after_stop: 0,
            reset_events: Ieee802154ObservedEventState::Clear,
            dual_observed_events: Ieee802154ObservedEventState::Clear,
            dual_latched_events: Ieee802154ObservedEventState::Clear,
            after_timer0_ack_events: Ieee802154ObservedEventState::Clear,
            after_timer1_ack_events: Ieee802154ObservedEventState::Clear,
            distinct_snapshot_events: Ieee802154ObservedEventState::Clear,
            distinct_before_ack_events: Ieee802154ObservedEventState::Clear,
            distinct_after_ack_events: Ieee802154ObservedEventState::Clear,
            cleanup_pending_events: Ieee802154ObservedEventState::Clear,
            final_events: Ieee802154ObservedEventState::Clear,
        }
    }
}

/// Closed register vocabulary consumed by the pure validation state machine.
///
/// The target implementation is the unique PAC lease in a validation build.
/// Tests implement this trait only inside this module; callers cannot provide
/// raw addresses or broaden the two selected writes.
pub(crate) trait Ieee802154EventStatusProbeBackend {
    fn event_enable_state(&mut self) -> Ieee802154ValidationEventEnableState;
    fn enable_timer_events(&mut self);
    fn disable_all_events(&mut self);
    fn interrupt_route_state(&mut self) -> Ieee802154RouteState;
    fn event_status_state(&mut self) -> Ieee802154ObservedEventState;
    fn timer0_value(&mut self) -> u32;
    fn timer1_value(&mut self) -> u32;
    fn set_timer_thresholds(&mut self, threshold: u32);
    fn start_timer0(&mut self);
    fn stop_timer0(&mut self);
    fn start_timer1(&mut self);
    fn stop_timer1(&mut self);
    fn write_timer0_event(&mut self);
    fn write_timer1_event(&mut self);
}

#[cfg(feature = "validation-probes")]
impl Ieee802154EventStatusProbeBackend for crate::ieee802154::backend::Ieee802154PacHal<'_> {
    fn event_enable_state(&mut self) -> Ieee802154ValidationEventEnableState {
        self.validation_event_enable_state()
    }

    fn enable_timer_events(&mut self) {
        self.validation_enable_timer_events();
    }

    fn disable_all_events(&mut self) {
        self.validation_disable_all_events();
    }

    fn interrupt_route_state(&mut self) -> Ieee802154RouteState {
        self.interrupt_route_state()
    }

    fn event_status_state(&mut self) -> Ieee802154ObservedEventState {
        self.validation_event_status_state()
    }

    fn timer0_value(&mut self) -> u32 {
        self.validation_event_timer0_value()
    }

    fn timer1_value(&mut self) -> u32 {
        self.validation_event_timer1_value()
    }

    fn set_timer_thresholds(&mut self, threshold: u32) {
        self.validation_set_event_timer_thresholds(threshold);
    }

    fn start_timer0(&mut self) {
        self.validation_start_event_timer0();
    }

    fn stop_timer0(&mut self) {
        self.validation_stop_event_timer0();
    }

    fn start_timer1(&mut self) {
        self.validation_start_event_timer1();
    }

    fn stop_timer1(&mut self) {
        self.validation_stop_event_timer1();
    }

    fn write_timer0_event(&mut self) {
        self.validation_write_event_timer0();
    }

    fn write_timer1_event(&mut self) {
        self.validation_write_event_timer1();
    }
}

fn poll_events<B, F>(
    backend: &mut B,
    poll_limit: u32,
    predicate: F,
) -> (bool, Ieee802154ObservedEventState)
where
    B: Ieee802154EventStatusProbeBackend,
    F: Fn(Ieee802154ObservedEventState) -> bool,
{
    let mut observed = Ieee802154ObservedEventState::Clear;
    for _ in 0..poll_limit {
        observed = backend.event_status_state();
        if predicate(observed) {
            return (true, observed);
        }
    }
    (false, observed)
}

fn finish<B>(
    backend: &mut B,
    mut evidence: Ieee802154EventStatusProbeEvidence,
    stop: Ieee802154EventStatusProbeStop,
    retained_events: Ieee802154ObservedEventState,
    status_cleanup_allowed: bool,
) -> Ieee802154EventStatusProbeEvidence
where
    B: Ieee802154EventStatusProbeBackend,
{
    backend.stop_timer0();
    backend.stop_timer1();
    backend.disable_all_events();
    evidence.event_enable_after = backend.event_enable_state();

    let pending = backend.event_status_state();
    evidence.cleanup_pending_events = pending;
    if evidence.event_enable_after == Ieee802154ValidationEventEnableState::AllMasked
        && status_cleanup_allowed
    {
        let selected = retained_events.union(pending);
        if selected.has_timer0() {
            backend.write_timer0_event();
        }
        if selected.has_timer1() {
            backend.write_timer1_event();
        }
    }

    evidence.final_events = backend.event_status_state();
    let route = backend.interrupt_route_state();
    evidence.stop = if evidence.final_events.is_clear()
        && evidence.event_enable_after == Ieee802154ValidationEventEnableState::AllMasked
        && route.is_reset_detached()
    {
        stop
    } else {
        Ieee802154EventStatusProbeStop::CleanupNotClear
    };
    evidence
}

fn finish_without_writes<B>(
    backend: &mut B,
    mut evidence: Ieee802154EventStatusProbeEvidence,
    stop: Ieee802154EventStatusProbeStop,
) -> Ieee802154EventStatusProbeEvidence
where
    B: Ieee802154EventStatusProbeBackend,
{
    evidence.final_events = backend.event_status_state();
    evidence.event_enable_after = backend.event_enable_state();
    evidence.stop = stop;
    evidence
}

/// Execute the pure sequence shared by the real PAC lease and host fake.
pub(crate) fn run_ieee802154_event_status_probe<B>(
    backend: &mut B,
    config: Ieee802154EventStatusProbeConfig,
) -> Ieee802154EventStatusProbeEvidence
where
    B: Ieee802154EventStatusProbeBackend,
{
    let mut evidence = Ieee802154EventStatusProbeEvidence::empty();

    evidence.event_enable_before = backend.event_enable_state();
    if evidence.event_enable_before != Ieee802154ValidationEventEnableState::AllMasked {
        return finish_without_writes(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::UnsupportedSetup,
        );
    }

    let route = backend.interrupt_route_state();
    if !route.is_reset_detached() {
        return finish_without_writes(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::RouteNotQuiesced,
        );
    }

    evidence.reset_events = backend.event_status_state();
    if !evidence.reset_events.is_clear() {
        return finish_without_writes(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::ResetNotClear,
        );
    }

    backend.set_timer_thresholds(config.timer_threshold());
    backend.enable_timer_events();
    evidence.event_enable_active = backend.event_enable_state();
    if evidence.event_enable_active != Ieee802154ValidationEventEnableState::TimerPairOnly {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::EventEnableReadbackMismatch,
            Ieee802154ObservedEventState::Clear,
            false,
        );
    }

    evidence.post_enable_events = backend.event_status_state();
    let mut retained_events = evidence.post_enable_events;
    if !evidence.post_enable_events.is_clear() {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::PostEnableStatusNotClear,
            retained_events,
            true,
        );
    }

    let route = backend.interrupt_route_state();
    if !route.is_reset_detached() {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::RouteNotQuiesced,
            retained_events,
            false,
        );
    }

    evidence.timer0_value_before_start = backend.timer0_value();
    evidence.timer1_value_before_start = backend.timer1_value();
    evidence.timer0_value_min = evidence.timer0_value_before_start;
    evidence.timer0_value_max = evidence.timer0_value_before_start;
    evidence.timer1_value_min = evidence.timer1_value_before_start;
    evidence.timer1_value_max = evidence.timer1_value_before_start;
    backend.start_timer0();
    backend.start_timer1();
    let mut timer0_changed = false;
    let mut timer1_changed = false;
    let mut dual_latched = false;
    for _ in 0..config.poll_limit() {
        let timer0_value = backend.timer0_value();
        let timer1_value = backend.timer1_value();
        evidence.timer0_value_min = evidence.timer0_value_min.min(timer0_value);
        evidence.timer0_value_max = evidence.timer0_value_max.max(timer0_value);
        evidence.timer1_value_min = evidence.timer1_value_min.min(timer1_value);
        evidence.timer1_value_max = evidence.timer1_value_max.max(timer1_value);
        if !timer0_changed && timer0_value != evidence.timer0_value_before_start {
            timer0_changed = true;
        }
        if !timer1_changed && timer1_value != evidence.timer1_value_before_start {
            timer1_changed = true;
        }
        evidence.dual_latched_events = backend.event_status_state();
        evidence.dual_observed_events = evidence
            .dual_observed_events
            .union(evidence.dual_latched_events);
        retained_events = retained_events.union(evidence.dual_latched_events);
        if evidence.dual_latched_events == Ieee802154ObservedEventState::Timer0AndTimer1 {
            dual_latched = true;
            break;
        }
    }

    // Freeze both independent stimuli before classifying the bounded wait or
    // testing selected acknowledgements. Counter samples distinguish timer activity
    // from absent status observations while exactly the timer events remain
    // enabled and the CPU route remains reset-detached.
    backend.stop_timer0();
    backend.stop_timer1();
    evidence.timer0_value_after_stop = backend.timer0_value();
    evidence.timer1_value_after_stop = backend.timer1_value();
    if !dual_latched {
        let stop = if timer0_changed && timer1_changed {
            Ieee802154EventStatusProbeStop::DualLatchTimeout
        } else {
            Ieee802154EventStatusProbeStop::TimerActivityTimeout
        };
        return finish(backend, evidence, stop, retained_events, true);
    }

    backend.write_timer0_event();
    evidence.after_timer0_ack_events = backend.event_status_state();
    retained_events = retained_events.union(evidence.after_timer0_ack_events);
    if evidence.after_timer0_ack_events != Ieee802154ObservedEventState::Timer1Only {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch,
            retained_events,
            true,
        );
    }

    backend.write_timer1_event();
    evidence.after_timer1_ack_events = backend.event_status_state();
    retained_events = retained_events.union(evidence.after_timer1_ack_events);
    if !evidence.after_timer1_ack_events.is_clear() {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch,
            retained_events,
            true,
        );
    }

    // Re-arm timer zero alone and retain the successful polling read as the
    // snapshot whose selected bit will be acknowledged later.
    backend.set_timer_thresholds(config.timer_threshold());
    backend.start_timer0();
    let (first_latched, first_events) = poll_events(backend, config.poll_limit(), |events| {
        events == Ieee802154ObservedEventState::Timer0Only
    });
    evidence.distinct_snapshot_events = first_events;
    retained_events = retained_events.union(first_events);
    if !first_latched {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::DistinctFirstLatchTimeout,
            retained_events,
            true,
        );
    }
    backend.stop_timer0();

    // Timer one is introduced strictly after the retained timer-zero sample
    // and before the selected acknowledgement of timer zero's earlier event.
    backend.start_timer1();
    let (second_latched, second_events) = poll_events(backend, config.poll_limit(), |events| {
        events == Ieee802154ObservedEventState::Timer0AndTimer1
    });
    evidence.distinct_before_ack_events = second_events;
    retained_events = retained_events.union(second_events);
    if !second_latched {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::DistinctSecondLatchTimeout,
            retained_events,
            true,
        );
    }
    backend.stop_timer1();

    backend.write_timer0_event();
    evidence.distinct_after_ack_events = backend.event_status_state();
    retained_events = retained_events.union(evidence.distinct_after_ack_events);
    if evidence.distinct_after_ack_events != Ieee802154ObservedEventState::Timer1Only {
        return finish(
            backend,
            evidence,
            Ieee802154EventStatusProbeStop::SelectiveAcknowledgeMismatch,
            retained_events,
            true,
        );
    }

    finish(
        backend,
        evidence,
        Ieee802154EventStatusProbeStop::Complete,
        retained_events,
        true,
    )
}

#[cfg(test)]
mod tests;

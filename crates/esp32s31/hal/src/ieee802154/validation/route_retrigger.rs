//! Validation-only answers to the two hardware questions the W1C access model
//! of `EVENT_STATUS` leaves open: whether a second arrival of an event whose
//! bit is still latched survives the acknowledgement of the first, and whether
//! the source-132 line retriggers the CPU route for an event that latched
//! after the ISR sampled its snapshot.
//!
//! The probe runs over the production MAC owners through the chip-neutral
//! low-level interface, with the two MAC timers as its only stimulus. The
//! polled phase needs no route. The routed phases run inside the validation
//! ISR the platform binds to source 132: each entry samples the complete
//! event field, optionally raises a further event, then consumes exactly the
//! sampled snapshot, as the production ISR does. The platform counts entries.

use oer_espressif_ieee802154_engine::{
    ll::{Ieee802154LowLevel, Ieee802154Timer},
    types::{Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservation},
};

/// The event and timer accessors the probe uses, from the chip-neutral
/// low-level interface the production MAC owners implement.
pub trait Ieee802154RouteProbeRegisters {
    /// `i_ll_get_events`.
    fn events(&mut self) -> Ieee802154EventObservation;
    /// `i_ll_clear_events`.
    fn clear_events(&mut self, mask: Ieee802154EventMask);
    /// The timer's threshold in microseconds.
    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32);
    /// The timer's start command.
    fn start_timer(&mut self, timer: Ieee802154Timer);
    /// The timer's stop command.
    fn stop_timer(&mut self, timer: Ieee802154Timer);
}

impl<L: Ieee802154LowLevel + ?Sized> Ieee802154RouteProbeRegisters for L {
    fn events(&mut self) -> Ieee802154EventObservation {
        Ieee802154LowLevel::events(self)
    }

    fn clear_events(&mut self, mask: Ieee802154EventMask) {
        Ieee802154LowLevel::clear_events(self, mask);
    }

    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32) {
        Ieee802154LowLevel::set_timer_threshold(self, timer, microseconds);
    }

    fn start_timer(&mut self, timer: Ieee802154Timer) {
        Ieee802154LowLevel::start_timer(self, timer);
    }

    fn stop_timer(&mut self, timer: Ieee802154Timer) {
        Ieee802154LowLevel::stop_timer(self, timer);
    }
}

/// Bounded timing of one probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RouteProbeConfig {
    threshold_micros: u32,
    settle_micros: u32,
}

impl Ieee802154RouteProbeConfig {
    /// Wire-compatible upper bound of the timer threshold.
    pub const MAX_THRESHOLD_MICROS: u32 = 10_000;
    /// Wire-compatible upper bound of one settling wait.
    pub const MAX_SETTLE_MICROS: u32 = 100_000;

    /// A probe whose timers fire `threshold_micros` after their start, and
    /// whose waits for a second arrival last `settle_micros`, which must be
    /// at least four thresholds so the timer certainly fired again.
    pub const fn new(threshold_micros: u32, settle_micros: u32) -> Option<Self> {
        if threshold_micros == 0
            || threshold_micros > Self::MAX_THRESHOLD_MICROS
            || settle_micros > Self::MAX_SETTLE_MICROS
            || settle_micros < threshold_micros.saturating_mul(4)
        {
            None
        } else {
            Some(Self {
                threshold_micros,
                settle_micros,
            })
        }
    }

    /// Timer threshold in microseconds.
    pub const fn threshold_micros(self) -> u32 {
        self.threshold_micros
    }

    /// Settling wait in microseconds.
    pub const fn settle_micros(self) -> u32 {
        self.settle_micros
    }
}

/// The timer events of one sample.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ieee802154TimerEvents {
    /// No event is latched.
    #[default]
    Clear,
    /// Exactly TIMER0 overflow.
    Timer0,
    /// Exactly TIMER1 overflow.
    Timer1,
    /// Exactly both timer overflows.
    Both,
    /// Any other event, or an unnamed bit, is latched.
    Other,
}

impl Ieee802154TimerEvents {
    fn of(observation: Ieee802154EventObservation) -> Self {
        let Ok(events) = observation.classification() else {
            return Self::Other;
        };
        let timer0 = Ieee802154Event::Timer0Overflow.mask();
        let timer1 = Ieee802154Event::Timer1Overflow.mask();
        let both = timer0.union(timer1);
        if events.is_empty() {
            Self::Clear
        } else if events == timer0 {
            Self::Timer0
        } else if events == timer1 {
            Self::Timer1
        } else if events == both {
            Self::Both
        } else {
            Self::Other
        }
    }
}

/// What the acknowledgement of the first arrival left of a second arrival of
/// the same, still latched event.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ieee802154SameBitOutcome {
    /// The phase did not reach the acknowledgement.
    #[default]
    NotRun,
    /// The acknowledgement also cleared the second arrival: the arrivals
    /// coalesce into one latched bit.
    Coalesced,
    /// The bit stayed latched after the acknowledgement.
    Retained,
}

/// Why a probe ended.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ieee802154RouteProbeStop {
    /// Every phase ran and the field is clear.
    Complete,
    /// The field was not clear before a phase began.
    #[default]
    NotClear,
    /// A timer event did not latch within the bound.
    LatchTimeout,
    /// A phase observed an event it did not raise.
    UnexpectedEvent,
}

/// The polled same-bit phase.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154PolledSameBit {
    /// The snapshot whose bit is later acknowledged.
    pub snapshot: Ieee802154TimerEvents,
    /// The field after acknowledging the snapshot once the timer fired
    /// again.
    pub after_acknowledgement: Ieee802154TimerEvents,
    /// Control: the field once the timer, restarted after an
    /// acknowledgement, fired again.
    pub control_rearmed: Ieee802154TimerEvents,
    pub outcome: Ieee802154SameBitOutcome,
}

/// Run the polled same-bit phase: latch TIMER0, restart it and let it fire
/// again while its bit is still latched, then acknowledge the first
/// snapshot. The control restarts the timer only after acknowledging, and
/// must see a new arrival.
///
/// `now_micros` is the platform's monotonic clock. TIMER0 overflow must be
/// enabled and the CPU route unbound.
pub fn run_polled_same_bit<L: Ieee802154RouteProbeRegisters + ?Sized>(
    ll: &mut L,
    config: Ieee802154RouteProbeConfig,
    now_micros: &mut dyn FnMut() -> u64,
) -> (
    Ieee802154PolledSameBit,
    Result<(), Ieee802154RouteProbeStop>,
) {
    let mut phase = Ieee802154PolledSameBit::default();
    let result = (|| {
        if !ll.events().is_clear() {
            return Err(Ieee802154RouteProbeStop::NotClear);
        }
        ll.set_timer_threshold(Ieee802154Timer::Timer0, config.threshold_micros());
        ll.start_timer(Ieee802154Timer::Timer0);
        let snapshot = wait_for(ll, Ieee802154Event::Timer0Overflow, config, now_micros)?;
        phase.snapshot = Ieee802154TimerEvents::of(snapshot);
        if phase.snapshot != Ieee802154TimerEvents::Timer0 {
            return Err(Ieee802154RouteProbeStop::UnexpectedEvent);
        }
        // Fire again while the first arrival is still latched.
        ll.stop_timer(Ieee802154Timer::Timer0);
        ll.start_timer(Ieee802154Timer::Timer0);
        settle(config, now_micros);
        ll.stop_timer(Ieee802154Timer::Timer0);
        acknowledge(ll, snapshot);
        phase.after_acknowledgement = Ieee802154TimerEvents::of(ll.events());
        phase.outcome = match phase.after_acknowledgement {
            Ieee802154TimerEvents::Clear => Ieee802154SameBitOutcome::Coalesced,
            Ieee802154TimerEvents::Timer0 => Ieee802154SameBitOutcome::Retained,
            _ => return Err(Ieee802154RouteProbeStop::UnexpectedEvent),
        };
        let remaining = ll.events();
        acknowledge(ll, remaining);
        // Control: a restart after the acknowledgement latches a new arrival.
        ll.start_timer(Ieee802154Timer::Timer0);
        settle(config, now_micros);
        ll.stop_timer(Ieee802154Timer::Timer0);
        let control = ll.events();
        phase.control_rearmed = Ieee802154TimerEvents::of(control);
        acknowledge(ll, control);
        if phase.control_rearmed != Ieee802154TimerEvents::Timer0 {
            return Err(Ieee802154RouteProbeStop::LatchTimeout);
        }
        Ok(())
    })();
    (phase, result)
}

/// What one validation ISR entry does before consuming its snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154RouteProbeAction {
    /// Consume the snapshot only.
    Acknowledge,
    /// Start TIMER1 and wait until it latches: a different event arrives
    /// after the snapshot.
    RaiseTimer1,
    /// Restart TIMER0 and let it fire again: the same event arrives after
    /// the snapshot.
    RefireTimer0,
}

/// One validation ISR entry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154RouteProbeEntry {
    /// The snapshot the entry sampled and consumed.
    pub snapshot: Ieee802154TimerEvents,
    /// The field just before the snapshot was consumed.
    pub before_acknowledgement: Ieee802154TimerEvents,
}

/// The body of one validation ISR entry: sample the complete event field,
/// perform `action`, then consume exactly the sampled snapshot.
pub fn route_probe_entry<L: Ieee802154RouteProbeRegisters + ?Sized>(
    ll: &mut L,
    action: Ieee802154RouteProbeAction,
    config: Ieee802154RouteProbeConfig,
    now_micros: &mut dyn FnMut() -> u64,
) -> Ieee802154RouteProbeEntry {
    let snapshot = ll.events();
    match action {
        Ieee802154RouteProbeAction::Acknowledge => {}
        Ieee802154RouteProbeAction::RaiseTimer1 => {
            ll.set_timer_threshold(Ieee802154Timer::Timer1, config.threshold_micros());
            ll.start_timer(Ieee802154Timer::Timer1);
            let _ = wait_for(ll, Ieee802154Event::Timer1Overflow, config, now_micros);
            ll.stop_timer(Ieee802154Timer::Timer1);
        }
        Ieee802154RouteProbeAction::RefireTimer0 => {
            ll.stop_timer(Ieee802154Timer::Timer0);
            ll.start_timer(Ieee802154Timer::Timer0);
            settle(config, now_micros);
            ll.stop_timer(Ieee802154Timer::Timer0);
        }
    }
    let before_acknowledgement = Ieee802154TimerEvents::of(ll.events());
    acknowledge(ll, snapshot);
    Ieee802154RouteProbeEntry {
        snapshot: Ieee802154TimerEvents::of(snapshot),
        before_acknowledgement,
    }
}

/// Start TIMER0 to raise the first routed event of a phase.
pub fn start_route_probe_phase<L: Ieee802154RouteProbeRegisters + ?Sized>(
    ll: &mut L,
    config: Ieee802154RouteProbeConfig,
) -> Result<(), Ieee802154RouteProbeStop> {
    if !ll.events().is_clear() {
        return Err(Ieee802154RouteProbeStop::NotClear);
    }
    ll.set_timer_threshold(Ieee802154Timer::Timer0, config.threshold_micros());
    ll.start_timer(Ieee802154Timer::Timer0);
    Ok(())
}

/// Stop both timers and consume whatever is latched. Returns what was
/// latched, and [`IRouteProbeStop::Complete`] only when nothing was.
pub fn finish_route_probe<L: Ieee802154RouteProbeRegisters + ?Sized>(
    ll: &mut L,
) -> (Ieee802154TimerEvents, Ieee802154RouteProbeStop) {
    ll.stop_timer(Ieee802154Timer::Timer0);
    ll.stop_timer(Ieee802154Timer::Timer1);
    let pending = ll.events();
    acknowledge(ll, pending);
    let pending = Ieee802154TimerEvents::of(pending);
    let stop = if pending == Ieee802154TimerEvents::Clear {
        Ieee802154RouteProbeStop::Complete
    } else {
        Ieee802154RouteProbeStop::NotClear
    };
    (pending, stop)
}

/// Consume exactly the named events of `snapshot`.
fn acknowledge<L: Ieee802154RouteProbeRegisters + ?Sized>(
    ll: &mut L,
    snapshot: Ieee802154EventObservation,
) {
    let events = snapshot
        .classification()
        .unwrap_or(Ieee802154EventMask::NONE);
    ll.clear_events(events);
}

fn settle(config: Ieee802154RouteProbeConfig, now_micros: &mut dyn FnMut() -> u64) {
    let until = now_micros() + u64::from(config.settle_micros());
    while now_micros() < until {}
}

fn wait_for<L: Ieee802154RouteProbeRegisters + ?Sized>(
    ll: &mut L,
    event: Ieee802154Event,
    config: Ieee802154RouteProbeConfig,
    now_micros: &mut dyn FnMut() -> u64,
) -> Result<Ieee802154EventObservation, Ieee802154RouteProbeStop> {
    let until = now_micros() + u64::from(config.settle_micros());
    loop {
        let observed = ll.events();
        if observed.contains(event) {
            return Ok(observed);
        }
        if now_micros() >= until {
            return Err(Ieee802154RouteProbeStop::LatchTimeout);
        }
    }
}

#[cfg(test)]
mod tests;

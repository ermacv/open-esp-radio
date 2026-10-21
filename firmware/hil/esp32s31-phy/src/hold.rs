//! Forced due tracking in the diagnostic image only, with bounded coarse edges.
mod recorder;
mod sar_tails;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_phy::{
    PhyTargetObserver,
    state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS,
    tracking::observation::{Event, Operation},
};
use oer_hil_protocol::{base::RejectReason, phy::*};
use oer_time::{Clock, Duration, Instant};

pub struct Hold {
    pub started: HoldStarted,
    pub samples: heapless::Vec<HoldSample, HOLD_TICKS>,
    pub invalid: bool,
    pub running: bool,
    pub deadline: Instant,
    pub saved_debug: [u8; 2],
    pub engine: HoldPolls,
}
impl Hold {
    pub fn new(started: HoldStarted, saved_debug: [u8; 2]) -> Self {
        Self {
            started,
            samples: heapless::Vec::new(),
            invalid: false,
            running: true,
            deadline: EmbassyClock
                .now()
                .saturating_add(Duration::from_micros(DEFAULT_PLL_TRACK_PERIOD_MICROS)),
            saved_debug,
            engine: HoldPolls::default(),
        }
    }
    pub fn page(&self, request: GetTrackingHold) -> Result<TrackingHold, RejectReason> {
        if request.id != self.started.id || request.first > self.started.ticks {
            return Err(RejectReason::InvalidState);
        }
        Ok(TrackingHold {
            engine: self.engine,
            id: request.id,
            first: request.first,
            available: self.samples.len() as u8,
            terminal: !self.running,
            invalid: self.invalid,
            sample: self.samples.get(usize::from(request.first)).cloned(),
        })
    }
}

/// Publish the completed observation after tracking, outside the task poll frame.
#[inline(never)]
pub fn finish_sample(hold: &mut Hold, recorder: recorder::Recorder, committed: u8, failed: bool) {
    let sample = recorder.finish(committed, failed);
    hold.invalid |= sample.invalid;
    if hold.samples.push(sample).is_err() {
        hold.invalid = true;
    }
    hold.running = !hold.invalid && hold.samples.len() < usize::from(hold.started.ticks);
}

pub struct Observer<'a>(
    pub recorder::Recorder,
    pub sar_tails::Tails,
    &'a mut HoldPolls,
    Option<&'a mut oer_esp32s31_phy::tracking::observation::ReadinessTrace>,
);
impl<'a> Observer<'a> {
    pub fn new(
        tick: u8,
        engine: &'a mut HoldPolls,
        trace: Option<&'a mut oer_esp32s31_phy::tracking::observation::ReadinessTrace>,
    ) -> Self {
        Self(
            recorder::Recorder::new(tick),
            sar_tails::Tails::default(),
            engine,
            trace,
        )
    }
}
impl PhyTargetObserver for &mut Observer<'_> {
    const OBSERVE_ENGINE_POLLS: bool = true;
    fn readiness_trace(
        &mut self,
    ) -> Option<&mut oer_esp32s31_phy::tracking::observation::ReadinessTrace> {
        self.3.as_deref_mut()
    }
    #[inline(never)]
    fn engine_completed(
        &mut self,
        operation: Operation,
        timing: oer_esp32s31_phy::tracking::observation::EngineTiming,
    ) {
        report_engine(operation, &timing);
        self.2.merge(HoldPolls {
            polls: timing.polls,
            pending: timing.pending,
            total_micros: timing.total_micros,
            maximum_micros: timing.maximum_micros,
            resumes: timing.resumes,
            lateness_micros: timing.lateness_micros,
            maximum_lateness_micros: timing.maximum_lateness_micros,
            ready_edges: timing.ready_edges,
            maximum_ready_micros: timing.maximum_ready_micros,
            invalid: timing.invalid,
        });
    }

    fn tx_sar_wait(&mut self, elapsed: u64, observations: u16, ready: bool) {
        self.1.observe(elapsed, observations, ready);
    }
    fn tracking_operation(&mut self, operation: Operation, event: Event) {
        let operation = match operation {
            Operation::Tracking => HoldOperation::Tracking,
            Operation::GrantProtectRfpll => HoldOperation::Rfpll,
            Operation::GrantProtectCommon => HoldOperation::Common,
            Operation::GrantProtectTransmit => HoldOperation::Transmit,
            Operation::WifiBasebandOff => HoldOperation::WifiBasebandOff,
            _ => return,
        };
        match event {
            Event::Started => {
                self.0
                    .observe(operation, true, false, EmbassyClock.now().as_micros())
            }
            Event::Completed | Event::Failed => self.0.observe(
                operation,
                false,
                event == Event::Failed,
                EmbassyClock.now().as_micros(),
            ),
            _ => {}
        }
    }
}

/// Family formatting stays outside the observer and measured driver frames.
#[inline(never)]
fn report_engine(
    operation: Operation,
    timing: &oer_esp32s31_phy::tracking::observation::EngineTiming,
) {
    use oer_esp32s31_board::rom::print_fmt;
    let family = match operation {
        Operation::Rfpll => 1,
        Operation::Temperature => 7,
        _ => 0,
    };
    print_fmt(format_args!(
        "OPEN_RADIO_HIL engine family={} polls={} pending={} ready={}\n",
        family,
        timing.polls,
        timing.pending,
        timing.polls.saturating_sub(timing.pending)
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL engine family={} total-us={} max-us={}\n",
        family, timing.total_micros, timing.maximum_micros
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL engine family={} resumes={} late-total-us={}\n",
        family, timing.resumes, timing.lateness_micros
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL engine family={} late-max-us={} invalid={}\n",
        family, timing.maximum_lateness_micros, timing.invalid
    ));
}

/// Report preparation only after maintenance returns, including failures.
#[inline(never)]
pub fn report_preparation(
    trace: &oer_esp32s31_phy::tracking::observation::ReadinessTrace,
    polls: &HoldPolls,
) {
    use oer_esp32s31_board::rom::print_fmt;
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-prepare polls={} max-poll-us={}\n",
        polls.polls, polls.maximum_micros
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-prepare resume-count={} late-max-us={} late-total-us={}\n",
        polls.resumes, polls.maximum_lateness_micros, polls.lateness_micros
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-prepare ready-count={} ready-max-us={}\n",
        polls.ready_edges, polls.maximum_ready_micros
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-prepare commands={} first-check-max-us={}\n",
        trace.commands, trace.maximum_first_check_micros
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-prepare yield-count={} yield-total-us={} yield-max-us={}\n",
        trace.yields, trace.yield_total_micros, trace.yield_maximum_micros
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-prepare failure-elapsed-us={} failure-late-us={}\n",
        trace.failure_elapsed_micros, trace.failure_lateness_micros
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-prepare failure-busy={:?} complete={}\n",
        trace.failure_busy,
        trace.complete()
    ));
    for sample in trace.not_ready_samples() {
        print_fmt(format_args!(
            "OPEN_RADIO_HIL t7-prepare not-ready command={} elapsed-us={}\n",
            sample.command, sample.elapsed_micros
        ));
    }
}

#[cfg(oer_profile)]
#[inline(never)]
pub fn report_timer(window: Option<oer_espressif_executor::timer_observation::Window>) {
    let Some(window) = window else {
        return;
    };
    let r = window.finish();
    use oer_esp32s31_board::rom::print_fmt;
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-timer registrations={} due-at-register={} irqs={}\n",
        r.registrations, r.due_at_registration, r.interrupts
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-timer program-max-us={} dispatch-max-us={} invalid={}\n",
        r.programming.maximum_micros, r.dispatch.maximum_micros, r.invalid
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-timer alarm-to-irq-max-us={} irq-late-max-us={} irq-to-dispatch-max-us={}\n",
        r.alarm_to_irq.maximum_micros,
        r.deadline_lateness.maximum_micros,
        r.irq_to_dispatch.maximum_micros
    ));
    print_fmt(format_args!(
        "OPEN_RADIO_HIL t7-timer unmatched-irqs={} unmatched-dispatch={} armed={}\n",
        r.unmatched_interrupts, r.unmatched_dispatches, r.armed_at_end
    ));
}

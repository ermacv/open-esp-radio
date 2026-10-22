//! Forced due tracking in the diagnostic image only, with bounded coarse edges.
pub mod metrics;
mod observer;
mod recorder;
pub use observer::Observer;
mod sar_tails;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_phy::state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS;
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

/// The diagnostic image's UART sink; observer tests inject their own writer.
pub struct Uart;
impl core::fmt::Write for Uart {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        oer_esp32s31_board::rom::print_fmt(format_args!("{}", value));
        Ok(())
    }
    fn write_fmt(&mut self, args: core::fmt::Arguments<'_>) -> core::fmt::Result {
        oer_esp32s31_board::rom::print_fmt(args);
        Ok(())
    }
}

impl<C: Clock, W: core::fmt::Write> oer_esp32s31_phy::PhyTargetObserver
    for &mut Observer<'_, C, W>
{
    const OBSERVE_ENGINE_POLLS: bool = true;
    fn channel_action_completed(
        &mut self,
        action: u8,
        timing: &oer_esp32s31_phy::tracking::observation::EngineTiming,
        entries: [u8; 2],
    ) {
        Observer::channel_action_completed(self, action, timing, entries);
    }
    fn rx_gain_execution(
        &mut self,
        execution: oer_esp32s31_phy::tracking::observation::RxGainExecution,
    ) {
        Observer::rx_gain_execution(self, execution);
    }
    fn rfpll_engine_trace(
        &mut self,
    ) -> Option<&mut oer_esp32s31_phy::tracking::observation::RfpllEngineTrace> {
        Observer::rfpll_engine_trace(self)
    }
    fn power_gain_completed(
        &mut self,
        operation: oer_esp32s31_phy::tracking::observation::Operation,
        timing: oer_esp32s31_phy::tracking::observation::GainTiming,
    ) {
        Observer::power_gain_completed(self, operation, timing);
    }
    fn readiness_trace(
        &mut self,
    ) -> Option<&mut oer_esp32s31_phy::tracking::observation::ReadinessTrace> {
        Observer::readiness_trace(self)
    }
    fn engine_completed(
        &mut self,
        operation: oer_esp32s31_phy::tracking::observation::Operation,
        timing: oer_esp32s31_phy::tracking::observation::EngineTiming,
    ) {
        Observer::engine_completed(self, operation, timing);
    }
    fn tx_sar_wait(&mut self, elapsed: u64, observations: u16, ready: bool) {
        Observer::tx_sar_wait(self, elapsed, observations, ready);
    }
    fn tracking_operation(
        &mut self,
        operation: oer_esp32s31_phy::tracking::observation::Operation,
        event: oer_esp32s31_phy::tracking::observation::Event,
    ) {
        Observer::tracking_operation(self, operation, event);
    }
}

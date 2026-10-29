//! Forced due tracking in the diagnostic image only, with bounded coarse edges.
mod observer;
#[cfg(target_arch = "riscv32")]
pub use observer::Observer;
#[cfg(target_arch = "riscv32")]
use oer_diagnostics::Note;
#[cfg(target_arch = "riscv32")]
use oer_embassy_time::EmbassyClock;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_phy::state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS;
use oer_esp32s31_phy_probe::hold::metrics::Metrics;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_phy_probe::hold::{notes::Notes, recorder::Recorder};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_phy_probe::*;
#[cfg(target_arch = "riscv32")]
use oer_hil_protocol::base::RejectReason;
use oer_time::Clock;
#[cfg(target_arch = "riscv32")]
use oer_time::{Duration, Instant};

#[cfg(target_arch = "riscv32")]
pub struct Hold {
    pub started: HoldStarted,
    pub samples: heapless::Vec<HoldSample, HOLD_TICKS>,
    pub invalid: bool,
    pub running: bool,
    pub deadline: Instant,
    pub saved_debug: [u8; 2],
    pub engine: HoldPolls,
}
#[cfg(target_arch = "riscv32")]
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
#[cfg(target_arch = "riscv32")]
#[inline(never)]
pub fn finish_sample(hold: &mut Hold, recorder: Recorder, committed: u8, failed: bool) {
    let sample = recorder.finish(committed, failed);
    hold.invalid |= sample.invalid;
    if hold.samples.push(sample).is_err() {
        hold.invalid = true;
    }
    hold.running = !hold.invalid && hold.samples.len() < usize::from(hold.started.ticks);
}

/// Report preparation only after maintenance returns, including failures.
#[cfg(target_arch = "riscv32")]
#[inline(never)]
pub async fn report_preparation(
    trace: &oer_esp32s31_phy::tracking::observation::ReadinessTrace,
    polls: &HoldPolls,
) {
    note(&Note::t7_prepare_polls(
        polls.polls.into(),
        polls.maximum_micros,
    ))
    .await;
    note(&Note::t7_prepare_resumes(
        polls.resumes.into(),
        polls.maximum_lateness_micros,
        polls.lateness_micros,
    ))
    .await;
    note(&Note::t7_prepare_ready(
        polls.ready_edges.into(),
        polls.maximum_ready_micros,
    ))
    .await;
    note(&Note::t7_prepare_commands(
        trace.commands.into(),
        trace.maximum_first_check_micros,
    ))
    .await;
    note(&Note::t7_prepare_yields(
        trace.yields.into(),
        trace.yield_total_micros,
        trace.yield_maximum_micros.into(),
    ))
    .await;
    note(&Note::t7_prepare_failure_times(
        trace.failure_elapsed_micros,
        trace.failure_lateness_micros,
    ))
    .await;
    note(&Note::t7_prepare_failure_busy(
        trace.failure_busy,
        trace.complete(),
    ))
    .await;
    for sample in trace.not_ready_samples() {
        note(&Note::t7_prepare_not_ready(
            sample.command.into(),
            sample.elapsed_micros.into(),
        ))
        .await;
    }
}

#[cfg(all(target_arch = "riscv32", oer_profile))]
#[inline(never)]
pub async fn report_timer(
    window: Option<
        oer_espressif_executor::timer_observation::Window<oer_esp32s31_compose::Esp32s31>,
    >,
) {
    let Some(window) = window else {
        return;
    };
    let r = window.finish();
    note(&Note::t7_timer_registrations(
        r.registrations.into(),
        r.due_at_registration.into(),
        r.interrupts.into(),
    ))
    .await;
    note(&Note::t7_timer_program(
        r.programming.maximum_micros.into(),
        r.dispatch.maximum_micros.into(),
        r.invalid.into(),
    ))
    .await;
    note(&Note::t7_timer_latency(
        r.alarm_to_irq.maximum_micros.into(),
        r.deadline_lateness.maximum_micros.into(),
        r.irq_to_dispatch.maximum_micros.into(),
    ))
    .await;
    note(&Note::t7_timer_unmatched(
        r.unmatched_interrupts.into(),
        r.unmatched_dispatches.into(),
        r.armed_at_end.into(),
    ))
    .await;
}

/// The diagnostic image's console: the notes go through its queue.
#[cfg(target_arch = "riscv32")]
pub struct Uart;
#[cfg(target_arch = "riscv32")]
impl Notes for Uart {
    async fn note(&mut self, note: &Note<'static>) {
        crate::transport::CONSOLE.note_reliably(note).await;
    }
}

#[cfg(target_arch = "riscv32")]
async fn note(note: &Note<'static>) {
    crate::transport::CONSOLE.note_reliably(note).await;
}

#[cfg(target_arch = "riscv32")]
impl<C: Clock, W: Notes> oer_esp32s31_phy::PhyTargetObserver for &mut Observer<'_, C, W> {
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

/// Keep root instrumentation identical for forced and natural policy.
pub async fn observe<F: core::future::Future, C: Clock>(
    future: F,
    clock: &C,
    metrics: &core::cell::RefCell<Metrics>,
) -> F::Output {
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|cx| {
        metrics.borrow_mut().enter(clock.now().as_micros());
        let result = future.as_mut().poll(cx);
        metrics
            .borrow_mut()
            .exit(clock.now().as_micros(), result.is_pending());
        result
    })
    .await
}

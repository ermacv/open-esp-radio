//! Image-local observer: callbacks only record; UART is flushed after root return.
use super::{Notes, metrics::Metrics, recorder::Recorder, sar_tails::Tails};
use core::cell::RefCell;
use oer_diagnostics::Note;
use oer_esp32s31_phy::tracking::observation::{
    EngineTiming, Event, GainTiming, Operation, ReadinessTrace, RfpllEngineTrace, RxGainExecution,
};
use oer_hil_protocol::phy::{HoldOperation, HoldPolls};
use oer_time::Clock;

pub struct Observer<'a, C, W> {
    pub recorder: Recorder,
    pub tails: Tails,
    engine: &'a mut HoldPolls,
    trace: Option<&'a mut ReadinessTrace>,
    engines: [HoldPolls; 9],
    gains: [Option<GainTiming>; 2],
    channels: heapless::Vec<u64, 32>,
    rx: Option<RxGainExecution>,
    clock: &'a C,
    output: &'a mut W,
    pub metrics: Option<&'a RefCell<Metrics>>,
}
impl<'a, C: Clock, W: Notes> Observer<'a, C, W> {
    pub fn new(
        tick: u8,
        engine: &'a mut HoldPolls,
        trace: Option<&'a mut ReadinessTrace>,
        clock: &'a C,
        output: &'a mut W,
    ) -> Self {
        Self {
            recorder: Recorder::new(tick),
            tails: Tails::default(),
            engine,
            trace,
            engines: [HoldPolls::default(); 9],
            gains: [None; 2],
            channels: heapless::Vec::new(),
            rx: None,
            clock,
            output,
            metrics: None,
        }
    }
    /// Called only after the root future returns, including failures.
    #[inline(never)]
    pub async fn report(&mut self, tick: u8) {
        for operation in [
            Operation::Rfpll,
            Operation::BluetoothIeee802154Power,
            Operation::WifiI2c,
            Operation::WifiPower,
            Operation::Dcode,
            Operation::RxGain,
            Operation::ChannelRestore,
            Operation::Temperature,
        ] {
            let timing = &self.engines[operation_family(operation)];
            if timing.polls == 0 {
                continue;
            }
            report_engine(operation, timing, self.output).await;
        }
        for (index, timing) in self.gains.iter().enumerate() {
            if let Some(timing) = timing {
                self.output
                    .note(&Note::gain_parts(
                        if index == 0 { 2 } else { 4 },
                        timing.lower_micros.into(),
                        timing.arithmetic_micros.into(),
                        timing.publication_micros.into(),
                    ))
                    .await;
            }
        }
        for packed in &self.channels {
            report_channel_action(*packed, self.output).await;
        }
        if let Some(execution) = &self.rx {
            report_rx_gain(execution, self.output).await;
        }
        self.tails.report(tick.into(), self.output).await;
    }
}
impl<C: Clock, W: Notes> Observer<'_, C, W> {
    pub fn channel_action_completed(
        &mut self,
        action: u8,
        timing: &EngineTiming,
        entries: [u8; 2],
    ) {
        let poll = u16::try_from(timing.maximum_micros);
        let atom = u16::try_from(timing.maximum_ready_micros);
        let invalid = poll.is_err() || atom.is_err() || timing.invalid;
        let packed = (u64::from(invalid) << 63)
            | (u64::from(action) << 32)
            | (u64::from(entries[0]) << 40)
            | (u64::from(entries[1]) << 48)
            | (u64::from(poll.unwrap_or(0)) << 16)
            | u64::from(atom.unwrap_or(0));
        // Preserve every completed action, including repeated tags and group positions.
        // Overflow invalidates the hold instead of silently losing an observation.
        if self.channels.push(packed).is_err() || invalid {
            self.engine.invalid = true;
        }
    }
    pub fn rx_gain_execution(&mut self, execution: RxGainExecution) {
        self.rx = Some(execution);
    }
    pub fn rfpll_engine_trace(&mut self) -> Option<&mut RfpllEngineTrace> {
        // Match the quiet hold method: per-command tracing perturbs RFPLL polls.
        // Aggregate engine timing remains enabled in both policy modes.
        None
    }
    pub fn power_gain_completed(&mut self, operation: Operation, timing: GainTiming) {
        self.gains[usize::from(operation == Operation::WifiPower)] = Some(timing);
    }
    pub fn readiness_trace(&mut self) -> Option<&mut ReadinessTrace> {
        self.trace.as_deref_mut()
    }
    #[inline(never)]
    pub fn engine_completed(&mut self, operation: Operation, timing: EngineTiming) {
        let counters = HoldPolls {
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
        };
        self.engines[operation_family(operation)].merge(counters);
        self.engine.merge(counters);
    }
    pub fn tx_sar_wait(&mut self, elapsed: u64, observations: u16, ready: bool) {
        self.tails.observe(elapsed, observations, ready);
    }
    pub fn tracking_operation(&mut self, operation: Operation, event: Event) {
        let operation = match operation {
            Operation::Tracking => HoldOperation::Tracking,
            Operation::GrantProtectRfpll => HoldOperation::Rfpll,
            Operation::GrantProtectCommon => HoldOperation::Common,
            Operation::GrantProtectTransmit => HoldOperation::Transmit,
            Operation::WifiBasebandOff => HoldOperation::WifiBasebandOff,
            _ => return,
        };
        let started = match event {
            Event::Started => true,
            Event::Completed | Event::Failed => false,
            _ => return,
        };
        if let Some(metrics) = self.metrics {
            metrics.borrow_mut().edge(operation as usize, started);
        }
        self.recorder.observe(
            operation,
            started,
            event == Event::Failed,
            self.clock.now().as_micros(),
        );
    }
}

/// Family formatting stays outside the observer and measured driver frames.
#[inline(never)]
async fn report_engine(operation: Operation, timing: &HoldPolls, output: &mut impl Notes) {
    let family = operation_family(operation) as u32;
    output
        .note(&Note::engine_polls(
            family,
            timing.polls.into(),
            timing.pending.into(),
            timing.polls.saturating_sub(timing.pending).into(),
        ))
        .await;
    output
        .note(&Note::engine_totals(
            family,
            timing.total_micros,
            timing.maximum_micros,
        ))
        .await;
    output
        .note(&Note::engine_resumes(
            family,
            timing.resumes.into(),
            timing.lateness_micros,
        ))
        .await;
    output
        .note(&Note::engine_lateness(
            family,
            timing.maximum_lateness_micros,
            timing.invalid,
        ))
        .await;
    if matches!(
        operation,
        Operation::WifiPower | Operation::BluetoothIeee802154Power
    ) {
        output
            .note(&Note::power_gain(
                family,
                timing.ready_edges.into(),
                timing.maximum_ready_micros,
            ))
            .await;
    }
    if operation == Operation::Dcode {
        output
            .note(&Note::dcode_atom(
                timing.ready_edges.into(),
                timing.maximum_ready_micros,
            ))
            .await;
    }
    if matches!(operation, Operation::RxGain | Operation::ChannelRestore) {
        output
            .note(&Note::calibration_atom(
                family,
                timing.ready_edges.into(),
                timing.maximum_ready_micros,
            ))
            .await;
    }
}

#[inline(never)]
async fn report_channel_action(packed: u64, output: &mut impl Notes) {
    output
        .note(&Note::channel_action(
            oer_esp32s31_phy::tracking::observation::calibration::name(true, (packed >> 32) as u8),
            packed,
        ))
        .await;
}

#[inline(never)]
async fn report_rx_gain(execution: &RxGainExecution, output: &mut impl Notes) {
    let [lower, hardware, commit] = execution.maximum_parts_micros;
    output
        .note(&Note::rx_gain_parts(lower, hardware, commit))
        .await;
    for (part, (action, micros)) in execution
        .maximum_parts_actions
        .into_iter()
        .zip(execution.maximum_parts_micros)
        .enumerate()
    {
        report_calibration_action(part as u8, action, micros, output).await;
    }
    for (part, (action, micros)) in execution
        .maximum_engine_actions
        .into_iter()
        .zip(execution.maximum_engine_micros)
        .enumerate()
    {
        report_calibration_action(3 + part as u8, action, micros, output).await;
    }
    report_rx_i2c_polls(execution, output).await;
}

#[inline(never)]
async fn report_rx_i2c_polls(execution: &RxGainExecution, output: &mut impl Notes) {
    for (stage, maxima) in execution.i2c_poll_micros.into_iter().enumerate() {
        for (repeat, micros) in maxima.into_iter().enumerate() {
            report_calibration_action(
                5 + stage as u8,
                11 | if repeat == 0 { 0x80 } else { 0 },
                micros,
                output,
            )
            .await;
        }
    }
    let samples = (u64::from(execution.i2c_samples[0]) << 32) | u64::from(execution.i2c_samples[1]);
    output.note(&Note::rx_i2c_samples(samples)).await;
}

#[inline(never)]
async fn report_calibration_action(part: u8, action: u8, micros: u32, output: &mut impl Notes) {
    output
        .note(&Note::calibration_name(
            part.into(),
            oer_esp32s31_phy::tracking::observation::calibration::name(false, action),
            action & 0x80 != 0,
            micros,
        ))
        .await;
}

fn operation_family(operation: Operation) -> usize {
    match operation {
        Operation::Rfpll => 1,
        Operation::BluetoothIeee802154Power => 2,
        Operation::WifiI2c => 3,
        Operation::WifiPower => 4,
        Operation::Dcode => 5,
        Operation::RxGain => 6,
        Operation::ChannelRestore => 8,
        Operation::Temperature => 7,
        _ => 0,
    }
}

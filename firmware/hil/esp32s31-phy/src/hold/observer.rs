//! Image-local observer: callbacks only record; UART is flushed after root return.
use super::{metrics::Metrics, recorder::Recorder, sar_tails::Tails};
use core::cell::RefCell;
use oer_esp32s31_phy::tracking::observation::{
    EngineTiming, Event, GainTiming, Operation, ReadinessTrace, RfpllEngineTrace,
};
use oer_hil_protocol::phy::{HoldOperation, HoldPolls};
use oer_time::Clock;

pub struct Observer<'a, C, W> {
    pub recorder: Recorder,
    pub tails: Tails,
    engine: &'a mut HoldPolls,
    trace: Option<&'a mut ReadinessTrace>,
    rfpll: RfpllEngineTrace,
    engines: [HoldPolls; 8],
    gains: [Option<GainTiming>; 2],
    clock: &'a C,
    output: &'a mut W,
    pub metrics: Option<&'a RefCell<Metrics>>,
}
impl<'a, C: Clock, W: core::fmt::Write> Observer<'a, C, W> {
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
            rfpll: RfpllEngineTrace::default(),
            engines: [HoldPolls::default(); 8],
            gains: [None; 2],
            clock,
            output,
            metrics: None,
        }
    }
    /// Called only after the root future returns, including failures.
    #[inline(never)]
    pub fn report(&mut self) {
        for operation in [
            Operation::Rfpll,
            Operation::BluetoothIeee802154Power,
            Operation::WifiI2c,
            Operation::WifiPower,
            Operation::Dcode,
            Operation::Temperature,
        ] {
            let timing = &self.engines[operation_family(operation)];
            if timing.polls == 0 {
                continue;
            }
            report_engine(operation, timing, self.output);
            if operation == Operation::Rfpll {
                report_rfpll_commands(&self.rfpll, self.output);
            }
        }
        for (index, timing) in self.gains.iter().enumerate() {
            if let Some(timing) = timing {
                print(
                    self.output,
                    format_args!(
                        "OPEN_RADIO_HIL gain-parts family={} lower-us={} arithmetic-us={} publication-us={}\n",
                        if index == 0 { 2 } else { 4 },
                        timing.lower_micros,
                        timing.arithmetic_micros,
                        timing.publication_micros
                    ),
                );
            }
        }
        print(
            self.output,
            format_args!("OPEN_RADIO_HIL sar_tail {:?}\n", self.tails),
        );
    }
}
impl<C: Clock, W: core::fmt::Write> Observer<'_, C, W> {
    pub fn rfpll_engine_trace(&mut self) -> Option<&mut RfpllEngineTrace> {
        self.rfpll = Default::default();
        Some(&mut self.rfpll)
    }
    pub fn power_gain_completed(&mut self, operation: Operation, timing: GainTiming) {
        self.gains[usize::from(operation == Operation::WifiPower)] = Some(timing);
    }
    pub fn readiness_trace(&mut self) -> Option<&mut ReadinessTrace> {
        self.trace.as_deref_mut()
    }
    #[inline(never)]
    pub fn engine_completed(&mut self, operation: Operation, mut timing: EngineTiming) {
        if operation == Operation::Rfpll {
            timing.invalid |= self.rfpll.invalid;
        }
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

fn print(output: &mut impl core::fmt::Write, args: core::fmt::Arguments<'_>) {
    output.write_fmt(args).expect("diagnostic UART output");
}
/// Family formatting stays outside the observer and measured driver frames.
#[inline(never)]
fn report_engine(operation: Operation, timing: &HoldPolls, output: &mut impl core::fmt::Write) {
    let family = operation_family(operation);
    print(
        output,
        format_args!(
            "OPEN_RADIO_HIL engine family={} polls={} pending={} ready={}\n",
            family,
            timing.polls,
            timing.pending,
            timing.polls.saturating_sub(timing.pending)
        ),
    );
    print(
        output,
        format_args!(
            "OPEN_RADIO_HIL engine family={} total-us={} max-us={}\n",
            family, timing.total_micros, timing.maximum_micros
        ),
    );
    print(
        output,
        format_args!(
            "OPEN_RADIO_HIL engine family={} resumes={} late-total-us={}\n",
            family, timing.resumes, timing.lateness_micros
        ),
    );
    print(
        output,
        format_args!(
            "OPEN_RADIO_HIL engine family={} late-max-us={} invalid={}\n",
            family, timing.maximum_lateness_micros, timing.invalid
        ),
    );
    if matches!(
        operation,
        Operation::WifiPower | Operation::BluetoothIeee802154Power
    ) {
        print(
            output,
            format_args!(
                "OPEN_RADIO_HIL power-gain family={} count={} max-us={}\n",
                family, timing.ready_edges, timing.maximum_ready_micros
            ),
        );
    }
    if operation == Operation::Dcode {
        print(
            output,
            format_args!(
                "OPEN_RADIO_HIL dcode-atom count={} max-us={}\n",
                timing.ready_edges, timing.maximum_ready_micros
            ),
        );
    }
}

/// Lossless UART quartet: invalid bit 63, command bits 48..62, then
/// poll/publication/readiness as three u16 values. One formatter argument
/// keeps the diagnostic frame/code inside the same application budgets.
#[inline(never)]
fn report_rfpll_commands(trace: &RfpllEngineTrace, output: &mut impl core::fmt::Write) {
    for (command, row) in trace.commands.iter().enumerate() {
        report_rfpll_command(command, row, trace.invalid, output);
    }
}

#[inline(never)]
fn report_rfpll_command(
    command: usize,
    row: &oer_esp32s31_phy::tracking::observation::RfpllCommandMaximum,
    invalid: bool,
    output: &mut impl core::fmt::Write,
) {
    let packed = (u64::from(invalid) << 63)
        | ((command as u64) << 48)
        | (u64::from(row.poll_micros) << 32)
        | (u64::from(row.publication_micros) << 16)
        | u64::from(row.readiness_micros);
    print(
        output,
        format_args!("OPEN_RADIO_HIL rfpll-command-packed={}\n", packed),
    );
}

fn operation_family(operation: Operation) -> usize {
    match operation {
        Operation::Rfpll => 1,
        Operation::BluetoothIeee802154Power => 2,
        Operation::WifiI2c => 3,
        Operation::WifiPower => 4,
        Operation::Dcode => 5,
        Operation::Temperature => 7,
        _ => 0,
    }
}

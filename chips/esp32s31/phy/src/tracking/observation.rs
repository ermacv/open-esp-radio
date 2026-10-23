//! Bounded timing observations of selected tracking operations.
//!
//! Timings include waits and nested children: parent and child totals must not
//! be summed as exclusive CPU or RF time. Completion means the operation's
//! completion was accepted, not that RF quality or protocol restoration passed.
//! An abandoned future leaves a started operation without a terminal event.

use crate::executor::wait;
pub mod calibration;

/// One not-ready status observation, timed from its command publication.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NotReadySample {
    pub command: u8,
    pub elapsed_micros: u8,
}

/// Bounded preparation evidence, borrowed only by the diagnostic caller.
/// Overflow is explicit: `complete()` is false if any sample did not fit.
/// This does not consume a late result or change its deadline.
pub struct ReadinessTrace {
    command_at: oer_time::Instant,
    not_ready: [NotReadySample; 16],
    pub maximum_first_check_micros: u32,
    pub failure_elapsed_micros: u32,
    pub failure_lateness_micros: u32,
    pub yield_total_micros: u32,
    pub yield_maximum_micros: u16,
    pub yields: u16,
    pub commands: u8,
    not_ready_count: u8,
    /// Both host busy bits, sampled without reading the result at failure.
    pub failure_busy: Option<u8>,
}
impl Default for ReadinessTrace {
    fn default() -> Self {
        Self {
            command_at: oer_time::Instant::EPOCH,
            not_ready: [NotReadySample::default(); 16],
            maximum_first_check_micros: 0,
            failure_elapsed_micros: 0,
            failure_lateness_micros: 0,
            yield_total_micros: 0,
            yield_maximum_micros: 0,
            yields: 0,
            commands: 0,
            not_ready_count: 0,
            failure_busy: None,
        }
    }
}
impl ReadinessTrace {
    pub fn not_ready_samples(&self) -> &[NotReadySample] {
        &self.not_ready[..usize::from(self.not_ready_count).min(self.not_ready.len())]
    }
    pub fn complete(&self) -> bool {
        self.commands < u8::MAX && usize::from(self.not_ready_count) <= self.not_ready.len()
    }
    #[cfg(any(target_arch = "riscv32", test))]
    #[inline(never)]
    pub(crate) fn yield_resumed(&mut self, elapsed: u64) {
        if let (Ok(elapsed), Some(count), Some(total)) = (
            u16::try_from(elapsed),
            self.yields.checked_add(1),
            u32::try_from(elapsed)
                .ok()
                .and_then(|elapsed| self.yield_total_micros.checked_add(elapsed)),
        ) {
            self.yields = count;
            self.yield_total_micros = total;
            self.yield_maximum_micros = self.yield_maximum_micros.max(elapsed);
        } else {
            self.not_ready_count = u8::MAX;
        }
    }
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn command_started(&mut self, at: oer_time::Instant) {
        self.commands = self.commands.saturating_add(1);
        self.command_at = at;
    }
    #[cfg(any(target_arch = "riscv32", test))]
    #[inline(never)]
    pub(crate) fn checked(
        &mut self,
        at: oer_time::Instant,
        first: bool,
        not_ready: bool,
        failure: Option<(u8, u64)>,
    ) {
        let Some(elapsed) = at
            .checked_duration_since(self.command_at)
            .and_then(|d| u32::try_from(d.as_micros()).ok())
        else {
            self.not_ready_count = u8::MAX;
            return;
        };
        if first {
            self.maximum_first_check_micros = self.maximum_first_check_micros.max(elapsed);
        }
        if not_ready {
            if let (Some(slot), Ok(elapsed_micros)) = (
                self.not_ready.get_mut(usize::from(self.not_ready_count)),
                u8::try_from(elapsed),
            ) {
                *slot = NotReadySample {
                    command: self.commands,
                    elapsed_micros,
                };
            } else {
                self.not_ready_count = u8::MAX;
            }
            self.not_ready_count = self.not_ready_count.saturating_add(1);
        }
        if let Some((busy, late)) = failure {
            self.failure_elapsed_micros = elapsed;
            self.failure_lateness_micros = u32::try_from(late).unwrap_or(u32::MAX);
            self.failure_busy = Some(busy);
        }
    }
}

/// RFPLL helper command whose complete publication/readiness fits one poll.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RfpllCommand {
    InitialLow,
    InitialHigh,
    EnableRead,
    EnableWrite,
    CapLow,
    CapHighRead,
    CapHighWrite,
    Status,
    Memory,
    Restore,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RfpllCommandSample {
    pub command: RfpllCommand,
    pub publication_micros: u16,
    pub readiness_micros: u16,
    pub invalid: bool,
}
impl RfpllCommandSample {
    #[inline(never)]
    pub(crate) fn intervals(
        command: RfpllCommand,
        started: oer_time::Instant,
        published: oer_time::Instant,
        finished: oer_time::Instant,
    ) -> Self {
        let publication = published
            .checked_duration_since(started)
            .and_then(|d| u16::try_from(d.as_micros()).ok());
        let readiness = finished
            .checked_duration_since(published)
            .and_then(|d| u16::try_from(d.as_micros()).ok());
        Self {
            command,
            publication_micros: publication.unwrap_or(0),
            readiness_micros: readiness.unwrap_or(0),
            invalid: publication.is_none() || readiness.is_none(),
        }
    }
    pub(crate) fn elapsed<C: oer_time::Clock>(
        command: RfpllCommand,
        started: Option<oer_time::Instant>,
        clock: &C,
    ) -> Self {
        if let Some(at) = started {
            let finished = clock.now();
            Self::intervals(command, at, finished, finished)
        } else {
            Self {
                command,
                publication_micros: 0,
                readiness_micros: 0,
                invalid: false,
            }
        }
    }
}

/// Correlated breakdown of the worst poll for one RFPLL command. The residual
/// poll minus publication/readiness is engine/model/observer overhead.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RfpllCommandMaximum {
    pub poll_micros: u16,
    pub publication_micros: u16,
    pub readiness_micros: u16,
}
/// Observer-owned bounded diagnostics; the admitted future borrows this store.
#[derive(Debug, Default)]
pub struct RfpllEngineTrace {
    pub commands: [RfpllCommandMaximum; 10],
    pub invalid: bool,
}
impl RfpllEngineTrace {
    fn prepare(&mut self, sample: RfpllCommandSample) -> &mut RfpllCommandMaximum {
        self.invalid |= sample.invalid;
        &mut self.commands[sample.command as usize]
    }
}
impl RfpllCommandMaximum {
    fn record(&mut self, sample: RfpllCommandSample, elapsed: u16) {
        if elapsed >= self.poll_micros {
            *self = Self {
                poll_micros: elapsed,
                publication_micros: sample.publication_micros,
                readiness_micros: sample.readiness_micros,
            };
        }
    }
}

/// Maximum cost of gain setup, one record calculation and one publication.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GainTiming {
    pub lower_micros: u16,
    pub arithmetic_micros: u16,
    pub publication_micros: u16,
}

/// Clock-free aggregate of actual engine polls and timer resumes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EngineTiming {
    pub polls: u16,
    pub pending: u16,
    pub total_micros: u32,
    pub maximum_micros: u32,
    pub resumes: u16,
    pub lateness_micros: u32,
    pub maximum_lateness_micros: u32,
    pub ready_edges: u16,
    pub maximum_ready_micros: u32,
    pub invalid: bool,
}
impl EngineTiming {
    /// Update the bounded event counters before taking the final poll stamp.
    /// Only the final duration sum/max stores follow that stamp.
    #[inline(never)]
    pub(crate) fn poll_finished<C: oer_time::Clock>(
        &mut self,
        clock: &C,
        started: oer_time::Instant,
        pending: bool,
        resumed: Option<(oer_time::Instant, oer_time::Instant)>,
        ready_elapsed: Option<oer_time::Duration>,
        rfpll: Option<(&mut RfpllEngineTrace, RfpllCommandSample)>,
    ) -> oer_time::Instant {
        fn increment(total: &mut u16, invalid: &mut bool) {
            if let Some(next) = total.checked_add(1) {
                *total = next;
            } else {
                *invalid = true;
            }
        }
        fn add(total: &mut u32, amount: u32, invalid: &mut bool) {
            if let Some(next) = total.checked_add(amount) {
                *total = next;
            } else {
                *invalid = true;
            }
        }
        increment(&mut self.polls, &mut self.invalid);
        if pending {
            increment(&mut self.pending, &mut self.invalid);
        }
        if let Some((deadline, at)) = resumed {
            if let Some(late) = at
                .checked_duration_since(deadline)
                .and_then(|d| u32::try_from(d.as_micros()).ok())
            {
                increment(&mut self.resumes, &mut self.invalid);
                add(&mut self.lateness_micros, late, &mut self.invalid);
                self.maximum_lateness_micros = self.maximum_lateness_micros.max(late);
            } else {
                self.invalid = true;
            }
        }
        if let Some(elapsed) = ready_elapsed {
            if let Ok(elapsed) = u32::try_from(elapsed.as_micros()) {
                increment(&mut self.ready_edges, &mut self.invalid);
                self.maximum_ready_micros = self.maximum_ready_micros.max(elapsed);
            } else {
                self.invalid = true;
            }
        }
        let command = rfpll.map(|(trace, sample)| (trace.prepare(sample), sample));
        // Includes the event/overflow/ready/command accounting above. No observer,
        // formatter or report runs inside this measured interval.
        let finished = clock.now();
        if let Some(elapsed) = finished
            .checked_duration_since(started)
            .and_then(|d| u32::try_from(d.as_micros()).ok())
        {
            add(&mut self.total_micros, elapsed, &mut self.invalid);
            self.maximum_micros = self.maximum_micros.max(elapsed);
            if let Some((row, sample)) = command {
                if let Ok(elapsed) = u16::try_from(elapsed) {
                    row.record(sample, elapsed);
                } else {
                    self.invalid = true;
                }
            }
        } else {
            self.invalid = true;
        }
        finished
    }
}

use super::{
    calibration::PhyCalibrationTrackingAction as Calibration,
    parameters::PhyParamTrackingAction as Parameter,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Operation {
    Rfpll,
    WifiPower,
    BluetoothIeee802154Power,
    WifiI2c,
    Calibration,
    Temperature,
    PbusClear,
    Dcode,
    RxGain,
    ChannelRestore,
    ForceTxRx,
    TxDcPwdet,
    TxGainPublication,
    FrequencySettle,
    RxGainDcPhase,
    RxGainPublishPhase,
    RxGainControlPhase,
    /// Entire parameter executor, including critical entry/exit. All other
    /// operation intervals are children, not additive execution time.
    Tracking,
    /// The PHY grant-protect request (coexistence event 48 on timer 5) held
    /// around the RFPLL capacitor correction, from its set to its release.
    /// It is the interval the request is programmed, not a proof that the
    /// arbiter excluded every other radio from the air.
    GrantProtectRfpll,
    /// The grant-protect request held around the common branch (DCODE and
    /// RX gain calibration).
    GrantProtectCommon,
    /// The grant-protect request held around the transmit branch (TXDC/PWDET
    /// of each requested class and the gain publication).
    GrantProtectTransmit,
    /// The Wi-Fi baseband held off by a calibration branch, from
    /// `DisableWifiBaseband` to `EnableMacBaseband`.
    WifiBasebandOff,
}

pub const OPERATION_COUNT: usize = 22;
const POLLED_COUNT: usize = 3;

impl Operation {
    /// Coarse phase boundaries independent of intrusive per-edge timing.
    ///
    /// Minimum searches and sampled inner/outer edges belong exclusively to
    /// the explicit RX-step profile. Even a start/terminal timestamp pair per
    /// minimum materially perturbs the calibration path it is meant to time.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) const fn is_rx_gain_region(self) -> bool {
        matches!(
            self,
            Self::RxGainDcPhase | Self::RxGainPublishPhase | Self::RxGainControlPhase
        )
    }

    const fn poll_index(self) -> Option<usize> {
        match self {
            Self::Dcode => Some(0),
            Self::RxGain => Some(1),
            Self::TxDcPwdet => Some(2),
            _ => None,
        }
    }

    pub(crate) fn parameter(action: Parameter) -> Option<Self> {
        Some(match action {
            Parameter::RfpllCapTrack { .. } => Self::Rfpll,
            Parameter::WifiTxPowerTrack { .. } => Self::WifiPower,
            Parameter::BluetoothIeee802154TxPowerTrack { .. } => Self::BluetoothIeee802154Power,
            Parameter::WifiI2cTrack => Self::WifiI2c,
            Parameter::CalibrationTrack { .. } => Self::Calibration,
            Parameter::TemperatureRead => Self::Temperature,
            Parameter::EnterCritical | Parameter::ExitCritical | Parameter::Complete(_) => {
                return None;
            }
        })
    }

    pub(crate) fn calibration(action: Calibration) -> Option<Self> {
        Some(match action {
            Calibration::ClearPbus => Self::PbusClear,
            Calibration::CalibrateDcode => Self::Dcode,
            Calibration::RecalibrateRxGain => Self::RxGain,
            Calibration::RestoreChipChannel { .. } => Self::ChannelRestore,
            Calibration::ForceTxRxOff { .. } => Self::ForceTxRx,
            Calibration::CalibrateTxDcPwdet { .. } => Self::TxDcPwdet,
            Calibration::PublishWifiTxGain { .. }
            | Calibration::PublishBluetoothIeee802154TxGain => Self::TxGainPublication,
            Calibration::AwaitSoftwareFrequencySettle => Self::FrequencySettle,
            Calibration::SetHardwareFrequencyControl { .. }
            | Calibration::SetForcedDigitalGain { .. }
            | Calibration::ConfigureBasebandChannel { .. }
            | Calibration::DisableWifiBaseband
            | Calibration::EnableMacBaseband
            | Calibration::RestoreTxGainCompensation
            | Calibration::SetGrantProtect { .. }
            | Calibration::Complete(_)
            | Calibration::Failed(_) => return None,
        })
    }
}

/// The RF-hold interval edge a calibration action opens or closes: the
/// grant-protect request held around `branch`, or the Wi-Fi baseband held
/// off. A terminal transition (no branch) programs no request.
pub(crate) const fn calibration_hold_edge(
    action: Calibration,
    branch: Option<super::calibration::PhyCalibrationTrackingBranch>,
) -> Option<(Operation, Event)> {
    use super::calibration::PhyCalibrationTrackingBranch as Branch;
    match action {
        Calibration::SetGrantProtect { enabled } => {
            let operation = match branch {
                Some(Branch::Common) => Operation::GrantProtectCommon,
                Some(Branch::Transmit) => Operation::GrantProtectTransmit,
                None => return None,
            };
            Some((operation, grant_protect_edge(enabled)))
        }
        Calibration::DisableWifiBaseband => Some((Operation::WifiBasebandOff, Event::Started)),
        Calibration::EnableMacBaseband => Some((Operation::WifiBasebandOff, Event::Completed)),
        _ => None,
    }
}

/// The grant-protect edge of an RFPLL thermal correction action.
pub(crate) const fn rfpll_hold_edge(
    action: super::rfpll::thermal::Action,
) -> Option<(Operation, Event)> {
    match action {
        super::rfpll::thermal::Action::SetGrantProtect { enabled } => {
            Some((Operation::GrantProtectRfpll, grant_protect_edge(enabled)))
        }
        _ => None,
    }
}

/// Setting the request opens its interval; releasing it closes it.
const fn grant_protect_edge(enabled: bool) -> Event {
    if enabled {
        Event::Started
    } else {
        Event::Completed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Started,
    Completed,
    Failed,
    PollStarted,
    PollPending,
    PollReady,
}

/// Wall time inside child future polls, including interrupts and observation
/// overhead. This is not a CPU cycle counter or exclusive execution time.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PollTiming {
    /// Polls returning Pending; completed calls have one final Ready each.
    pub pending: u32,
    /// Wall time from Pending return to the next poll entry. Includes timer
    /// waits and scheduling latency; neither readiness nor CPU idle is known.
    pub suspended_micros: u32,
    pub maximum_suspension_micros: u32,
    pub polls: u32,
    pub elapsed_micros: u32,
    pub maximum_micros: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Timing {
    pub started: u16,
    pub completed: u16,
    pub failed: u16,
    /// Sum of terminal attempts, including failures, in microseconds.
    pub elapsed_micros: u32,
    pub maximum_micros: u32,
}

/// Clock-free execution counts published once after an RX-gain child.
///
/// These counters describe the variable hardware search path without placing
/// timestamp callbacks in its hot loop. `minimum_operations` counts estimator
/// actions across every minimum search; `outer_operations` counts the remaining
/// RX-gain root actions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxGainExecution {
    /// Per-atom maximum: cursor/descriptor, hardware transaction, commit (microseconds).
    pub maximum_parts_micros: [u32; 3],
    /// Same action/first-entry tags as the engine maxima, correlated per part.
    pub maximum_parts_actions: [u8; 3],
    pub maximum_engine_micros: [u32; 2],
    pub maximum_engine_actions: [u8; 2],
    /// Poll maxima by I2C stage (separate preparation, complete helper+commit),
    /// then first/repeated action within the same retained RX phase.
    /// The retained program folds preparation into its descriptor step, so
    /// there is no separate preparation poll to record in the first row.
    pub i2c_poll_micros: [[u32; 2]; 2],
    /// Completed helper polls, first/repeated within the RX phase.
    pub i2c_samples: [u32; 2],
    /// Complete RX DC product quality; absent when that child did not complete.
    pub quality: Option<crate::rx::gain_calibration::PhyRxGainDcQuality>,
    pub minimum_searches: u32,
    pub minimum_operations: u32,
    pub outer_operations: u32,
    pub settle_1us: u32,
    pub settle_2us: u32,
    pub settle_10us: u32,
}

impl RxGainExecution {
    /// Associate an engine maximum with its retained action after the poll stamp.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn observe_engine(&mut self, timing: &EngineTiming, action: u8) {
        for (index, elapsed) in [timing.maximum_micros, timing.maximum_ready_micros]
            .into_iter()
            .enumerate()
        {
            if elapsed > self.maximum_engine_micros[index] {
                self.maximum_engine_micros[index] = elapsed;
                self.maximum_engine_actions[index] = action;
            }
        }
    }
}

impl EngineTiming {
    /// Merge completed action records outside the measured driver.
    #[cfg(any(target_arch = "riscv32", test))]
    pub(crate) fn merge(&mut self, other: &Self) {
        self.invalid |= other.invalid;
        macro_rules! add {
            ($($field:ident),*) => { $(
                if let Some(sum) = self.$field.checked_add(other.$field) { self.$field = sum; }
                else { self.invalid = true; }
            )* };
        }
        add!(
            polls,
            pending,
            total_micros,
            resumes,
            lateness_micros,
            ready_edges
        );
        self.maximum_micros = self.maximum_micros.max(other.maximum_micros);
        self.maximum_lateness_micros = self
            .maximum_lateness_micros
            .max(other.maximum_lateness_micros);
        self.maximum_ready_micros = self.maximum_ready_micros.max(other.maximum_ready_micros);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Report {
    pub engine: EngineTiming,
    pub rfpll: Option<super::rfpll::Observation>,
    pub temperature: Option<crate::analog::temperature::PhyTemperatureOutcome>,
    pub timings: [Timing; OPERATION_COUNT],
    pub polls: [PollTiming; POLLED_COUNT],
    pub rx_gain_execution: Option<RxGainExecution>,
    /// Clock reversal, overflow or malformed observations invalidate timing.
    pub invalid: bool,
    pub dcode_waits: wait::Report,
    pub tx_waits: wait::tx::Report,
}

impl Default for Report {
    fn default() -> Self {
        Self {
            engine: EngineTiming::default(),
            rfpll: None,
            temperature: None,
            timings: [Timing::default(); OPERATION_COUNT],
            polls: [PollTiming::default(); POLLED_COUNT],
            rx_gain_execution: None,
            invalid: false,
            dcode_waits: Default::default(),
            tx_waits: Default::default(),
        }
    }
}

impl Report {
    pub const fn timing(&self, operation: Operation) -> Timing {
        self.timings[operation as usize]
    }

    pub const fn poll_timing(&self, operation: Operation) -> Option<PollTiming> {
        match operation.poll_index() {
            Some(index) => Some(self.polls[index]),
            None => None,
        }
    }
}

/// One serialized tracking invocation; different operation kinds may nest.
/// The caller owns the monotonic clock and decides where this storage lives.
pub struct Recorder {
    engine: EngineTiming,
    rfpll: Option<super::rfpll::Observation>,
    temperature: Option<crate::analog::temperature::PhyTemperatureOutcome>,
    timings: [Timing; OPERATION_COUNT],
    polls: [PollTiming; POLLED_COUNT],
    rx_gain_execution: Option<RxGainExecution>,
    invalid: bool,
    active: [Option<u64>; OPERATION_COUNT],
    active_poll: [Option<u64>; POLLED_COUNT],
    pending_since: [Option<u64>; POLLED_COUNT],
    poll_ready: [bool; POLLED_COUNT],
    last_observed: Option<u64>,
    wait: wait::Recorder,
    tx_wait: wait::tx::Recorder,
}

impl Default for Recorder {
    fn default() -> Self {
        Self {
            engine: EngineTiming::default(),
            rfpll: None,
            temperature: None,
            timings: [Timing::default(); OPERATION_COUNT],
            polls: [PollTiming::default(); POLLED_COUNT],
            rx_gain_execution: None,
            invalid: false,
            active: [None; OPERATION_COUNT],
            active_poll: [None; POLLED_COUNT],
            pending_since: [None; POLLED_COUNT],
            poll_ready: [false; POLLED_COUNT],
            last_observed: None,
            wait: Default::default(),
            tx_wait: Default::default(),
        }
    }
}

impl Recorder {
    pub fn observe_engine(&mut self, timing: EngineTiming) {
        self.engine = timing;
    }
    pub fn observe_temperature(
        &mut self,
        outcome: crate::analog::temperature::PhyTemperatureOutcome,
    ) {
        if self.active[Operation::Temperature as usize].is_none()
            || self.temperature.replace(outcome).is_some()
        {
            self.invalid = true;
        }
    }

    pub fn observe_rx_gain_execution(&mut self, execution: RxGainExecution) {
        if self.active[Operation::RxGain as usize].is_none() || self.rx_gain_execution.is_some() {
            self.invalid = true;
            return;
        }
        self.rx_gain_execution = Some(execution);
    }

    pub fn observe_rfpll(&mut self, observation: super::rfpll::Observation) {
        if self.active[Operation::Rfpll as usize].is_none() || self.rfpll.is_some() {
            self.invalid = true;
            return;
        }
        self.rfpll = Some(observation);
    }

    pub fn observe(&mut self, operation: Operation, event: Event, now_micros: u64) {
        if self.last_observed.is_some_and(|last| now_micros < last) {
            self.invalid = true;
            return;
        }
        self.last_observed = Some(now_micros);
        if matches!(
            event,
            Event::PollStarted | Event::PollPending | Event::PollReady
        ) {
            self.observe_poll(operation, event, now_micros);
            return;
        }
        let index = operation as usize;
        let timing = &mut self.timings[index];
        if event == Event::Started {
            if self.active[index].is_some() {
                self.invalid = true;
                return;
            }
            self.active[index] = Some(now_micros);
            if let Some(poll) = operation.poll_index() {
                self.poll_ready[poll] = false;
            }
            let (count, overflow) = timing.started.overflowing_add(1);
            self.invalid |= overflow;
            timing.started = if overflow { u16::MAX } else { count };
            return;
        }
        let Some(started) = self.active[index].take() else {
            self.invalid = true;
            return;
        };
        if let Some(index) = operation.poll_index() {
            self.invalid |=
                self.active_poll[index].is_some() || self.pending_since[index].is_some();
        }
        let count = if event == Event::Completed {
            &mut timing.completed
        } else {
            &mut timing.failed
        };
        let (next, overflow) = count.overflowing_add(1);
        self.invalid |= overflow;
        *count = if overflow { u16::MAX } else { next };
        let Some(elapsed) = now_micros
            .checked_sub(started)
            .and_then(|value| u32::try_from(value).ok())
        else {
            self.invalid = true;
            return;
        };
        let (total, overflow) = timing.elapsed_micros.overflowing_add(elapsed);
        self.invalid |= overflow;
        timing.elapsed_micros = if overflow { u32::MAX } else { total };
        timing.maximum_micros = timing.maximum_micros.max(elapsed);
    }

    fn observe_poll(&mut self, operation: Operation, event: Event, now_micros: u64) {
        let Some(index) = operation.poll_index() else {
            self.invalid = true;
            return;
        };
        let Some(operation_start) = self.active[operation as usize] else {
            self.invalid = true;
            return;
        };
        if now_micros < operation_start {
            self.invalid = true;
            return;
        }
        if event == Event::PollStarted {
            if self.active_poll[index].is_some() || self.poll_ready[index] {
                self.invalid = true;
                return;
            }
            if let Some(pending) = self.pending_since[index].take() {
                let timing = &mut self.polls[index];
                let Some(gap) = now_micros
                    .checked_sub(pending)
                    .and_then(|gap| u32::try_from(gap).ok())
                else {
                    self.invalid = true;
                    return;
                };
                let Some(total) = timing.suspended_micros.checked_add(gap) else {
                    self.invalid = true;
                    return;
                };
                timing.suspended_micros = total;
                timing.maximum_suspension_micros = timing.maximum_suspension_micros.max(gap);
            }
            self.active_poll[index] = Some(now_micros);
            return;
        }
        let Some(started) = self.active_poll[index].take() else {
            self.invalid = true;
            return;
        };
        let Some(elapsed) = now_micros
            .checked_sub(started)
            .and_then(|value| u32::try_from(value).ok())
        else {
            self.invalid = true;
            return;
        };
        let timing = &mut self.polls[index];
        let Some(polls) = timing.polls.checked_add(1) else {
            self.invalid = true;
            return;
        };
        let Some(total) = timing.elapsed_micros.checked_add(elapsed) else {
            self.invalid = true;
            return;
        };
        if event == Event::PollPending {
            let Some(pending) = timing.pending.checked_add(1) else {
                self.invalid = true;
                return;
            };
            timing.pending = pending;
            self.pending_since[index] = Some(now_micros);
        } else {
            self.poll_ready[index] = true;
        }
        timing.polls = polls;
        timing.elapsed_micros = total;
        timing.maximum_micros = timing.maximum_micros.max(elapsed);
    }

    pub fn observe_tx_wait(
        &mut self,
        scope: wait::tx::Scope,
        kind: wait::Kind,
        event: wait::Event,
    ) {
        if self.active[Operation::TxDcPwdet as usize].is_none() {
            self.invalid = true;
            return;
        }
        self.tx_wait.observe(scope, kind, event);
    }

    pub fn observe_tx_sar_ready(&mut self, ready: bool) {
        if self.active[Operation::TxDcPwdet as usize].is_none() {
            self.invalid = true;
            return;
        }
        self.tx_wait.sar_ready(ready);
    }

    pub fn observe_dcode_wait(&mut self, scope: wait::Scope, kind: wait::Kind, event: wait::Event) {
        if self.active[Operation::Dcode as usize].is_none() {
            self.invalid = true;
            return;
        }
        self.wait.observe(scope, kind, event);
    }

    pub fn observe_dcode_pll_lock(&mut self, locked: bool) {
        if self.active[Operation::Dcode as usize].is_none() {
            self.invalid = true;
            return;
        }
        self.wait.pll_lock(locked);
    }

    pub fn report(&self) -> Report {
        Report {
            rfpll: self.rfpll,
            temperature: self.temperature,
            engine: self.engine,
            timings: self.timings,
            polls: self.polls,
            rx_gain_execution: self.rx_gain_execution,
            dcode_waits: self.wait.report(),
            tx_waits: self.tx_wait.report(),
            invalid: self.invalid
                || self.engine.invalid
                || !self.wait.is_complete()
                || !self.tx_wait.is_complete(),
        }
    }
}

/// Observe existing polls without scheduling another poll or changing a waker.
#[cfg(test)]
pub(crate) fn observe_polls<F: core::future::Future, O: FnMut(Event)>(
    future: F,
    observe: O,
) -> Observed<F, O> {
    Observed { future, observe }
}

#[cfg(test)]
pin_project_lite::pin_project! {
    /// Store the child directly, without another async state machine or a
    /// separately pinned local. Projection preserves its pinning and drop.
    pub(crate) struct Observed<F, O> {
        #[pin]
        future: F,
        observe: O,
    }
}

#[cfg(test)]
impl<F: core::future::Future, O: FnMut(Event)> core::future::Future for Observed<F, O> {
    type Output = F::Output;

    #[inline(never)]
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        type PollFn<F> = for<'future, 'context, 'wake> fn(
            core::pin::Pin<&'future mut F>,
            &'context mut core::task::Context<'wake>,
        ) -> core::task::Poll<
            <F as core::future::Future>::Output,
        >;
        let this = self.project();
        (this.observe)(Event::PollStarted);
        // Keep the concrete child poll out of this observer's frame under LTO.
        let poll: PollFn<F> = F::poll;
        let result = core::hint::black_box(poll)(this.future, cx);
        (this.observe)(if result.is_pending() {
            Event::PollPending
        } else {
            Event::PollReady
        });
        result
    }
}

#[cfg(test)]
mod tests;

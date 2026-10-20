//! Bounded timing observations of selected tracking operations.
//!
//! Timings include waits and nested children: parent and child totals must not
//! be summed as exclusive CPU or RF time. Completion means the operation's
//! completion was accepted, not that RF quality or protocol restoration passed.
//! An abandoned future leaves a started operation without a terminal event.

use crate::executor::wait;

/// Actual engine poll boundaries and requested timer deadlines. These events
/// carry no UART work and borrow no hardware state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineEvent {
    Poll {
        started: u64,
        finished: u64,
        ready: bool,
    },
    TimerResumed {
        deadline: u64,
        resumed: u64,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EngineTiming {
    pub polls: u32,
    pub pending: u32,
    pub total_micros: u32,
    pub maximum_micros: u32,
    pub resumes: u32,
    pub lateness_micros: u32,
    pub maximum_lateness_micros: u32,
    pub invalid: bool,
}

impl EngineTiming {
    pub fn observe(&mut self, event: EngineEvent) {
        let (start, end) = match event {
            EngineEvent::Poll {
                started, finished, ..
            } => (started, finished),
            EngineEvent::TimerResumed { deadline, resumed } => (deadline, resumed),
        };
        let Some(elapsed) = end.checked_sub(start).and_then(|v| u32::try_from(v).ok()) else {
            self.invalid = true;
            return;
        };
        let (count, total, maximum) = match event {
            EngineEvent::Poll { ready, .. } => {
                if !ready {
                    let Some(pending) = self.pending.checked_add(1) else {
                        self.invalid = true;
                        return;
                    };
                    self.pending = pending;
                }
                (
                    &mut self.polls,
                    &mut self.total_micros,
                    &mut self.maximum_micros,
                )
            }
            EngineEvent::TimerResumed { .. } => (
                &mut self.resumes,
                &mut self.lateness_micros,
                &mut self.maximum_lateness_micros,
            ),
        };
        let (Some(next_count), Some(next_total)) =
            (count.checked_add(1), total.checked_add(elapsed))
        else {
            self.invalid = true;
            return;
        };
        *count = next_count;
        *total = next_total;
        *maximum = (*maximum).max(elapsed);
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
    /// Complete RX DC product quality; absent when that child did not complete.
    pub quality: Option<crate::rx::gain_calibration::PhyRxGainDcQuality>,
    pub minimum_searches: u32,
    pub minimum_operations: u32,
    pub outer_operations: u32,
    pub settle_1us: u32,
    pub settle_2us: u32,
    pub settle_10us: u32,
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
    pub fn observe_engine(&mut self, event: EngineEvent) {
        self.engine.observe(event);
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

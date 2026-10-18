//! Execution of the station power manager inside the connected control core.
//!
//! [`ModemSleep`] decides; this module performs its effects. Station TBTT,
//! TX-queue gating and beacon receive programming are MAC registers the
//! control core owns and are written at once. Coexistence requests, RF
//! sleep and wake and the release of held network frames belong to owners
//! outside the core; they are queued as [`ConnectedPowerCommand`]s for the
//! runtime, which drains them in order after each control step.

use oer_esp32s31_hal::types::{MacPti, StaTbttSchedule};

use oer_ieee80211_mac::station_power_save::StaPowerManagement;

use oer_esp32s31_ieee80211::datapath::{DatapathControlContext, DatapathControlProgress};

use oer_ieee80211_trace::{NetworkTxPowerTrace, PowerInputKind, PowerInputTrace};

use crate::{
    hardware::control::ConnectedControlHardware,
    modem_sleep::{
        CoexPhaseView, CoexView, ModemSleep, PmAction, PmActions, PmBeacon, PmClock, PmCoexAction,
        PmState, PmTimer, PmTraffic, SleepType,
    },
};

use oer_ieee80211_mac::tsf::{TsfDuration, TsfInstant};

use super::{
    ConnectedControlCore, ConnectedControlError, ConnectedControlTx, ConnectedDisconnectReason,
    ControlInFlight,
};

/// Capacity of the queue of power commands waiting for the runtime.
pub const POWER_COMMAND_CAPACITY: usize = 16;

/// A power-management effect performed outside the control core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
// CAPABILITY: coex-protocol-integration-and-lifetime-live-wifi-requests
pub enum ConnectedPowerCommand {
    /// A coexistence effect, which the shared radio system performs.
    Coex(PmCoexAction),
    /// Put the station's RF to sleep, keeping its registration.
    RfSleep,
    /// Wake the station's RF.
    RfWake,
}

/// What the TX owner did with network data frames since control last read
/// it: `pm_tx_data_process` and `pm_tx_data_done_process` of the vendor.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NetworkTxPowerReport {
    /// A network data transaction started.
    pub started: bool,
    /// A network data transaction ended; `true` when the peer acknowledged
    /// at least one of its frames.
    pub completed: Option<bool>,
}

impl NetworkTxPowerReport {
    pub const fn is_empty(self) -> bool {
        !self.started && self.completed.is_none()
    }
}

/// Coexistence as the runtime reads it before a control step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerCoexSnapshot {
    pub view: CoexView,
    /// The receive priority of the beacon-window event.
    pub beacon_pti: MacPti,
}

const TIMERS: [PmTimer; 5] = [
    PmTimer::SliceEnd,
    PmTimer::Active,
    PmTimer::SleepDelay,
    PmTimer::Dream,
    PmTimer::Preemption,
];

const fn timer_index(timer: PmTimer) -> usize {
    match timer {
        PmTimer::SliceEnd => 0,
        PmTimer::Active => 1,
        PmTimer::SleepDelay => 2,
        PmTimer::Dream => 3,
        PmTimer::Preemption => 4,
    }
}

/// The access point's beacon or probe response a station joined from, with
/// the station's monotonic time of its reception; `None` when its receive
/// timestamp had no place in the clock relation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JoinBeacon {
    pub beacon: PmBeacon,
    pub received_at: Option<oer_time::Instant>,
}

impl JoinBeacon {
    /// The access point's TSF at `now`: its timestamp advanced by the time
    /// since the frame arrived; `Ok(None)` without a reception time, which
    /// leaves the TSF to the first beacon.
    ///
    /// # Errors
    ///
    /// [`JoinTsfUnrepresentable`] when the sum crosses 2^64: the station
    /// cannot follow the access point's TSF, so the association ends
    /// ([`ConnectedDisconnectReason::JoinTsfUnrepresentable`]) instead of
    /// starting power management from it.
    ///
    /// SOURCE: complete pinned `libpp.a[if_hwctrl.o]::ic_update_sta_tsf`
    /// adds the difference of a free-running microsecond counter between now
    /// and the frame's reception to the node's timestamp and passes the sum
    /// to `hal_set_sta_tsf`; `libnet80211.a[ieee80211_sta.o]::
    /// sta_recv_assoc` calls it with the joined node's timestamp.
    pub fn access_point_tsf_at(
        self,
        now: oer_time::Instant,
    ) -> Result<Option<TsfInstant>, JoinTsfUnrepresentable> {
        let Some(received_at) = self.received_at else {
            return Ok(None);
        };
        access_point_tsf_after(
            self.beacon.timestamp_tsf,
            tsf_elapsed_at_nominal_rate(now.saturating_duration_since(received_at).as_micros()),
        )
        .map(Some)
        .ok_or(JoinTsfUnrepresentable)
    }
}

/// A join beacon's TSF advanced to the start of power management crosses
/// 2^64.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JoinTsfUnrepresentable;

/// The access point TSF `timestamp_tsf` advanced by `elapsed`, the time
/// since its frame arrived; `None` when the sum crosses 2^64, which the TSF
/// never does (`docs/architecture.md`, radio time domains).
pub const fn access_point_tsf_after(
    timestamp_tsf: TsfInstant,
    elapsed: TsfDuration,
) -> Option<TsfInstant> {
    timestamp_tsf.checked_add(elapsed)
}

/// The access point's TSF span of `elapsed_micros`, an elapsed time a local
/// counter of this station measured (the monotonic clock or the MAC local
/// time), taken at the TSF's nominal rate of one microsecond per counter
/// microsecond.
///
/// No relation between the local counter and the access point's TSF exists
/// here; the span ignores their drift, of at most
/// [`TSF_DRIFT_PPM`](oer_ieee80211_lower_mac::TSF_DRIFT_PPM) of it. This is
/// the only place the station's direct path carries a local span into the
/// TSF, as the vendor does.
///
/// SOURCE: complete pinned `libpp.a[if_hwctrl.o]::ic_update_sta_tsf` adds
/// the difference of a free-running microsecond counter to the node's
/// timestamp unscaled.
pub const fn tsf_elapsed_at_nominal_rate(elapsed_micros: u64) -> TsfDuration {
    TsfDuration::from_micros(elapsed_micros)
}

/// Whether the station can follow a beacon's TSF after any time its 32-bit
/// MAC local time measures since the frame arrived.
///
/// The station advances a beacon's timestamp by that wrapping counter's
/// difference before it writes the TSF, so a later timestamp crosses 2^64
/// within one counter period. The receive path drops such a beacon whole:
/// its TSF, TBTT schedule, TIM and liveness all come from one frame.
pub const fn beacon_tsf_followable(timestamp_tsf: TsfInstant) -> bool {
    access_point_tsf_after(timestamp_tsf, tsf_elapsed_at_nominal_rate(u32::MAX as u64)).is_some()
}

/// Power management of one association and the inputs waiting for it.
#[derive(Debug)]
pub(super) struct ConnectedPower {
    pub(super) engine: ModemSleep,
    /// The join beacon of an association whose power management has not
    /// started yet.
    start: Option<JoinBeacon>,
    /// The coexistence snapshot of a started association.
    coex: Option<PowerCoexSnapshot>,
    deadlines: [Option<oer_time::Instant>; 5],
    tbtt: bool,
    phase: Option<CoexPhaseView>,
    preemption: Option<Option<u64>>,
    /// Nulls requested while another control frame owned the TX path.
    nulls: [Option<bool>; 2],
    commands: [Option<ConnectedPowerCommand>; POWER_COMMAND_CAPACITY],
    command_head: usize,
    command_len: usize,
    /// The MAC TX queues are blocked for power management.
    pub(super) tx_blocked: bool,
    /// A network frame waits for the station to leave power save.
    network_held: bool,
    /// The current network frame was offered to power management.
    network_offered: bool,
    /// Power performed an input while a control event waited, so the
    /// next step delivers that event before another power input.
    pub(super) control_event_turn: bool,
}

/// The power input the next step performs, in priority order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PowerInput {
    Start(JoinBeacon),
    Null(bool),
    Tbtt,
    Phase(CoexPhaseView),
    Preemption(Option<u64>),
    Timer(usize),
    NetworkTxReport,
    NetworkTxOffer,
}

impl ConnectedPower {
    pub(super) const fn new() -> Self {
        Self {
            engine: ModemSleep::new(SleepType::None),
            start: None,
            coex: None,
            deadlines: [None; 5],
            tbtt: false,
            phase: None,
            preemption: None,
            nulls: [None; 2],
            commands: [None; POWER_COMMAND_CAPACITY],
            command_head: 0,
            command_len: 0,
            tx_blocked: false,
            network_held: false,
            network_offered: false,
            control_event_turn: false,
        }
    }

    /// Start a new association in place, without a temporary of the whole
    /// state on the caller's stack.
    fn restart(&mut self, sleep_type: SleepType, join_beacon: JoinBeacon, coex: PowerCoexSnapshot) {
        self.engine = ModemSleep::new(sleep_type);
        self.start = Some(join_beacon);
        self.coex = Some(coex);
        self.deadlines = [None; 5];
        self.tbtt = false;
        self.phase = None;
        self.preemption = None;
        self.nulls = [None; 2];
        self.commands.fill(None);
        self.command_head = 0;
        self.command_len = 0;
        self.tx_blocked = false;
        self.network_held = false;
        self.network_offered = false;
        self.control_event_turn = false;
    }

    pub(super) fn coex_view(&self) -> CoexView {
        match self.coex {
            Some(coex) => coex.view,
            None => CoexView::INACTIVE,
        }
    }

    pub(super) fn has_input(&self) -> bool {
        self.start.is_some()
            || self.tbtt
            || self.phase.is_some()
            || self.preemption.is_some()
            || self.nulls[0].is_some()
    }

    pub(super) fn next_deadline(&self) -> Option<oer_time::Instant> {
        self.deadlines.iter().fold(None, |earliest, deadline| {
            oer_ieee80211_sta::time::earliest(earliest, *deadline)
        })
    }

    /// Whether frames other than power management's Null must wait: the
    /// TX queues are blocked outside the Wi-Fi slice, or the RF sleeps.
    pub(super) fn blocks_tx(&self) -> bool {
        self.tx_blocked || self.engine.state() == PmState::Dozing
    }

    pub(super) fn push_command(
        &mut self,
        command: ConnectedPowerCommand,
    ) -> Result<(), ConnectedControlError> {
        if self.command_len == POWER_COMMAND_CAPACITY {
            return Err(ConnectedControlError::PowerCommandOverflow);
        }
        let index = (self.command_head + self.command_len) % POWER_COMMAND_CAPACITY;
        self.commands[index] = Some(command);
        self.command_len += 1;
        Ok(())
    }

    fn take_command(&mut self) -> Option<ConnectedPowerCommand> {
        if self.command_len == 0 {
            return None;
        }
        let command = self.commands[self.command_head].take();
        self.command_head = (self.command_head + 1) % POWER_COMMAND_CAPACITY;
        self.command_len -= 1;
        command
    }

    /// The index of the earliest timer expired at `now`.
    fn expired_timer(&self, now: oer_time::Instant) -> Option<usize> {
        self.deadlines
            .iter()
            .enumerate()
            .filter_map(|(index, deadline)| deadline.map(|deadline| (index, deadline)))
            .filter(|(_, deadline)| *deadline <= now)
            .min_by_key(|(_, deadline)| *deadline)
            .map(|(index, _)| index)
    }
}

impl ConnectedControlCore {
    /// Start power management for this association. `join_beacon` is the
    /// access point's last beacon or probe response before it; the next
    /// control step sets the station TSF from its timestamp and places the
    /// TBTT schedule from it.
    pub fn enable_power_management(
        &mut self,
        sleep_type: SleepType,
        join_beacon: JoinBeacon,
        coex: PowerCoexSnapshot,
    ) {
        self.power.restart(sleep_type, join_beacon, coex);
    }

    /// The station's power management.
    pub const fn power_management(&self) -> &ModemSleep {
        &self.power.engine
    }

    /// Power management with its pending inputs, timers and commands, for
    /// diagnostics.
    pub fn power_debug(&self) -> &dyn core::fmt::Debug {
        &self.power
    }

    /// Whether data frames of the access point matter to power management:
    /// the station advertises power save.
    pub fn wants_power_save_data(&self) -> bool {
        self.power.engine.state() != PmState::Awake
    }

    /// Whether control frames other than power management's Null must wait
    /// for the station's next Wi-Fi slice.
    pub fn power_blocks_tx(&self) -> bool {
        self.power.blocks_tx()
    }

    /// Whether the datapath may publish a network frame now.
    pub fn admits_network_tx(&self) -> bool {
        !self.power.blocks_tx() && !self.power.network_held
    }

    /// Whether a queued network frame must be offered to power management
    /// before the datapath may publish it.
    pub fn network_tx_needs_offer(&self, context: DatapathControlContext) -> bool {
        self.power.engine.is_started()
            && context.network_tx_pending
            && !self.power.network_offered
            && self.power.engine.state() != PmState::Awake
    }

    /// Replace the coexistence snapshot the next power input reads.
    pub fn set_power_coex(&mut self, coex: PowerCoexSnapshot) {
        if self.power.coex.is_some() {
            self.power.coex = Some(coex);
        }
    }

    /// The station TBTT fired.
    pub fn observe_tbtt(&mut self) {
        self.power.tbtt = true;
    }

    /// The coexistence schedule entered a phase that notifies Wi-Fi.
    pub fn observe_coex_phase(&mut self, phase: CoexPhaseView) {
        self.power.phase = Some(phase);
    }

    /// Bluetooth reported when its isochronous preemption ends, or that no
    /// preemption runs.
    pub fn observe_preemption_end(&mut self, end_micros: Option<u64>) {
        self.power.preemption = Some(end_micros);
    }

    /// The next power command for the runtime, in order.
    pub fn take_power_command(&mut self) -> Option<ConnectedPowerCommand> {
        self.power.take_command()
    }

    pub(super) fn power_traffic(
        &self,
        context: DatapathControlContext,
        control_event_pending: bool,
    ) -> PmTraffic {
        let control =
            self.in_flight.is_some() || control_event_pending || self.tx_block_ack.has_pending();
        PmTraffic {
            tx_pending: context.network_tx_pending || control,
            connection_pending: control,
        }
    }

    /// The power input the next step performs: the start, a queued Null,
    /// the TBTT, a coexistence phase, a preemption report, an expired timer
    /// or network TX, in that order.
    fn next_power_input<X: ConnectedControlTx>(
        &self,
        tx: &X,
        context: DatapathControlContext,
    ) -> Option<PowerInput> {
        let power = &self.power;
        if let Some(join) = power.start {
            Some(PowerInput::Start(join))
        } else if let Some(power_save) = power.nulls[0] {
            Some(PowerInput::Null(power_save))
        } else if power.tbtt {
            Some(PowerInput::Tbtt)
        } else if let Some(phase) = power.phase {
            Some(PowerInput::Phase(phase))
        } else if let Some(end) = power.preemption {
            Some(PowerInput::Preemption(end))
        } else if let Some(index) = power.expired_timer(tx.now()) {
            Some(PowerInput::Timer(index))
        } else if power.engine.is_started() && tx.has_network_tx_report() {
            Some(PowerInput::NetworkTxReport)
        } else if self.network_tx_needs_offer(context) {
            Some(PowerInput::NetworkTxOffer)
        } else {
            None
        }
    }

    /// Whether the next step performs a power input rather than take a
    /// waiting control event. After each power input performed while an
    /// event waited, the event takes the next step, so a stream of power
    /// inputs under saturated traffic cannot starve received beacons, and
    /// a stream of events cannot starve the TBTT.
    pub fn power_input_first<X: ConnectedControlTx>(
        &self,
        tx: &X,
        context: DatapathControlContext,
    ) -> bool {
        !self.power.control_event_turn && self.next_power_input(tx, context).is_some()
    }

    /// Perform at most one power input. `control_event_pending` is whether
    /// a control event waits for the next step.
    pub(super) fn service_power<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        context: DatapathControlContext,
        control_event_pending: bool,
    ) -> Result<Option<DatapathControlProgress<ConnectedDisconnectReason>>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let Some(input) = self.next_power_input(tx, context) else {
            return Ok(None);
        };
        self.power.control_event_turn = control_event_pending;
        trace_power_input(input, control_event_pending);
        let coex = self.power.coex_view();
        let traffic = self.power_traffic(context, control_event_pending);
        let clock = power_clock(tx);
        let mut actions = PmActions::new();
        match input {
            PowerInput::Start(join) => {
                self.power.start = None;
                // The TBTT schedule is in the access point's TSF, so the
                // station takes it before power management places its first
                // TBTT, as the vendor does on the Association Response.
                let Ok(join_tsf) = join.access_point_tsf_at(clock.now) else {
                    return Ok(Some(DatapathControlProgress::Exit(
                        ConnectedDisconnectReason::JoinTsfUnrepresentable,
                    )));
                };
                if let Some(tsf) = join_tsf {
                    self.station_tsf
                        .set(hardware, tsf)
                        .map_err(ConnectedControlError::TsfTiming)?;
                }
                self.power.engine.start(join.beacon, coex, &mut actions);
            }
            PowerInput::Null(power_save) => {
                self.power.nulls = [self.power.nulls[1], None];
                return self.start_null(hardware, tx, power_save).map(Some);
            }
            PowerInput::Tbtt => {
                self.power.tbtt = false;
                self.power.engine.tbtt(clock, coex, traffic, &mut actions);
            }
            PowerInput::Phase(phase) => {
                self.power.phase = None;
                self.power
                    .engine
                    .coex_phase(phase, clock, coex, &mut actions);
            }
            PowerInput::Preemption(end) => {
                self.power.preemption = None;
                self.power
                    .engine
                    .preemption_end(end, clock, coex, traffic, &mut actions);
            }
            PowerInput::Timer(index) => {
                self.power.deadlines[index] = None;
                match TIMERS[index] {
                    PmTimer::SliceEnd => {
                        self.power
                            .engine
                            .slice_end_timer(coex, traffic, &mut actions)
                    }
                    PmTimer::Active => self.power.engine.active_timer(coex, traffic, &mut actions),
                    PmTimer::SleepDelay => {
                        self.power
                            .engine
                            .sleep_delay_timer(clock, coex, traffic, &mut actions)
                    }
                    PmTimer::Dream => self.power.engine.dream_timer(&mut actions),
                    PmTimer::Preemption => {
                        self.power
                            .engine
                            .preemption_timer(coex, traffic, &mut actions)
                    }
                }
            }
            PowerInput::NetworkTxReport => {
                let report = tx.take_network_tx_report();
                if report.started && !core::mem::take(&mut self.power.network_offered) {
                    let _ = self
                        .power
                        .engine
                        .tx_data(true, clock, coex, traffic, &mut actions);
                }
                if let Some(acknowledged) = report.completed {
                    self.power.engine.tx_data_done(
                        acknowledged,
                        clock,
                        coex,
                        traffic,
                        &mut actions,
                    );
                }
            }
            PowerInput::NetworkTxOffer => {
                self.power.network_offered = true;
                let hold = self
                    .power
                    .engine
                    .tx_data(true, clock, coex, traffic, &mut actions);
                self.power.network_held = hold;
            }
        }
        self.apply_power_actions(hardware, tx, actions)?;
        Ok(Some(self.start_queued_null(hardware, tx)?))
    }

    /// Start the first queued Null when the TX path is free.
    fn start_queued_null<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        match self.power.nulls[0] {
            Some(power_save) if self.in_flight.is_none() => {
                self.power.nulls = [self.power.nulls[1], None];
                self.start_null(hardware, tx, power_save)
            }
            _ => Ok(DatapathControlProgress::More),
        }
    }

    fn start_null<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        power_save: bool,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        let power_management = if power_save {
            StaPowerManagement::PowerSave
        } else {
            StaPowerManagement::Active
        };
        let progress = tx.start_power_management_null(hardware, power_management)?;
        self.in_flight = Some(ControlInFlight::PowerManagement(power_management));
        Ok(progress)
    }

    /// Perform one input's effects in order.
    pub(super) fn apply_power_actions<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        actions: PmActions,
    ) -> Result<(), ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        for action in actions.iter() {
            match action {
                PmAction::SendNull { power_save } => {
                    let slot = self
                        .power
                        .nulls
                        .iter_mut()
                        .find(|slot| slot.is_none())
                        .ok_or(ConnectedControlError::PowerCommandOverflow)?;
                    *slot = Some(power_save);
                }
                PmAction::Coex(action) => self
                    .power
                    .push_command(ConnectedPowerCommand::Coex(action))?,
                PmAction::RfSleep => self.power.push_command(ConnectedPowerCommand::RfSleep)?,
                PmAction::RfWake => self.power.push_command(ConnectedPowerCommand::RfWake)?,
                PmAction::BlockTx => {
                    self.power.tx_blocked = true;
                    hardware.set_power_save_tx_block(true);
                }
                PmAction::UnblockTx => {
                    self.power.tx_blocked = false;
                    hardware.set_power_save_tx_block(false);
                }
                PmAction::Arm { timer, after } => {
                    // A deadline past the representable time never comes.
                    self.power.deadlines[timer_index(timer)] = tx.now().checked_add(after);
                }
                PmAction::Disarm(timer) => self.power.deadlines[timer_index(timer)] = None,
                PmAction::ReleaseHeldFrames => self.power.network_held = false,
                PmAction::StartTbtt(schedule) => hardware.start_station_tbtt(StaTbttSchedule {
                    first_tbtt_tsf: schedule.first_tbtt.as_micros(),
                    interval_micros: schedule.interval_micros,
                    ahead_micros: schedule.ahead_micros,
                    wake_ahead_micros: schedule.wake_ahead_micros,
                }),
                PmAction::SetTbttInterval(interval) => {
                    hardware.set_station_tbtt_interval(interval)?
                }
                PmAction::StopTbtt => hardware.stop_station_tbtt(),
                PmAction::SetTbttAhead {
                    ahead_micros,
                    wake_ahead_micros,
                } => hardware.set_station_tbtt_ahead(ahead_micros, wake_ahead_micros),
                PmAction::RxBeaconPriority(true) => hardware.set_rx_beacon_pti(
                    self.power
                        .coex
                        .expect("power management starts with a coexistence snapshot")
                        .beacon_pti,
                ),
                PmAction::RxBeaconPriority(false) => hardware.set_rx_beacon_pti(
                    MacPti::new(0).expect("priority zero is a valid MAC priority"),
                ),
                PmAction::ClearRxBeaconPriority => hardware.clear_rx_beacon_pti(),
                PmAction::RxBeaconTime {
                    window_micros,
                    time_micros,
                    // The register holds the window's low 16 bits, as the
                    // vendor writes it.
                } => hardware.set_rx_beacon_time(window_micros as u16, time_micros),
            }
        }
        Ok(())
    }

    /// Forget every pending power input after the association stopped.
    pub(super) fn clear_power_inputs(&mut self) {
        let engine = self.power.engine.clone();
        let mut commands = ConnectedPower::new();
        while let Some(command) = self.power.take_command() {
            // The runtime still performs the stop's releases and RF wake.
            let _ = commands.push_command(command);
        }
        self.power = commands;
        self.power.engine = engine;
    }
}

fn trace_power_input(input: PowerInput, control_event_waiting: bool) {
    let input = match input {
        PowerInput::Start(_) => PowerInputKind::Start,
        PowerInput::Null(_) => PowerInputKind::Null,
        PowerInput::Tbtt => PowerInputKind::Tbtt,
        PowerInput::Phase(_) => PowerInputKind::CoexPhase,
        PowerInput::Preemption(_) => PowerInputKind::Preemption,
        PowerInput::Timer(_) => PowerInputKind::Timer,
        PowerInput::NetworkTxReport | PowerInput::NetworkTxOffer => {
            oer_trace::emit(&NetworkTxPowerTrace {
                offer: input == PowerInput::NetworkTxOffer,
                control_event_waiting,
            });
            return;
        }
    };
    oer_trace::emit(&PowerInputTrace {
        input,
        control_event_waiting,
    });
}

pub(super) fn power_clock<X: ConnectedControlTx>(tx: &X) -> PmClock {
    PmClock { now: tx.now() }
}

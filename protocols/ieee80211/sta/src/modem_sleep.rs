//! Vendor station power management for one associated station.
//!
//! This is the connected-station part of the vendor power manager
//! (`libpp.a[pm.o]`, `pm_coex.o`) as an executor-independent state machine.
//! Every input is one vendor entry point; every effect is a [`PmAction`] the
//! caller performs in order: frames to send, coexistence requests, TX queue
//! gating, RF sleep and wake, timers and the station TBTT schedule.
//!
//! Under active coexistence a station follows the schedule's Wi-Fi slices
//! even when power save is off: it advertises power save and blocks its TX
//! queues when its slice ends, wakes at its next slice or beacon, and
//! requests the air for its slice, its beacon window and buffered traffic.
//! Without coexistence, sleep type `None` keeps the station awake and the
//! modem sleep types follow the DTIM or the listen interval.
//!
//! Only an associated station is modelled. The vendor's disconnected power
//! save, connectionless wake windows, TWT, mesh and hardware beacon
//! monitoring are separate features and absent here.
//!
//! SOURCE: complete pinned `libpp.a[pm.o]` and `libpp.a[pm_coex.o]` of
//! espressif/esp32-wifi-lib `af55a0ca`; each method names the function it
//! follows.

use oer_ieee80211_wire::tsf::TsfInstant;
use oer_time::{Duration, Instant};

/// Beacon receive window requested at each TBTT (`g_pm_cfg[20]`).
pub const BEACON_WINDOW_MICROS: u32 = 25_000;
/// Beacon receive window while a Bluetooth isochronous preemption runs.
pub const PREEMPTED_BEACON_WINDOW_MICROS: u32 = 15_000;
/// Second beacon-time argument (`g_pm_cfg[24]`).
pub const BEACON_TIME_MICROS: u32 = 10_000;
/// Second beacon-time argument while a preemption runs.
pub const PREEMPTED_BEACON_TIME_MICROS: u32 = 5_000;
/// Sleep delay after the last activity, and the margin before a slice end
/// at which a power-save station leaves the air (`g_pm_cfg[36]`).
pub const SLEEP_DELAY_MICROS: u32 = 5_000;
/// Slice-end margin of a station that is not in power save.
pub const IDLE_SLICE_MARGIN_MICROS: u32 = 1_000;
/// Idle time after which an awake modem-sleep station sleeps again
/// (`g_pm_cfg[32]`).
pub const ACTIVE_TIMEOUT_MICROS: u32 = 50_000;
/// Time a station keeps receiving buffered group traffic (`g_pm_cfg[72]`).
pub const DREAM_TIMEOUT_MICROS: u32 = 15_000;
/// TBTT event lead without coexistence (`g_pm_cfg[8]`).
pub const TBTT_AHEAD_MICROS: u16 = 3_000;
/// TBTT event lead while another radio shares the air (`g_pm_cfg[12]`).
pub const SHARED_TBTT_AHEAD_MICROS: u16 = 3_500;
/// Wake lead added to the TBTT lead (`g_pm_cfg[16]`).
pub const TBTT_WAKE_WINDOW_MICROS: u16 = 1_500;
/// Minimum slice time left for a unicast wake or a TX wake.
pub const WAKE_SLICE_THRESHOLD_MICROS: u32 = 5_000;
/// Minimum slice time left for a group-traffic wake.
pub const GROUP_SLICE_THRESHOLD_MICROS: u32 = 1_000;
/// Minimum slice time left to keep the slice request.
pub const SLICE_REQUEST_THRESHOLD_MICROS: u32 = 5_000;
/// Consecutive active Null failures after which the station stops retrying.
pub const NULL_RETRY_LIMIT: u8 = 15;
/// Beacon interval assumed until the access point advertises one.
pub const DEFAULT_BEACON_INTERVAL_MICROS: u32 = 0x19_000;
const DEFAULT_BEACON_PERIOD: core::num::NonZeroU32 = {
    #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
    const fn checked() -> core::num::NonZeroU32 {
        match core::num::NonZeroU32::new(DEFAULT_BEACON_INTERVAL_MICROS) {
            Some(period) => period,
            None => panic!("the default beacon interval is positive"),
        }
    }
    checked()
};

/// Zero TU already means the default interval in the recovered PM policy.
/// Validate once, retaining the positive period through every division.
fn beacon_period(interval_tu: u16) -> core::num::NonZeroU32 {
    core::num::NonZeroU32::new(u32::from(interval_tu) << 10).unwrap_or(DEFAULT_BEACON_PERIOD)
}

/// Coexistence schedule interval unit, in microseconds.
const SCHEDULE_INTERVAL_UNIT_MICROS: u32 = 100;

/// Coexistence events Wi-Fi requests from power management.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PmCoexEvent {
    /// Event 0: the beacon receive window at a TBTT.
    BeaconWindow,
    /// Event 1: the Wi-Fi slice, or awake time inside it.
    Slice,
    /// Event 2: buffered group traffic after a DTIM.
    GroupTraffic,
}

impl PmCoexEvent {
    /// The vendor event number.
    pub const fn id(self) -> u8 {
        match self {
            Self::BeaconWindow => 0,
            Self::Slice => 1,
            Self::GroupTraffic => 2,
        }
    }
}

/// The station's configured power-save type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SleepType {
    /// No power save: the station sleeps only for coexistence.
    None,
    /// Modem sleep waking for every DTIM.
    MinModem,
    /// Modem sleep waking at the listen interval, rounded to the DTIM.
    MaxModem(crate::request::StationListenInterval),
}

impl SleepType {
    /// The configured listen interval: the max-modem setting, and the
    /// station's default otherwise (the vendor reads one listen interval for
    /// every type and uses it only for max-modem).
    const fn listen_interval(self) -> crate::request::StationListenInterval {
        match self {
            Self::MaxModem(listen_interval) => listen_interval,
            Self::None | Self::MinModem => crate::request::StationListenInterval::DEFAULT,
        }
    }
}

impl From<crate::request::StationPowerMode> for SleepType {
    fn from(mode: crate::request::StationPowerMode) -> Self {
        use crate::request::StationPowerMode;
        match mode {
            StationPowerMode::None => Self::None,
            StationPowerMode::MinModem => Self::MinModem,
            StationPowerMode::MaxModem(listen_interval) => Self::MaxModem(listen_interval),
        }
    }
}

/// Power-management state (`g_pm[1]`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PmState {
    /// Awake with power save not advertised.
    Awake,
    /// Power save advertised; the modem is still awake.
    PowerSave,
    /// Power save advertised and the station's RF asleep.
    Dozing,
}

/// Timers the power manager arms.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PmTimer {
    /// End of the Wi-Fi slice (`g_pm+240`).
    SliceEnd,
    /// Idle time before an awake modem-sleep station sleeps (`g_pm+200`).
    Active,
    /// Delay before a power-save station dozes (`g_pm+260`).
    SleepDelay,
    /// Time to keep receiving buffered group traffic (`g_pm+180`).
    Dream,
    /// End of a Bluetooth isochronous preemption (`g_pm+1140`).
    Preemption,
}

/// A schedule cannot be represented without changing its meaning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PmScheduleError {
    /// A phase percentage is outside 0 through 100.
    InvalidSharePercent(u8),
    /// An absolute deadline is outside the monotonic clock's range.
    DeadlineOverflow,
    /// The supplied clock precedes this association's cycle anchor.
    ClockBeforeCycleAnchor,
    /// Active coexistence has no positive cycle to schedule.
    ZeroCycle,
    /// A coexistence request exceeds the backend's duration field.
    RequestDurationOutOfRange { micros: u64 },
    /// A TBTT interval exceeds the backend's interval field.
    TbttIntervalOutOfRange { micros: u64 },
}

/// One station TBTT schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmTbttSchedule {
    /// A TBTT of the schedule, in the station TSF.
    pub first_tbtt: TsfInstant,
    pub interval_micros: u64,
    pub ahead_micros: u16,
    pub wake_ahead_micros: u16,
}

/// One coexistence effect of the power manager. The coexistence schedule
/// belongs to the shared radio every radio protocol shares, not to the
/// Wi-Fi MAC, so its owner performs these.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PmCoexAction {
    /// Request the air for a coexistence event.
    Request {
        event: PmCoexEvent,
        duration_micros: u32,
    },
    /// Withdraw a coexistence event request.
    Release(PmCoexEvent),
    /// Set the coexistence schedule interval, in 100 µs units.
    SetInterval(u32),
    /// Restart the coexistence phases at phase 0.
    RestartPhases,
    /// Set the coexistence flexible period.
    SetFlexiblePeriod(u8),
}

/// One effect of the power manager, performed in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PmAction {
    /// Send a Null Data frame advertising (`true`) or leaving power save.
    SendNull { power_save: bool },
    /// A coexistence effect, which the owner of the shared radio performs.
    Coex(PmCoexAction),
    /// Block the MAC TX queues, as the slice end does.
    BlockTx,
    /// Unblock the MAC TX queues.
    UnblockTx,
    /// Put the station's RF to sleep, keeping its registration.
    RfSleep,
    /// Wake the station's RF.
    RfWake,
    /// Arm a timer to expire after the given time.
    Arm { timer: PmTimer, after: Duration },
    /// Arm at an already computed monotonic deadline. Applying a batch
    /// later must not extend a coexistence slice or preemption gap.
    ArmAt { timer: PmTimer, deadline: Instant },
    /// The deadline has already arrived. Queue this timer as ready work;
    /// do not turn an elapsed margin into a future delay.
    Due(PmTimer),
    /// Disarm a timer.
    Disarm(PmTimer),
    /// Transmit the frames held while the station was in power save.
    ReleaseHeldFrames,
    /// Program the station TBTT schedule.
    StartTbtt(PmTbttSchedule),
    /// Replace the TBTT interval.
    SetTbttInterval(u64),
    /// Stop the station TBTT.
    StopTbtt,
    /// Replace the TBTT lead and the wake lead beside it.
    SetTbttAhead {
        ahead_micros: u16,
        wake_ahead_micros: u16,
    },
    /// Publish the beacon-window event's priority as the beacon receive
    /// priority (`true`), or priority zero (`false`).
    ///
    /// SOURCE(esp32s31): complete pinned `libpp[pm_coex.o]::pm_coex_update_rx_beacon_pti` passes `coex_pti_get(0)`, or zero, as
    /// both arguments of `hal_set_rx_beacon_pti`. It selects event 1 only
    /// under the hardware beacon monitor, which this model leaves out.
    RxBeaconPriority(bool),
    /// Clear the beacon receive priority after a beacon.
    ClearRxBeaconPriority,
    /// Set the hardware beacon receive time.
    RxBeaconTime {
        window_micros: u32,
        time_micros: u32,
    },
}

/// Maximum reserved places for one input's effects. TBTT reserves
/// 1 normalization + 1 RF wake + 13 coexistence + 1 interval + 8 parameter
/// effects. Every other input reserves fewer places. Helpers take fixed-size
/// arrays of affine places, so exceeding either bound fails to compile.
pub const PM_ACTION_CAPACITY: usize = 24;

/// The ordered effects of one input.
///
/// Each public input returns a fresh batch. There is no append operation;
/// helpers reserve their complete output before changing state. Unused
/// places are skipped without changing the order of the occupied places.
///
/// ```
/// use oer_ieee80211_sta::modem_sleep::{CoexView, ModemSleep, PmBeacon, SleepType};
/// use oer_ieee80211_wire::tsf::TsfInstant;
/// let mut pm = ModemSleep::new(SleepType::None);
/// let beacon = PmBeacon { timestamp_tsf: TsfInstant::from_micros(0), interval_tu: 100, tim: None };
/// let start = pm.start(beacon, CoexView::INACTIVE);
/// assert!(!start.is_empty());
/// let stop = pm.stop(CoexView::INACTIVE);
/// assert!(!stop.is_empty());
/// for effect in start.into_iter().chain(stop) { let _ = effect; }
/// ```
///
/// Callers cannot accumulate several inputs in one batch:
///
/// ```compile_fail,E0599
/// use oer_ieee80211_sta::modem_sleep::{CoexView, ModemSleep, PmActions, PmBeacon, SleepType};
/// use oer_ieee80211_wire::tsf::TsfInstant;
/// let mut pm = ModemSleep::new(SleepType::None);
/// let beacon = PmBeacon { timestamp_tsf: TsfInstant::from_micros(0), interval_tu: 100, tim: None };
/// let mut actions = PmActions::new();
/// pm.start(beacon, CoexView::INACTIVE, &mut actions);
/// pm.stop(CoexView::INACTIVE, &mut actions);
/// ```
#[derive(Debug, Eq, PartialEq)]
#[must_use = "perform this input's effects in order"]
pub struct PmActions {
    items: [Option<PmAction>; PM_ACTION_CAPACITY],
}

impl PmActions {
    /// Check every relative timer before performing any effect in this batch.
    /// Use this same `now` when arming the validated timers. A refusal keeps
    /// every effect available for the caller's terminal failure path.
    pub fn validate_deadlines(&self, now: Instant) -> Result<(), PmScheduleError> {
        for action in self.iter() {
            if let PmAction::Arm { after, .. } = action {
                now.checked_add(after)
                    .ok_or(PmScheduleError::DeadlineOverflow)?;
            }
        }
        Ok(())
    }

    /// Number of effects this input produced.
    pub fn len(&self) -> usize {
        self.iter().count()
    }

    /// Whether this input produced no effects.
    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    /// Inspect the effects in execution order.
    pub fn iter(&self) -> impl Iterator<Item = PmAction> + '_ {
        self.items.iter().flatten().copied()
    }
}

impl IntoIterator for PmActions {
    type Item = PmAction;
    type IntoIter =
        core::iter::Flatten<core::array::IntoIter<Option<PmAction>, PM_ACTION_CAPACITY>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().flatten()
    }
}

/// One reserved output place. Writing consumes it; a helper can neither
/// append beyond its reservation nor overwrite another helper's effect.
struct EffectPlace<'a>(&'a mut Option<PmAction>);

impl EffectPlace<'_> {
    fn write(self, action: PmAction) {
        *self.0 = Some(action);
    }
}

type Places<'a, const N: usize> = [EffectPlace<'a>; N];

struct PhaseTiming {
    slice_end: u64,
    duration_micros: u32,
    deadline: Option<u64>,
}

/// Coexistence as Wi-Fi reads it at one instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexView {
    /// `coex_status_get` for Wi-Fi: coexistence started and another radio
    /// publishes status.
    pub active: bool,
    /// `coex_schm_curr_period_get`.
    pub current_period: u8,
    /// `coex_schm_flexible_period_get`.
    pub flexible_period: u8,
    /// `coex_schm_interval_get`, in 100 µs units.
    pub interval: u32,
    /// Wi-Fi share of phase 0, `coex_schm_get_phase_by_idx(0)`.
    pub phase0_share_percent: u8,
}

impl CoexView {
    /// Coexistence inactive.
    pub const INACTIVE: Self = Self {
        active: false,
        current_period: 0,
        flexible_period: 0,
        interval: 0,
        phase0_share_percent: 0,
    };

    /// `pm_coex_schm_overall_period_get`: the larger of the current and the
    /// flexible period.
    pub const fn overall_period(self) -> u8 {
        if self.current_period < self.flexible_period {
            self.flexible_period
        } else {
            self.current_period
        }
    }

    /// One schedule cycle, in microseconds.
    #[cfg_attr(
        not(test),
        expect(clippy::arithmetic_side_effects, reason = "u8 * u32 * 100 fits u64")
    )]
    const fn cycle_micros(self) -> u64 {
        // u8 * u32 * 100 is at most 109_521_666_022_500.
        (self.overall_period() as u64)
            * (self.interval as u64)
            * (SCHEDULE_INTERVAL_UNIT_MICROS as u64)
    }
}

/// One coexistence phase as Wi-Fi's phase callback reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPhaseView {
    /// The phase's share of the period, in percent.
    pub share_percent: u8,
    /// The phase's Wi-Fi flags.
    pub wifi: u8,
}

impl CoexPhaseView {
    /// Wi-Fi owns this phase: it requests its slice and may transmit.
    pub const WIFI_SLICE: u8 = 0x02;
    /// This phase ends a Wi-Fi slice: arm the slice-end timer.
    pub const SLICE_END: u8 = 0x01;
    /// This phase lends the air to Wi-Fi without its own slice.
    pub const SHARED: u8 = 0x04;
}

/// The beacon a station received from its access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmBeacon {
    /// The beacon's timestamp in the station TSF.
    pub timestamp_tsf: TsfInstant,
    /// The advertised beacon interval, in time units.
    pub interval_tu: u16,
    /// The TIM of the beacon, when present.
    pub tim: Option<PmTim>,
}

/// The part of a TIM power management reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmTim {
    pub dtim_count: u8,
    pub dtim_period: u8,
    /// The access point buffers individually addressed traffic for us.
    pub unicast: bool,
    /// The access point buffers group traffic after this DTIM.
    pub group: bool,
}

/// The Null in-flight flags before a [`PmAction::SendNull`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmNullFlags {
    active_in_flight: bool,
    power_save_in_flight: bool,
}

/// Observation of `pm_go_to_wake`; state and actions always retain its effect.
/// Event handlers only execute that transition. The TX path reads this value
/// to decide whether its frame must remain held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WakeResult {
    NotStarted,
    /// The wake waits for the next slice (vendor return 1).
    Deferred,
    /// The station is awake (vendor return 2).
    Awake,
    /// A Null leaving power save is in flight (vendor return 3).
    Waking,
}

/// The clock one power-management input reads.
///
/// The vendor keeps timer deadlines in `esp_timer_get_time` and measures the
/// station's position in the schedule cycle between two readings of the
/// free-running MAC microsecond counter at `0x2010_d800`. Only differences
/// of that counter matter, so one monotonic microsecond clock serves both.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmClock {
    /// The image's monotonic time.
    pub now: Instant,
}

/// What the TX path reports when power management asks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PmTraffic {
    /// `ppCheckTxIdle` is nonzero: a frame waits in a TX queue.
    pub tx_pending: bool,
    /// `ppCheckTxConnTrafficIdle` is nonzero: connection traffic waits.
    pub connection_pending: bool,
}

impl PmTraffic {
    pub const IDLE: Self = Self {
        tx_pending: false,
        connection_pending: false,
    };
}

/// Power management of one associated station (`g_pm`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModemSleep {
    sleep_type: SleepType,
    /// `[14]`: power management runs for an associated station.
    started: bool,
    /// `[1]`
    state: PmState,
    /// `[26]`: the station is inside its Wi-Fi slice.
    in_slice: bool,
    /// `[27]`: a wake waits for the next Wi-Fi slice or TBTT.
    wake_deferred: bool,
    /// `[21]`: a Null leaving power save is in flight.
    active_null_in_flight: bool,
    /// `[22]`: a Null entering power save is in flight.
    power_save_null_in_flight: bool,
    /// `[19]`: a beacon is expected since the last TBTT.
    beacon_expected: bool,
    /// `[1137]`: a beacon was still expected when the last slice ended.
    beacon_expected_at_slice_end: bool,
    /// `[20]`: buffered traffic is being received.
    receiving: bool,
    /// `[9]`: failed Nulls leaving power save.
    active_null_failures: u8,
    /// `[10]`: failed Nulls entering power save.
    power_save_null_failures: u8,
    /// `[18]`: program the TBTT schedule at the next beacon.
    update_tbtt_at_next_beacon: bool,
    /// `[17]`: a beacon has been parsed since the start.
    beacon_parsed: bool,
    /// `[52]`
    beacon_interval_micros: core::num::NonZeroU32,
    /// `[11]`
    dtim_period: u8,
    /// `[34]`: the listen interval in beacons, rounded to the DTIM
    /// (`scale_listen_interval`); max-modem's TBTT interval. `pm_attach`
    /// starts it at one.
    listen_interval_beacons: u64,
    /// `[12]`: the overall period the TBTT interval was last derived from.
    tbtt_period: u8,
    /// `[112]`: end of the Wi-Fi slice, monotonic.
    slice_end: u64,
    /// `[120]`: when the station leaves the slice, monotonic.
    slice_deadline: u64,
    /// `[432]`: rest of the cycle after the Wi-Fi slice.
    cycle_remainder: u64,
    /// `[96]`: time of the TBTT the cycle position counts from.
    cycle_anchor: u64,
    /// `[440]`: the sleep-delay timer is armed.
    sleep_delay_armed: bool,
    /// `[292]`: outstanding explicit wake holds.
    wake_holds: u32,
    /// `[76]`: activity since the last TBTT.
    activity: u32,
    /// `[1139]`: a Bluetooth isochronous preemption runs.
    preempted: bool,
    /// `[1160]`: when the preemption ends, monotonic.
    preemption_end: u64,
    /// `[60]`: the beacon window requested at each TBTT.
    beacon_window_micros: u32,
    /// `[56]`: the active timeout.
    active_timeout_micros: u64,
    /// `[44]`: the TBTT lead.
    tbtt_ahead_micros: u16,
    /// `[48]`: the wake window added to the TBTT lead.
    tbtt_window_micros: u16,
    /// `[13]`: coexistence activity at the last parameter update.
    coex_was_active: bool,
}

impl ModemSleep {
    /// Reserve the complete effect batch before an input changes state.
    /// Every place is minted once and consumed by exactly one helper write.
    fn transition<R>(
        &mut self,
        step: impl FnOnce(&mut Self, Places<'_, PM_ACTION_CAPACITY>) -> R,
    ) -> (R, PmActions) {
        let mut actions = PmActions {
            items: [None; PM_ACTION_CAPACITY],
        };
        let result = step(self, actions.items.each_mut().map(EffectPlace));
        (result, actions)
    }

    /// Arithmetic refusals retain the input state and publish no effects.
    /// This state contains only values; no hardware or affine owner is cloned.
    fn try_transition<R>(
        &mut self,
        clock: PmClock,
        step: impl FnOnce(&mut Self, Places<'_, PM_ACTION_CAPACITY>) -> Result<R, PmScheduleError>,
    ) -> Result<(R, PmActions), PmScheduleError> {
        let mut next = self.clone();
        let mut actions = PmActions {
            items: [None; PM_ACTION_CAPACITY],
        };
        let result = step(&mut next, actions.items.each_mut().map(EffectPlace))?;
        actions.validate_deadlines(clock.now)?;
        // Every input is a captured value: the validated copy is the
        // committed state.
        *self = next;
        Ok((result, actions))
    }

    /// Stopped power management with the vendor attach defaults.
    ///
    /// SOURCE(esp32s31): `pm_attach`.
    pub const fn new(sleep_type: SleepType) -> Self {
        Self {
            sleep_type,
            started: false,
            state: PmState::Awake,
            in_slice: true,
            wake_deferred: false,
            active_null_in_flight: false,
            power_save_null_in_flight: false,
            beacon_expected: false,
            beacon_expected_at_slice_end: false,
            receiving: false,
            active_null_failures: 0,
            power_save_null_failures: 0,
            update_tbtt_at_next_beacon: false,
            beacon_parsed: false,
            beacon_interval_micros: DEFAULT_BEACON_PERIOD,
            dtim_period: 1,
            listen_interval_beacons: 1,
            tbtt_period: 0,
            slice_end: 0,
            slice_deadline: 0,
            cycle_remainder: 0,
            cycle_anchor: 0,
            sleep_delay_armed: false,
            wake_holds: 0,
            activity: 0,
            preempted: false,
            preemption_end: 0,
            beacon_window_micros: BEACON_WINDOW_MICROS,
            active_timeout_micros: ACTIVE_TIMEOUT_MICROS as u64,
            tbtt_ahead_micros: TBTT_AHEAD_MICROS,
            tbtt_window_micros: TBTT_WAKE_WINDOW_MICROS,
            coex_was_active: false,
        }
    }

    pub const fn state(&self) -> PmState {
        self.state
    }

    /// Enter `state`.
    fn enter(&mut self, state: PmState) {
        self.state = state;
    }

    pub const fn is_started(&self) -> bool {
        self.started
    }

    pub const fn in_slice(&self) -> bool {
        self.in_slice
    }

    pub const fn beacon_interval_micros(&self) -> u32 {
        self.beacon_interval_micros.get()
    }

    /// Normalize the state and emit the vendor's wake effects. TBTT needs
    /// this transition independently of the sleep-admission decision.
    fn normalize_sleep_state(&mut self, coex: CoexView, places: Places<'_, 1>) {
        let [wake] = places;
        if self.sleep_type == SleepType::None && !coex.active {
            if self.state != PmState::Awake {
                self.dream(coex, None, [wake]);
                self.enter(PmState::Awake);
            }
            return;
        }
        if !self.started && self.state != PmState::Awake {
            self.dream(coex, None, [wake]);
            self.enter(PmState::Awake);
        }
    }

    /// `pm_check_state`: without coexistence, sleep type None stays awake.
    /// Under coexistence it may sleep; admission also requires a started link.
    fn may_sleep(&mut self, coex: CoexView, places: Places<'_, 1>) -> bool {
        let [normalize] = places;
        self.normalize_sleep_state(coex, [normalize]);
        self.started && (self.sleep_type != SleepType::None || coex.active)
    }

    const fn is_sleeping(&self) -> bool {
        matches!(self.state, PmState::Dozing)
    }

    fn incr_activity(&mut self) {
        // Presence counter, not a time or deadline. Zero means no activity;
        // preserving a nonzero value across modular rollover is intentional.
        self.activity = self.activity.wrapping_add(1);
        if self.activity == 0 {
            self.activity = 1;
        }
    }

    /// `pm_start`: an association completed. `join_beacon` is the access
    /// point's last beacon or probe response before the association; its
    /// timestamp places the first TBTT schedule, as the vendor passes its
    /// node timestamp without a TIM.
    pub fn start(&mut self, join_beacon: PmBeacon, coex: CoexView) -> PmActions {
        self.transition(|this, [a, b, c, d, e, f, g, h, ..]| {
            this.start_into(join_beacon, coex, [a, b, c, d, e, f, g, h])
        })
        .1
    }

    fn start_into(&mut self, join_beacon: PmBeacon, coex: CoexView, places: Places<'_, 8>) {
        let [
            interval,
            scale,
            power,
            tbtt,
            priority,
            time,
            active0,
            active1,
        ] = places;
        if self.started {
            return;
        }
        self.active_null_in_flight = false;
        self.power_save_null_in_flight = false;
        self.activity = 0;
        self.beacon_interval_micros = beacon_period(join_beacon.interval_tu);
        if coex.active {
            interval.write(PmAction::Coex(PmCoexAction::SetInterval(
                self.beacon_interval_micros.get() / SCHEDULE_INTERVAL_UNIT_MICROS,
            )));
        }
        self.dtim_period = 1;
        self.scale_listen_interval(coex, [scale]);
        self.coex_pwr_update(coex, [power]);
        self.coex_was_active = coex.active;
        self.update_tbtt_at_next_beacon = true;
        self.cycle_anchor = 0;
        self.update_next_tbtt(
            PmBeacon {
                tim: None,
                ..join_beacon
            },
            coex,
            [tbtt],
        );
        priority.write(PmAction::RxBeaconPriority(true));
        time.write(PmAction::RxBeaconTime {
            window_micros: self.beacon_window_micros,
            time_micros: BEACON_TIME_MICROS,
        });
        self.enable_active_timer([active0, active1]);
        self.started = true;
        self.preempted = false;
    }

    /// `pm_stop`: the association ends.
    pub fn stop(&mut self, coex: CoexView) -> PmActions {
        self.transition(|this, [a, b, c, d, e, f, g, h, i, j, ..]| {
            this.stop_into(coex, [a, b, c, d, e, f, g, h, i, j])
        })
        .1
    }

    fn stop_into(&mut self, coex: CoexView, places: Places<'_, 10>) {
        let [
            unblock,
            delay,
            active,
            tbtt,
            priority,
            dream_timer,
            wake,
            held,
            slice,
            power,
        ] = places;
        if !self.started {
            return;
        }
        unblock.write(PmAction::UnblockTx);
        self.disable_sleep_delay_timer([delay]);
        active.write(PmAction::Disarm(PmTimer::Active));
        tbtt.write(PmAction::StopTbtt);
        priority.write(PmAction::RxBeaconPriority(false));
        dream_timer.write(PmAction::Disarm(PmTimer::Dream));
        if self.state != PmState::Awake {
            self.dream(coex, None, [wake]);
            self.enter(PmState::Awake);
            held.write(PmAction::ReleaseHeldFrames);
        }
        self.dtim_period = 1;
        self.in_slice = true;
        self.wake_deferred = false;
        self.started = false;
        self.beacon_parsed = false;
        slice.write(PmAction::Disarm(PmTimer::SliceEnd));
        self.coex_pwr_update(coex, [power]);
    }

    /// `pm_coex_pwr_update` with the vendor default coexistence power
    /// configuration off: the flexible period is one.
    fn coex_pwr_update(&mut self, coex: CoexView, places: Places<'_, 1>) {
        let [flexible] = places;
        flexible.write(PmAction::Coex(PmCoexAction::SetFlexiblePeriod(1)));
        let coex = CoexView {
            flexible_period: 1,
            ..coex
        };
        self.tbtt_period = coex.overall_period();
        if self.started && coex.active {
            self.update_tbtt_at_next_beacon = true;
        }
    }

    fn enable_active_timer(&mut self, places: Places<'_, 2>) {
        let [disarm, arm] = places;
        disarm.write(PmAction::Disarm(PmTimer::Active));
        if self.sleep_type != SleepType::None {
            arm.write(PmAction::Arm {
                timer: PmTimer::Active,
                after: Duration::from_micros(self.active_timeout_micros),
            });
        }
    }

    fn enable_sleep_delay_timer(&mut self, places: Places<'_, 2>) {
        let [disarm, arm] = places;
        disarm.write(PmAction::Disarm(PmTimer::SleepDelay));
        arm.write(PmAction::Arm {
            timer: PmTimer::SleepDelay,
            after: Duration::from_micros(u64::from(SLEEP_DELAY_MICROS)),
        });
        self.sleep_delay_armed = true;
    }

    fn disable_sleep_delay_timer(&mut self, places: Places<'_, 1>) {
        let [disarm] = places;
        disarm.write(PmAction::Disarm(PmTimer::SleepDelay));
        self.sleep_delay_armed = false;
    }

    fn enable_dream_timer(&mut self, places: Places<'_, 2>) {
        let [disarm, arm] = places;
        disarm.write(PmAction::Disarm(PmTimer::Dream));
        arm.write(PmAction::Arm {
            timer: PmTimer::Dream,
            after: Duration::from_micros(u64::from(DREAM_TIMEOUT_MICROS)),
        });
    }

    /// `pm_dream`: wake a dozing station's RF and return to advertised power
    /// save; the slice-end timer is re-armed when still inside the slice.
    fn dream(&mut self, _coex: CoexView, _clock: Option<PmClock>, places: Places<'_, 1>) {
        let [wake] = places;
        if self.state == PmState::Dozing {
            wake.write(PmAction::RfWake);
        }
        self.enter(PmState::PowerSave);
    }

    /// `pm_coex_recalculate_wifi_time_slice`: place the station in the cycle
    /// counted from the TBTT anchor and re-arm the slice-end timer.
    #[cfg_attr(
        not(test),
        expect(
            clippy::arithmetic_side_effects,
            reason = "bounded u8/u32 products fit u64; offset <= now and validated share <= cycle"
        )
    )]
    fn recalculate_slice(
        &mut self,
        clock: PmClock,
        force: bool,
        coex: CoexView,
        places: Places<'_, 2>,
    ) -> Result<(), PmScheduleError> {
        let [disarm, arm] = places;
        if !coex.active {
            return Ok(());
        }
        let now = clock.now.as_micros();
        let cycle_end = self
            .slice_end
            .checked_add(self.cycle_remainder)
            .ok_or(PmScheduleError::DeadlineOverflow)?;
        if !force && now < cycle_end {
            return Ok(());
        }
        if coex.phase0_share_percent > 100 {
            return Err(PmScheduleError::InvalidSharePercent(
                coex.phase0_share_percent,
            ));
        }
        let cycle =
            core::num::NonZeroU64::new(coex.cycle_micros()).ok_or(PmScheduleError::ZeroCycle)?;
        let offset = now
            .checked_sub(self.cycle_anchor)
            .ok_or(PmScheduleError::ClockBeforeCycleAnchor)?
            % cycle;
        let slice = u64::from(coex.phase0_share_percent)
            * u64::from(coex.interval)
            * u64::from(coex.current_period);
        self.slice_end = (now - offset)
            .checked_add(slice)
            .ok_or(PmScheduleError::DeadlineOverflow)?;
        let margin = if self.started {
            u64::from(SLEEP_DELAY_MICROS)
        } else {
            u64::from(IDLE_SLICE_MARGIN_MICROS)
        };
        self.cycle_remainder = cycle.get() - slice;
        disarm.write(PmAction::Disarm(PmTimer::SliceEnd));
        match self.slice_end.checked_sub(margin) {
            Some(deadline) if deadline > now => {
                self.slice_deadline = deadline;
                self.in_slice = true;
                arm.write(PmAction::ArmAt {
                    timer: PmTimer::SliceEnd,
                    deadline: Instant::from_micros(deadline),
                });
            }
            _ => {
                self.slice_deadline = now;
                self.in_slice = false;
                arm.write(PmAction::Due(PmTimer::SliceEnd));
            }
        }
        Ok(())
    }

    /// `pm_is_in_wifi_slice_threshold`: whether at least `threshold` of the
    /// Wi-Fi slice remains, and of the preemption gap when one runs.
    fn in_slice_threshold(
        &mut self,
        clock: PmClock,
        threshold: u32,
        coex: CoexView,
        places: Places<'_, 2>,
    ) -> Result<bool, PmScheduleError> {
        let [disarm, arm] = places;
        self.recalculate_slice(clock, false, coex, [disarm, arm])?;
        if !coex.active {
            return Ok(true);
        }
        let now = clock.now.as_micros();
        let threshold = u64::from(threshold);
        if now >= self.slice_deadline || self.slice_deadline.saturating_sub(now) < threshold {
            return Ok(false);
        }
        if self.preempted
            && (now >= self.preemption_end || self.preemption_end.saturating_sub(now) < threshold)
        {
            return Ok(false);
        }
        Ok(true)
    }

    /// `pm_send_nullfunc`: send a Null advertising `power_save`. The caller
    /// reports a frame it could not queue with [`Self::null_not_sent`].
    fn send_null(&mut self, power_save: bool, places: Places<'_, 1>) {
        let [send] = places;
        self.power_save_null_in_flight = power_save;
        self.active_null_in_flight = !power_save;
        send.write(PmAction::SendNull { power_save });
    }

    /// The Null of the last [`PmAction::SendNull`] could not be queued, so
    /// the in-flight flags return to their state before it, as the vendor
    /// does when its Null callback fails. The saved flags contain the whole
    /// restoration state; the failed Null's PM bit is not another input.
    pub fn null_not_sent(&mut self, before: PmNullFlags) {
        self.active_null_in_flight = before.active_in_flight;
        self.power_save_null_in_flight = before.power_save_in_flight;
    }

    /// The Null in-flight flags, to restore with [`Self::null_not_sent`].
    pub const fn null_flags(&self) -> PmNullFlags {
        PmNullFlags {
            active_in_flight: self.active_null_in_flight,
            power_save_in_flight: self.power_save_null_in_flight,
        }
    }

    /// `pm_go_to_sleep`: advertise power save.
    fn go_to_sleep(&mut self, coex: CoexView, places: Places<'_, 2>) {
        let [normalize, send] = places;
        if !self.may_sleep(coex, [normalize]) || self.state == PmState::Dozing {
            return;
        }
        self.enter(PmState::PowerSave);
        if !self.power_save_null_in_flight {
            self.send_null(true, [send]);
        }
    }

    /// `pm_go_to_wake`: leave power save when the slice allows it.
    ///
    /// Pending traffic wakes the station at once: its data frames carry
    /// power management zero. Otherwise, or in addition while enough of the
    /// slice remains, a Null leaving power save is sent. With `tx_wake` the
    /// caller is the TX path and the station always becomes awake.
    fn go_to_wake(
        &mut self,
        tx_wake: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 7>,
    ) -> Result<WakeResult, PmScheduleError> {
        let [wake, active0, active1, held, slice0, slice1, send] = places;
        if !self.started {
            return Ok(WakeResult::NotStarted);
        }
        if !self.in_slice {
            self.wake_deferred = true;
            return Ok(WakeResult::Deferred);
        }
        if self.state == PmState::Dozing {
            self.dream(coex, Some(clock), [wake]);
        }
        if self.active_null_in_flight || tx_wake {
            self.active_null_in_flight = true;
            self.active_wake_acknowledged([active0, active1, held]);
            return Ok(WakeResult::Awake);
        }
        let pending = traffic.connection_pending;
        if pending {
            self.active_null_in_flight = true;
            self.active_wake_acknowledged([active0, active1, held]);
        }
        if self.in_slice_threshold(clock, WAKE_SLICE_THRESHOLD_MICROS, coex, [slice0, slice1])? {
            self.send_null(false, [send]);
            return Ok(WakeResult::Waking);
        }
        if pending {
            return Ok(WakeResult::Awake);
        }
        self.wake_deferred = true;
        Ok(WakeResult::Deferred)
    }

    /// Complete an acknowledged active Null. Callers already woke a dozing
    /// RF. go_to_wake also established started + in_slice, so bypassing
    /// null_done's normalization emits no fewer effects: that normalization
    /// could only enter Awake here, which this transition itself does.
    fn active_wake_acknowledged(&mut self, [disarm, arm, held]: Places<'_, 3>) {
        self.active_null_failures = 0;
        if !self.active_null_in_flight {
            return;
        }
        self.active_null_in_flight = false;
        if self.in_slice {
            self.receiving = false;
            self.enter(PmState::Awake);
            self.enable_active_timer([disarm, arm]);
            held.write(PmAction::ReleaseHeldFrames);
        } else {
            self.wake_deferred = true;
        }
    }

    /// `pm_sleep`: doze when nothing keeps the station awake.
    fn sleep(&mut self, clock: PmClock, coex: CoexView, traffic: PmTraffic, places: Places<'_, 5>) {
        let [slice, group, beacon, priority, sleep] = places;
        let quiet = self.beacon_parsed
            && !self.beacon_expected
            && !self.receiving
            && !self.active_null_in_flight
            && !self.power_save_null_in_flight
            && self.started
            && self.wake_holds == 0;
        if !quiet {
            return;
        }
        let may = !traffic.tx_pending
            || (coex.active
                && (!self.in_slice
                    || (self.preempted && clock.now.as_micros() >= self.preemption_end)));
        if !may {
            return;
        }
        slice.write(PmAction::Coex(PmCoexAction::Release(PmCoexEvent::Slice)));
        group.write(PmAction::Coex(PmCoexAction::Release(
            PmCoexEvent::GroupTraffic,
        )));
        beacon.write(PmAction::Coex(PmCoexAction::Release(
            PmCoexEvent::BeaconWindow,
        )));
        priority.write(PmAction::ClearRxBeaconPriority);
        sleep.write(PmAction::RfSleep);
        self.enter(PmState::Dozing);
    }

    /// `pm_tx_null_data_done_process`: a Null completed.
    pub fn null_done(
        &mut self,
        power_save: bool,
        acknowledged: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
    ) -> Result<PmActions, PmScheduleError> {
        self.try_transition(clock, |this, [a, b, c, d, e, f, g, ..]| {
            this.null_done_into(
                power_save,
                acknowledged,
                clock,
                coex,
                traffic,
                [a, b, c, d, e, f, g],
            )
        })
        .map(|(_, actions)| actions)
    }

    fn null_done_into(
        &mut self,
        power_save: bool,
        acknowledged: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 7>,
    ) -> Result<(), PmScheduleError> {
        let [normalize, wake, a, b, c, d, e] = places;
        let succeeded =
            acknowledged || (!power_save && self.active_null_failures > NULL_RETRY_LIMIT);
        if !self.may_sleep(coex, [normalize]) && !self.started {
            return Ok(());
        }
        if !self.active_null_in_flight && !self.power_save_null_in_flight {
            return Ok(());
        }
        if succeeded {
            if self.state == PmState::Dozing {
                self.dream(coex, Some(clock), [wake]);
            }
            if !power_save {
                self.active_wake_acknowledged([a, b, c]);
                return Ok(());
            }
            self.power_save_null_failures = 0;
            if !self.power_save_null_in_flight {
                return Ok(());
            }
            self.power_save_null_in_flight = false;
            if self.preempted && clock.now.as_micros() >= self.preemption_end {
                self.sleep(clock, coex, traffic, [a, b, c, d, e]);
            } else {
                self.enable_sleep_delay_timer([a, b]);
            }
            return Ok(());
        }
        if !power_save {
            if !self.active_null_in_flight {
                return Ok(());
            }
            self.active_null_in_flight = false;
            if self.in_slice_threshold(clock, WAKE_SLICE_THRESHOLD_MICROS, coex, [a, b])? {
                self.active_null_failures = self.active_null_failures.saturating_add(1);
                self.send_null(false, [c]);
            } else {
                self.active_null_failures = 0;
                self.wake_deferred = true;
            }
            return Ok(());
        }
        if !self.power_save_null_in_flight {
            return Ok(());
        }
        self.power_save_null_in_flight = false;
        if self.in_slice {
            self.power_save_null_failures = self.power_save_null_failures.saturating_add(1);
            self.send_null(true, [a]);
        } else {
            self.power_save_null_failures = 0;
            self.wake_deferred = true;
            self.enable_sleep_delay_timer([a, b]);
        }
        Ok(())
    }

    /// `pm_tbtt_process` with `pm_coex_tbtt_process`, `pm_set_next_tbtt`
    /// and `pm_update_params`: the station TBTT event fired.
    pub fn tbtt(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
    ) -> Result<PmActions, PmScheduleError> {
        self.try_transition(clock, |this, places| {
            this.tbtt_into(clock, coex, traffic, places)
        })
        .map(|(_, actions)| actions)
    }

    fn tbtt_into(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 24>,
    ) -> Result<(), PmScheduleError> {
        let [
            normalize,
            wake,
            c0,
            c1,
            c2,
            c3,
            c4,
            c5,
            c6,
            c7,
            c8,
            c9,
            c10,
            c11,
            c12,
            next,
            u0,
            u1,
            u2,
            u3,
            u4,
            u5,
            u6,
            u7,
        ] = places;
        self.normalize_sleep_state(coex, [normalize]);
        if !self.started {
            c0.write(PmAction::Disarm(PmTimer::SliceEnd));
            self.in_slice = true;
            c1.write(PmAction::UnblockTx);
            return Ok(());
        }
        let beacon_was_expected = self.beacon_expected;
        self.beacon_expected = true;
        if self.state == PmState::Dozing {
            self.dream(coex, Some(clock), [wake]);
            self.receiving = false;
        }
        // The cycle counts from the handling of this TBTT unless the
        // associated station missed the previous beacon, or a beacon was
        // still expected when its slice ended under coexistence.
        //
        // SOURCE(esp32s31): complete pinned `libpp[pm.o]::pm_tbtt_process`
        // stores the MAC local time (`WIFI_MAC_LOCAL_TIME`, 32 bits) read
        // while it handles the TBTT event as the 64-bit anchor at `g_pm[96]`
        // (high word zero); `libpp[pm_coex.o]::pm_coex_recalculate_wifi_time_slice` places the station at
        // `(WIFI_MAC_LOCAL_TIME - anchor) % (overall period * interval * 100)`.
        // The anchor is the handling of the event, not the TBTT itself.
        // The ROM's copy of the TBTT handler is not reviewed.
        //
        // Known difference: the vendor counts in the MAC local time, this
        // model in monotonic time. Both count the crystal's microseconds, so
        // the distances agree while the MAC counter runs. It runs through
        // the RF sleeps of both modem-sleep depths: hardware runs at DTIM 1
        // and DTIM 3 with min-modem (353 sleeps of 41 to 304 ms) and with
        // max-modem at listen interval 10 (256 sleeps of 41 to 99 ms) found
        // the MAC and monotonic distances within 1 µs. Those max-modem sleeps
        // were taken while this model still woke max-modem for every beacon;
        // sleeping the scaled listen interval (`scale_listen_interval`), a
        // max-modem run at listen interval 10, DTIM 1 found 58 sleeps of 941
        // to 1021 ms within 1 µs too. Light sleep, where the chip's power
        // domains may switch off, is not measured: equivalence there needs
        // the same measurement. The vendor's subtraction
        // of the 32-bit counter from the 64-bit anchor misplaces the station
        // once the counter wraps (every ~71.6 minutes); that is a vendor
        // defect and is not reproduced.
        let keep_anchor = beacon_was_expected || (coex.active && self.beacon_expected_at_slice_end);
        if !keep_anchor {
            self.cycle_anchor = clock.now.as_micros();
        }
        self.coex_tbtt(
            clock,
            coex,
            traffic,
            [c0, c1, c2, c3, c4, c5, c6, c7, c8, c9, c10, c11, c12],
        )?;
        self.set_next_tbtt(coex, [next]);
        self.update_params(clock, coex, traffic, [u0, u1, u2, u3, u4, u5, u6, u7])?;
        Ok(())
    }

    /// `pm_coex_tbtt_process`.
    #[cfg_attr(
        not(test),
        expect(
            clippy::arithmetic_side_effects,
            reason = "u8 period * u32 beacon interval fits u64; absolute time uses checked_add"
        )
    )]
    fn coex_tbtt(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 13>,
    ) -> Result<(), PmScheduleError> {
        let [
            disarm,
            interval,
            restart,
            priority,
            request,
            unblock,
            w0,
            w1,
            w2,
            w3,
            w4,
            w5,
            w6,
        ] = places;
        disarm.write(PmAction::Disarm(PmTimer::SliceEnd));
        self.in_slice = true;
        if coex.active {
            self.slice_deadline = clock
                .now
                .as_micros()
                .checked_add(
                    u64::from(coex.overall_period()) * u64::from(self.beacon_interval_micros.get()),
                )
                .ok_or(PmScheduleError::DeadlineOverflow)?;
            interval.write(PmAction::Coex(PmCoexAction::SetInterval(
                self.beacon_interval_micros.get() / SCHEDULE_INTERVAL_UNIT_MICROS,
            )));
            restart.write(PmAction::Coex(PmCoexAction::RestartPhases));
            priority.write(PmAction::RxBeaconPriority(true));
            request.write(PmAction::Coex(PmCoexAction::Request {
                event: PmCoexEvent::BeaconWindow,
                duration_micros: self.beacon_window_micros,
            }));
            self.activity = 0;
            if !self.is_sleeping() {
                unblock.write(PmAction::UnblockTx);
            }
            if self.sleep_type == SleepType::None || self.wake_deferred {
                self.wake_deferred = false;
                self.go_to_wake(false, clock, coex, traffic, [w0, w1, w2, w3, w4, w5, w6])?;
            }
            return Ok(());
        }
        if !self.is_sleeping() {
            unblock.write(PmAction::UnblockTx);
            if traffic.connection_pending {
                self.go_to_wake(true, clock, coex, traffic, [w0, w1, w2, w3, w4, w5, w6])?;
            }
        }
        Ok(())
    }

    /// `pm_set_next_tbtt`: follow a changed schedule period.
    #[cfg_attr(
        not(test),
        expect(
            clippy::arithmetic_side_effects,
            reason = "u8 period * u32 beacon interval fits u64"
        )
    )]
    fn set_next_tbtt(&mut self, coex: CoexView, places: Places<'_, 1>) {
        let [interval] = places;
        let period = coex.overall_period();
        if !coex.active || self.tbtt_period == period {
            return;
        }
        self.tbtt_period = period;
        interval.write(PmAction::SetTbttInterval(
            u64::from(period) * u64::from(self.beacon_interval_micros.get()),
        ));
        self.update_tbtt_at_next_beacon = true;
    }

    /// `pm_update_params`.
    #[cfg_attr(
        not(test),
        expect(
            clippy::arithmetic_side_effects,
            reason = "u8 * u32 fits u64; the two fixed wake leads sum to at most 5000"
        )
    )]
    fn update_params(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 8>,
    ) -> Result<(), PmScheduleError> {
        let [set_ahead, w0, w1, w2, w3, w4, w5, w6] = places;
        let (active_timeout, ahead) = if coex.active {
            (
                u64::from(coex.overall_period()) * u64::from(self.beacon_interval_micros.get()),
                SHARED_TBTT_AHEAD_MICROS,
            )
        } else {
            (u64::from(ACTIVE_TIMEOUT_MICROS), TBTT_AHEAD_MICROS)
        };
        self.active_timeout_micros = active_timeout;
        if self.tbtt_ahead_micros != ahead || self.tbtt_window_micros != TBTT_WAKE_WINDOW_MICROS {
            self.tbtt_ahead_micros = ahead;
            self.tbtt_window_micros = TBTT_WAKE_WINDOW_MICROS;
            set_ahead.write(PmAction::SetTbttAhead {
                ahead_micros: ahead,
                wake_ahead_micros: TBTT_WAKE_WINDOW_MICROS + ahead,
            });
        }
        if coex.active != self.coex_was_active {
            self.update_tbtt_at_next_beacon = true;
            // A station without power save that slept only for coexistence
            // wakes when coexistence ends.
            if self.coex_was_active && self.sleep_type == SleepType::None {
                self.go_to_wake(false, clock, coex, traffic, [w0, w1, w2, w3, w4, w5, w6])?;
            }
            self.coex_was_active = coex.active;
        }
        Ok(())
    }

    /// `pm_on_beacon_rx` with `pm_parse_beacon`, `pm_update_next_tbtt`,
    /// `pm_rx_beacon_process` and `pm_process_tim`: a beacon of the
    /// associated access point arrived.
    pub fn beacon(
        &mut self,
        beacon: PmBeacon,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
    ) -> Result<PmActions, PmScheduleError> {
        self.try_transition(
            clock,
            |this, [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p, ..]| {
                this.beacon_into(
                    beacon,
                    clock,
                    coex,
                    traffic,
                    [a, b, c, d, e, f, g, h, i, j, k, l, m, n, o, p],
                )
            },
        )
        .map(|(_, actions)| actions)
    }

    fn beacon_into(
        &mut self,
        beacon: PmBeacon,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 16>,
    ) -> Result<(), PmScheduleError> {
        let [
            p0,
            p1,
            p2,
            tbtt,
            t0,
            t1,
            t2,
            t3,
            t4,
            t5,
            t6,
            t7,
            t8,
            t9,
            release,
            priority,
        ] = places;
        if !self.started {
            return Ok(());
        }
        self.parse_beacon(beacon, coex, [p0, p1, p2]);
        self.update_next_tbtt(beacon, coex, [tbtt]);
        self.beacon_expected_at_slice_end = false;
        self.beacon_expected = false;
        let tim = beacon.tim;
        self.process_tim(
            tim.is_some_and(|tim| tim.unicast),
            tim.is_some_and(|tim| tim.group),
            clock,
            coex,
            traffic,
            [t0, t1, t2, t3, t4, t5, t6, t7, t8, t9],
        )?;
        release.write(PmAction::Coex(PmCoexAction::Release(
            PmCoexEvent::BeaconWindow,
        )));
        priority.write(PmAction::ClearRxBeaconPriority);
        Ok(())
    }

    /// `pm_parse_beacon`.
    fn parse_beacon(&mut self, beacon: PmBeacon, coex: CoexView, places: Places<'_, 3>) {
        let [scale, set_interval, dtim] = places;
        let interval = beacon_period(beacon.interval_tu);
        let dtim_period = beacon
            .tim
            .map_or(self.dtim_period, |tim| tim.dtim_period.max(1));
        if !self.beacon_parsed {
            self.beacon_interval_micros = interval;
            self.dtim_period = dtim_period;
            self.scale_listen_interval(coex, [scale]);
            if coex.active {
                set_interval.write(PmAction::Coex(PmCoexAction::SetInterval(
                    interval.get() / SCHEDULE_INTERVAL_UNIT_MICROS,
                )));
            }
            self.beacon_parsed = true;
            self.update_tbtt_at_next_beacon = true;
            return;
        }
        if self.beacon_interval_micros != interval {
            self.beacon_interval_micros = interval;
            self.scale_listen_interval(coex, [scale]);
            if coex.active {
                set_interval.write(PmAction::Coex(PmCoexAction::SetInterval(
                    interval.get() / SCHEDULE_INTERVAL_UNIT_MICROS,
                )));
            }
            self.update_tbtt_at_next_beacon = true;
        }
        if self.dtim_period != dtim_period {
            self.dtim_period = dtim_period;
            self.scale_listen_interval(coex, [dtim]);
            if self.sleep_type != SleepType::None {
                self.update_tbtt_at_next_beacon = true;
            }
        }
    }

    /// `pm_scale_listen_interval`: the configured listen interval, counted
    /// in 100-TU units, as a number of this access point's beacons, rounded
    /// to the nearest multiple of the DTIM period (at least one DTIM) or,
    /// below one DTIM, to the nearest divisor of it; a tie rounds up. A
    /// change under max-modem reprograms the TBTT at the next beacon.
    ///
    /// SOURCE(esp32s31): complete pinned `libpp[pm.o]::pm_scale_listen_interval`
    /// at esp32-wifi-lib `af55a0ca`: `wifi_nvs_get_sta_listen_interval() *
    /// 102400 / g_pm[52]` truncated to 16 bits (0 becomes 1), the rounding
    /// against `g_pm[11]`, and on a change the store to `g_pm[34]` with,
    /// for sleep type 2, `g_pm[18] = 1` and the coexistence power update
    /// (this model's `coex_pwr_update`, which keeps the flexible period at
    /// one: the vendor's sets it to the listen interval or the DTIM when its
    /// configuration byte `g_pm_cfg[86]` is set, not modelled). Its tail
    /// call `pm_on_sample_beacon` serves the beacon-offset feature, which
    /// this model does not run. Callers here: `pm_start` (DTIM 1) and
    /// `libpp[pm.o]::pm_parse_beacon` (first beacon, beacon interval or
    /// DTIM change).
    /// The vendor also rescales when a probe response carries a new beacon
    /// interval (`pm_on_probe_resp_rx`, which this model does not handle)
    /// and when the listen interval is reconfigured while associated
    /// (`ic_update_listen_interval`; here the power mode is fixed for a
    /// whole association).
    ///
    /// Intentional arithmetic difference: retain the full u64 product and
    /// beacon count. The vendor's u32 multiplication and u16 truncation are
    /// not valid for this monotonic schedule. Backend projection is checked.
    ///
    /// The ROM carries its own copies, which this model does not follow.
    /// Its scaling computes the same value and store, then calls two of its
    /// function-table entries (offsets 1428, then 1472 as a tail call)
    /// whatever the outcome, where the library copy runs the power update
    /// only on a change under max-modem. Vendor builds link the library
    /// copy, whose definition takes precedence over a ROM symbol the linker
    /// script only provides (not checked against the pinned script). The
    /// ROM's beacon parser, which calls through such tables, is not
    /// reviewed.
    fn scale_listen_interval(&mut self, coex: CoexView, places: Places<'_, 1>) {
        let [power] = places;
        let scaled = scaled_listen_interval(
            self.sleep_type.listen_interval().get(),
            self.beacon_interval_micros,
            self.dtim_period,
        );
        if scaled == self.listen_interval_beacons {
            return;
        }
        self.listen_interval_beacons = scaled;
        if matches!(self.sleep_type, SleepType::MaxModem(_)) {
            self.update_tbtt_at_next_beacon = true;
            self.coex_pwr_update(coex, [power]);
        }
    }

    /// `pm_handle_tbtt_interval`: a TBTT interval of `beacons` beacon
    /// intervals kept in step with the DTIM. When the DTIM is not yet
    /// aligned with that period, the interval first runs up to the DTIM
    /// and is re-derived at the next beacon.
    #[cfg_attr(
        not(test),
        expect(
            clippy::arithmetic_side_effects,
            reason = "private callers pass at most u16::MAX * 102400 + 255 beacons; their u32-period product fits u64"
        )
    )]
    fn handle_tbtt_interval(&mut self, beacons: u64, tim: Option<PmTim>) -> u64 {
        let Some(tim) = tim else {
            self.update_tbtt_at_next_beacon = true;
            return u64::from(self.beacon_interval_micros.get());
        };
        let beacons = core::num::NonZeroU64::new(beacons).unwrap_or(core::num::NonZeroU64::MIN);
        let period = u64::from(tim.dtim_period.max(1));
        let compatible =
            beacons.get().is_multiple_of(period) || period.is_multiple_of(beacons.get());
        let offset = u64::from(tim.dtim_count) % beacons;
        if !compatible || offset == 0 {
            return beacons.get() * u64::from(self.beacon_interval_micros.get());
        }
        self.update_tbtt_at_next_beacon = true;
        offset * u64::from(self.beacon_interval_micros.get())
    }

    /// `pm_update_next_tbtt`: program the TBTT schedule from this beacon.
    #[cfg_attr(
        not(test),
        expect(
            clippy::arithmetic_side_effects,
            reason = "the two fixed wake leads sum to at most 5000"
        )
    )]
    fn update_next_tbtt(&mut self, beacon: PmBeacon, coex: CoexView, places: Places<'_, 1>) {
        let [tbtt] = places;
        if !self.update_tbtt_at_next_beacon {
            return;
        }
        self.update_tbtt_at_next_beacon = false;
        let ahead = if coex.active {
            SHARED_TBTT_AHEAD_MICROS
        } else {
            TBTT_AHEAD_MICROS
        };
        self.tbtt_ahead_micros = ahead;
        self.tbtt_window_micros = TBTT_WAKE_WINDOW_MICROS;
        let interval = if coex.active {
            self.handle_tbtt_interval(u64::from(coex.overall_period()), beacon.tim)
        } else {
            match self.sleep_type {
                SleepType::MinModem => {
                    self.handle_tbtt_interval(u64::from(self.dtim_period), beacon.tim)
                }
                SleepType::MaxModem(_) => {
                    self.handle_tbtt_interval(self.listen_interval_beacons, beacon.tim)
                }
                SleepType::None => u64::from(self.beacon_interval_micros.get()),
            }
        };
        let timestamp = beacon.timestamp_tsf.as_micros();
        let phase = timestamp % core::num::NonZeroU64::from(self.beacon_interval_micros);
        tbtt.write(PmAction::StartTbtt(PmTbttSchedule {
            first_tbtt: TsfInstant::from_micros(timestamp.saturating_sub(phase)),
            interval_micros: interval,
            ahead_micros: ahead,
            wake_ahead_micros: TBTT_WAKE_WINDOW_MICROS + ahead,
        }));
    }

    /// `pm_process_tim`.
    #[cfg_attr(
        not(test),
        expect(
            clippy::arithmetic_side_effects,
            reason = "the branch establishes slice_end > now before subtraction"
        )
    )]
    fn process_tim(
        &mut self,
        unicast: bool,
        group: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 10>,
    ) -> Result<(), PmScheduleError> {
        let [a, b, c, d, e, f, g, h, i, j] = places;
        match self.state {
            PmState::PowerSave => {
                if unicast {
                    if self.in_slice_threshold(clock, WAKE_SLICE_THRESHOLD_MICROS, coex, [a, b])? {
                        self.receiving = true;
                        self.request_slice_until_end(clock.now.as_micros(), [c])?;
                        self.go_to_wake(false, clock, coex, traffic, [d, e, f, g, h, i, j])?;
                    } else {
                        self.wake_deferred = true;
                    }
                } else if group {
                    if self.in_slice_threshold(clock, GROUP_SLICE_THRESHOLD_MICROS, coex, [a, b])? {
                        self.receiving = true;
                        self.enable_dream_timer([c, d]);
                        let now = clock.now.as_micros();
                        if now < self.slice_end {
                            // The beacon-window bound fits u32; cap the physical
                            // request to that window before projecting it.
                            let remaining = (self.slice_end - now)
                                .min(u64::from(self.beacon_window_micros))
                                as u32;
                            e.write(PmAction::Coex(PmCoexAction::Request {
                                event: PmCoexEvent::GroupTraffic,
                                duration_micros: remaining.min(self.beacon_window_micros),
                            }));
                        }
                    }
                } else {
                    self.receiving = false;
                    if !self.active_null_in_flight && !self.power_save_null_in_flight {
                        self.disable_sleep_delay_timer([a]);
                        self.sleep(clock, coex, traffic, [b, c, d, e, f]);
                    }
                }
            }
            PmState::Awake | PmState::Dozing => {}
        }
        Ok(())
    }

    /// `pm_tx_data_process`: the TX path offers a frame. Returns whether the
    /// frame must wait for the station to leave power save, together with
    /// this input's complete ordered effects.
    #[must_use = "honor the hold decision and perform this input's effects"]
    pub fn tx_data(
        &mut self,
        is_data: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
    ) -> Result<(bool, PmActions), PmScheduleError> {
        self.try_transition(clock, |this, [a, b, c, d, e, f, g, h, i, j, k, l, ..]| {
            this.tx_data_into(
                is_data,
                clock,
                coex,
                traffic,
                [a, b, c, d, e, f, g, h, i, j, k, l],
            )
        })
    }

    fn tx_data_into(
        &mut self,
        is_data: bool,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 12>,
    ) -> Result<bool, PmScheduleError> {
        let [normalize, a, b, c, d, e, f, g, h, i, j, k] = places;
        if !self.may_sleep(coex, [normalize]) {
            return Ok(false);
        }
        match self.state {
            PmState::PowerSave | PmState::Dozing => {
                let dozing = self.state == PmState::Dozing;
                let mut hold = false;
                if self.in_slice_threshold(clock, WAKE_SLICE_THRESHOLD_MICROS, coex, [a, b])? {
                    self.request_slice_until_end(clock.now.as_micros(), [c])?;
                    if dozing {
                        self.dream(coex, Some(clock), [d]);
                    }
                    if is_data {
                        let woke =
                            self.go_to_wake(true, clock, coex, traffic, [e, f, g, h, i, j, k])?;
                        hold = matches!(woke, WakeResult::NotStarted | WakeResult::Deferred);
                    }
                } else if is_data {
                    self.wake_deferred = true;
                    hold = true;
                }
                if is_data {
                    self.incr_activity();
                }
                Ok(hold)
            }
            PmState::Awake => {
                if is_data {
                    if self.sleep_type != SleepType::None {
                        self.enable_active_timer([a, b]);
                    }
                    self.incr_activity();
                }
                Ok(false)
            }
        }
    }

    /// `pm_tx_data_done_process`: a data frame completed.
    pub fn tx_data_done(
        &mut self,
        acknowledged: bool,
        coex: CoexView,
        traffic: PmTraffic,
    ) -> PmActions {
        self.transition(|this, [a, b, c, d, e, ..]| {
            this.tx_data_done_into(acknowledged, coex, traffic, [a, b, c, d, e])
        })
        .1
    }

    fn tx_data_done_into(
        &mut self,
        acknowledged: bool,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 5>,
    ) {
        let [delay0, delay1, normalize, active0, active1] = places;
        if !traffic.tx_pending && self.state == PmState::PowerSave {
            self.enable_sleep_delay_timer([delay0, delay1]);
        }
        if !self.may_sleep(coex, [normalize]) || !acknowledged {
            return;
        }
        self.incr_activity();
        if !self.in_slice {
            self.wake_deferred = true;
        }
        if self.state == PmState::Awake {
            self.enable_active_timer([active0, active1]);
        }
    }

    /// `pm_rx_data_process`: a data frame of the access point arrived.
    pub fn rx_data(&mut self, group: bool, more_data: bool, coex: CoexView) -> PmActions {
        self.transition(|this, [a, b, c, d, ..]| {
            this.rx_data_into(group, more_data, coex, [a, b, c, d])
        })
        .1
    }

    fn rx_data_into(
        &mut self,
        group: bool,
        more_data: bool,
        coex: CoexView,
        places: Places<'_, 4>,
    ) {
        let [normalize, a, b, c] = places;
        if !self.may_sleep(coex, [normalize]) {
            return;
        }
        self.incr_activity();
        match self.state {
            PmState::PowerSave => {
                if !self.in_slice {
                    return;
                }
                if group {
                    if more_data {
                        self.receiving = true;
                        self.enable_dream_timer([a, b]);
                    } else {
                        self.receiving = false;
                        a.write(PmAction::Disarm(PmTimer::Dream));
                        self.enable_sleep_delay_timer([b, c]);
                    }
                } else {
                    self.receiving = more_data;
                }
            }
            PmState::Awake => self.enable_active_timer([a, b]),
            PmState::Dozing => {}
        }
    }

    /// `pm_coex_schm_process`: the schedule entered a phase that notifies
    /// Wi-Fi.
    #[cfg_attr(
        not(test),
        expect(
            clippy::arithmetic_side_effects,
            reason = "validated percentage <= 100 times u8 and u32 fits u64"
        )
    )]
    pub fn coex_phase(
        &mut self,
        phase: CoexPhaseView,
        clock: PmClock,
        coex: CoexView,
    ) -> Result<PmActions, PmScheduleError> {
        // Validate the complete schedule before changing state or releasing
        // any effect. The bounded source fields make these products fit u64.
        if phase.share_percent > 100 {
            return Err(PmScheduleError::InvalidSharePercent(phase.share_percent));
        }
        let slice = u64::from(phase.share_percent)
            * u64::from(coex.interval)
            * u64::from(coex.current_period);
        let slice_end = clock
            .now
            .as_micros()
            .checked_add(slice)
            .ok_or(PmScheduleError::DeadlineOverflow)?;
        let duration_micros = u32::try_from(slice)
            .map_err(|_| PmScheduleError::RequestDurationOutOfRange { micros: slice })?;
        let margin = if self.started {
            SLEEP_DELAY_MICROS
        } else {
            IDLE_SLICE_MARGIN_MICROS
        };
        let deadline = match slice_end.checked_sub(u64::from(margin)) {
            Some(deadline) if deadline > clock.now.as_micros() => Some(deadline),
            _ => None,
        };
        Ok(self
            .transition(|this, [a, b, c, d, e, f, ..]| {
                this.coex_phase_into(
                    phase,
                    clock,
                    coex,
                    PhaseTiming {
                        slice_end,
                        duration_micros,
                        deadline,
                    },
                    [a, b, c, d, e, f],
                )
            })
            .1)
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::arithmetic_side_effects,
            reason = "validated share <= 100 and current_period <= overall_period establish slice <= cycle"
        )
    )]
    fn coex_phase_into(
        &mut self,
        phase: CoexPhaseView,
        clock: PmClock,
        coex: CoexView,
        timing: PhaseTiming,
        places: Places<'_, 6>,
    ) {
        let [request, unblock, disarm, arm, shared, block] = places;
        let PhaseTiming {
            slice_end,
            duration_micros,
            deadline,
        } = timing;
        let now = clock.now.as_micros();
        let slice = u64::from(duration_micros);
        if phase.wifi & CoexPhaseView::WIFI_SLICE != 0 {
            request.write(PmAction::Coex(PmCoexAction::Request {
                event: PmCoexEvent::Slice,
                duration_micros,
            }));
            self.in_slice = true;
            self.slice_end = slice_end;
            // share <= 100 and current_period <= overall_period.
            self.cycle_remainder = coex.cycle_micros() - slice;
            if !self.is_sleeping() {
                unblock.write(PmAction::UnblockTx);
            }
        }
        if phase.wifi & CoexPhaseView::SLICE_END != 0 {
            disarm.write(PmAction::Disarm(PmTimer::SliceEnd));
            match deadline {
                Some(deadline) => {
                    self.slice_deadline = deadline;
                    arm.write(PmAction::ArmAt {
                        timer: PmTimer::SliceEnd,
                        deadline: Instant::from_micros(deadline),
                    });
                }
                None => {
                    self.slice_deadline = now;
                    arm.write(PmAction::Due(PmTimer::SliceEnd));
                }
            }
        }
        if phase.wifi & CoexPhaseView::SHARED != 0 {
            if !self.is_sleeping() {
                shared.write(PmAction::Coex(PmCoexAction::Request {
                    event: PmCoexEvent::Slice,
                    duration_micros,
                }));
            }
            block.write(PmAction::BlockTx);
        }
    }

    /// `pm_coex_go_to_sleep`: the slice ended, or a preemption did.
    fn coex_go_to_sleep(
        &mut self,
        from_slice_end: bool,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 3>,
    ) {
        let [block, a, b] = places;
        if !self.started || !coex.active {
            return;
        }
        if from_slice_end {
            self.in_slice = false;
        }
        self.beacon_expected_at_slice_end = self.beacon_expected;
        self.beacon_expected = false;
        self.receiving = false;
        block.write(PmAction::BlockTx);
        if self.state != PmState::Awake && !self.active_null_in_flight {
            if self.state == PmState::PowerSave {
                self.enable_sleep_delay_timer([a, b]);
            }
            return;
        }
        if self.activity != 0 || traffic.connection_pending {
            self.wake_deferred = true;
        }
        self.go_to_sleep(coex, [a, b]);
    }

    /// The slice-end timer expired (`pm_coex_slice_timeout_process`).
    pub fn slice_end_timer(&mut self, coex: CoexView, traffic: PmTraffic) -> PmActions {
        self.transition(|this, [a, b, c, ..]| this.slice_end_timer_into(coex, traffic, [a, b, c]))
            .1
    }

    fn slice_end_timer_into(&mut self, coex: CoexView, traffic: PmTraffic, places: Places<'_, 3>) {
        let [block, a, b] = places;
        self.coex_go_to_sleep(true, coex, traffic, [block, a, b]);
    }

    /// The preemption timer expired (`pm_coex_preemption_timeout_process`).
    pub fn preemption_timer(&mut self, coex: CoexView, traffic: PmTraffic) -> PmActions {
        self.transition(|this, [a, b, c, ..]| this.preemption_timer_into(coex, traffic, [a, b, c]))
            .1
    }

    fn preemption_timer_into(&mut self, coex: CoexView, traffic: PmTraffic, places: Places<'_, 3>) {
        let [block, a, b] = places;
        self.coex_go_to_sleep(false, coex, traffic, [block, a, b]);
    }

    /// The active timer expired (`pm_active_timeout_process`).
    pub fn active_timer(&mut self, coex: CoexView, traffic: PmTraffic) -> PmActions {
        self.transition(|this, [a, b, ..]| this.active_timer_into(coex, traffic, [a, b]))
            .1
    }

    fn active_timer_into(&mut self, coex: CoexView, traffic: PmTraffic, places: Places<'_, 2>) {
        let [a, b] = places;
        if !self.started || self.state != PmState::Awake {
            return;
        }
        if self.receiving || traffic.tx_pending {
            self.enable_active_timer([a, b]);
            return;
        }
        self.go_to_sleep(coex, [a, b]);
    }

    /// The sleep-delay timer expired (`pm_sleep_delay_timeout_process`).
    pub fn sleep_delay_timer(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
    ) -> PmActions {
        self.transition(|this, [a, b, c, d, e, ..]| {
            this.sleep_delay_timer_into(clock, coex, traffic, [a, b, c, d, e])
        })
        .1
    }

    fn sleep_delay_timer_into(
        &mut self,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 5>,
    ) {
        let [a, b, c, d, e] = places;
        self.sleep_delay_armed = false;
        if self.state == PmState::PowerSave {
            self.sleep(clock, coex, traffic, [a, b, c, d, e]);
        }
    }

    /// The dream timer expired (`pm_dream_timeout_process`).
    pub fn dream_timer(&mut self) -> PmActions {
        self.transition(|this, [a, b, ..]| this.dream_timer_into([a, b]))
            .1
    }

    fn dream_timer_into(&mut self, places: Places<'_, 2>) {
        let [disarm, arm] = places;
        self.receiving = false;
        self.enable_sleep_delay_timer([disarm, arm]);
    }

    /// `pm_on_coex_preemption_end`: Bluetooth reports when its isochronous
    /// preemption ends, or that the end is unknown.
    pub fn preemption_end(
        &mut self,
        end_micros: Option<u64>,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
    ) -> Result<PmActions, PmScheduleError> {
        self.try_transition(
            clock,
            |this, [a, b, c, d, e, f, g, h, i, j, k, l, m, n, ..]| {
                this.preemption_end_into(
                    end_micros,
                    clock,
                    coex,
                    traffic,
                    [a, b, c, d, e, f, g, h, i, j, k, l, m, n],
                )
            },
        )
        .map(|(_, actions)| actions)
    }

    fn preemption_end_into(
        &mut self,
        end_micros: Option<u64>,
        clock: PmClock,
        coex: CoexView,
        traffic: PmTraffic,
        places: Places<'_, 14>,
    ) -> Result<(), PmScheduleError> {
        let [disarm, arm, a, b, c, d, e, f, g, h, i, j, k, set_time] = places;
        let now = clock.now.as_micros();
        let was_preempted = self.preempted;
        self.preempted = end_micros.is_some();
        let margin = if self.started { 2_000 } else { 1_000 };
        // An expired margin is due in this input, including before clock
        // origin. The branch below executes the timeout when awake.
        self.preemption_end = match end_micros.and_then(|end| end.checked_sub(margin)) {
            Some(deadline) if deadline > now => deadline,
            _ => now,
        };
        disarm.write(PmAction::Disarm(PmTimer::Preemption));
        if self.preempted && now < self.preemption_end {
            arm.write(PmAction::ArmAt {
                timer: PmTimer::Preemption,
                deadline: Instant::from_micros(self.preemption_end),
            });
        }
        if self.started {
            if self.state != PmState::Awake {
                if self.wake_deferred && self.in_slice_threshold(clock, 2_000, coex, [a, b])? {
                    self.wake_deferred = false;
                    if self.state == PmState::Dozing {
                        self.dream(coex, Some(clock), [c]);
                    }
                    self.go_to_wake(false, clock, coex, traffic, [d, e, f, g, h, i, j])?;
                    self.request_slice_until_end(now, [k])?;
                } else {
                    c.write(PmAction::Disarm(PmTimer::Preemption));
                }
            } else if self.preempted && now >= self.preemption_end {
                self.preemption_timer_into(coex, traffic, [a, b, c]);
            }
        }
        if self.preempted != was_preempted {
            let (window, time) = if self.preempted {
                (PREEMPTED_BEACON_WINDOW_MICROS, PREEMPTED_BEACON_TIME_MICROS)
            } else {
                (BEACON_WINDOW_MICROS, BEACON_TIME_MICROS)
            };
            self.beacon_window_micros = window;
            set_time.write(PmAction::RxBeaconTime {
                window_micros: window,
                time_micros: time,
            });
        }
        Ok(())
    }

    /// Request the slice until its end when it has not ended.
    #[cfg_attr(
        not(test),
        expect(
            clippy::arithmetic_side_effects,
            reason = "the branch establishes slice_end > now before subtraction"
        )
    )]
    fn request_slice_until_end(
        &mut self,
        now: u64,
        places: Places<'_, 1>,
    ) -> Result<(), PmScheduleError> {
        let [request] = places;
        if now < self.slice_end {
            request.write(PmAction::Coex(PmCoexAction::Request {
                event: PmCoexEvent::Slice,
                duration_micros: u32::try_from(self.slice_end - now).map_err(|_| {
                    PmScheduleError::RequestDurationOutOfRange {
                        micros: self.slice_end - now,
                    }
                })?,
            }));
        }
        Ok(())
    }
}

/// `pm_scale_listen_interval`'s arithmetic: `listen_interval` 100-TU units
/// in beacons of `beacon_interval_micros`, rounded to the DTIM period.
#[cfg_attr(
    not(test),
    expect(
        clippy::arithmetic_side_effects,
        reason = "u16 * 102400 + u8 fits u64; divisor is nonzero; down <= beacons <= up"
    )
)]
fn scaled_listen_interval(
    listen_interval: u16,
    beacon_interval_micros: core::num::NonZeroU32,
    dtim_period: u8,
) -> u64 {
    // The configured u16 count is in 100 TU units. Keep the product and
    // the rounded beacon count wide, even with a one-microsecond divisor.
    let raw = u64::from(listen_interval) * 102_400 / u64::from(beacon_interval_micros.get());
    let beacons = raw.max(1);
    let dtim =
        core::num::NonZeroU64::new(u64::from(dtim_period)).unwrap_or(core::num::NonZeroU64::MIN);
    let (down, up) = if beacons >= dtim.get() {
        let down = beacons.saturating_sub(beacons % dtim);
        (down, down + dtim.get())
    } else {
        let up = (beacons..=dtim.get())
            .find(|divisor| dtim.get().checked_rem(*divisor) == Some(0))
            .unwrap_or(dtim.get());
        let down = (1..=beacons)
            .rev()
            .find(|divisor| dtim.get().checked_rem(*divisor) == Some(0))
            .unwrap_or(1);
        (down, up)
    };
    if beacons.saturating_sub(down) < up - beacons {
        down
    } else {
        up
    }
}

#[cfg(test)]
mod tests;

//! The report block W1 prints after its exit, its gates and its verdict.
//!
//! The block runs from `OER-W1-BEGIN v=1` to `OER-W1-END lines=<n>`, one
//! `keyword key=value …` line each, values without spaces; `cargo fw`
//! recomputes every gate from the block's other lines (`tools/image/cli/src/w1.rs`
//! holds its spec). BSSIDs are masked to their last two octets, SSIDs are
//! printed only as their length and, per BSS, whether the stored SSID is the
//! stand's (`ssid_ok=`).

use oer_diagnostics::Note;
use oer_esp32s31_hal::ieee80211::init::MAC_STATION_INTERRUPT_MASK;
use oer_esp32s31_ieee80211_driver::rx::{
    control::{WifiRxCause, WifiRxEnd, WifiRxExit, WifiRxQuarantine},
    counters::{WifiRxExhaustion, WifiRxReport},
    receive::RELOAD_SETTLE_LIMIT,
};
use oer_espressif_executor::poll_timing::CycleHistogram;
use oer_ieee80211_runtime::survey::{BssTally, Survey};
use oer_ieee80211_wire::address::Bssid;

use crate::{
    irq::{BUFFER, COUNT, SLOTS},
    measure,
    measure::{CLOCKS, ClockOp, Phase, TIMED_PHASES, Tick, TimerLine},
    own::{CYCLES, Cycle},
};

/// The survey's table size.
pub const BSS: usize = 64;
/// Microseconds per time unit.
const TU_US: u64 = 1024;

/// The hold: the leases held, and what the receive counters moved while
/// every one of them was held.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Hold {
    pub reached: usize,
    pub units: u32,
    pub mac_irqs: u32,
    pub stage_capacity_blocks: u32,
    pub admission_blocks: u32,
}

impl Hold {
    /// `reached` leases held from `before` to `after`.
    pub fn between(reached: usize, before: &WifiRxReport, after: &WifiRxReport) -> Self {
        Self {
            reached,
            units: after.units.wrapping_sub(before.units),
            mac_irqs: after.mac_irqs.wrapping_sub(before.mac_irqs),
            stage_capacity_blocks: after
                .stage_capacity_blocks
                .wrapping_sub(before.stage_capacity_blocks),
            admission_blocks: after.admission_blocks.wrapping_sub(before.admission_blocks),
        }
    }
}

/// The window units whose handoff lag the block lists.
pub const LAG_UNITS: usize = 64;
/// How far after the next unit's stamp a handoff may lie and still count as
/// waiting for that arrival.
pub const NEXT_ARRIVAL_TOLERANCE_US: u32 = 2_000;

/// One window unit: its receive stamp and its handoff lag, the low 32 bits
/// of the handoff time less the stamp (µs, wrapping, as the survey's).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lag {
    pub stamp: u32,
    pub lag_us: u32,
}

/// The first [`LAG_UNITS`] stamped units the survey took in the window.
pub struct LagLog(heapless::Vec<Lag, LAG_UNITS>);

impl LagLog {
    pub const fn new() -> Self {
        Self(heapless::Vec::new())
    }

    /// Log a unit stamped `stamp` and handed off at `handoff`, while there
    /// is room.
    pub fn take(&mut self, stamp: u32, handoff: oer_time::Instant) {
        let lag_us = (handoff.as_micros() as u32).wrapping_sub(stamp);
        let _full = self.0.push(Lag { stamp, lag_us });
    }

    pub fn units(&self) -> &[Lag] {
        &self.0
    }
}

/// The handoff delays of a lag log. The stamp clock runs at the handoff
/// clock's rate, so a unit's lag is its delay plus one constant offset; the
/// smallest lag stands for that offset: `delay = lag − min lag` (taken
/// relative to the first unit's lag, so a lag wrap is harmless).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LagFit {
    /// The lower median delay.
    pub median_us: u32,
    pub max_us: u32,
    /// Units whose offset-corrected handoff lies 0 to
    /// [`NEXT_ARRIVAL_TOLERANCE_US`] after the next unit's stamp.
    pub next_arrival_matches: u32,
}

/// The fit of `units`; `None` without a unit.
pub fn lag_fit(units: &[Lag]) -> Option<LagFit> {
    let first = units.first()?;
    let relative = |lag: &Lag| i64::from(lag.lag_us.wrapping_sub(first.lag_us) as i32);
    let floor = units.iter().map(relative).min()?;
    let delay = |lag: &Lag| relative(lag) - floor;
    let mut delays = heapless::Vec::<i64, LAG_UNITS>::new();
    for lag in units {
        let _full = delays.push(delay(lag));
    }
    oer_sort::sort(&mut delays);
    let narrow = |value: i64| u32::try_from(value).unwrap_or(u32::MAX);
    let median_us = narrow(*delays.get((delays.len() - 1) / 2)?);
    let max_us = narrow(*delays.last()?);
    let next_arrival_matches = units
        .windows(2)
        .filter(|pair| match pair {
            [unit, next] => {
                let until_next = i64::from(next.stamp.wrapping_sub(unit.stamp));
                let after = delay(unit) - until_next;
                (0..=i64::from(NEXT_ARRIVAL_TOLERANCE_US)).contains(&after)
            }
            _ => false,
        })
        .count();
    Some(LagFit {
        median_us,
        max_us,
        next_arrival_matches: narrow(next_arrival_matches as i64),
    })
}

/// The stand beacons whose TSF and handoff the block lists.
pub const TSF_PAIRS: usize = 64;

/// One stand beacon: its TSF and its handoff, both µs, low 32 bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TsfPair {
    pub tsf: u32,
    pub handoff: u32,
}

/// The first [`TSF_PAIRS`] beacons of the stand BSS the survey counted in
/// the window (new TSF, no duplicate), with the stand's table index.
pub struct TsfLog {
    stand: Option<usize>,
    pairs: heapless::Vec<TsfPair, TSF_PAIRS>,
}

impl TsfLog {
    pub const fn new() -> Self {
        Self {
            stand: None,
            pairs: heapless::Vec::new(),
        }
    }

    /// The stand's beacons and duplicates before the survey takes a frame.
    pub fn before(&self, survey: &Survey<BSS>) -> Option<(u32, u32)> {
        let (_, tally) = survey.bss(self.stand?)?;
        Some((tally.beacons, tally.duplicates))
    }

    /// After the survey took a frame handed off at `handoff`: a new stand
    /// beacon (one more beacon, no more duplicates than `before`) is logged
    /// with the TSF the survey's record now holds.
    pub fn after(
        &mut self,
        survey: &Survey<BSS>,
        stand: Bssid,
        before: Option<(u32, u32)>,
        handoff: oer_time::Instant,
    ) {
        if self.stand.is_none() {
            self.stand = survey
                .entries()
                .position(|(record, _)| record.bssid == stand);
        }
        let Some((record, tally)) = self.stand.and_then(|index| survey.bss(index)) else {
            return;
        };
        let (beacons, duplicates) = before.unwrap_or((0, 0));
        if tally.beacons > beacons && tally.duplicates == duplicates {
            let _full = self.pairs.push(TsfPair {
                tsf: record.timestamp.as_micros() as u32,
                handoff: handoff.as_micros() as u32,
            });
        }
    }

    pub fn pairs(&self) -> &[TsfPair] {
        &self.pairs
    }

    /// The stand's survey index, once it was heard.
    pub const fn stand(&self) -> Option<usize> {
        self.stand
    }
}

/// The handoff times of beacons against their TSF, the linear trend
/// removed: least squares of handoff on TSF (integer arithmetic, both
/// relative to the first pair, so a wrap is harmless), the residuals
/// floored to µs and taken less the smallest one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TsfFit {
    /// The lower median, the 90th percentile (rank ⌈0.9·n⌉) and the maximum
    /// of residual − min residual.
    pub median_us: u32,
    pub p90_us: u32,
    pub max_us: u32,
    /// (slope − 1) · 10⁶, floored.
    pub slope_ppm: i32,
}

/// The fit of `pairs`; `None` below two pairs or without a TSF spread.
pub fn tsf_fit(pairs: &[TsfPair]) -> Option<TsfFit> {
    let first = pairs.first()?;
    let point = |pair: &TsfPair| {
        (
            i128::from(pair.tsf.wrapping_sub(first.tsf) as i32),
            i128::from(pair.handoff.wrapping_sub(first.handoff) as i32),
        )
    };
    let n = i128::try_from(pairs.len()).ok()?;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0_i128, 0_i128, 0_i128, 0_i128);
    for (x, y) in pairs.iter().map(point) {
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let dxx = n * sxx - sx * sx;
    let dxy = n * sxy - sx * sy;
    if n < 2 || dxx == 0 {
        return None;
    }
    // residual = (n·dxx·y − (sy·dxx − dxy·sx) − n·dxy·x) / (n·dxx)
    let denominator = n * dxx;
    let intercept = sy * dxx - dxy * sx;
    let mut residuals = heapless::Vec::<i128, TSF_PAIRS>::new();
    for (x, y) in pairs.iter().map(point) {
        let _full = residuals.push((n * dxx * y - intercept - n * dxy * x).div_euclid(denominator));
    }
    let floor = *residuals.iter().min()?;
    let mut spread = heapless::Vec::<u32, TSF_PAIRS>::new();
    for residual in &residuals {
        let _full = spread.push(u32::try_from(residual - floor).unwrap_or(u32::MAX));
    }
    oer_sort::sort(&mut spread);
    let count = spread.len();
    let p90_rank = (count * 9).div_ceil(10);
    let slope_ppm = ((dxy - dxx) * 1_000_000).div_euclid(dxx);
    Some(TsfFit {
        median_us: *spread.get((count - 1) / 2)?,
        p90_us: *spread.get(p90_rank.checked_sub(1)?)?,
        max_us: *spread.last()?,
        slope_ppm: i32::try_from(slope_ppm).unwrap_or(if slope_ppm < 0 {
            i32::MIN
        } else {
            i32::MAX
        }),
    })
}

/// The executors by comparator: 0 is the radio executor's, 1 the
/// application's.
const COMPARATORS: [u8; 2] = [measure::P2_COMPARATOR, measure::P1_COMPARATOR];
/// The phases the image waits in.
const WAIT_PHASES: [Phase; 6] = [
    Phase::BringUp,
    Phase::FirstFrame,
    Phase::Window,
    Phase::Stress,
    Phase::Hold,
    Phase::Stop,
];

/// Everything the block reports, captured after the exit.
pub struct Run<'a> {
    pub channel: u8,
    pub applied_channel: u8,
    pub stand: Bssid,
    /// The stand's SSID, which its beacons must carry.
    pub ssid: &'a [u8],
    pub window_ms: u32,
    pub cpu_hz: u32,
    pub survey: &'a Survey<BSS>,
    /// The measured window; zero when no frame opened it.
    pub window_us: u64,
    pub rx: WifiRxReport,
    pub unserviced_mac: u32,
    pub unserviced_pwr: u32,
    /// The hold cycles; all zero when no frame opened the window.
    pub cycles: &'a [Cycle; CYCLES],
    /// The first window units' handoff lags.
    pub lags: &'a LagLog,
    /// The first stand beacons' TSF and handoff.
    pub tsf: &'a TsfLog,
    /// Frames the application's normalize admitted.
    pub admitted: u32,
    pub exit: WifiRxExit,
    pub ticks: &'a [Tick],
    pub timers: &'a [TimerLine],
    pub irq_bytes: usize,
    pub irq_free: usize,
}

/// A gate's result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Pass,
    Fail(&'static str),
    Unexercised(&'static str),
}

/// One gate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gate {
    pub name: &'static str,
    pub outcome: Outcome,
}

/// The stand's beacon figures the `beacons` gate rests on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Verdict {
    pub beacons: u64,
    pub bi_us: u64,
    pub window_us: u64,
    /// `None` at a zero beacon interval or without the stand.
    pub required: Option<u64>,
}

/// The stand's survey entry: its beacon interval, whether its stored SSID
/// is the stand's, and its tally.
fn stand<'a>(run: &Run<'a>) -> Option<(u16, bool, &'a BssTally)> {
    run.survey
        .entries()
        .find(|(record, _)| record.bssid == run.stand)
        .map(|(record, tally)| {
            (
                record.beacon_interval_tu,
                record.ssid_bytes() == run.ssid,
                tally,
            )
        })
}

/// The BSSs whose stored SSID is the stand's.
fn ssid_matches(run: &Run<'_>) -> usize {
    run.survey
        .entries()
        .filter(|(record, _)| record.ssid_bytes() == run.ssid)
        .count()
}

/// ⌊8·window_us / (10·bi_us)⌋, 80 % of the expected beacons; `None` at a
/// zero interval.
pub fn required_beacons(window_us: u64, bi_us: u64) -> Option<u64> {
    if bi_us == 0 {
        return None;
    }
    let required = 8 * u128::from(window_us) / (10 * u128::from(bi_us));
    Some(u64::try_from(required).unwrap_or(u64::MAX))
}

/// The beacon figures and the `beacons` gate.
fn beacons(run: &Run<'_>) -> (Verdict, Outcome) {
    let unidentified = Verdict {
        window_us: run.window_us,
        ..Verdict::default()
    };
    let Some((bi_tu, ssid_ok, tally)) = stand(run) else {
        let reason = if run.survey.counters().table_full > 0 {
            "stand-not-tracked"
        } else {
            "stand-absent"
        };
        return (unidentified, Outcome::Fail(reason));
    };
    // The stand BSSID heard without the stand's SSID is another network.
    if !ssid_ok {
        return (unidentified, Outcome::Fail("stand-mismatch"));
    }
    let beacons = u64::from(tally.beacons.saturating_sub(tally.duplicates));
    let bi_us = u64::from(bi_tu) * TU_US;
    let required = required_beacons(run.window_us, bi_us);
    let outcome = match required {
        None => Outcome::Fail("zero-interval"),
        Some(required) if beacons >= required => Outcome::Pass,
        Some(_) => Outcome::Fail("beacons"),
    };
    (
        Verdict {
            beacons,
            bi_us,
            window_us: run.window_us,
            required,
        },
        outcome,
    )
}

fn check(pass: bool, reason: &'static str) -> Outcome {
    if pass {
        Outcome::Pass
    } else {
        Outcome::Fail(reason)
    }
}

/// Whether any timer wait of the image returned before its deadline.
fn woke_early() -> bool {
    COMPARATORS.iter().any(|&comparator| {
        WAIT_PHASES
            .iter()
            .any(|&phase| measure::waits(comparator, phase).1 > 0)
    })
}

/// Both cycles: a changed held lease or a chained drop in a release window
/// fails it (`foreign-write`); otherwise a cycle whose hold was not full
/// (`pool-not-exhausted`) or whose leases did not cover the ring
/// (`ring-order`) leaves it unexercised.
fn ownership(cycles: &[Cycle]) -> Outcome {
    if cycles.iter().any(|cycle| {
        cycle.hold_mismatch > 0
            || cycle.dwell_mismatch > 0
            || cycle.release_mismatch > 0
            || cycle.chained > 0
    }) {
        Outcome::Fail("foreign-write")
    } else if cycles.iter().any(|cycle| !cycle.exercised()) {
        Outcome::Unexercised("pool-not-exhausted")
    } else if cycles.iter().any(|cycle| cycle.first_drop.is_none()) {
        Outcome::Unexercised("ring-order")
    } else {
        Outcome::Pass
    }
}

/// The number of gates.
pub const GATES: usize = 12;

/// Every gate, in the block's order, and the beacon figures.
pub fn gates(run: &Run<'_>) -> (Verdict, [Gate; GATES]) {
    let (verdict, beacons) = beacons(run);
    // The first cycle: every lease held, nothing received meanwhile (the
    // walker ran out), frames after the release; how the ring recovered is
    // report-only.
    let first = &run.cycles[0];
    let walker_restart = if !first.full.held() {
        Outcome::Unexercised("pool-not-exhausted")
    } else if first.hold.units > 0 {
        Outcome::Fail("received-while-held")
    } else {
        check(first.frames_after_release > 0, "no-frames-after-release")
    };
    let gates = [
        Gate {
            name: "beacons",
            outcome: beacons,
        },
        Gate {
            name: "channel",
            outcome: check(run.applied_channel == run.channel, "channel-mismatch"),
        },
        Gate {
            name: "exit",
            outcome: check(
                run.exit.cause == WifiRxCause::StopRequested && run.exit.end == WifiRxEnd::Halted,
                "not-stopped",
            ),
        },
        Gate {
            name: "walker_restart",
            outcome: walker_restart,
        },
        Gate {
            name: "ownership",
            outcome: ownership(run.cycles),
        },
        Gate {
            name: "service_errors",
            outcome: check(run.rx.service_errors == 0, "service-errors"),
        },
        Gate {
            name: "unserviced",
            outcome: check(
                run.unserviced_mac.saturating_add(run.unserviced_pwr) == 0,
                "unserviced-interrupts",
            ),
        },
        Gate {
            name: "timer_wake",
            outcome: check(!woke_early(), "woke-early"),
        },
        // The epoch wrote BASE once: the cold publication of the reserved
        // descriptor; every later publication went through the doorbell.
        Gate {
            name: "base_writes",
            outcome: check(run.rx.reload.base_writes == 1, "base-writes"),
        },
        // At most the epoch's first frame landed in the reserved
        // descriptor, and nothing after the walker reached the ring.
        Gate {
            name: "reserved",
            outcome: check(
                run.rx.reload.reserved_landings <= 1 && run.rx.reload.reserved_late_landings == 0,
                "reserved-landing",
            ),
        },
        // Every held lease's descriptor carried the owner bit clear.
        Gate {
            name: "owner_bit",
            outcome: check(
                run.cycles.iter().all(|cycle| {
                    usize::try_from(cycle.owner_clear).is_ok_and(|clear| clear == cycle.held)
                }),
                "owner-bit-set",
            ),
        },
        // No NEXT read named a detached descriptor.
        Gate {
            name: "walker_on_detached",
            outcome: check(run.rx.reload.walker_on_detached == 0, "walker-on-detached"),
        },
    ];
    (verdict, gates)
}

/// Emits the notes of the block and counts its lines: a line is one note or
/// several that the host joins.
struct Block<'e, E, F: FnMut(&Note<'_>) -> Result<(), E>> {
    emit: &'e mut F,
    lines: u32,
}

impl<E, F: FnMut(&Note<'_>) -> Result<(), E>> Block<'_, E, F> {
    /// One note of the line that is being written.
    fn part(&mut self, note: &Note<'_>) -> Result<(), E> {
        (self.emit)(note)
    }

    /// The line that its parts made is complete.
    fn end_line(&mut self) {
        self.lines += 1;
    }
}

/// Emits the notes one after the other: each is built, written and dropped
/// before the next, so that a long line costs the frame one note.
macro_rules! parts {
    ($block:expr, $($note:expr),+ $(,)?) => {{
        $($block.part(&$note)?;)+
    }};
}

/// [`parts!`] and the end of the line.
macro_rules! line {
    ($block:expr, $($note:expr),+ $(,)?) => {{
        parts!($block, $($note),+);
        $block.end_line();
    }};
}

/// The last two octets of `bssid`: the masked identity a note carries.
fn mac_tail(bssid: Bssid) -> [u8; 2] {
    let [.., fifth, sixth] = bssid.get();
    [fifth, sixth]
}

/// The exit cause as one token.
pub const fn cause(cause: WifiRxCause) -> &'static str {
    match cause {
        WifiRxCause::StopRequested => "StopRequested",
        WifiRxCause::Prepare(_) => "Prepare",
        WifiRxCause::LineRefused => "LineRefused",
        WifiRxCause::Start(_) => "Start",
        WifiRxCause::Service(_) => "Service",
        WifiRxCause::ForeignExecutor => "ForeignExecutor",
    }
}

/// The exit end as one token.
pub const fn end_name(end: WifiRxEnd) -> &'static str {
    match end {
        WifiRxEnd::Halted => "Halted",
        WifiRxEnd::NotStarted => "NotStarted",
        WifiRxEnd::Quarantined {
            reason: WifiRxQuarantine::Fault,
            walker_stopped: true,
        } => "Quarantined:Fault:walker-stopped",
        WifiRxEnd::Quarantined {
            reason: WifiRxQuarantine::Fault,
            walker_stopped: false,
        } => "Quarantined:Fault:walker-live",
        WifiRxEnd::Quarantined {
            reason: WifiRxQuarantine::MaskTimeout,
            walker_stopped: true,
        } => "Quarantined:MaskTimeout:walker-stopped",
        WifiRxEnd::Quarantined {
            reason: WifiRxQuarantine::MaskTimeout,
            walker_stopped: false,
        } => "Quarantined:MaskTimeout:walker-live",
        WifiRxEnd::Quarantined {
            reason: WifiRxQuarantine::DrainTimeout,
            walker_stopped: true,
        } => "Quarantined:DrainTimeout:walker-stopped",
        WifiRxEnd::Quarantined {
            reason: WifiRxQuarantine::DrainTimeout,
            walker_stopped: false,
        } => "Quarantined:DrainTimeout:walker-live",
        WifiRxEnd::Quarantined {
            reason: WifiRxQuarantine::StopRefused(_),
            walker_stopped: true,
        } => "Quarantined:StopRefused:walker-stopped",
        WifiRxEnd::Quarantined {
            reason: WifiRxQuarantine::StopRefused(_),
            walker_stopped: false,
        } => "Quarantined:StopRefused:walker-live",
    }
}

/// Cycles as microseconds at `cpu_hz`.
fn micros(cycles: u32, cpu_hz: u32) -> u64 {
    if cpu_hz == 0 {
        return 0;
    }
    u64::from(cycles) * 1_000_000 / u64::from(cpu_hz)
}

/// The longest and the 99th-percentile sample, in µs; zero without one.
fn max_p99(histogram: &CycleHistogram, cpu_hz: u32) -> (u64, u64) {
    let snapshot = histogram.snapshot();
    (
        micros(snapshot.max().unwrap_or(0), cpu_hz),
        micros(snapshot.percentile(99).unwrap_or(0), cpu_hz),
    )
}

fn block_lines<E, F: FnMut(&Note<'_>) -> Result<(), E>>(
    block: &mut Block<'_, E, F>,
    run: &Run<'_>,
) -> Result<(), E> {
    let (verdict, gates) = gates(run);
    let hz = run.cpu_hz;
    line!(
        block,
        Note::w1_config_a(
            u32::from(run.channel),
            u32::from(run.applied_channel),
            mac_tail(run.stand),
            ssid_matches(run) as u32,
            run.window_ms,
            hz,
            (COUNT as u32, BUFFER as u32),
        ),
        Note::w1_config_b(SLOTS as u32, MAC_STATION_INTERRUPT_MASK.bits()),
    );
    for (index, (record, tally)) in run.survey.entries().enumerate() {
        let who = if record.bssid == run.stand {
            "stand"
        } else {
            "other"
        };
        line!(
            block,
            Note::w1_bss_a(
                index as u32,
                who,
                mac_tail(record.bssid),
                u32::from(record.beacon_interval_tu),
                u32::from(record.channel),
                tally.beacons,
                tally.duplicates,
                tally.probe_responses,
            ),
            Note::w1_bss_b(
                tally
                    .rssi_dbm
                    .map(|range| (i32::from(range.min), i32::from(range.max))),
                tally.without_rssi,
                tally.late_gaps,
                tally.max_beacon_gap_us,
                tally.missed_beacons,
            ),
            Note::w1_bss_c(
                tally.handoff_lag_us.map(|range| (range.min, range.max)),
                record.ssid_bytes().len() as u32,
                record.ssid_bytes() == run.ssid,
                tally.stamp_span_us(),
            ),
            Note::w1_bss_d(tally.handoff_span_us()),
        );
    }
    for gap in run.survey.gap_log() {
        line!(
            block,
            Note::w1_gap(gap.stamp, gap.gap_us, gap.missed_by_tsf)
        );
    }
    line!(block, Note::w1_gaps(run.survey.gap_log_dropped()));
    for tick in run.ticks {
        line!(block, Note::w1_tick(tick.start, tick.end, tick.ok));
    }
    let rx = &run.rx;
    line!(
        block,
        Note::w1_rx_a(
            rx.units,
            rx.bytes,
            rx.mac_irqs,
            rx.power_irqs,
            run.unserviced_mac,
            run.unserviced_pwr,
            rx.doorbell_unmasks,
            rx.ignored_unmasks,
        ),
        Note::w1_rx_b(
            rx.chained_drops,
            rx.oversized_drops,
            rx.empty_drops,
            rx.too_long_drops,
            rx.overload_drops,
            rx.admission_blocks,
            rx.stage_capacity_blocks,
            rx.service_passes,
        ),
        Note::w1_rx_c(
            rx.service_errors,
            rx.capacity_drains,
            run.cycles[0].frames_after_release,
        ),
    );
    for (index, cycle) in run.cycles.iter().enumerate() {
        let hold = &cycle.hold;
        let end = cycle.end.as_ref();
        line!(
            block,
            Note::w1_hold_a(
                index as u32,
                cycle.fill_ms,
                hold.reached as u32,
                SLOTS as u32,
                cycle.full.held(),
                cycle.full.reason(),
                hold.units,
                hold.mac_irqs,
                hold.stage_capacity_blocks,
            ),
            Note::w1_hold_b(
                hold.admission_blocks,
                end.map(|end| end.released),
                end.map(|end| end.detached),
                end.map(|end| end.staged),
            ),
            Note::w1_hold_c(
                end.map(|end| end.observed),
                end.map(|end| end.recycle_start),
                end.map(|end| end.accepted_tail),
                end.map(|end| end.next_low),
            ),
            Note::w1_hold_d(end.map(|end| end.last_low)),
        );
    }
    for (index, cycle) in run.cycles.iter().enumerate() {
        exhaust(block, index, cycle)?;
    }
    for (index, cycle) in run.cycles.iter().enumerate() {
        parts!(
            block,
            Note::w1_own_a(
                index as u32,
                cycle.order.name(),
                cycle.held as u32,
                cycle.owner_clear,
                cycle.first_drop.and_then(|index| u32::try_from(index).ok()),
                cycle.hold_mismatch,
                cycle.dwell_mismatch,
                cycle.release_mismatch,
            ),
            Note::w1_own_b(
                cycle.first_mismatch,
                &cycle.first_after_release,
                cycle.order_breaks,
                cycle.chained,
            ),
            Note::w1_own_c(cycle.oversized),
        );
        own_discards(block, &cycle.discards)?;
        block.end_line();
    }
    let prefix = &rx.prefix;
    line!(
        block,
        Note::w1_prefix_a(
            prefix.invalid,
            prefix.chain,
            prefix.bounds,
            prefix.rx_failure,
            prefix.mic_failure,
            prefix.quarantined,
            prefix.output_small,
            prefix.metadata,
        ),
        Note::w1_prefix_b(
            prefix.ignored,
            prefix.unsupported,
            prefix.strict_rxend_refusable,
            prefix.strict_dump_refusable,
            run.admitted,
        ),
    );
    let reload = &rx.reload;
    line!(
        block,
        Note::w1_reload_a(
            reload.max_reload_attempts,
            RELOAD_SETTLE_LIMIT.limit().as_micros() as u64,
            reload.observations,
            reload.reload_rerings,
            reload.rering_failures,
            reload.base_writes,
            reload.reserved_landings,
        ),
        Note::w1_reload_b(reload.reserved_late_landings, reload.walker_on_detached),
    );
    let continuations = &rx.continuations;
    line!(
        block,
        Note::w1_continuation_a(
            continuations.immediate,
            continuations.timed,
            continuations.drained,
            continuations.upper_layer_blocked_but_droppable,
            continuations.probe_pending,
            continuations.recycled_append_pending,
            continuations.budget_exhausted,
            continuations.network_backpressured,
        ),
        Note::w1_continuation_b(
            continuations.protocol_blocked_by_tx,
            continuations.longest_streak,
        ),
    );
    line!(
        block,
        Note::w1_teardown(
            rx.teardown.mask_confirm_us,
            rx.teardown.drain_us,
            rx.teardown.drain_polls,
        )
    );
    line!(
        block,
        Note::w1_exit(cause(run.exit.cause), end_name(run.exit.end))
    );
    let bound_cycles = u32::try_from(
        oer_esp32s31_radio_client::ACTOR_STEP_BOUND.as_micros() * u64::from(hz) / 1_000_000,
    )
    .unwrap_or(u32::MAX);
    for (slot, phase) in TIMED_PHASES.iter().enumerate() {
        let (Some(swi2), Some(actor), Some(receive), Some(mac), Some(ring)) = (
            measure::SWI2.get(slot),
            measure::ACTOR.get(slot),
            measure::RECEIVE.get(slot),
            measure::MAC_TO_POLL_MAX.get(slot),
            measure::RING_TO_DOORBELL_MAX.get(slot),
        ) else {
            continue;
        };
        let (swi2_max, swi2_p99) = max_p99(swi2, hz);
        let (actor_max, actor_p99) = max_p99(actor, hz);
        let (receive_max, receive_p99) = max_p99(receive, hz);
        let over = actor.snapshot().count_over(bound_cycles);
        line!(
            block,
            Note::w1_p2_a(phase.name(), swi2_max, swi2_p99, actor_max, actor_p99),
            Note::w1_p2_b((over.at_least, over.at_most), receive_max, receive_p99),
            Note::w1_p2_c(
                micros(mac.load(core::sync::atomic::Ordering::Relaxed), hz),
                micros(ring.load(core::sync::atomic::Ordering::Relaxed), hz),
            ),
        );
    }
    for clock in CLOCKS {
        for op in ClockOp::ALL {
            for phase in Phase::ALL {
                let (count, max) = measure::clock_stats(clock, op, phase);
                if count == 0 {
                    continue;
                }
                line!(
                    block,
                    Note::w1_clock(
                        op.name(),
                        measure::clock_name(clock),
                        phase.name(),
                        count,
                        micros(max, hz),
                    )
                );
            }
        }
    }
    line!(
        block,
        Note::w1_stack(run.irq_bytes as u32, run.irq_free as u32)
    );
    for timer in run.timers {
        let report = &timer.report;
        line!(
            block,
            Note::w1_timer_a(
                u32::from(timer.comparator),
                timer.phase.name(),
                report.early_interrupts,
                report.deadline_lateness.count,
                u64::from(report.deadline_lateness.maximum_micros),
                u64::from(report.deadline_lateness.total_micros),
            ),
            Note::w1_timer_b(
                report.due_at_program_return,
                report.unmatched_interrupts,
                report.coalesced_interrupts,
                report.overlapping_interrupts,
                report.armed_at_end,
                measure::waits(timer.comparator, timer.phase).0,
                measure::waits(timer.comparator, timer.phase).1,
            ),
        );
    }
    for comparator in COMPARATORS {
        for phase in WAIT_PHASES {
            let (waits, early) = measure::waits(comparator, phase);
            if waits == 0 {
                continue;
            }
            line!(
                block,
                Note::w1_wake(u32::from(comparator), phase.name(), waits, early,)
            );
        }
    }
    let units = run.lags.units();
    for (index, unit) in units.iter().enumerate() {
        let since_prev = index
            .checked_sub(1)
            .and_then(|previous| units.get(previous))
            .map(|previous| unit.stamp.wrapping_sub(previous.stamp));
        let until_next = units
            .get(index + 1)
            .map(|next| next.stamp.wrapping_sub(unit.stamp));
        line!(
            block,
            Note::w1_lag(
                index as u32,
                unit.stamp,
                unit.lag_us,
                since_prev,
                until_next,
            )
        );
    }
    let fit = lag_fit(units);
    line!(
        block,
        Note::w1_lagfit(
            units.len() as u32,
            fit.map(|fit| fit.median_us),
            fit.map(|fit| fit.max_us),
            fit.map_or(0, |fit| fit.next_arrival_matches),
            NEXT_ARRIVAL_TOLERANCE_US,
        )
    );
    for (index, pair) in run.tsf.pairs().iter().enumerate() {
        line!(block, Note::w1_tsf(index as u32, pair.tsf, pair.handoff));
    }
    let fit = tsf_fit(run.tsf.pairs());
    line!(
        block,
        Note::w1_tsffit_a(
            run.tsf.pairs().len() as u32,
            run.tsf
                .stand()
                .map(|index| u32::try_from(index).unwrap_or(u32::MAX)),
            fit.map(|fit| fit.median_us),
            fit.map(|fit| fit.p90_us),
        ),
        Note::w1_tsffit_b(fit.map(|fit| fit.max_us), fit.map(|fit| fit.slope_ppm)),
    );
    let survey = run.survey.counters();
    line!(
        block,
        Note::w1_survey(
            survey.frames,
            survey.header_refusals,
            survey.data,
            survey.ignored_management,
            survey.table_full,
            survey.off_channel,
        )
    );
    line!(
        block,
        Note::w1_verdict(
            verdict.beacons,
            verdict.bi_us,
            verdict.window_us,
            verdict.required.unwrap_or(0),
        )
    );
    for gate in gates {
        block.part(&Note::w1_gate_name(gate.name))?;
        block.part(&match gate.outcome {
            Outcome::Pass => Note::w1_gate_pass(),
            Outcome::Fail(reason) => Note::w1_gate_fail(reason),
            Outcome::Unexercised(reason) => Note::w1_gate_unexercised(reason),
        })?;
        block.end_line();
    }
    Ok(())
}

/// The discard counts of an own line: the nonzero ones, or a dash.
fn own_discards<E, F: FnMut(&Note<'_>) -> Result<(), E>>(
    block: &mut Block<'_, E, F>,
    counts: &[u32; 15],
) -> Result<(), E> {
    if counts.iter().all(|&count| count == 0) {
        return block.part(&Note::w1_discards_none());
    }
    let low_any = counts.iter().take(8).any(|&count| count != 0);
    block.part(&Note::w1_discards_a(counts))?;
    block.part(&Note::w1_discards_b((counts, low_any)))
}

/// The walker and ring snapshot that the first drop of hold cycle `index`
/// took; every value `-` without one.
fn exhaust<E, F: FnMut(&Note<'_>) -> Result<(), E>>(
    block: &mut Block<'_, E, F>,
    index: usize,
    cycle: &Cycle,
) -> Result<(), E> {
    let snapshot = cycle.exhaust;
    let take = |field: fn(&WifiRxExhaustion) -> Option<u32>| snapshot.as_ref().and_then(field);
    let flag = |field: fn(&WifiRxExhaustion) -> bool| snapshot.as_ref().map(field);
    line!(
        block,
        Note::w1_exhaust_a(
            index as u32,
            cycle.snapshots,
            take(|s| Some(s.blocked_us)),
            take(|s| Some(s.next_low)),
            flag(|s| s.next_upper),
        ),
        Note::w1_exhaust_b(
            take(|s| Some(s.last_low)),
            take(|s| s.base),
            flag(|s| s.walker_enabled),
            flag(|s| s.reload_doorbell),
            take(|s| s.buffer_full.map(u32::from)),
        ),
        Note::w1_exhaust_c(
            take(|s| Some(s.ring_base)),
            take(|s| Some(s.recycle_start)),
            take(|s| Some(s.accepted_tail)),
            take(|s| Some(s.observed)),
        ),
        Note::w1_exhaust_d(
            take(|s| Some(s.completed)),
            take(|s| s.remaining),
            flag(|s| s.reload_pending),
            take(|s| Some(s.released)),
        ),
        Note::w1_exhaust_e(
            take(|s| Some(s.detached)),
            take(|s| Some(s.staged)),
            take(|s| s.next_after_doorbell),
            take(|s| s.last_after_doorbell),
        ),
        Note::w1_exhaust_f(flag(|s| s.descriptors_changed)),
    );
    Ok(())
}

/// Print the block and the verdict line as notes.
pub fn print<E>(mut emit: impl FnMut(&Note<'_>) -> Result<(), E>, run: &Run<'_>) -> Result<(), E> {
    emit(&Note::w1_begin())?;
    let mut block = Block {
        emit: &mut emit,
        lines: 0,
    };
    block_lines(&mut block, run)?;
    let lines = block.lines;
    emit(&Note::w1_end(lines))?;
    let (verdict, gates) = gates(run);
    if gates.iter().all(|gate| gate.outcome == Outcome::Pass) {
        return emit(&Note::w1_pass(
            verdict.beacons,
            verdict.required.unwrap_or(0),
        ));
    }
    emit(&Note::w1_fail_begin())?;
    let mut first = true;
    for gate in gates {
        if gate.outcome == Outcome::Pass {
            continue;
        }
        if !first {
            emit(&Note::w1_fail_separator())?;
        }
        emit(&Note::w1_fail_gate(gate.name))?;
        match gate.outcome {
            Outcome::Pass => {}
            Outcome::Fail(reason) => emit(&Note::w1_fail_reason(reason))?,
            Outcome::Unexercised(_) => emit(&Note::w1_fail_not_exercised())?,
        }
        first = false;
    }
    emit(&Note::w1_fail_end())
}

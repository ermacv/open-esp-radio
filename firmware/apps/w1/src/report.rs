//! The report block W1 prints after its exit, its gates and its verdict.
//!
//! The block runs from `OER-W1-BEGIN v=1` to `OER-W1-END lines=<n>`, one
//! `keyword key=value …` line each, values without spaces; `cargo fw`
//! recomputes every gate from the block's other lines (`tools/fw/src/w1.rs`
//! holds its spec). BSSIDs are masked to their last two octets, SSIDs are
//! printed only as their length and, per BSS, whether the stored SSID is the
//! stand's (`ssid_ok=`).

use core::fmt::{self, Write};

use oer_esp32s31_ieee80211::rx::{
    control::{WifiRxCause, WifiRxEnd, WifiRxExit, WifiRxQuarantine},
    counters::{WifiRxExhaustion, WifiRxReport},
};
use oer_esp32s31_ieee80211_dma::rx_ring::RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT;
use oer_esp32s31_ieee80211_mac::init::MAC_COLD_RX_INTERRUPT_MASK;
use oer_espressif_executor_embassy::poll_timing::CycleHistogram;
use oer_ieee80211_mac::address::Bssid;
use oer_ieee80211_runtime::survey::{BssTally, Survey};

use crate::{
    irq::{BUFFER, COUNT, SLOTS},
    measure::{self, CLOCKS, ClockOp, Phase, TIMED_PHASES, Tick, TimerLine},
};

/// The survey's table size.
pub const BSS: usize = 64;
/// The interrupt stack the `irq_stack` gate requires to stay free.
pub const MIN_IRQ_STACK_FREE: usize = 4096;
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
    delays.sort_unstable();
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
    spread.sort_unstable();
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
    pub frames_after_release: u32,
    /// The hold; all zero when no frame opened the window.
    pub hold: Hold,
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

/// Every gate, in the block's order, and the beacon figures.
pub fn gates(run: &Run<'_>) -> (Verdict, [Gate; 8]) {
    let (verdict, beacons) = beacons(run);
    // Every lease held, nothing received meanwhile (the walker ran out),
    // frames after the release; how the ring recovered is report-only.
    let walker_restart = if run.hold.reached < SLOTS {
        Outcome::Unexercised("pool-not-exhausted")
    } else if run.hold.units > 0 {
        Outcome::Fail("received-while-held")
    } else {
        check(run.frames_after_release > 0, "no-frames-after-release")
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
            name: "irq_stack",
            outcome: check(run.irq_free >= MIN_IRQ_STACK_FREE, "irq-stack-low"),
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
    ];
    (verdict, gates)
}

/// A writer that counts the lines it was given.
struct Block<'w, W: Write> {
    out: &'w mut W,
    lines: u32,
}

impl<W: Write> Block<'_, W> {
    fn line(&mut self, args: fmt::Arguments<'_>) -> fmt::Result {
        self.out.write_fmt(args)?;
        self.out.write_str("\r\n")?;
        self.lines += 1;
        Ok(())
    }
}

/// `xx:xx:xx:xx:HH:HH`.
struct Masked(Bssid);

impl fmt::Display for Masked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [.., fifth, sixth] = self.0.get();
        write!(f, "xx:xx:xx:xx:{fifth:02X}:{sixth:02X}")
    }
}

/// `<min>..<max>`, or `-` without a sample.
struct Range<T>(Option<(T, T)>);

impl<T: fmt::Display> fmt::Display for Range<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some((min, max)) => write!(f, "{min}..{max}"),
            None => f.write_str("-"),
        }
    }
}

/// A number, or `-`.
struct OrDash(Option<u32>);

impl fmt::Display for OrDash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => write!(f, "{value}"),
            None => f.write_str("-"),
        }
    }
}

/// The exit cause as one token.
const fn cause(cause: WifiRxCause) -> &'static str {
    match cause {
        WifiRxCause::StopRequested => "StopRequested",
        WifiRxCause::Prepare(_) => "Prepare",
        WifiRxCause::LineRefused => "LineRefused",
        WifiRxCause::Start(_) => "Start",
        WifiRxCause::Service(_) => "Service",
    }
}

/// The exit end as one token.
struct End(WifiRxEnd);

impl fmt::Display for End {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (reason, walker_stopped) = match self.0 {
            WifiRxEnd::Halted => return f.write_str("Halted"),
            WifiRxEnd::NotStarted => return f.write_str("NotStarted"),
            WifiRxEnd::Quarantined {
                reason,
                walker_stopped,
            } => (reason, walker_stopped),
        };
        let reason = match reason {
            WifiRxQuarantine::Fault => "Fault",
            WifiRxQuarantine::MaskTimeout => "MaskTimeout",
            WifiRxQuarantine::DrainTimeout => "DrainTimeout",
            WifiRxQuarantine::StopRefused(_) => "StopRefused",
        };
        let walker = if walker_stopped {
            "walker-stopped"
        } else {
            "walker-live"
        };
        write!(f, "Quarantined:{reason}:{walker}")
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

/// A count over a threshold: exact, or `<at_least>..<at_most>`.
struct Over(u64, u64);

impl fmt::Display for Over {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == self.1 {
            write!(f, "{}", self.0)
        } else {
            write!(f, "{}..{}", self.0, self.1)
        }
    }
}

fn block_lines<W: Write>(block: &mut Block<'_, W>, run: &Run<'_>) -> fmt::Result {
    let (verdict, gates) = gates(run);
    let hz = run.cpu_hz;
    block.line(format_args!(
        "w1.config channel={} applied_channel={} stand={} ssid_matches={} window_ms={} cpu_hz={hz} arena={COUNT}x{BUFFER} slots={SLOTS} mask={:#010x}",
        run.channel,
        run.applied_channel,
        Masked(run.stand),
        ssid_matches(run),
        run.window_ms,
        MAC_COLD_RX_INTERRUPT_MASK.bits(),
    ))?;
    for (index, (record, tally)) in run.survey.entries().enumerate() {
        let who = if record.bssid == run.stand {
            "stand"
        } else {
            "other"
        };
        block.line(format_args!(
            "w1.bss idx={index} who={who} bssid={} bi_tu={} ch={} beacons={} dup={} probe_rsp={} rssi={} no_rssi={} gaps_late={} max_gap_us={} missed={} lag_us={} ssid_len={} ssid_ok={} stamp_span_us={} handoff_span_us={}",
            Masked(record.bssid),
            record.beacon_interval_tu,
            record.channel,
            tally.beacons,
            tally.duplicates,
            tally.probe_responses,
            Range(tally.rssi_dbm.map(|range| (range.min, range.max))),
            tally.without_rssi,
            tally.late_gaps,
            tally.max_beacon_gap_us,
            tally.missed_beacons,
            Range(tally.handoff_lag_us.map(|range| (range.min, range.max))),
            record.ssid_bytes().len(),
            u8::from(record.ssid_bytes() == run.ssid),
            OrDash(tally.stamp_span_us()),
            OrDash(tally.handoff_span_us()),
        ))?;
    }
    for gap in run.survey.gap_log() {
        block.line(format_args!(
            "w1.gap stamp={} gap_us={} missed={}",
            gap.stamp,
            gap.gap_us,
            OrDash(gap.missed_by_tsf)
        ))?;
    }
    block.line(format_args!(
        "w1.gaps dropped={}",
        run.survey.gap_log_dropped()
    ))?;
    for tick in run.ticks {
        block.line(format_args!(
            "w1.tick start={} end={} ok={}",
            tick.start,
            tick.end,
            u8::from(tick.ok)
        ))?;
    }
    let rx = &run.rx;
    block.line(format_args!(
        "w1.rx units={} bytes={} mac_irqs={} power_irqs={} unserviced_mac={} unserviced_pwr={} doorbell_unmasks={} ignored_unmasks={} chained={} oversized={} empty={} too_long={} overload={} admission_blocks={} stage_capacity_blocks={} service_passes={} service_errors={} capacity_drains={} frames_after_release={}",
        rx.units,
        rx.bytes,
        rx.mac_irqs,
        rx.power_irqs,
        run.unserviced_mac,
        run.unserviced_pwr,
        rx.doorbell_unmasks,
        rx.ignored_unmasks,
        rx.chained_drops,
        rx.oversized_drops,
        rx.empty_drops,
        rx.too_long_drops,
        rx.overload_drops,
        rx.admission_blocks,
        rx.stage_capacity_blocks,
        rx.service_passes,
        rx.service_errors,
        rx.capacity_drains,
        run.frames_after_release,
    ))?;
    let hold = &run.hold;
    block.line(format_args!(
        "w1.hold reached={} slots={SLOTS} units={} mac_irqs={} stage_capacity_blocks={} admission_blocks={}",
        hold.reached, hold.units, hold.mac_irqs, hold.stage_capacity_blocks, hold.admission_blocks,
    ))?;
    exhaust(block, rx)?;
    let prefix = &rx.prefix;
    block.line(format_args!(
        "w1.prefix invalid={} chain={} bounds={} rx_failure={} mic_failure={} quarantined={} output_small={} metadata={} ignored={} unsupported={} strict_rxend_refusable={} strict_dump_refusable={} admitted={}",
        prefix.invalid,
        prefix.chain,
        prefix.bounds,
        prefix.rx_failure,
        prefix.mic_failure,
        prefix.quarantined,
        prefix.output_small,
        prefix.metadata,
        prefix.ignored,
        prefix.unsupported,
        prefix.strict_rxend_refusable,
        prefix.strict_dump_refusable,
        run.admitted,
    ))?;
    let reload = &rx.reload;
    block.line(format_args!(
        "w1.reload max_attempts={} limit={RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT} observations={} base_repairs={} exhausted_republications={} republications_accepted={}",
        reload.max_reload_attempts,
        reload.observations,
        reload.base_repairs,
        reload.exhausted_republications,
        reload.republications_accepted,
    ))?;
    let continuations = &rx.continuations;
    block.line(format_args!(
        "w1.continuation immediate={} timed={} drained={} upper_layer_blocked_but_droppable={} probe_pending={} recycled_append_pending={} budget_exhausted={} network_backpressured={} protocol_blocked_by_tx={} longest_streak={}",
        continuations.immediate,
        continuations.timed,
        continuations.drained,
        continuations.upper_layer_blocked_but_droppable,
        continuations.probe_pending,
        continuations.recycled_append_pending,
        continuations.budget_exhausted,
        continuations.network_backpressured,
        continuations.protocol_blocked_by_tx,
        continuations.longest_streak,
    ))?;
    block.line(format_args!(
        "w1.teardown mask_confirm_us={} drain_us={} drain_polls={}",
        rx.teardown.mask_confirm_us, rx.teardown.drain_us, rx.teardown.drain_polls
    ))?;
    block.line(format_args!(
        "w1.exit cause={} end={}",
        cause(run.exit.cause),
        End(run.exit.end)
    ))?;
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
        block.line(format_args!(
            "w1.p2 phase={} swi2_max_us={swi2_max} swi2_p99_us={swi2_p99} actor_max_us={actor_max} actor_p99_us={actor_p99} actor_over_bound={} receive_max_us={receive_max} receive_p99_us={receive_p99} mac_to_poll_max_us={} ring_to_doorbell_max_us={}",
            phase.name(),
            Over(over.at_least, over.at_most),
            micros(mac.load(core::sync::atomic::Ordering::Relaxed), hz),
            micros(ring.load(core::sync::atomic::Ordering::Relaxed), hz),
        ))?;
    }
    for clock in CLOCKS {
        for op in ClockOp::ALL {
            for phase in Phase::ALL {
                let (count, max) = measure::clock_stats(clock, op, phase);
                if count == 0 {
                    continue;
                }
                block.line(format_args!(
                    "w1.clock op={} clock={} phase={} count={count} max_us={}",
                    op.name(),
                    measure::clock_name(clock),
                    phase.name(),
                    micros(max, hz),
                ))?;
            }
        }
    }
    block.line(format_args!(
        "w1.stack irq_bytes={} irq_free={} min_free={MIN_IRQ_STACK_FREE}",
        run.irq_bytes, run.irq_free
    ))?;
    for timer in run.timers {
        let report = &timer.report;
        block.line(format_args!(
            "w1.timer cmp={} phase={} early={} late_count={} late_max_us={} late_total_us={} missed={} unmatched={} coalesced={} overlapping={} armed_at_end={} waits={} woke_early={}",
            timer.comparator,
            timer.phase.name(),
            report.early_interrupts,
            report.deadline_lateness.count,
            report.deadline_lateness.maximum_micros,
            report.deadline_lateness.total_micros,
            report.due_at_program_return,
            report.unmatched_interrupts,
            report.coalesced_interrupts,
            report.overlapping_interrupts,
            u8::from(report.armed_at_end),
            measure::waits(timer.comparator, timer.phase).0,
            measure::waits(timer.comparator, timer.phase).1,
        ))?;
    }
    for comparator in COMPARATORS {
        for phase in WAIT_PHASES {
            let (waits, early) = measure::waits(comparator, phase);
            if waits == 0 {
                continue;
            }
            block.line(format_args!(
                "w1.wake cmp={comparator} phase={} waits={waits} woke_early={early}",
                phase.name()
            ))?;
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
        block.line(format_args!(
            "w1.lag i={index} stamp={} lag_us={} since_prev_us={} until_next_us={}",
            unit.stamp,
            unit.lag_us,
            OrDash(since_prev),
            OrDash(until_next),
        ))?;
    }
    let fit = lag_fit(units);
    block.line(format_args!(
        "w1.lagfit n={} median_us={} max_us={} next_arrival_matches={} tolerance_us={NEXT_ARRIVAL_TOLERANCE_US}",
        units.len(),
        OrDash(fit.map(|fit| fit.median_us)),
        OrDash(fit.map(|fit| fit.max_us)),
        fit.map_or(0, |fit| fit.next_arrival_matches),
    ))?;
    for (index, pair) in run.tsf.pairs().iter().enumerate() {
        block.line(format_args!(
            "w1.tsf i={index} tsf={} handoff={}",
            pair.tsf, pair.handoff
        ))?;
    }
    let fit = tsf_fit(run.tsf.pairs());
    block.line(format_args!(
        "w1.tsffit n={} bss={} median_us={} p90_us={} max_us={} slope_ppm={}",
        run.tsf.pairs().len(),
        OrDash(
            run.tsf
                .stand()
                .map(|index| u32::try_from(index).unwrap_or(u32::MAX))
        ),
        OrDash(fit.map(|fit| fit.median_us)),
        OrDash(fit.map(|fit| fit.p90_us)),
        OrDash(fit.map(|fit| fit.max_us)),
        Signed(fit.map(|fit| fit.slope_ppm)),
    ))?;
    let survey = run.survey.counters();
    block.line(format_args!(
        "w1.survey frames={} header_refusals={} data={} ignored_management={} table_full={} off_channel={}",
        survey.frames,
        survey.header_refusals,
        survey.data,
        survey.ignored_management,
        survey.table_full,
        survey.off_channel,
    ))?;
    block.line(format_args!(
        "w1.verdict beacons={} bi_us={} window_us={} required={}",
        verdict.beacons,
        verdict.bi_us,
        verdict.window_us,
        verdict.required.unwrap_or(0),
    ))?;
    for gate in gates {
        match gate.outcome {
            Outcome::Pass => block.line(format_args!("w1.gate name={} result=pass", gate.name))?,
            Outcome::Fail(reason) => block.line(format_args!(
                "w1.gate name={} result=fail reason={reason}",
                gate.name
            ))?,
            Outcome::Unexercised(reason) => block.line(format_args!(
                "w1.gate name={} result=unexercised reason={reason}",
                gate.name
            ))?,
        }
    }
    Ok(())
}

/// A signed number, or `-`.
struct Signed(Option<i32>);

impl fmt::Display for Signed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => write!(f, "{value}"),
            None => f.write_str("-"),
        }
    }
}

/// `0x<hex>`, or `-`.
struct HexOrDash(Option<u32>);

impl fmt::Display for HexOrDash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(value) => write!(f, "{value:#x}"),
            None => f.write_str("-"),
        }
    }
}

/// `0`, `1`, or `-`.
struct Flag(Option<bool>);

impl fmt::Display for Flag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(set) => write!(f, "{}", u8::from(set)),
            None => f.write_str("-"),
        }
    }
}

/// The walker and ring snapshot of the longest stage-capacity block that a
/// returned lease ended; every value `-` without one.
fn exhaust<W: Write>(block: &mut Block<'_, W>, rx: &WifiRxReport) -> fmt::Result {
    let snapshot = rx.exhaustion.longest;
    let take = |field: fn(&WifiRxExhaustion) -> Option<u32>| snapshot.as_ref().and_then(field);
    let flag = |field: fn(&WifiRxExhaustion) -> bool| Flag(snapshot.as_ref().map(field));
    block.line(format_args!(
        "w1.exhaust snapshots={} blocked_us={} next_low={} next_upper={} last_low={} base={} walker={} doorbell={} buffer_full={} ring_base={} recycle_start={} accepted_tail={} observed={} completed={} remaining={} reload_pending={} released={} detached={} staged={}",
        rx.exhaustion.snapshots,
        OrDash(take(|s| Some(s.blocked_us))),
        HexOrDash(take(|s| Some(s.next_low))),
        flag(|s| s.next_upper),
        HexOrDash(take(|s| Some(s.last_low))),
        HexOrDash(take(|s| s.base)),
        flag(|s| s.walker_enabled),
        flag(|s| s.reload_doorbell),
        OrDash(take(|s| s.buffer_full.map(u32::from))),
        HexOrDash(take(|s| Some(s.ring_base))),
        OrDash(take(|s| Some(s.recycle_start))),
        OrDash(take(|s| Some(s.accepted_tail))),
        OrDash(take(|s| Some(s.observed))),
        OrDash(take(|s| Some(s.completed))),
        OrDash(take(|s| s.remaining)),
        flag(|s| s.reload_pending),
        OrDash(take(|s| Some(s.released))),
        OrDash(take(|s| Some(s.detached))),
        OrDash(take(|s| Some(s.staged))),
    ))
}

/// A gate's name in a verdict reason: `walker-restart`.
struct Phrase(&'static str);

impl fmt::Display for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for part in self.0.split('_').enumerate() {
            if part.0 > 0 {
                f.write_char('-')?;
            }
            f.write_str(part.1)?;
        }
        Ok(())
    }
}

/// Print the block and the verdict line.
pub fn print<W: Write>(out: &mut W, run: &Run<'_>) -> fmt::Result {
    out.write_str("OER-W1-BEGIN v=1\r\n")?;
    let mut block = Block { out, lines: 0 };
    block_lines(&mut block, run)?;
    let lines = block.lines;
    write!(out, "OER-W1-END lines={lines}\r\n")?;
    let (verdict, gates) = gates(run);
    if gates.iter().all(|gate| gate.outcome == Outcome::Pass) {
        return write!(
            out,
            "OER-RESULT: pass w1 beacons={} required={}\r\n",
            verdict.beacons,
            verdict.required.unwrap_or(0)
        );
    }
    out.write_str("OER-RESULT: fail w1")?;
    let mut first = true;
    for gate in gates {
        let separator = if first { " " } else { "; " };
        match gate.outcome {
            Outcome::Pass => continue,
            Outcome::Fail(reason) => write!(out, "{separator}{} {reason}", Phrase(gate.name))?,
            Outcome::Unexercised(_) => {
                write!(out, "{separator}{} not exercised", Phrase(gate.name))?;
            }
        }
        first = false;
    }
    out.write_str("\r\n")
}

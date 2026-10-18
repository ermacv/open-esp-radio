//! The report block W1 prints after its exit, its gates and its verdict.
//!
//! The block runs from `OER-W1-BEGIN v=1` to `OER-W1-END lines=<n>`, one
//! `keyword key=value …` line each, values without spaces; `cargo fw`
//! recomputes every gate from the block's other lines (`tools/fw/src/w1.rs`
//! holds its spec). BSSIDs are masked to their last two octets, SSIDs are
//! printed only as their length.

use core::fmt::{self, Write};

use oer_esp32s31_ieee80211::rx::{
    control::{WifiRxCause, WifiRxEnd, WifiRxExit, WifiRxQuarantine},
    counters::WifiRxReport,
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

/// Everything the block reports, captured after the exit.
pub struct Run<'a> {
    pub channel: u8,
    pub applied_channel: u8,
    pub stand: Bssid,
    pub window_ms: u32,
    pub cpu_hz: u32,
    pub survey: &'a Survey<BSS>,
    /// The measured window; zero when no frame opened it.
    pub window_us: u64,
    pub rx: WifiRxReport,
    pub unserviced_mac: u32,
    pub unserviced_pwr: u32,
    pub frames_after_release: u32,
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

/// The stand's survey entry: its table index and tally.
fn stand<'a>(run: &Run<'a>) -> Option<(usize, u16, &'a BssTally)> {
    run.survey
        .entries()
        .enumerate()
        .find(|(_, (record, _))| record.bssid == run.stand)
        .map(|(index, (record, tally))| (index, record.beacon_interval_tu, tally))
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
    let Some((_, bi_tu, tally)) = stand(run) else {
        let reason = if run.survey.counters().table_full > 0 {
            "stand-not-tracked"
        } else {
            "stand-absent"
        };
        let verdict = Verdict {
            window_us: run.window_us,
            ..Verdict::default()
        };
        return (verdict, Outcome::Fail(reason));
    };
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

/// Every gate, in the block's order, and the beacon figures.
pub fn gates(run: &Run<'_>) -> (Verdict, [Gate; 7]) {
    let (verdict, beacons) = beacons(run);
    let reload = &run.rx.reload;
    let walker_restart = if reload.exhausted_republications == 0 {
        Outcome::Unexercised("pool-not-exhausted")
    } else if run.frames_after_release == 0 {
        Outcome::Fail("no-frames-after-release")
    } else {
        check(run.rx.capacity_drains >= 1, "no-capacity-drain")
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
        "w1.config channel={} applied_channel={} stand={} window_ms={} cpu_hz={hz} arena={COUNT}x{BUFFER} slots={SLOTS} mask={:#010x}",
        run.channel,
        run.applied_channel,
        Masked(run.stand),
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
            "w1.bss idx={index} who={who} bssid={} bi_tu={} ch={} beacons={} dup={} probe_rsp={} rssi={} no_rssi={} gaps_late={} max_gap_us={} missed={} lag_us={} ssid_len={}",
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
            "w1.timer cmp={} phase={} early={} late_count={} late_max_us={} late_total_us={} missed={} unmatched={} coalesced={} overlapping={} armed_at_end={}",
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
        ))?;
    }
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

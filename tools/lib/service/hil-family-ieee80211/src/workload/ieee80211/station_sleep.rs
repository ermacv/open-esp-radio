//! A station in modem sleep: the fixture runs the scenario's beacon
//! schedule, the station its power save, and ICMP probing keeps the link in
//! use while the station's RF sleeps between beacons.
//!
//! The RF sleep edges carry the raw MAC local-time counter beside monotonic
//! time (`RfSleepEntered`, `RfWoke`). Across one sleep the two distances
//! agree when the counter runs, and the MAC distance falls short when it
//! holds still: whether the coexistence cycle, which the vendor counts in
//! MAC local time and the model in monotonic time, is the same through sleep.

use std::{fs, path::Path, time::Duration};

use oer_hil_link::SerialCapture;
use oer_hil_protocol::telemetry::{TraceControl, TraceEntry};
use oer_hil_workload::context::Context;
use oer_ieee80211_trace::{RfSleepEntered, RfWoke};
use oer_trace::Event;
use serde::Serialize;

use crate::{Result, scenario::station::StationSleep, workload::traffic::icmp_latency};
use oer_hil_family_ieee80211_fixture::prepared::Prepared;

/// How far the counter's distance across one sleep may differ from the
/// monotonic distance while it runs: the two readings of each edge are back
/// to back in one task, but an interrupt may fall between them, and the MAC
/// clock may drift by its 1 ppm bound over the sleep.
const SKEW_MICROS: u32 = 64;

/// What the MAC counter did across one RF sleep.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CounterAcrossSleep {
    /// Its distance equals the monotonic distance.
    Runs,
    /// Its distance falls short: it stood still for part of the sleep.
    HoldsStill,
    /// Its distance exceeds the monotonic distance.
    Jumps,
}

/// One RF sleep: the counter's and the monotonic distance from the sleep
/// edge to the wake edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct RfSleep {
    pub mac_micros: u32,
    pub monotonic_micros: u32,
    pub counter: CounterAcrossSleep,
}

impl RfSleep {
    fn new(sleep: RfSleepEntered, wake: RfWoke) -> Self {
        let mac_micros = wake.mac_local_time.wrapping_sub(sleep.mac_local_time);
        let monotonic_micros = wake.monotonic_micros.wrapping_sub(sleep.monotonic_micros);
        let tolerance = SKEW_MICROS.saturating_add(monotonic_micros / 1_000_000);
        let counter = if mac_micros.abs_diff(monotonic_micros) <= tolerance {
            CounterAcrossSleep::Runs
        } else if mac_micros < monotonic_micros {
            CounterAcrossSleep::HoldsStill
        } else {
            CounterAcrossSleep::Jumps
        };
        Self {
            mac_micros,
            monotonic_micros,
            counter,
        }
    }
}

/// The RF sleeps of a trace, each an entry of its sleep edge followed by its
/// wake edge, in trace time order. An edge without its partner (the trace
/// started or wrapped between them) is left out.
pub(crate) fn rf_sleeps(entries: &[TraceEntry]) -> Vec<RfSleep> {
    let sleep_kind = RfSleepEntered::KIND.raw();
    let wake_kind = RfWoke::KIND.raw();
    let mut edges: Vec<&TraceEntry> = entries
        .iter()
        .filter(|entry| entry.kind == sleep_kind || entry.kind == wake_kind)
        .collect();
    edges.sort_by_key(|entry| entry.tag);
    let mut sleeps = Vec::new();
    let mut pending = None;
    for entry in edges {
        if entry.kind == sleep_kind {
            pending = RfSleepEntered::decode(entry.words);
        } else if let (Some(sleep), Some(wake)) = (pending.take(), RfWoke::decode(entry.words)) {
            sleeps.push(RfSleep::new(sleep, wake));
        }
    }
    sleeps
}

#[derive(Debug, Serialize)]
struct Report<'a> {
    schema: u32,
    power_save: oer_hil_protocol::wifi::WifiStationPowerSave,
    access_point_beacon: oer_hil_scenario_catalog::link::AccessPointBeacon,
    rf_sleeps: usize,
    counter_runs: usize,
    counter_holds_still: usize,
    counter_jumps: usize,
    sleeps: &'a [RfSleep],
}

/// The ICMP probing of a sleeping station.
fn icmp_config(workload: &StationSleep) -> icmp_latency::Config {
    icmp_latency::Config {
        count: workload.count,
        interval: Duration::from_millis(u64::from(workload.interval_ms)),
        timeout: Duration::from_millis(u64::from(workload.timeout_ms)),
        payload_bytes: usize::from(workload.payload_bytes),
        // Sleep stretches round trips to the DTIM; latency and loss are
        // recorded, not judged, beyond one reply proving the link.
        maximum_lost: workload.count - 1,
        maximum_p95: None,
        ..Default::default()
    }
}

pub fn run(
    workload: &StationSleep,
    output: &Path,
    context: &Context<'_>,
    _fixture: &Prepared,
) -> Result<()> {
    fs::create_dir_all(output)?;
    let mask = RfSleepEntered::CHANNEL.mask() | RfWoke::CHANNEL.mask();
    let entries = icmp_latency::run_observed(
        icmp_config(workload),
        output,
        context,
        false,
        |capture: &SerialCapture| {
            let status = capture
                .request(
                    0,
                    oer_hil_protocol::telemetry::ControlTrace(TraceControl::Start { mask }),
                    std::time::Duration::from_secs(5),
                )
                .map(|state| state.0)?;
            if !status.installed || !status.running {
                return Err(format!("the RF sleep trace did not start: {status:?}").into());
            }
            Ok(())
        },
        |capture: &SerialCapture| {
            let status = capture
                .request(
                    0,
                    oer_hil_protocol::telemetry::ControlTrace(TraceControl::Status),
                    std::time::Duration::from_secs(5),
                )
                .map(|state| state.0)?;
            capture.trace_entries(status.entries)
        },
    )?;
    let sleeps = rf_sleeps(&entries);
    let count = |counter| {
        sleeps
            .iter()
            .filter(|sleep| sleep.counter == counter)
            .count()
    };
    let report = Report {
        schema: 1,
        power_save: workload.power_save,
        access_point_beacon: workload.access_point_beacon,
        rf_sleeps: sleeps.len(),
        counter_runs: count(CounterAcrossSleep::Runs),
        counter_holds_still: count(CounterAcrossSleep::HoldsStill),
        counter_jumps: count(CounterAcrossSleep::Jumps),
        sleeps: &sleeps,
    };
    context.results.observe("station-sleep", &report);
    context
        .measurements
        .check("wifi.station.rf-sleep-observed", !sleeps.is_empty());
    eprintln!(
        "station_sleep rf_sleeps={} counter_runs={} counter_holds_still={} counter_jumps={} \
         beacon_interval_tu={} dtim_period={}",
        report.rf_sleeps,
        report.counter_runs,
        report.counter_holds_still,
        report.counter_jumps,
        workload.access_point_beacon.interval_tu,
        workload.access_point_beacon.dtim_period,
    );
    if sleeps.is_empty() {
        return Err("the station's RF never slept under its power save".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(tag: u16, kind: u16, mac: u32, monotonic: u32) -> TraceEntry {
        TraceEntry {
            tag,
            kind,
            t_us: monotonic,
            words: [mac, monotonic],
        }
    }

    #[test]
    fn the_sleep_probing_is_a_valid_icmp_session_for_every_count() {
        for count in [1, 60, u16::MAX] {
            let workload = StationSleep {
                link: crate::scenario::LinkExpectation {
                    phy: oer_hil_scenario_catalog::link::PhyExpectation::He20,
                    minimum_mcs: None,
                    guard_interval: Default::default(),
                    management_frame_protection: Default::default(),
                    access_point_security: Default::default(),
                },
                power_save: oer_hil_protocol::wifi::WifiStationPowerSave::MinModem,
                access_point_beacon: oer_hil_scenario_catalog::link::AccessPointBeacon {
                    interval_tu: 100,
                    dtim_period: 3,
                },
                count,
                interval_ms: 500,
                timeout_ms: 2000,
                payload_bytes: 56,
            };
            assert!(icmp_config(&workload).validate().is_ok());
        }
    }

    #[test]
    fn rf_sleeps_pair_edges_and_classify_the_counter() {
        let sleep = RfSleepEntered::KIND.raw();
        let wake = RfWoke::KIND.raw();
        let entries = [
            // A wake whose sleep the trace missed is left out.
            entry(1, wake, 50, 50),
            // The counter runs across the sleep.
            entry(2, sleep, 1_000, 10_000),
            entry(3, wake, 101_000, 110_010),
            // It holds still: 90 ms of sleep, 300 µs of counter.
            entry(4, sleep, 200_000, 200_000),
            entry(5, wake, 200_300, 290_000),
            // It jumps, across its 32-bit wrap.
            entry(6, sleep, u32::MAX - 10, 300_000),
            entry(7, wake, 50_000, 310_000),
        ];
        let sleeps = rf_sleeps(&entries);
        assert_eq!(
            sleeps.iter().map(|sleep| sleep.counter).collect::<Vec<_>>(),
            [
                CounterAcrossSleep::Runs,
                CounterAcrossSleep::HoldsStill,
                CounterAcrossSleep::Jumps,
            ]
        );
        assert_eq!(sleeps[0].mac_micros, 100_000);
        assert_eq!(sleeps[0].monotonic_micros, 100_010);
        assert_eq!(sleeps[2].mac_micros, 50_011);
    }
}

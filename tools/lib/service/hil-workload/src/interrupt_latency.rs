//! The latency of a software interrupt raised at the radio interrupts'
//! level, timed on the device (`system/interrupt-latency/measure`, ).
//!
//! A workload calls [`measure`] while its traffic runs, spreading the
//! samples over its traffic window after a lead for the traffic to start,
//! and again once it ended. [`record`] writes both as the `interrupt-latency` observation, in
//! nanoseconds as well as cycles, and refuses a missed sample.

use std::time::Duration;

use oer_hil_link::SerialCapture;
use oer_hil_protocol::system::{
    InterruptLatencyEvidence, InterruptLatencyRequest, MeasureInterruptLatency,
};
use serde::Serialize;

use crate::{Result, results::Results};

/// What the workload's clients did while the samples were taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Load {
    /// The link was up, with no traffic.
    Idle,
    /// The workload's traffic ran.
    Traffic,
}

/// One request's samples.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Sample {
    pub load: Load,
    pub evidence: InterruptLatencyEvidence,
}

#[derive(Serialize)]
struct Nanoseconds {
    load: Load,
    minimum: u64,
    median: u64,
    p99: u64,
    maximum: u64,
}

#[derive(Serialize)]
struct Observation<'a> {
    nanoseconds: Vec<Nanoseconds>,
    samples: &'a [Sample],
}

/// The time the traffic gets to start before samples are taken under it.
pub const TRAFFIC_LEAD: Duration = Duration::from_secs(2);
/// The interval between idle samples.
const IDLE_INTERVAL: Duration = Duration::from_millis(1);
/// How long a request may outlast its samples' spread.
const REQUEST_MARGIN: Duration = Duration::from_secs(10);

/// Take `samples` samples on the device under `load`, spread evenly over
/// `window`, the time from now that the load lasts. An idle load takes them
/// at a fixed short interval instead.
///
/// # Errors
///
/// The request failed.
pub fn measure(
    capture: &SerialCapture,
    samples: u16,
    load: Load,
    window: Option<Duration>,
) -> Result<Sample> {
    let interval = match (load, window) {
        (Load::Traffic, Some(window)) => {
            std::thread::sleep(TRAFFIC_LEAD.min(window));
            spread(window.saturating_sub(TRAFFIC_LEAD), samples)
        }
        _ => IDLE_INTERVAL,
    };
    let interval_micros = u32::try_from(interval.as_micros()).unwrap_or(u32::MAX);
    let evidence = capture
        .request(
            0,
            MeasureInterruptLatency(InterruptLatencyRequest {
                samples,
                interval_micros,
            }),
            interval * u32::from(samples) + REQUEST_MARGIN,
        )?
        .0;
    Ok(Sample { load, evidence })
}

/// The interval that spreads `samples` samples over `window`, leaving the
/// last tenth for the request to end inside it.
fn spread(window: Duration, samples: u16) -> Duration {
    window.mul_f64(0.9) / u32::from(samples.max(1))
}

fn nanoseconds(cycles: u32, cpu_hz: u32) -> u64 {
    u64::from(cycles) * 1_000_000_000 / u64::from(cpu_hz.max(1))
}

/// Write `samples` as the `interrupt-latency` observation.
///
/// # Errors
///
/// A request missed samples or took none.
pub fn record(results: &Results, samples: &[Sample]) -> Result<()> {
    let nanoseconds = samples
        .iter()
        .map(|sample| {
            let evidence = sample.evidence;
            let ns = |cycles| nanoseconds(cycles, evidence.cpu_hz);
            Nanoseconds {
                load: sample.load,
                minimum: ns(evidence.minimum_cycles),
                median: ns(evidence.median_cycles),
                p99: ns(evidence.p99_cycles),
                maximum: ns(evidence.maximum_cycles),
            }
        })
        .collect();
    results.observe(
        "interrupt-latency",
        &Observation {
            nanoseconds,
            samples,
        },
    );
    check(samples)
}

fn check(samples: &[Sample]) -> Result<()> {
    for sample in samples {
        if sample.evidence.missed != 0 || sample.evidence.samples == 0 {
            return Err(format!("the interrupt-latency probe missed samples: {sample:?}").into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

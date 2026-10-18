//! Independent agreement check between target alarms and its monotonic clock.

use oer_hil_workload::{boots::for_each_boot, context::Context, require_keys};
use std::{
    path::Path,
    time::{Duration, Instant},
};

use oer_hil_protocol::system::{
    ProbeTimebase, TimebaseProbe, TimebaseProbeEvidence, TimebaseProbeRequest, TimebaseProbed,
};
use serde::Serialize;

use crate::Result;
use oer_hil_link::SerialCapture;

const COMMAND_SLACK: Duration = Duration::from_secs(5);

pub struct Config {
    pub boots: u8,
    pub intervals: u16,
    pub period_millis: u16,
}

#[derive(Serialize)]
struct BootReport {
    host_elapsed_micros: u64,
    target: TimebaseProbeEvidence,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    let expected_micros = u64::from(config.period_millis) * 1_000 * u64::from(config.intervals);
    for_each_boot(
        context,
        output,
        config.boots,
        |_, capture| probe(capture, &config),
        |_, report| validate(report.target, report.host_elapsed_micros, expected_micros),
    )?;
    eprintln!("timebase=PASS boots={}", config.boots);
    Ok(())
}

fn probe(capture: &SerialCapture, config: &Config) -> Result<BootReport> {
    require_keys::<TimebaseProbe>(capture)?;
    let period_micros = u32::from(config.period_millis) * 1_000;
    let request = TimebaseProbeRequest {
        intervals: config.intervals,
        period_micros,
    };
    let expected_micros = u64::from(period_micros) * u64::from(config.intervals);
    let started = Instant::now();
    let TimebaseProbed(target) = capture.request(
        0,
        ProbeTimebase(request),
        Duration::from_micros(expected_micros).saturating_add(COMMAND_SLACK),
    )?;
    let host_elapsed_micros = started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64;
    Ok(BootReport {
        host_elapsed_micros,
        target,
    })
}

fn validate(
    evidence: TimebaseProbeEvidence,
    host_elapsed_micros: u64,
    expected_micros: u64,
) -> Result<()> {
    let device_min = expected_micros * 95 / 100;
    let device_max = expected_micros * 110 / 100;
    let host_min = expected_micros * 95 / 100;
    let host_max = expected_micros * 120 / 100;
    let interval_min = u64::from(evidence.period_micros) * 95 / 100;
    let interval_max = u64::from(evidence.period_micros) * 150 / 100;
    if evidence.elapsed_micros < device_min
        || evidence.elapsed_micros > device_max
        || u64::from(evidence.minimum_interval_micros) < interval_min
        || u64::from(evidence.maximum_interval_micros) > interval_max
        || evidence.early_intervals != 0
        || host_elapsed_micros < host_min
        || host_elapsed_micros > host_max
    {
        return Err(format!(
            "timebase agreement failed: expected_us={expected_micros} host_us={host_elapsed_micros} target={evidence:?}"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;

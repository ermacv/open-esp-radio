//! The RF hold of one shared-PHY tracking tick, timed on the device
//! (`phy/tracking/hold/measure`, ).
//!
//! A workload of another family calls this while its own clients use the
//! radio. [`suspend`] stops the device's periodic tracking timer, so the
//! timed ticks are the only tracking. [`measure`] forces each
//! [`CONFIGURATIONS`] branch set with a zero threshold and times
//! `ticks` due ticks of it, under the workload's [`Load`]. [`record`] writes
//! the samples as the `phy-tracking-hold` observation and refuses a failed,
//! skipped or malformed tick. [`resume`] restarts the timer.
//!
//! A grant-protect interval is the time the PHY's coexistence request was
//! programmed, not a proof that the arbiter kept every other radio off the
//! air.

use std::time::{Duration, Instant};

use oer_tool_service_hil_link::SerialCapture;
use oer_tool_data_hil_protocol::phy::{
    ControlTracking, MeasureTrackingHold, PhyTrackingCommand, PhyTrackingHoldEvidence,
    PhyTrackingHoldRequest, PhyTrackingHoldTick, PhyTrackingHoldTiming,
};
use serde::Serialize;

use crate::{Result, results::Results};

/// One forced branch set.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Configuration {
    pub name: &'static str,
    pub rfpll: bool,
    pub calibration: bool,
}

/// RFPLL alone, calibration (common RX and TXDC/PWDET) alone, and both.
pub const CONFIGURATIONS: [Configuration; 3] = [
    Configuration {
        name: "rfpll",
        rfpll: true,
        calibration: false,
    },
    Configuration {
        name: "calibration",
        rfpll: false,
        calibration: true,
    },
    Configuration {
        name: "both",
        rfpll: true,
        calibration: true,
    },
];

/// What the workload's clients did while the ticks ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Load {
    /// The link or session was up, with no traffic.
    Idle,
    /// The workload's traffic ran: interrupts compete with the tick.
    Traffic,
}

/// One timed tick.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Sample {
    pub configuration: &'static str,
    pub load: Load,
    pub evidence: PhyTrackingHoldEvidence,
}

/// The longest hold of one configuration under one load.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Bound {
    pub ticks: u16,
    pub tracking_micros: u32,
    pub grant_protect_rfpll_micros: u32,
    pub grant_protect_common_micros: u32,
    pub grant_protect_transmit_micros: u32,
    pub wifi_baseband_off_micros: u32,
}

#[derive(Serialize)]
struct Observation<'a> {
    bounds: Vec<(&'static str, Load, Bound)>,
    samples: &'a [Sample],
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// The time one due tick takes to come round: the tracking period is one
/// second, and a tick that was not due waits for the next.
const TICK_WINDOW: Duration = Duration::from_secs(2);

/// The time [`measure`] needs for `ticks` due ticks of every configuration.
pub fn window(ticks: u16) -> Duration {
    TICK_WINDOW * u32::from(ticks) * CONFIGURATIONS.len() as u32
}
/// How long to wait before asking again when no tracking was due.
const NOT_DUE_RETRY: Duration = Duration::from_millis(250);

/// Stop the device's periodic tracking timer.
///
/// # Errors
///
/// The request failed or the timer still runs.
pub fn suspend(capture: &SerialCapture) -> Result<()> {
    let state = capture
        .request(
            0,
            ControlTracking(PhyTrackingCommand::Suspend),
            REQUEST_TIMEOUT,
        )?
        .0;
    if state.running {
        return Err(format!("PHY tracking did not suspend: {state:?}").into());
    }
    Ok(())
}

/// Restart the device's periodic tracking timer.
///
/// # Errors
///
/// The request failed or the timer did not restart.
pub fn resume(capture: &SerialCapture) -> Result<()> {
    let state = capture
        .request(
            0,
            ControlTracking(PhyTrackingCommand::Resume),
            REQUEST_TIMEOUT,
        )?
        .0;
    if !state.running {
        return Err(format!("PHY tracking did not resume: {state:?}").into());
    }
    Ok(())
}

/// Time up to `ticks` due ticks of each configuration under `load`, until
/// `deadline`. A tick that was not due is asked again after a short wait.
/// The samples include every timed tick, whatever its outcome.
///
/// # Errors
///
/// A request failed.
pub fn measure(
    capture: &SerialCapture,
    ticks: u16,
    load: Load,
    deadline: Instant,
) -> Result<Vec<Sample>> {
    let mut samples = Vec::new();
    for configuration in CONFIGURATIONS {
        let mut timed = 0;
        while timed < ticks && Instant::now() < deadline {
            let evidence = capture
                .request(
                    0,
                    MeasureTrackingHold(PhyTrackingHoldRequest {
                        rfpll: configuration.rfpll,
                        calibration: configuration.calibration,
                        threshold: 0,
                    }),
                    REQUEST_TIMEOUT,
                )?
                .0;
            if evidence.tick == PhyTrackingHoldTick::NotDue {
                std::thread::sleep(NOT_DUE_RETRY);
                continue;
            }
            samples.push(Sample {
                configuration: configuration.name,
                load,
                evidence,
            });
            timed += 1;
        }
    }
    Ok(samples)
}

/// The longest hold per configuration and load.
pub fn bounds(samples: &[Sample]) -> Vec<(&'static str, Load, Bound)> {
    let mut bounds: Vec<(&'static str, Load, Bound)> = Vec::new();
    for sample in samples {
        if sample.evidence.tick != PhyTrackingHoldTick::Tracked {
            continue;
        }
        let index = match bounds
            .iter()
            .position(|(name, load, _)| *name == sample.configuration && *load == sample.load)
        {
            Some(index) => index,
            None => {
                bounds.push((sample.configuration, sample.load, Bound::default()));
                bounds.len() - 1
            }
        };
        let bound = &mut bounds[index].2;
        let evidence = &sample.evidence;
        let longest =
            |current: u32, timing: PhyTrackingHoldTiming| current.max(timing.maximum_micros);
        bound.ticks += 1;
        bound.tracking_micros = longest(bound.tracking_micros, evidence.tracking);
        bound.grant_protect_rfpll_micros = longest(
            bound.grant_protect_rfpll_micros,
            evidence.grant_protect_rfpll,
        );
        bound.grant_protect_common_micros = longest(
            bound.grant_protect_common_micros,
            evidence.grant_protect_common,
        );
        bound.grant_protect_transmit_micros = longest(
            bound.grant_protect_transmit_micros,
            evidence.grant_protect_transmit,
        );
        bound.wifi_baseband_off_micros =
            longest(bound.wifi_baseband_off_micros, evidence.wifi_baseband_off);
    }
    bounds
}

/// Write `samples` as the `phy-tracking-hold` observation.
///
/// # Errors
///
/// A tick failed, was skipped or has malformed timing, or a configuration
/// under a measured load timed no tick.
pub fn record(results: &Results, samples: &[Sample]) -> Result<()> {
    let bounds = bounds(samples);
    results.observe(
        "phy-tracking-hold",
        &Observation {
            bounds: bounds.clone(),
            samples,
        },
    );
    check(samples, &bounds)
}

fn check(samples: &[Sample], bounds: &[(&'static str, Load, Bound)]) -> Result<()> {
    if let Some(sample) = samples.iter().find(|sample| {
        sample.evidence.invalid
            || matches!(
                sample.evidence.tick,
                PhyTrackingHoldTick::Failed | PhyTrackingHoldTick::Skipped
            )
    }) {
        return Err(format!("a timed tracking tick did not complete: {sample:?}").into());
    }
    for load in [Load::Idle, Load::Traffic] {
        if !samples.iter().any(|sample| sample.load == load) {
            continue;
        }
        for configuration in CONFIGURATIONS {
            if !bounds
                .iter()
                .any(|(name, measured, _)| *name == configuration.name && *measured == load)
            {
                return Err(format!(
                    "no tracking tick of `{}` was timed under {load:?}",
                    configuration.name
                )
                .into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

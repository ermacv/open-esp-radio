//! Host validation of the single-device IEEE 802.15.4 on-air check.
//!
//! Each cycle starts IEEE 802.15.4 as a client of the shared radio arbiter,
//! runs an energy scan, a clear-channel assessment, a direct transmit without
//! an acknowledgement request, two transmits scheduled through the MAC timer
//! and ETM route, a receive window and a receive window scheduled through
//! the MAC timer, then stops the client. The accepted result proves that the
//! composed client starts, completes each operation with a terminal event
//! and stops again, that scheduled transmits neither start early nor miss
//! their window, and that the scheduled receive window receives nothing
//! before it opens and ends at its end or with its first frame. No peer
//! observes the air.

use oer_hil_link::SerialCapture;
use oer_hil_workload::{boots::for_each_boot, context::Context, require_keys};
use std::{path::Path, time::Duration};

use oer_hil_protocol::{
    ieee802154::Ieee802154AirCcaOutcome, ieee802154::Ieee802154AirCheckEvidence,
    ieee802154::Ieee802154AirCheckRequest, ieee802154::Ieee802154AirCheckStop,
    ieee802154::Ieee802154AirCycle, ieee802154::Ieee802154AirEnergyOutcome,
    ieee802154::Ieee802154AirTransmit, ieee802154::Ieee802154AirTxOutcome,
    ieee802154::Ieee802154AirWindow,
};

use crate::Result;

const RESULT: &str = "arbiter-client-start-operations-stop";
/// What a passing air check does not establish.
const NOT_PROVEN: &[&str] = &[
    "peer-reception-of-transmitted-frames",
    "acknowledged-exchange",
    "calibrated-output-power",
    "calibrated-energy-or-rssi",
    "receive-filtering-with-traffic",
    "scheduled-receive-of-a-peer-frame",
    "periodic-phy-tracking-while-running",
];

/// Longest accepted delay from a scheduled start to its completion event:
/// the frame's airtime at 250 kbit/s plus event delivery to the task.
pub const SCHEDULED_COMPLETION_BOUND_MICROS: u64 = 10_000;

/// Plausible range of an averaged ED reading in dBm.
const ENERGY_RANGE_DBM: core::ops::RangeInclusive<i8> = -110..=10;

pub struct Config {
    pub boots: u8,
    pub request: Ieee802154AirCheckRequest,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    if !config.request.validate() {
        return Err("invalid IEEE 802.15.4 air check bounds".into());
    }
    context.results.claim(RESULT, NOT_PROVEN);
    let timeout = command_timeout(config.request);
    for_each_boot(
        context,
        output,
        config.boots,
        |_, capture| check(capture, config.request, timeout),
        |_, target| validate(*target, config.request),
    )?;
    println!(
        "ieee802154_air_check=PASS result={RESULT} boots={} cycles={}",
        config.boots, config.request.cycles
    );
    Ok(())
}

/// Every cycle runs its bounded operations plus bring-up, which may register
/// and calibrate the shared PHY.
fn command_timeout(request: Ieee802154AirCheckRequest) -> Duration {
    let per_cycle = Duration::from_secs(5)
        + Duration::from_millis(u64::from(request.receive_window_millis))
        + Duration::from_micros(3 * u64::from(request.scheduled_lead_micros))
        + Duration::from_micros(u64::from(request.scheduled_window_micros));
    Duration::from_secs(20) + per_cycle * u32::from(request.cycles)
}

fn check(
    capture: &SerialCapture,
    request: Ieee802154AirCheckRequest,
    timeout: Duration,
) -> Result<Ieee802154AirCheckEvidence> {
    require_keys::<oer_hil_protocol::ieee802154::AirCheck>(capture)?;
    let oer_hil_protocol::ieee802154::AirCheckCompleted(evidence) = capture.request(
        0,
        oer_hil_protocol::ieee802154::RunAirCheck(request),
        timeout,
    )?;
    Ok(evidence)
}

fn validate(
    evidence: Ieee802154AirCheckEvidence,
    request: Ieee802154AirCheckRequest,
) -> Result<()> {
    if evidence.stop != Ieee802154AirCheckStop::Complete {
        return Err(format!(
            "IEEE 802.15.4 air check stopped at {:?} after {} cycles",
            evidence.stop, evidence.completed_cycles
        )
        .into());
    }
    if evidence.completed_cycles != request.cycles {
        return Err(format!(
            "IEEE 802.15.4 air check completed {} of {} cycles",
            evidence.completed_cycles, request.cycles
        )
        .into());
    }
    for (index, cycle) in evidence.cycles[..usize::from(request.cycles)]
        .iter()
        .enumerate()
    {
        validate_cycle(index + 1, cycle)?;
    }
    Ok(())
}

fn validate_cycle(cycle_number: usize, cycle: &Ieee802154AirCycle) -> Result<()> {
    let fail = |what: String| -> Result<()> {
        Err(format!("IEEE 802.15.4 air check cycle {cycle_number}: {what}").into())
    };
    match cycle.energy {
        Ieee802154AirEnergyOutcome::Energy(dbm) if ENERGY_RANGE_DBM.contains(&dbm) => {}
        other => return fail(format!("energy scan {other:?}")),
    }
    if !matches!(
        cycle.cca,
        Ieee802154AirCcaOutcome::Clear | Ieee802154AirCcaOutcome::Busy
    ) {
        return fail(format!("clear-channel assessment {:?}", cycle.cca));
    }
    validate_transmit(&cycle.direct).or_else(|what| fail(format!("direct transmit {what}")))?;
    for (index, scheduled) in cycle.scheduled.iter().enumerate() {
        validate_transmit(scheduled)
            .or_else(|what| fail(format!("scheduled transmit {} {what}", index + 1)))?;
        let late = scheduled.done_at_micros - scheduled.requested_at_micros;
        if late > SCHEDULED_COMPLETION_BOUND_MICROS {
            return fail(format!(
                "scheduled transmit {} completed {late} us after its start",
                index + 1
            ));
        }
    }
    if cycle.scheduled[1].requested_at_micros <= cycle.scheduled[0].done_at_micros {
        return fail("scheduled transmits overlap".into());
    }
    validate_window(&cycle.scheduled_window)
        .or_else(|what| fail(format!("scheduled receive window {what}")))
}

/// A scheduled window receives nothing before it opens. Without frames it
/// ends at its end; its first frame ends it, as the vendor driver's receive
/// path does. Either end is observed within the completion bound.
pub(crate) fn validate_window(window: &Ieee802154AirWindow) -> core::result::Result<(), String> {
    if !window.ended {
        return Err("did not end".into());
    }
    if let Some(first) = window.first_frame_at_micros
        && first < window.start_micros
    {
        return Err(format!(
            "received a frame at {first}, before it opened at {}",
            window.start_micros
        ));
    }
    let (earliest, latest) = match window.first_frame_at_micros {
        None => (
            window.end_micros,
            window.end_micros + SCHEDULED_COMPLETION_BOUND_MICROS,
        ),
        Some(first) => (first, first + SCHEDULED_COMPLETION_BOUND_MICROS),
    };
    if window.done_at_micros < earliest || window.done_at_micros > latest {
        return Err(format!(
            "ended at {}, outside {earliest}..={latest}",
            window.done_at_micros
        ));
    }
    if (window.received_frames == 0) != window.first_frame_at_micros.is_none() {
        return Err(format!(
            "counted {} frames against a first-frame time {:?}",
            window.received_frames, window.first_frame_at_micros
        ));
    }
    Ok(())
}

fn validate_transmit(transmit: &Ieee802154AirTransmit) -> core::result::Result<(), String> {
    if transmit.outcome != Ieee802154AirTxOutcome::Success {
        return Err(format!("ended {:?}", transmit.outcome));
    }
    if transmit.done_at_micros < transmit.requested_at_micros {
        return Err(format!(
            "completed at {} before its start {}",
            transmit.done_at_micros, transmit.requested_at_micros
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

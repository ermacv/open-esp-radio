//! Background shared PHY maintenance of the running IEEE 802.15.4 client.
//!
//! One session runs with background maintenance for several tracking
//! periods: the device runs the PHY's periodic tracking loop with the radio
//! running, as ESP-IDF's vendor timer does, and never pauses its MAC. The
//! device must track the shared PHY on its own and still transmit
//! afterwards. No peer is needed.

use std::{path::Path, time::Duration};

use oer_hil_link::SerialCapture;
use oer_hil_protocol::{
    ieee802154::Ieee802154AirTxOutcome, ieee802154::Ieee802154SessionConfig,
    ieee802154::Ieee802154SessionStopEvidence, ieee802154::Ieee802154SessionTransmitRequest,
    ieee802154::Ieee802154SessionTxMode,
};
use oer_hil_workload::{boots::for_each_boot, context::Context, require_keys};
use serde::Serialize;

use super::peer_exchange::{DEVICE_SHORT, PEER_SHORT, data_frame, expect_session, session_frame};
use crate::Result;

const START_TIMEOUT: Duration = Duration::from_secs(30);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Config {
    pub boots: u8,
    pub channel: u8,
    /// Whole tracking periods the session runs.
    pub periods: u8,
}

/// The session lasts this long; the first attempt comes one period after
/// start, so `periods` attempts fit.
fn run_time(periods: u8) -> Duration {
    Duration::from_millis(1_000 * u64::from(periods) + 500)
}

/// Tracking must run on at least every other attempt: the domain's schedule
/// is due one period after the last tracking, and an attempt landing just
/// before that instant finds it not yet due.
pub(crate) fn validate(evidence: &Ieee802154SessionStopEvidence, periods: u8) -> Result<()> {
    expect_session("stop", evidence.result)?;
    let counts = evidence.maintenance;
    if counts.failed {
        return Err("background PHY maintenance failed".into());
    }
    let minimum = u16::from(periods).div_ceil(2);
    if counts.tracked < minimum {
        return Err(format!(
            "background maintenance tracked {} times in {periods} periods, expected at least {minimum}",
            counts.tracked
        )
        .into());
    }
    Ok(())
}

#[derive(Serialize)]
struct BootReport {
    tracked: u16,
    not_due: u16,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    context.results.claim(
        "periodic-tracking-with-the-radio-running",
        &[
            "tracking-during-traffic",
            "joint-maintenance-with-other-clients",
        ],
    );
    for_each_boot(
        context,
        output,
        config.boots,
        |_, capture| {
            let evidence = session(capture, &config)?;
            validate(&evidence, config.periods)?;
            Ok(BootReport {
                tracked: evidence.maintenance.tracked,
                not_due: evidence.maintenance.not_due,
            })
        },
        |_, _| Ok(()),
    )?;
    println!(
        "ieee802154_background_maintenance=PASS boots={} periods={}",
        config.boots, config.periods
    );
    Ok(())
}

fn session(capture: &SerialCapture, config: &Config) -> Result<Ieee802154SessionStopEvidence> {
    require_keys::<oer_hil_protocol::ieee802154::Session>(capture)?;
    expect_session(
        "start",
        capture
            .request(
                0,
                oer_hil_protocol::ieee802154::StartSession(Ieee802154SessionConfig {
                    channel: config.channel,
                    pan_id: super::peer_exchange::PAN_ID,
                    short_address: DEVICE_SHORT,
                    extended_address: [0; 8],
                    promiscuous: false,
                    background_maintenance: true,
                    enhanced_ack: false,
                    wifi_coexistence: false,
                }),
                START_TIMEOUT,
            )
            .map(|response| response.0)?,
    )?;
    capture
        .request(
            0,
            oer_hil_protocol::ieee802154::ReceiveSession,
            std::time::Duration::from_secs(5),
        )
        .map(drop)?;
    std::thread::sleep(run_time(config.periods));
    let transmitted = capture
        .request(
            0,
            oer_hil_protocol::ieee802154::TransmitSession(Ieee802154SessionTransmitRequest {
                frame: session_frame(&data_frame(false, 1, PEER_SHORT, DEVICE_SHORT))?,
                mode: Ieee802154SessionTxMode::Direct,
                max_frame_retries: 0,
            }),
            COMMAND_TIMEOUT,
        )
        .map(|response| response.0)?;
    expect_session("transmit", transmitted.result)?;
    if transmitted.outcome != Ieee802154AirTxOutcome::Success {
        return Err(format!("transmit after maintenance ended {:?}", transmitted.outcome).into());
    }
    capture
        .request(
            0,
            oer_hil_protocol::ieee802154::StopSession,
            COMMAND_TIMEOUT,
        )
        .map(|response| response.0)
}

#[cfg(test)]
mod tests {
    use oer_hil_protocol::{
        ieee802154::Ieee802154SessionMaintenanceCounts, ieee802154::Ieee802154SessionResult,
    };

    use super::*;

    fn evidence(tracked: u16) -> Ieee802154SessionStopEvidence {
        Ieee802154SessionStopEvidence {
            result: Ieee802154SessionResult::Done,
            maintenance: Ieee802154SessionMaintenanceCounts {
                not_due: 1,
                tracked,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn tracking_must_run_on_every_other_attempt() {
        validate(&evidence(2), 4).unwrap();
        validate(&evidence(2), 3).unwrap();
        assert!(validate(&evidence(1), 3).is_err());
        assert!(run_time(3) > Duration::from_secs(3));
    }

    #[test]
    fn a_failed_maintenance_fails_the_run() {
        let mut failed = evidence(4);
        failed.maintenance.failed = true;
        assert!(validate(&failed, 4).is_err());
        let mut stopped = evidence(4);
        stopped.result = Ieee802154SessionResult::StopFailed;
        assert!(validate(&stopped, 4).is_err());
    }
}

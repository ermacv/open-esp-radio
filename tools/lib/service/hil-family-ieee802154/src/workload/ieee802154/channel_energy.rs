//! IEEE 802.15.4 channel energy and clear-channel assessment against a busy
//! channel.
//!
//! The reference peer (the IEEE 802.15.4 peer project of `hil/peers`) transmits one
//! maximum-length frame back to back on the session channel. One session on
//! the device under test measures the energy on that channel and on a far
//! channel and assesses both, retuning between them:
//!
//! 1. before the burst, the session channel's energy is the baseline;
//! 2. during the burst, the session channel's energy rises above the
//!    baseline, its assessments report it busy, and transmissions that
//!    assess the channel first end with a busy channel;
//! 3. retuned to the far channel, the energy falls back below the burst's;
//!    retuned back, it returns to the burst's level;
//! 4. the energy of the burst agrees with the RSSI of its frames, which the
//!    device receives on the session channel;
//! 5. after the burst, the session channel's energy returns to the baseline.

use std::{path::Path, time::Duration};

use oer_hil_link::SerialCapture;
use oer_hil_protocol::{
    ieee802154::Ieee802154AirCcaOutcome, ieee802154::Ieee802154AirEnergyOutcome,
    ieee802154::Ieee802154AirTxOutcome, ieee802154::Ieee802154SessionAssessRequest,
    ieee802154::Ieee802154SessionAssessment, ieee802154::Ieee802154SessionConfig,
    ieee802154::Ieee802154SessionReceiveEvidence, ieee802154::Ieee802154SessionTransmitRequest,
    ieee802154::Ieee802154SessionTxMode, ieee802154::ieee802154_frame_crc32c,
};
use oer_hil_workload::{
    boots::{boot_directory, for_each_boot},
    context::Context,
    require_keys,
};
use serde::Serialize;

use super::peer_exchange::{
    COMMAND_TIMEOUT, DEVICE_EXTENDED, DEVICE_SHORT, PAN_ID, PEER_EXTENDED, PEER_SHORT,
    START_TIMEOUT, data_frame, expect_session, session_frame,
};
use crate::{
    Result,
    peer::{PEER_TRANSCRIPT, Peer, PeerBurst, PeerConfig},
};
use oer_hil_link::peer::{PeerLink, PeerTranscript};

/// The burst frame fills the PSDU: 125 MAC bytes and the FCS.
const BURST_FRAME_LENGTH: usize = 125;
/// Time for the burst to reach its steady state before measuring.
const BURST_SETTLE: Duration = Duration::from_millis(50);
/// Reception of the burst for the RSSI comparison.
const RECEIVE_WINDOW: Duration = Duration::from_millis(200);

/// The energy of the burst on its channel exceeds the baseline, and the far
/// channel's during the burst, by at least this much.
const MIN_BURST_RISE_DB: i16 = 15;
/// Retuned back to the burst's channel, the energy is within this of the
/// first burst measurement; after the burst, within this of the baseline.
const MAX_RETURN_DB: i16 = 6;
/// The burst's averaged energy and the RSSI of its frames differ by at most
/// this much: the energy scan averages over gaps between frames.
const MAX_ENERGY_RSSI_DB: i16 = 10;

pub struct Config {
    pub boots: u8,
    pub channel: u8,
    pub far_channel: u8,
    /// Assessments per step, and transmissions assessing the busy channel.
    pub samples: u8,
    pub energy_scan_micros: u32,
}

/// The maximum-length data frame the peer repeats, without acknowledgement
/// request, addressed to the device so that the device reports it.
pub fn burst_frame() -> Vec<u8> {
    let mut frame = data_frame(false, 0xb0, DEVICE_SHORT, PEER_SHORT);
    frame.resize(BURST_FRAME_LENGTH, b'B');
    frame
}

/// Everything one boot measured.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Measurements {
    pub baseline: Vec<Assessed>,
    pub baseline_far: Vec<Assessed>,
    pub burst: Vec<Assessed>,
    pub burst_far: Vec<Assessed>,
    pub burst_back: Vec<Assessed>,
    pub after: Vec<Assessed>,
    /// Outcomes of transmissions assessing the busy channel first.
    pub assessed_transmits: Vec<String>,
    /// RSSI of the burst frames the device received.
    pub burst_rssi_dbm: Vec<i8>,
    pub burst_frames_received: u16,
    pub peer_sent: u32,
    pub peer_failed: u32,
}

/// One completed assessment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Assessed {
    pub energy_dbm: i8,
    pub busy: bool,
}

impl Assessed {
    /// A completed scan and assessment; anything else fails the cell.
    pub fn from_assessment(assessment: Ieee802154SessionAssessment) -> Result<Self> {
        expect_session("assessment", assessment.result)?;
        let Ieee802154AirEnergyOutcome::Energy(energy_dbm) = assessment.energy else {
            return Err(format!("energy scan ended {:?}", assessment.energy).into());
        };
        let busy = match assessment.cca {
            Ieee802154AirCcaOutcome::Clear => false,
            Ieee802154AirCcaOutcome::Busy => true,
            other => return Err(format!("clear-channel assessment ended {other:?}").into()),
        };
        Ok(Self { energy_dbm, busy })
    }
}

/// The measured levels the cell decides on.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Summary {
    pub baseline_dbm: i16,
    pub burst_dbm: i16,
    pub burst_far_dbm: i16,
    pub burst_back_dbm: i16,
    pub after_dbm: i16,
    pub burst_rssi_dbm: i16,
    pub baseline_busy: usize,
    pub burst_busy: usize,
    pub assessed_transmits_busy: usize,
}

fn median(values: impl IntoIterator<Item = i8>) -> Option<i16> {
    let mut values: Vec<i16> = values.into_iter().map(i16::from).collect();
    values.sort_unstable();
    let middle = values.len() / 2;
    match values.len() {
        0 => None,
        len if len % 2 == 1 => Some(values[middle]),
        _ => Some((values[middle - 1] + values[middle]) / 2),
    }
}

fn energy(step: &str, assessed: &[Assessed]) -> Result<i16> {
    median(assessed.iter().map(|assessed| assessed.energy_dbm))
        .ok_or_else(|| format!("no {step} assessment").into())
}

fn busy(assessed: &[Assessed]) -> usize {
    assessed.iter().filter(|assessed| assessed.busy).count()
}

/// Decide the cell from one boot's measurements.
pub fn evaluate(measurements: &Measurements) -> Result<Summary> {
    let summary = Summary {
        baseline_dbm: energy("baseline", &measurements.baseline)?,
        burst_dbm: energy("burst", &measurements.burst)?,
        burst_far_dbm: energy("far-channel", &measurements.burst_far)?,
        burst_back_dbm: energy("retuned", &measurements.burst_back)?,
        after_dbm: energy("after-burst", &measurements.after)?,
        burst_rssi_dbm: median(measurements.burst_rssi_dbm.iter().copied())
            .ok_or("the device received no burst frame")?,
        baseline_busy: busy(&measurements.baseline),
        burst_busy: busy(&measurements.burst),
        assessed_transmits_busy: measurements
            .assessed_transmits
            .iter()
            .filter(|outcome| *outcome == "ChannelBusy")
            .count(),
    };
    if measurements.peer_sent == 0 {
        return Err("the peer sent no burst frame".into());
    }
    if summary.burst_dbm - summary.baseline_dbm < MIN_BURST_RISE_DB {
        return Err(format!(
            "burst energy {} dBm is not {MIN_BURST_RISE_DB} dB above the baseline {} dBm",
            summary.burst_dbm, summary.baseline_dbm
        )
        .into());
    }
    if summary.burst_dbm - summary.burst_far_dbm < MIN_BURST_RISE_DB {
        return Err(format!(
            "far-channel energy {} dBm is not {MIN_BURST_RISE_DB} dB below the burst's {} dBm",
            summary.burst_far_dbm, summary.burst_dbm
        )
        .into());
    }
    if (summary.burst_back_dbm - summary.burst_dbm).abs() > MAX_RETURN_DB {
        return Err(format!(
            "retuned energy {} dBm left the burst's {} dBm",
            summary.burst_back_dbm, summary.burst_dbm
        )
        .into());
    }
    if summary.after_dbm - summary.baseline_dbm > MAX_RETURN_DB {
        return Err(format!(
            "energy {} dBm after the burst did not return to the baseline {} dBm",
            summary.after_dbm, summary.baseline_dbm
        )
        .into());
    }
    if (summary.burst_dbm - summary.burst_rssi_dbm).abs() > MAX_ENERGY_RSSI_DB {
        return Err(format!(
            "burst energy {} dBm disagrees with its frames' RSSI {} dBm",
            summary.burst_dbm, summary.burst_rssi_dbm
        )
        .into());
    }
    let majority = measurements.burst.len().div_ceil(2);
    if summary.burst_busy < majority || summary.burst_busy <= summary.baseline_busy {
        return Err(format!(
            "{} of {} burst assessments were busy, {} of the baseline's",
            summary.burst_busy,
            measurements.burst.len(),
            summary.baseline_busy
        )
        .into());
    }
    if summary.assessed_transmits_busy < measurements.assessed_transmits.len().div_ceil(2) {
        return Err(format!(
            "{} of {} transmissions assessing the busy channel ended busy",
            summary.assessed_transmits_busy,
            measurements.assessed_transmits.len()
        )
        .into());
    }
    Ok(summary)
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    context.results.claim(
        "busy-channel-energy-assessment-and-retune-against-vendor-peer",
        &[
            "absolute-energy-accuracy",
            "cca-threshold-calibration",
            "csma-ca-backoff-under-load",
        ],
    );
    let peer_config = context.lab.peer()?;
    for_each_boot(
        context,
        output,
        config.boots,
        |boot, capture| {
            let transcript = PeerTranscript::default();
            let mut peer = Peer::open_recorded(&peer_config.serial()?, &transcript)?;
            let measured = measure(capture, &mut peer, &config);
            if measured.is_err() {
                // A failed step must not leave the peer transmitting; with
                // no burst running the peer refuses the stop.
                let _ = peer.stop_burst();
            }
            transcript.save(&boot_directory(output, boot).join(PEER_TRANSCRIPT))?;
            measured
        },
        |boot, measurements| {
            let summary = evaluate(measurements)?;
            context
                .results
                .observe(format!("boot-{boot:03}-summary"), &summary);
            Ok(())
        },
    )?;
    println!(
        "ieee802154_channel_energy=PASS boots={} samples={}",
        config.boots, config.samples
    );
    Ok(())
}

fn assess(
    capture: &SerialCapture,
    channel: u8,
    config: &Config,
    into: &mut Vec<Assessed>,
) -> Result<()> {
    for _ in 0..config.samples {
        let assessment = capture
            .request(
                0,
                oer_hil_protocol::ieee802154::AssessSessionChannel(
                    Ieee802154SessionAssessRequest {
                        channel,
                        energy_scan_micros: config.energy_scan_micros,
                    },
                ),
                COMMAND_TIMEOUT,
            )
            .map(|response| response.0)?;
        into.push(Assessed::from_assessment(assessment)?);
    }
    Ok(())
}

/// RSSI of the received frames that are the burst's.
fn burst_rssi(evidence: &Ieee802154SessionReceiveEvidence, frame: &[u8]) -> Result<Vec<i8>> {
    expect_session("receive", evidence.result)?;
    let digest = ieee802154_frame_crc32c(frame);
    Ok(evidence
        .frames
        .iter()
        .filter(|received| received.crc32c == digest && usize::from(received.length) == frame.len())
        .map(|received| received.rssi_dbm)
        .collect())
}

fn measure<L: PeerLink>(
    capture: &SerialCapture,
    peer: &mut Peer<L>,
    config: &Config,
) -> Result<Measurements> {
    require_keys::<oer_hil_protocol::ieee802154::Session>(capture)?;
    peer.configure(&PeerConfig {
        channel: config.channel,
        pan_id: PAN_ID,
        short_address: PEER_SHORT,
        extended_address: PEER_EXTENDED,
        promiscuous: false,
        power_dbm: 0,
    })?;
    peer.receive()?;
    expect_session(
        "start",
        capture
            .request(
                0,
                oer_hil_protocol::ieee802154::StartSession(Ieee802154SessionConfig {
                    channel: config.channel,
                    pan_id: PAN_ID,
                    short_address: DEVICE_SHORT,
                    extended_address: DEVICE_EXTENDED,
                    promiscuous: false,
                    background_maintenance: false,
                    enhanced_ack: false,
                    wifi_coexistence: false,
                }),
                START_TIMEOUT,
            )
            .map(|response| response.0)?,
    )?;

    let mut measurements = Measurements::default();
    assess(capture, config.channel, config, &mut measurements.baseline)?;
    assess(
        capture,
        config.far_channel,
        config,
        &mut measurements.baseline_far,
    )?;

    let frame = burst_frame();
    peer.burst(&frame)?;
    std::thread::sleep(BURST_SETTLE);
    assess(capture, config.channel, config, &mut measurements.burst)?;
    assess(
        capture,
        config.far_channel,
        config,
        &mut measurements.burst_far,
    )?;
    assess(
        capture,
        config.channel,
        config,
        &mut measurements.burst_back,
    )?;
    for sequence in 0..config.samples {
        let probe = data_frame(false, sequence, PEER_SHORT, DEVICE_SHORT);
        let evidence = capture
            .request(
                0,
                oer_hil_protocol::ieee802154::TransmitSession(Ieee802154SessionTransmitRequest {
                    frame: session_frame(&probe)?,
                    mode: Ieee802154SessionTxMode::ClearChannelAssessment,
                    max_frame_retries: 0,
                }),
                COMMAND_TIMEOUT,
            )
            .map(|response| response.0)?;
        expect_session("assessed transmit", evidence.result)?;
        if !matches!(
            evidence.outcome,
            Ieee802154AirTxOutcome::Success | Ieee802154AirTxOutcome::ChannelBusy
        ) {
            return Err(format!("assessed transmit ended {:?}", evidence.outcome).into());
        }
        measurements
            .assessed_transmits
            .push(format!("{:?}", evidence.outcome));
    }
    // The assessments left the receiver on other channels: drop what it
    // gathered, receive on the burst's channel, then collect.
    let _ = capture
        .request(
            0,
            oer_hil_protocol::ieee802154::CollectSession,
            std::time::Duration::from_secs(5),
        )
        .map(|response| response.0)?;
    capture
        .request(
            0,
            oer_hil_protocol::ieee802154::ReceiveSession,
            std::time::Duration::from_secs(5),
        )
        .map(drop)?;
    std::thread::sleep(RECEIVE_WINDOW);
    let received = capture
        .request(
            0,
            oer_hil_protocol::ieee802154::CollectSession,
            std::time::Duration::from_secs(5),
        )
        .map(|response| response.0)?;
    measurements.burst_frames_received = received.total;
    measurements.burst_rssi_dbm = burst_rssi(&received, &frame)?;
    let PeerBurst { sent, failed } = peer.stop_burst()?;
    measurements.peer_sent = sent;
    measurements.peer_failed = failed;

    assess(capture, config.channel, config, &mut measurements.after)?;
    let stopped = capture
        .request(
            0,
            oer_hil_protocol::ieee802154::StopSession,
            COMMAND_TIMEOUT,
        )
        .map(|response| response.0)?;
    expect_session("stop", stopped.result)?;
    peer.sleep()?;
    Ok(measurements)
}

#[cfg(test)]
mod tests;

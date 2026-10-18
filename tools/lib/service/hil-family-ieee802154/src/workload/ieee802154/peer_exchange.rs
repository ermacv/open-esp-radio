//! IEEE 802.15.4 exchange between the device under test and the reference
//! peer.
//!
//! The peer runs the vendor driver (the IEEE 802.15.4 peer project of `hil/peers`).
//! One session on the device under test covers these cells on one channel
//! and PAN:
//!
//! 1. the device transmits data frames requesting acknowledgement by CSMA-CA
//!    with retries: each is acknowledged with its sequence number and
//!    arrives at the peer intact;
//! 2. the peer transmits data frames to the device: the device acknowledges
//!    each, and reports the same frames by digest;
//! 3. the peer transmits to another short address: the device neither
//!    acknowledges nor reports it; a device frame to that address ends
//!    unacknowledged after its retries;
//! 4. with the peer's short address in the device's pending table, the
//!    device's acknowledgement of the peer's data request carries frame
//!    pending, and its enhanced ACK of the peer's 2015 frame is the one
//!    IEEE 802.15.4-2015 prescribes;
//! 5. after one tracking period, the device runs shared PHY tracking and then
//!    exchanges one acknowledged frame in each direction again.

use std::{path::Path, time::Duration};

use oer_hil_link::SerialCapture;
use oer_hil_link::peer::{PeerLink, PeerTranscript};
use oer_hil_protocol::{
    ieee802154::Ieee802154AirTxOutcome, ieee802154::Ieee802154SessionCoexistence,
    ieee802154::Ieee802154SessionConfig, ieee802154::Ieee802154SessionFrame,
    ieee802154::Ieee802154SessionPendingMode, ieee802154::Ieee802154SessionPendingRequest,
    ieee802154::Ieee802154SessionPhyMaintenance, ieee802154::Ieee802154SessionReceiveEvidence,
    ieee802154::Ieee802154SessionResult, ieee802154::Ieee802154SessionTransmitEvidence,
    ieee802154::Ieee802154SessionTransmitRequest, ieee802154::Ieee802154SessionTxMode,
    ieee802154::ieee802154_frame_crc32c,
};
use oer_hil_workload::{
    boots::{boot_directory, for_each_boot},
    context::Context,
    require_keys,
};
use serde::Serialize;

use crate::{
    Result,
    peer::{PEER_TRANSCRIPT, Peer, PeerConfig, PeerEvent},
};

/// Session start registers and calibrates the shared PHY.
pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound on one peer report after its trigger.
pub(crate) const PEER_EVENT_TIMEOUT: Duration = Duration::from_secs(1);
/// Longer than the shared PHY domain's one-second tracking period.
const TRACKING_DUE_AFTER: Duration = Duration::from_millis(1_200);
/// Maintenance runs the tracking transaction.
const MAINTENANCE_TIMEOUT: Duration = Duration::from_secs(10);

pub const PAN_ID: u16 = 0x4f45;
pub const DEVICE_SHORT: u16 = 0x0001;
pub const PEER_SHORT: u16 = 0x0002;
/// A short address nobody in the cell owns.
pub const FOREIGN_SHORT: u16 = 0x0099;
pub(crate) const DEVICE_EXTENDED: [u8; 8] = [0x01, 0x54, 0x31, 0x53, 0x33, 0x32, 0x65, 0x4f];
pub(crate) const PEER_EXTENDED: [u8; 8] = [0x02, 0x54, 0x31, 0x43, 0x35, 0x70, 0x65, 0x4f];

/// Sequence numbers of peer-originated frames start here, apart from the
/// device's.
const PEER_SEQUENCE_BASE: u8 = 0x80;
/// The device's data frames acquire the channel as OpenThread does by
/// default: CSMA-CA with `macMaxCsmaBackoffs` 4.
const DEVICE_TX_MODE: Ieee802154SessionTxMode = Ieee802154SessionTxMode::CsmaCa { max_backoffs: 4 };
/// Retransmissions of the device's data frames; OpenThread's default of
/// fifteen would only lengthen the unacknowledged check.
const DEVICE_FRAME_RETRIES: u8 = 3;

pub struct Config {
    pub boots: u8,
    pub channel: u8,
    pub frames: u8,
    /// Take part in coexistence with Wi-Fi for the session.
    pub wifi_coexistence: bool,
}

/// The session took part in coexistence exactly when asked, and left it
/// cleanly.
pub(crate) fn validate_coexistence(
    coexistence: Ieee802154SessionCoexistence,
    requested: bool,
) -> Result<()> {
    if coexistence.enabled != requested {
        return Err(format!(
            "coexistence with Wi-Fi was {}, requested {requested}",
            coexistence.enabled
        )
        .into());
    }
    if coexistence.disable_failed {
        return Err("leaving coexistence with Wi-Fi failed".into());
    }
    Ok(())
}

/// A data frame with PAN ID compression and short addresses.
pub fn data_frame(ack_request: bool, sequence: u8, destination: u16, source: u16) -> Vec<u8> {
    let control: u16 = 0x8841 | if ack_request { 0x0020 } else { 0 };
    let mut frame = control.to_le_bytes().to_vec();
    frame.push(sequence);
    frame.extend_from_slice(&PAN_ID.to_le_bytes());
    frame.extend_from_slice(&destination.to_le_bytes());
    frame.extend_from_slice(&source.to_le_bytes());
    frame.extend_from_slice(b"OER-154-");
    frame.push(sequence);
    frame
}

/// A data-request MAC command requesting acknowledgement.
pub fn data_request(sequence: u8, destination: u16, source: u16) -> Vec<u8> {
    let mut frame = 0x8863_u16.to_le_bytes().to_vec();
    frame.push(sequence);
    frame.extend_from_slice(&PAN_ID.to_le_bytes());
    frame.extend_from_slice(&destination.to_le_bytes());
    frame.extend_from_slice(&source.to_le_bytes());
    frame.push(0x04);
    frame
}

/// A 2015 data frame with PAN ID compression and short addresses,
/// requesting an ACK.
pub fn data_frame_2015(sequence: u8, destination: u16, source: u16) -> Vec<u8> {
    let mut frame = data_frame(true, sequence, destination, source);
    frame[1] = 0xa8;
    frame
}

/// The enhanced acknowledgement IEEE 802.15.4-2015 prescribes for
/// [`data_frame_2015`] from `source`: an unsecured 2015 ACK with the
/// frame-pending bit, the echoed sequence, the destination PAN ID and
/// `source` as destination, without source address or PAN ID compression.
pub fn enhanced_acknowledgement(sequence: u8, source: u16, pending: bool) -> Vec<u8> {
    let control: u16 = 0x2802 | if pending { 0x0010 } else { 0 };
    let mut frame = control.to_le_bytes().to_vec();
    frame.push(sequence);
    frame.extend_from_slice(&PAN_ID.to_le_bytes());
    frame.extend_from_slice(&source.to_le_bytes());
    frame
}

/// An immediate acknowledgement of `sequence`, and its frame-pending bit.
pub fn acknowledgement(frame: &[u8], sequence: u8) -> Option<bool> {
    match frame {
        [control, 0x00, ack_sequence] if control & 0x07 == 0x02 && *ack_sequence == sequence => {
            Some(control & 0x10 != 0)
        }
        _ => None,
    }
}

#[derive(Serialize)]
struct BootReport {
    device_to_peer: Vec<String>,
    peer_to_device: Vec<String>,
    filtered: String,
    pending: String,
    enhanced_ack: String,
    maintenance: String,
}

pub fn run(config: Config, output: &Path, context: &Context<'_>) -> Result<()> {
    context.results.claim(
        "acknowledged-exchange-filtering-and-pending-with-vendor-peer",
        &[
            "busy-channel-csma-ca",
            "retry-count-on-air",
            "security",
            "calibrated-output-power",
            "timed-exchange",
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
            let exchanged = exchange(capture, &mut peer, &config);
            // What the peer saw tells a missing transmission from a missing
            // acknowledgement.
            transcript.save(&boot_directory(output, boot).join(PEER_TRANSCRIPT))?;
            exchanged
        },
        |_, _| Ok(()),
    )?;
    println!(
        "ieee802154_peer_exchange=PASS boots={} frames={}",
        config.boots, config.frames
    );
    Ok(())
}

pub(crate) fn session_frame(bytes: &[u8]) -> Result<Ieee802154SessionFrame> {
    Ieee802154SessionFrame::from_slice(bytes)
        .map_err(|_| "frame exceeds the session capacity".into())
}

fn exchange<L: PeerLink>(
    capture: &SerialCapture,
    peer: &mut Peer<L>,
    config: &Config,
) -> Result<BootReport> {
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
                    enhanced_ack: true,
                    wifi_coexistence: config.wifi_coexistence,
                }),
                START_TIMEOUT,
            )
            .map(|response| response.0)?,
    )?;

    let mut device_to_peer = Vec::new();
    for sequence in 0..config.frames {
        let frame = data_frame(true, sequence, PEER_SHORT, DEVICE_SHORT);
        let evidence = capture
            .request(
                0,
                oer_hil_protocol::ieee802154::TransmitSession(Ieee802154SessionTransmitRequest {
                    frame: session_frame(&frame)?,
                    mode: DEVICE_TX_MODE,
                    max_frame_retries: DEVICE_FRAME_RETRIES,
                }),
                COMMAND_TIMEOUT,
            )
            .map(|response| response.0)?;
        check_device_transmit(&evidence, sequence)?;
        check_peer_received(peer.next_event(PEER_EVENT_TIMEOUT)?, &frame)?;
        device_to_peer.push(format!("seq={sequence} CSMA-CA acknowledged delivered"));
    }

    capture
        .request(
            0,
            oer_hil_protocol::ieee802154::ReceiveSession,
            std::time::Duration::from_secs(5),
        )
        .map(drop)?;
    let mut sent = Vec::new();
    let mut peer_to_device = Vec::new();
    for index in 0..config.frames {
        let sequence = PEER_SEQUENCE_BASE + index;
        let frame = data_frame(true, sequence, DEVICE_SHORT, PEER_SHORT);
        peer.transmit(false, &frame)?;
        check_peer_acknowledged(peer.next_event(PEER_EVENT_TIMEOUT)?, sequence, false)?;
        sent.push(frame);
        peer_to_device.push(format!("seq={sequence} acknowledged"));
    }
    check_device_received(
        &capture
            .request(
                0,
                oer_hil_protocol::ieee802154::CollectSession,
                std::time::Duration::from_secs(5),
            )
            .map(|response| response.0)?,
        &sent,
    )?;

    // Nobody acknowledges a foreign destination: the device retries, then
    // reports no acknowledgement.
    let unanswered = data_frame(true, 0x73, FOREIGN_SHORT, DEVICE_SHORT);
    check_device_unacknowledged(
        &capture
            .request(
                0,
                oer_hil_protocol::ieee802154::TransmitSession(Ieee802154SessionTransmitRequest {
                    frame: session_frame(&unanswered)?,
                    mode: DEVICE_TX_MODE,
                    max_frame_retries: DEVICE_FRAME_RETRIES,
                }),
                COMMAND_TIMEOUT,
            )
            .map(|response| response.0)?,
        0x73,
    )?;

    let foreign = data_frame(true, 0x70, FOREIGN_SHORT, PEER_SHORT);
    peer.transmit(false, &foreign)?;
    check_peer_unacknowledged(peer.next_event(PEER_EVENT_TIMEOUT)?)?;
    check_device_received(
        &capture
            .request(
                0,
                oer_hil_protocol::ieee802154::CollectSession,
                std::time::Duration::from_secs(5),
            )
            .map(|response| response.0)?,
        &[],
    )?;

    capture
        .request(
            0,
            oer_hil_protocol::ieee802154::SetSessionPending(Ieee802154SessionPendingRequest {
                mode: Ieee802154SessionPendingMode::Enabled,
                short_address: Some(PEER_SHORT),
            }),
            std::time::Duration::from_secs(5),
        )
        .map(drop)?;
    let request = data_request(0x71, DEVICE_SHORT, PEER_SHORT);
    peer.transmit(false, &request)?;
    check_peer_acknowledged(peer.next_event(PEER_EVENT_TIMEOUT)?, 0x71, true)?;
    let _ = capture
        .request(
            0,
            oer_hil_protocol::ieee802154::CollectSession,
            std::time::Duration::from_secs(5),
        )
        .map(|response| response.0)?;

    // The peer's address is in the pending table, so the enhanced ACK of
    // its 2015 frame carries frame pending.
    let frame = data_frame_2015(0x72, DEVICE_SHORT, PEER_SHORT);
    peer.transmit(false, &frame)?;
    check_peer_enhanced_ack(
        peer.next_event(PEER_EVENT_TIMEOUT)?,
        &enhanced_acknowledgement(0x72, PEER_SHORT, true),
    )?;
    check_device_received(
        &capture
            .request(
                0,
                oer_hil_protocol::ieee802154::CollectSession,
                std::time::Duration::from_secs(5),
            )
            .map(|response| response.0)?,
        &[frame],
    )?;

    std::thread::sleep(TRACKING_DUE_AFTER);
    let maintained = capture
        .request(
            0,
            oer_hil_protocol::ieee802154::MaintainSessionPhy,
            MAINTENANCE_TIMEOUT,
        )
        .map(|response| response.0)?;
    if maintained != Ieee802154SessionPhyMaintenance::Tracked {
        return Err(format!("due PHY maintenance ended {maintained:?}").into());
    }
    let frame = data_frame(true, 0x60, PEER_SHORT, DEVICE_SHORT);
    check_device_transmit(
        &capture
            .request(
                0,
                oer_hil_protocol::ieee802154::TransmitSession(Ieee802154SessionTransmitRequest {
                    frame: session_frame(&frame)?,
                    mode: Ieee802154SessionTxMode::Direct,
                    max_frame_retries: 0,
                }),
                COMMAND_TIMEOUT,
            )
            .map(|response| response.0)?,
        0x60,
    )?;
    check_peer_received(peer.next_event(PEER_EVENT_TIMEOUT)?, &frame)?;
    let frame = data_frame(true, 0x61, DEVICE_SHORT, PEER_SHORT);
    peer.transmit(false, &frame)?;
    check_peer_acknowledged(peer.next_event(PEER_EVENT_TIMEOUT)?, 0x61, false)?;
    check_device_received(
        &capture
            .request(
                0,
                oer_hil_protocol::ieee802154::CollectSession,
                std::time::Duration::from_secs(5),
            )
            .map(|response| response.0)?,
        &[frame],
    )?;

    let stopped = capture
        .request(
            0,
            oer_hil_protocol::ieee802154::StopSession,
            COMMAND_TIMEOUT,
        )
        .map(|response| response.0)?;
    expect_session("stop", stopped.result)?;
    validate_coexistence(stopped.coexistence, config.wifi_coexistence)?;
    peer.sleep()?;
    Ok(BootReport {
        device_to_peer,
        peer_to_device,
        filtered: String::from(
            "foreign destination unacknowledged and unreported; device frame to it retried, then unacknowledged",
        ),
        pending: String::from("data request acknowledged with frame pending"),
        enhanced_ack: String::from("2015 frame acknowledged with the enhanced ACK"),
        maintenance: String::from("tracked, then exchanged in both directions"),
    })
}

pub(crate) fn expect_session(step: &str, result: Ieee802154SessionResult) -> Result<()> {
    if result == Ieee802154SessionResult::Done {
        Ok(())
    } else {
        Err(format!("IEEE 802.15.4 session {step} ended {result:?}").into())
    }
}

pub(crate) fn check_device_transmit(
    evidence: &Ieee802154SessionTransmitEvidence,
    sequence: u8,
) -> Result<()> {
    expect_session("transmit", evidence.result)?;
    if evidence.outcome != Ieee802154AirTxOutcome::Success {
        return Err(format!(
            "device transmit seq={sequence} ended {:?}",
            evidence.outcome
        )
        .into());
    }
    match evidence
        .acknowledgement
        .as_ref()
        .and_then(|ack| acknowledgement(&ack.frame, sequence))
    {
        Some(false) => Ok(()),
        Some(true) => Err(format!("peer acknowledged seq={sequence} with frame pending").into()),
        None => {
            Err(format!("device transmit seq={sequence} has no matching acknowledgement").into())
        }
    }
}

pub(crate) fn check_device_unacknowledged(
    evidence: &Ieee802154SessionTransmitEvidence,
    sequence: u8,
) -> Result<()> {
    expect_session("transmit", evidence.result)?;
    if evidence.outcome == Ieee802154AirTxOutcome::NoAcknowledgement
        && evidence.acknowledgement.is_none()
    {
        Ok(())
    } else {
        Err(format!(
            "device transmit seq={sequence} to a foreign destination ended {:?}",
            evidence.outcome
        )
        .into())
    }
}

pub(crate) fn check_peer_received(event: Option<PeerEvent>, frame: &[u8]) -> Result<()> {
    match event {
        Some(PeerEvent::Received(received)) if received.bytes == frame => Ok(()),
        other => Err(format!("peer did not receive the device frame: {other:?}").into()),
    }
}

pub(crate) fn check_peer_acknowledged(
    event: Option<PeerEvent>,
    sequence: u8,
    pending: bool,
) -> Result<()> {
    match event {
        Some(PeerEvent::Transmitted {
            acknowledgement: Some(ack),
        }) if acknowledgement(&ack.bytes, sequence) == Some(pending) => Ok(()),
        other => Err(format!(
            "device acknowledgement of seq={sequence} (pending={pending}) missing: {other:?}"
        )
        .into()),
    }
}

pub(crate) fn check_peer_enhanced_ack(event: Option<PeerEvent>, expected: &[u8]) -> Result<()> {
    match event {
        Some(PeerEvent::Transmitted {
            acknowledgement: Some(ack),
        }) if ack.bytes == expected => Ok(()),
        other => Err(format!("device enhanced ACK {expected:02x?} missing: {other:?}").into()),
    }
}

pub(crate) fn check_peer_unacknowledged(event: Option<PeerEvent>) -> Result<()> {
    match event {
        Some(PeerEvent::TransmitFailed(_)) => Ok(()),
        other => Err(format!("a foreign-address frame was acknowledged: {other:?}").into()),
    }
}

pub(crate) fn check_device_received(
    evidence: &Ieee802154SessionReceiveEvidence,
    sent: &[Vec<u8>],
) -> Result<()> {
    expect_session("collect", evidence.result)?;
    if usize::from(evidence.total) != sent.len() || evidence.frames.len() != sent.len() {
        return Err(format!(
            "device received {} frames, expected {}",
            evidence.total,
            sent.len()
        )
        .into());
    }
    for (index, (received, frame)) in evidence.frames.iter().zip(sent).enumerate() {
        if usize::from(received.length) != frame.len()
            || received.crc32c != ieee802154_frame_crc32c(frame)
        {
            return Err(format!("device frame {index} differs from the peer's").into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

//! Validate fragment headers, role mapping and authenticated payload identity.
//!
//! The fixed MAC header and optional QoS control are split as arrays once.
//! Fragment admission carries that decoded sequence control and payload view;
//! it does not revisit raw offsets after validating the header.

use crate::ccmp::CcmpHeader;
use crate::data::IEEE80211_LEGACY_DATA_HEADER_LEN;
use crate::sequence::SequenceNumber;

use super::{
    CcmpPacketNumber, DATA, DataFragmentProtection, DataInterfaceRole, FROM_DS, MORE_FRAGMENTS,
    ORDER, OpenDataFragment, OpenDataFragmentError, OpenDataFragmentIdentity, PROTECTED,
    QOS_AMSDU_PRESENT, QOS_DATA, RETRY, TO_DS, TYPE_AND_SUBTYPE,
};

/// Parse an Open data fragment from one normalized (FCS-free) MPDU.
///
/// A successful value proves exact three-address role mapping with an
/// individual receiver and logical destination, an unprotected
/// Data/QoS-Data subtype, no HT-Control or A-MSDU, and a nonempty fragment
/// body. Unfragmented MPDUs remain with the ordinary decapsulation path.
pub fn parse_open_data_fragment(
    role: DataInterfaceRole,
    mpdu: &[u8],
) -> Result<OpenDataFragment<'_>, OpenDataFragmentError> {
    let header = parse_data_header(role, mpdu, DataFragmentProtection::Open)?;
    parse_data_fragment(header, header.payload, None)
}

/// Parse one hardware-authenticated CCMP data fragment.
///
/// The caller must supply only the plaintext payload slice released by a
/// successful CCMP MIC-verification result. This codec validates all public
/// 802.11 identity/AAD fields and binds the parsed Key ID and PN to the
/// returned fragment; it does not itself own or advance a replay frontier.
pub fn parse_ccmp_data_fragment<'frame>(
    role: DataInterfaceRole,
    mpdu: &'frame [u8],
    authenticated_payload: &'frame [u8],
    ccmp_header: CcmpHeader,
) -> Result<OpenDataFragment<'frame>, OpenDataFragmentError> {
    let header = parse_data_header(
        role,
        mpdu,
        DataFragmentProtection::Ccmp {
            key_id: ccmp_header.key_id(),
        },
    )?;
    parse_data_fragment(
        header,
        authenticated_payload,
        Some(ccmp_header.packet_number()),
    )
}

fn parse_data_fragment<'frame>(
    header: ParsedOpenDataHeader<'_>,
    payload: &'frame [u8],
    packet_number: Option<CcmpPacketNumber>,
) -> Result<OpenDataFragment<'frame>, OpenDataFragmentError> {
    if header.qos_amsdu_present {
        return Err(OpenDataFragmentError::AmsduUnsupported);
    }
    if header.identity.receiver_address == [0; 6] || header.identity.receiver_address[0] & 1 != 0 {
        // IEEE fragmentation is defined only for an individual Address 1.
        // Ordinary group-addressed MPDUs remain valid and therefore this
        // check belongs to the fragment constructor, not the shared identity
        // parser used by unfragmented receive.
        return Err(OpenDataFragmentError::InvalidReceiver);
    }
    let destination = header.identity.destination();
    if destination == [0; 6] || destination[0] & 1 != 0 {
        // In To-DS/AP direction Address 1 is the individual BSSID while
        // Address 3 is the MSDU destination. Fragmentation is forbidden for
        // that group-addressed MSDU even though its immediate receiver is
        // individual.
        return Err(OpenDataFragmentError::InvalidDestination);
    }
    let sequence_control = header.sequence_control;
    let fragment_number = (sequence_control & 0x000f) as u8;
    let more_fragments = header.frame_control & MORE_FRAGMENTS != 0;
    if fragment_number == 0 && !more_fragments {
        return Err(OpenDataFragmentError::NotFragmented);
    }
    if payload.is_empty() {
        return Err(OpenDataFragmentError::EmptyPayload);
    }
    Ok(OpenDataFragment {
        identity: header.identity,
        sequence_control,
        fragment_number,
        more_fragments,
        retry: header.frame_control & RETRY != 0,
        packet_number,
        payload,
    })
}

/// Parse the exact role/address/sequence identity of any Open Data or
/// QoS-Data MPDU, including an unfragmented unit.
///
/// Receive dispatchers consult this identity before ordinary decapsulation
/// so clearing More Fragments on a retry cannot bypass an active fragment
/// context and publish a partial MSDU as standalone Ethernet.
pub fn parse_open_data_identity(
    role: DataInterfaceRole,
    mpdu: &[u8],
) -> Result<OpenDataFragmentIdentity, OpenDataFragmentError> {
    parse_data_header(role, mpdu, DataFragmentProtection::Open).map(|header| header.identity)
}

/// Parse the role/address/sequence identity of an authenticated CCMP data
/// MPDU without exposing or retaining its payload.
pub fn parse_ccmp_data_identity(
    role: DataInterfaceRole,
    mpdu: &[u8],
    ccmp_header: CcmpHeader,
) -> Result<OpenDataFragmentIdentity, OpenDataFragmentError> {
    parse_data_header(
        role,
        mpdu,
        DataFragmentProtection::Ccmp {
            key_id: ccmp_header.key_id(),
        },
    )
    .map(|header| header.identity)
}

#[derive(Clone, Copy)]
struct ParsedOpenDataHeader<'frame> {
    identity: OpenDataFragmentIdentity,
    frame_control: u16,
    sequence_control: u16,
    payload: &'frame [u8],
    qos_amsdu_present: bool,
}

fn parse_data_header(
    role: DataInterfaceRole,
    mpdu: &[u8],
    protection: DataFragmentProtection,
) -> Result<ParsedOpenDataHeader<'_>, OpenDataFragmentError> {
    let (header, remainder) = mpdu
        .split_first_chunk::<IEEE80211_LEGACY_DATA_HEADER_LEN>()
        .ok_or(OpenDataFragmentError::Truncated)?;
    let [
        fc0,
        fc1,
        _duration0,
        _duration1,
        r0,
        r1,
        r2,
        r3,
        r4,
        r5,
        t0,
        t1,
        t2,
        t3,
        t4,
        t5,
        a0,
        a1,
        a2,
        a3,
        a4,
        a5,
        sc0,
        sc1,
    ] = *header;
    let receiver_address = [r0, r1, r2, r3, r4, r5];
    let transmitter_address = [t0, t1, t2, t3, t4, t5];
    let address3 = [a0, a1, a2, a3, a4, a5];
    let frame_control = u16::from_le_bytes([fc0, fc1]);
    let subtype = frame_control & TYPE_AND_SUBTYPE;
    if frame_control & 0x0003 != 0 || (subtype != DATA && subtype != QOS_DATA) {
        return Err(OpenDataFragmentError::NotData);
    }
    match (protection, frame_control & PROTECTED != 0) {
        (DataFragmentProtection::Open, true) => {
            return Err(OpenDataFragmentError::Protected);
        }
        (DataFragmentProtection::Ccmp { .. }, false) => {
            return Err(OpenDataFragmentError::Unprotected);
        }
        _ => {}
    }
    if frame_control & ORDER != 0 {
        return Err(OpenDataFragmentError::OrderedUnsupported);
    }
    let role_matches = matches!(
        (role, frame_control & (TO_DS | FROM_DS)),
        (DataInterfaceRole::Station, FROM_DS) | (DataInterfaceRole::AccessPoint, TO_DS)
    );
    if !role_matches {
        return Err(OpenDataFragmentError::RoleMismatch);
    }

    let (qos_control, payload) = if subtype == QOS_DATA {
        let (qos, payload) = remainder
            .split_first_chunk()
            .ok_or(OpenDataFragmentError::Truncated)?;
        (Some(u16::from_le_bytes(*qos)), payload)
    } else {
        (None, remainder)
    };
    let sequence_control = u16::from_le_bytes([sc0, sc1]);
    if transmitter_address == [0; 6] || transmitter_address[0] & 1 != 0 {
        return Err(OpenDataFragmentError::InvalidTransmitter);
    }
    Ok(ParsedOpenDataHeader {
        identity: OpenDataFragmentIdentity {
            role,
            protection,
            receiver_address,
            transmitter_address,
            address3,
            sequence_number: SequenceNumber::from_sequence_control(sequence_control),
            qos_control,
        },
        frame_control,
        sequence_control,
        payload,
        qos_amsdu_present: qos_control.is_some_and(|qos| qos & u16::from(QOS_AMSDU_PRESENT) != 0),
    })
}

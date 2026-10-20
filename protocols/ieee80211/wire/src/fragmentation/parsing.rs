//! Validate fragment headers, role mapping and authenticated payload identity.
//!
//! The fixed MAC header and optional QoS control are split as arrays once.
//! Fragment admission carries that decoded sequence control and payload view;
//! it does not revisit raw offsets after validating the header.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use crate::frame::{FrameControlFields, QosControl};

use crate::ccmp::CcmpHeader;
use crate::data::IEEE80211_LEGACY_DATA_HEADER_LEN;
use crate::sequence::SequenceNumber;

use super::{
    CcmpPacketNumber, DataFragmentProtection, DataInterfaceRole, OpenDataFragment,
    OpenDataFragmentError, OpenDataFragmentIdentity,
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
/// 802.11 identity/AAD fields and binds the PN to the returned fragment.
/// The upper-MAC admission epoch identifies the key installation; its wire
/// KeyID cannot distinguish a rekey. This codec owns no replay frontier.
pub fn parse_ccmp_data_fragment<'frame>(
    role: DataInterfaceRole,
    mpdu: &'frame [u8],
    authenticated_payload: &'frame [u8],
    ccmp_header: CcmpHeader,
) -> Result<OpenDataFragment<'frame>, OpenDataFragmentError> {
    let header = parse_data_header(role, mpdu, DataFragmentProtection::Ccmp)?;
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
    if header.identity.receiver_address == [0; 6]
        || crate::address::AddressFlags::from_bytes([header.identity.receiver_address[0]]).group()
    {
        // IEEE fragmentation is defined only for an individual Address 1.
        // Ordinary group-addressed MPDUs remain valid and therefore this
        // check belongs to the fragment constructor, not the shared identity
        // parser used by unfragmented receive.
        return Err(OpenDataFragmentError::InvalidReceiver);
    }
    let destination = header.identity.destination();
    if destination == [0; 6] || crate::address::AddressFlags::from_bytes([destination[0]]).group() {
        // In To-DS/AP direction Address 1 is the individual BSSID while
        // Address 3 is the MSDU destination. Fragmentation is forbidden for
        // that group-addressed MSDU even though its immediate receiver is
        // individual.
        return Err(OpenDataFragmentError::InvalidDestination);
    }
    let sequence_control = header.sequence_control;
    let fragment_number =
        crate::sequence::SequenceControl::from_bytes((sequence_control).to_le_bytes()).fragment();
    let more_fragments =
        FrameControlFields::from_bytes(header.frame_control.to_le_bytes()).more_fragments();
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
        retry: FrameControlFields::from_bytes(header.frame_control.to_le_bytes()).retry(),
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
    // The wire ID does not identify the installation that authenticated it.
) -> Result<OpenDataFragmentIdentity, OpenDataFragmentError> {
    parse_data_header(role, mpdu, DataFragmentProtection::Ccmp).map(|header| header.identity)
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
    let mut reader = oer_wire::Reader::new(mpdu);
    let header = reader
        .array::<IEEE80211_LEGACY_DATA_HEADER_LEN>()
        .map_err(|_| OpenDataFragmentError::Truncated)?;
    let remainder = reader.remaining();
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
    let subtype = FrameControlFields::from_raw(frame_control).type_and_subtype();
    if FrameControlFields::from_raw(frame_control).version() != 0 || (subtype != 2 && subtype != 34)
    {
        return Err(OpenDataFragmentError::NotData);
    }
    match (
        protection,
        FrameControlFields::from_raw(frame_control).protected(),
    ) {
        (DataFragmentProtection::Open, true) => {
            return Err(OpenDataFragmentError::Protected);
        }
        (DataFragmentProtection::Ccmp, false) => {
            return Err(OpenDataFragmentError::Unprotected);
        }
        _ => {}
    }
    if FrameControlFields::from_raw(frame_control).order() {
        return Err(OpenDataFragmentError::OrderedUnsupported);
    }
    let role_matches = matches!(
        (role, FrameControlFields::from_raw(frame_control).ds()),
        (DataInterfaceRole::Station, 2) | (DataInterfaceRole::AccessPoint, 1)
    );
    if !role_matches {
        return Err(OpenDataFragmentError::RoleMismatch);
    }

    let (qos_control, payload) = if subtype == 34 {
        let mut reader = oer_wire::Reader::new(remainder);
        let qos = reader
            .u16_le()
            .map_err(|_| OpenDataFragmentError::Truncated)?;
        (Some(qos), reader.remaining())
    } else {
        (None, remainder)
    };
    let sequence_control = u16::from_le_bytes([sc0, sc1]);
    if transmitter_address == [0; 6]
        || crate::address::AddressFlags::from_bytes([transmitter_address[0]]).group()
    {
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
        qos_amsdu_present: qos_control
            .is_some_and(|qos| QosControl::from_bytes(qos.to_le_bytes()).amsdu()),
    })
}

#[cfg(test)]
mod fuzz_tests;

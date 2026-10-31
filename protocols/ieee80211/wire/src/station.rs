//! Allocation-free STA authentication, association and data frame formats.
//!
//! Frame construction, response parsing and WPA2-Personal RSN selection use
//! caller-owned output buffers. Sequence counters, deadlines and retry
//! decisions belong to the cores that drive these codecs.

use crate::{
    ccmp::CCMP_HEADER_LEN,
    data::{
        DataHeControl, DataInterfaceRole, plan_data_encapsulation,
        plan_data_encapsulation_with_he_control,
    },
    extensions::wmm::{WmmParameterSet, parse_wmm_parameter_element},
    management::{MANAGEMENT_HEADER_LEN, MAX_SSID_LEN, MAX_SUPPORTED_RATES_LEN},
    qos::WmmUserPriority,
    scan::ScanRecord,
    security::LinkProtection,
    sequence::SequenceNumber,
};

const OPEN_AUTHENTICATION_FRAME_CONTROL: u16 = 0x00b0;
#[cfg(test)]
const DISASSOCIATION_FRAME_CONTROL: u16 = 0x00a0;
const DEAUTHENTICATION_FRAME_CONTROL: u16 = 0x00c0;
const ACTION_FRAME_CONTROL: u16 = 0x00d0;
/// Frame Control Protected Frame bit.
const ASSOCIATION_REQUEST_FRAME_CONTROL: u16 = 0x0000;
const ASSOCIATION_RESPONSE_FRAME_CONTROL: u16 = 0x0010;
const OPEN_SYSTEM_ALGORITHM: u16 = 0;
const OPEN_SYSTEM_REQUEST_SEQUENCE: u16 = 1;
const OPEN_SYSTEM_RESPONSE_SEQUENCE: u16 = 2;
const ASSOCIATION_FIXED_BODY_LEN: usize = 4;
const SUPPORTED_RATES_ELEMENT_CAPACITY: usize = 8;
const SELECTED_RSN_IE_LEN: usize = 22;
/// The RSN element and, under SAE, an RSNXE of one capability octet.
const SELECTED_SECURITY_IES_CAPACITY: usize = SELECTED_RSN_IE_LEN + PMKID_LIST_LEN + 3;
/// A PMKID list naming one PMKID.
const PMKID_LIST_LEN: usize = 2 + 16;
const HE_UL_MU_POWER_CAPABILITY_IE_LEN: usize = 14;
const HE_UL_MU_POWER_CAPABILITY_EXTENSION_ID: u8 = 60;
const POWER_CAPABILITY_IE_LEN: usize = 4;

/// Baseline HT A-MSDU limit selected when HT Capabilities Info bit 11 is
/// clear. The 7,935-byte extension remains deliberately unadvertised.
const HT_AMSDU_BASELINE_MAX_LEN: usize = 3_839;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationFrameError {
    InvalidBssid,
    SsidTooLong,
    NoSupportedRates,
    TooManySupportedRates,
    NoAmsduFrames,
    AmsduTooLong { length: usize, maximum: usize },
    UserPriorityOutOfRange,
    HeControlRequiresQos,
    OutputTooSmall { required: usize },
}

pub mod association;
mod data;
mod management;
mod security;

pub use association::{
    AssociationCapabilities, AssociationRequest, AssociationRequestError, AssociationResponse,
    HeUlMuPowerCapability, HeUlMuPowerCapabilityError, StaPowerCapability, StaPowerCapabilityError,
    parse_association_response,
};
pub use data::{
    StaDataFrame, StaOpenDataFrame, StaProtectedAmsduFrame, StaProtectedDataFrame,
    sta_protected_amsdu_frame_length,
};
pub use management::{
    OpenAuthenticationRequest, OpenAuthenticationResponse, SAE_AUTHENTICATION_ALGORITHM,
    SAE_COMMIT_TRANSACTION, SAE_CONFIRM_TRANSACTION, SaeAuthentication, SaeAuthenticationFrame,
    StaDisconnect, StaDisconnectKind, StaManagementFrame, StaManagementSubtype,
    StaProtectedManagementFrame, parse_open_authentication_response, parse_sae_authentication,
    parse_sta_disconnect,
};
pub use security::SelectedRsn;

pub(crate) fn validate_peer(bssid: [u8; 6]) -> Result<(), StationFrameError> {
    if bssid == [0; 6]
        || bssid == [0xff; 6]
        || crate::address::AddressFlags::from_bytes([bssid[0]]).group()
    {
        return Err(StationFrameError::InvalidBssid);
    }
    Ok(())
}

fn management_header(
    frame_control: u16,
    destination: [u8; 6],
    source: [u8; 6],
    bssid: [u8; 6],
    sequence_number: SequenceNumber,
) -> [u8; MANAGEMENT_HEADER_LEN] {
    let mut frame = [0; MANAGEMENT_HEADER_LEN];
    crate::encoding::write_array::<0, _, _>(&mut frame, &frame_control.to_le_bytes());
    crate::encoding::write_array::<4, _, _>(&mut frame, &destination);
    crate::encoding::write_array::<10, _, _>(&mut frame, &source);
    crate::encoding::write_array::<16, _, _>(&mut frame, &bssid);
    crate::encoding::write_array::<22, _, _>(
        &mut frame,
        &sequence_number.sequence_control().to_le_bytes(),
    );
    frame
}

/// The fields of a received management header the station parsers match.
struct ReceivedManagementHeader {
    frame_control: u16,
    receiver: [u8; 6],
    transmitter: [u8; 6],
    bssid: [u8; 6],
}

/// Split a received management frame into its header fields, the six-byte
/// fixed body that follows (the Authentication algorithm, transaction and
/// status, or the Association Response capability, status and AID) and the
/// rest; `None` when the frame is shorter than both.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
fn split_received_management(frame: &[u8]) -> Option<(ReceivedManagementHeader, [u16; 3], &[u8])> {
    let mut reader = oer_wire::Reader::new(frame);
    let header = reader.array::<MANAGEMENT_HEADER_LEN>().ok()?;
    let body = reader.remaining();
    let mut reader = oer_wire::Reader::new(body);
    let &[f0, f1, f2, f3, f4, f5] = reader.array().ok()?;
    let rest = reader.remaining();
    let [
        c0,
        c1,
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
        b0,
        b1,
        b2,
        b3,
        b4,
        b5,
        _sequence0,
        _sequence1,
    ] = *header;
    Some((
        ReceivedManagementHeader {
            frame_control: u16::from_le_bytes([c0, c1]),
            receiver: [r0, r1, r2, r3, r4, r5],
            transmitter: [t0, t1, t2, t3, t4, t5],
            bssid: [b0, b1, b2, b3, b4, b5],
        },
        [
            u16::from_le_bytes([f0, f1]),
            u16::from_le_bytes([f2, f3]),
            u16::from_le_bytes([f4, f5]),
        ],
        rest,
    ))
}

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;

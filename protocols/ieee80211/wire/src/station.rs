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
use oer_protocols_ieee802_wire::Eui48;

const OPEN_AUTHENTICATION_FRAME_CONTROL: crate::frame::FrameControlFields =
    crate::frame::FrameControlFields::from_bytes([0xb0, 0x00]);
#[cfg(test)]
const DISASSOCIATION_FRAME_CONTROL: crate::frame::FrameControlFields =
    crate::frame::FrameControlFields::from_bytes([0xa0, 0x00]);
const DEAUTHENTICATION_FRAME_CONTROL: crate::frame::FrameControlFields =
    crate::frame::FrameControlFields::from_bytes([0xc0, 0x00]);
const ACTION_FRAME_CONTROL: crate::frame::FrameControlFields =
    crate::frame::FrameControlFields::from_bytes([0xd0, 0x00]);
/// Frame Control Protected Frame bit.
const ASSOCIATION_REQUEST_FRAME_CONTROL: crate::frame::FrameControlFields =
    crate::frame::FrameControlFields::from_bytes([0x00, 0x00]);
const ASSOCIATION_RESPONSE_FRAME_CONTROL: crate::frame::FrameControlFields =
    crate::frame::FrameControlFields::from_bytes([0x10, 0x00]);
const ASSOCIATION_FIXED_BODY_LEN: usize = 4;
const SUPPORTED_RATES_ELEMENT_CAPACITY: usize = 8;
const SELECTED_RSN_IE_LEN: usize = 22;
/// The RSN element and, under SAE, an RSNXE of one capability octet.
const SELECTED_SECURITY_IES_CAPACITY: usize = SELECTED_RSN_IE_LEN + PMKID_LIST_LEN + 3;
/// A PMKID list naming one PMKID.
const PMKID_LIST_LEN: usize = 2 + 16;
const HE_UL_MU_POWER_CAPABILITY_IE_LEN: usize = 14;
const POWER_CAPABILITY_IE_LEN: usize = 4;

/// Baseline HT A-MSDU limit selected when HT Capabilities Info bit 11 is
/// clear. The 7,935-byte extension remains deliberately unadvertised.
const HT_AMSDU_BASELINE_MAX_LEN: usize = 3_839;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationFrameError {
    SsidTooLong,
    NoSupportedRates,
    TooManySupportedRates,
    NoAmsduFrames,
    AmsduTooLong { length: usize, maximum: usize },
    HeControlRequiresQos,
    OutputTooSmall { required: usize },
}

pub mod association;
mod data;
mod management;
mod security;

pub use association::{
    AssociationCapabilities, AssociationPhyElements, AssociationRequest, AssociationRequestError,
    AssociationResponse, HeUlMuPowerCapability, HeUlMuPowerCapabilityError, HtCapabilityElements,
    ResponseHeElements, StaPowerCapability, StaPowerCapabilityError, parse_association_response,
};
pub use data::{
    StaDataFrame, StaDataHeaderFields, StaDataHeaderTemplate, StaOpenDataFrame,
    StaProtectedAmsduFrame, StaProtectedDataFrame, sta_protected_amsdu_frame_length,
};
pub use management::{
    OpenAuthenticationRequest, OpenAuthenticationResponse, SaeAuthentication,
    SaeAuthenticationFrame, StaDisconnect, StaDisconnectKind, StaManagementFrame,
    StaManagementSubtype, StaProtectedManagementFrame, parse_open_authentication_response,
    parse_sae_authentication, parse_sta_disconnect,
};
pub use security::SelectedRsn;

fn management_header(
    frame_control: crate::frame::FrameControlFields,
    destination: Eui48,
    source: Eui48,
    bssid: Eui48,
    sequence_number: SequenceNumber,
) -> [u8; MANAGEMENT_HEADER_LEN] {
    let mut frame = [0; MANAGEMENT_HEADER_LEN];
    crate::encoding::write_array::<0, _, _>(&mut frame, &frame_control.to_bytes());
    crate::encoding::write_array::<4, _, _>(&mut frame, &destination.octets());
    crate::encoding::write_array::<10, _, _>(&mut frame, &source.octets());
    crate::encoding::write_array::<16, _, _>(&mut frame, &bssid.octets());
    crate::encoding::write_array::<22, _, _>(
        &mut frame,
        &sequence_number.sequence_control().to_bytes(),
    );
    frame
}

/// The fields of a received management header the station parsers match.
struct ReceivedManagementHeader {
    frame_control: crate::frame::FrameControlFields,
    receiver: Eui48,
    transmitter: Eui48,
    bssid: Eui48,
}

/// Split a received management frame into its header fields, the six-byte
/// fixed body that follows (the Authentication algorithm, transaction and
/// status, or the Association Response capability, status and AID) and the
/// rest; `None` when the frame is shorter than both.
fn split_received_management(frame: &[u8]) -> Option<(ReceivedManagementHeader, [u8; 6], &[u8])> {
    let mut reader = oer_portable_wire::Reader::new(frame);
    let frame_control = crate::frame::FrameControlFields::from_bytes(reader.read().ok()?);
    let _duration: [u8; 2] = reader.read().ok()?;
    let receiver = Eui48::read(&mut reader).ok()?;
    let transmitter = Eui48::read(&mut reader).ok()?;
    let bssid = Eui48::read(&mut reader).ok()?;
    let _sequence_control: [u8; 2] = reader.read().ok()?;
    let fixed = reader.read().ok()?;
    Some((
        ReceivedManagementHeader {
            frame_control,
            receiver,
            transmitter,
            bssid,
        },
        fixed,
        reader.remaining(),
    ))
}

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;

//! Station data encapsulation into caller-owned output.

use super::*;
use crate::address::{Bssid, OwnAddress};
use crate::data::MsduHeader;

mod amsdu;
pub use amsdu::{StaProtectedAmsduFrame, sta_protected_amsdu_frame_length};

/// One unprotected 802.11 data MPDU sent by a station through its AP.
///
/// This is the frame shape used for EAPOL before CCMP keys are installed.
/// The caller owns the MSDU payload and the output DMA buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaDataFrame<'a> {
    pub source: OwnAddress,
    pub bssid: Bssid,
    pub destination: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub ether_type: u16,
    pub payload: &'a [u8],
}

impl StaDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        StaOpenDataFrame {
            source: self.source,
            bssid: self.bssid,
            destination: self.destination,
            sequence_number: self.sequence_number,
            priority: WmmUserPriority::UP7,
            peer_qos: false,
            ether_type: self.ether_type,
            payload: self.payload,
        }
        .encode(output)
    }
}

/// One unprotected data MPDU sent by a station through its AP, QoS data
/// when the access point negotiated QoS.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.2.1 (Data frame format: To DS set,
/// Address 1 the BSSID, Address 2 the source, Address 3 the destination,
/// QoS Control carrying the TID) and 5.1.4 (RFC 1042 LLC/SNAP encapsulation).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaOpenDataFrame<'a> {
    pub source: OwnAddress,
    pub bssid: Bssid,
    pub destination: [u8; 6],
    pub sequence_number: SequenceNumber,
    /// The TID of a QoS data MPDU; unused without QoS.
    pub priority: WmmUserPriority,
    pub peer_qos: bool,
    pub ether_type: u16,
    pub payload: &'a [u8],
}

impl StaOpenDataFrame<'_> {
    /// Write the MPDU without FCS; returns its length.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid.get())?;
        let plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            self.bssid.get(),
            self.source.get(),
            MsduHeader {
                destination: self.destination,
                source: self.source.get(),
                ether_type: self.ether_type,
            },
            self.priority,
            self.peer_qos,
            false,
        );
        let mut header = plan.header;
        crate::encoding::write_array::<22, _, _>(
            &mut header,
            &self.sequence_number.sequence_control().to_le_bytes(),
        );
        let mut reader = oer_wire::Reader::new(&header);
        let header = reader.take(usize::from(plan.header_len)).map_err(|_| {
            StationFrameError::OutputTooSmall {
                required: usize::from(plan.header_len),
            }
        })?;
        crate::encoding::FrameParts([header, &plan.llc_snap, self.payload])
            .write(output)
            .map_err(|required| StationFrameError::OutputTooSmall { required })
    }
}

/// One protected data MPDU prepared for backend CCMP encryption.
///
/// The CCMP header carries the packet number owned by the installed hardware
/// key token. The payload remains plaintext in DMA memory; the MAC encrypts it
/// and writes the eight-byte CCMP MIC into caller-reserved trailer space.
///
/// `peer_qos` is the association result. It selects the same legacy/QoS header
/// boundary as the recovered `net80211_encap::plan_data_encapsulation` path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaProtectedDataFrame<'a> {
    pub source: OwnAddress,
    pub bssid: Bssid,
    pub destination: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub user_priority: u8,
    pub peer_qos: bool,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub ether_type: u16,
    pub payload: &'a [u8],
}

impl StaProtectedDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        self.encode_with_he_control(DataHeControl::Disabled, output)
    }

    /// Encode a protected QoS MPDU with a hardware-generated HE-Control field.
    ///
    /// The returned chip-independent DMA image keeps CCMP immediately after
    /// the 26-byte QoS header. The backend accounts for the four bytes inserted
    /// on air through its explicit HE-Control insertion capability.
    pub fn encode_with_he_control(
        self,
        he_control: DataHeControl,
        output: &mut [u8],
    ) -> Result<usize, StationFrameError> {
        validate_peer(self.bssid.get())?;
        let priority = WmmUserPriority::new(self.user_priority)
            .ok_or(StationFrameError::UserPriorityOutOfRange)?;
        if !self.peer_qos
            && matches!(
                he_control,
                DataHeControl::HardwareGeneratedBufferStatusReport
            )
        {
            return Err(StationFrameError::HeControlRequiresQos);
        }
        let mut plan = plan_data_encapsulation_with_he_control(
            DataInterfaceRole::Station,
            self.bssid.get(),
            self.source.get(),
            MsduHeader {
                destination: self.destination,
                source: self.source.get(),
                ether_type: self.ether_type,
            },
            priority,
            self.peer_qos,
            false,
            he_control,
        )
        .map_err(|_| StationFrameError::HeControlRequiresQos)?;
        // Exact `net80211_tx::encapsulate_ordinary` mutation after successful
        // CCMP key selection.
        plan.set_protected(true);
        let mut header = plan.header;
        crate::encoding::write_array::<22, _, _>(
            &mut header,
            &self.sequence_number.sequence_control().to_le_bytes(),
        );
        let mut reader = oer_wire::Reader::new(&header);
        let header = reader.take(usize::from(plan.header_len)).map_err(|_| {
            StationFrameError::OutputTooSmall {
                required: usize::from(plan.header_len),
            }
        })?;
        crate::encoding::FrameParts([header, &self.ccmp_header, &plan.llc_snap, self.payload])
            .write(output)
            .map_err(|required| StationFrameError::OutputTooSmall { required })
    }
}

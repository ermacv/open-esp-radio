//! Station data encapsulation into caller-owned output and Ethernet allocations.

use super::*;
use crate::address::{Bssid, OwnAddress};

mod amsdu;
pub use amsdu::{
    StaProtectedAmsduFrame, sta_protected_amsdu_frame_length, sta_protected_amsdu_pair_frame_length,
};

/// One unprotected 802.11 data MPDU sent by a station through its AP.
///
/// This is the frame shape used for EAPOL before CCMP keys are installed.
/// The caller owns the Ethernet payload and the output DMA buffer.
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
        validate_peer(self.bssid.get())?;
        let ethernet = ethernet_header(self.destination, self.source.get(), self.ether_type);
        let plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            self.bssid.get(),
            self.source.get(),
            ethernet,
            WmmUserPriority::UP7,
            false,
            false,
        );
        let mut header = plan.header;
        header[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
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
        let ethernet = ethernet_header(self.destination, self.source.get(), self.ether_type);
        let mut plan = plan_data_encapsulation_with_he_control(
            DataInterfaceRole::Station,
            self.bssid.get(),
            self.source.get(),
            ethernet,
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
        header[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
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

/// Metadata for converting one owned Ethernet frame to a protected data MPDU
/// in its existing allocation.
///
/// The buffer owner reserves prefix space before exposing the Ethernet slice
/// to a network stack. Once the stack returns ownership, [`Self::encode_in_place`]
/// replaces only the Ethernet header and reserved prefix. The payload is
/// already at its final DMA offset and is never copied.
///
/// This is the chip-independent half of the vendor cache-TX ESF contract.
/// Retaining the allocation until TX/BlockAck completion remains the
/// responsibility of the chip-specific DMA owner.
///
/// SOURCE(esp32s31): complete `libnet80211[ieee80211_output.o]::ieee80211_alloc_tx_buf` type-nine branch stores the referenced netstack
/// data pointer in the ESF DMA descriptor and calls `s_netstack_ref`.
/// Complete `libnet80211[ieee80211_output.o]::ieee80211_encap_esfbuf` mutates the ESF data boundary and writes
/// the 802.11/LLC prefix without copying the retained payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaProtectedEthernetFrame {
    pub bssid: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub user_priority: u8,
    pub peer_qos: bool,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
}

/// Location of an MPDU produced inside a larger owned allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodedStaFrame {
    pub offset: usize,
    pub length: usize,
}

impl StaProtectedEthernetFrame {
    /// Convert one Ethernet-II frame to a protected MPDU without moving its
    /// payload.
    ///
    /// `ethernet_offset` points to the DA byte written by the network stack;
    /// `ethernet_length` includes the fourteen-byte Ethernet header. On
    /// success the returned region starts in the reserved headroom and ends at
    /// exactly the same byte as the input Ethernet frame.
    pub fn encode_in_place(
        self,
        storage: &mut [u8],
        ethernet_offset: usize,
        ethernet_length: usize,
        he_control: DataHeControl,
    ) -> Result<EncodedStaFrame, StationFrameError> {
        validate_peer(self.bssid)?;
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
        if ethernet_length < ETHERNET_HEADER_LEN {
            return Err(StationFrameError::EthernetFrameTooShort);
        }
        let ethernet_end = ethernet_offset
            .checked_add(ethernet_length)
            .ok_or(StationFrameError::LengthOverflow)?;
        if storage.len() < ethernet_end {
            return Err(StationFrameError::OutputTooSmall {
                required: ethernet_end,
            });
        }

        // Preserve the bytes that the new CCMP/LLC prefix overlaps before
        // mutating the shared allocation.
        let Some(&ethernet) = storage
            .get(ethernet_offset..ethernet_end)
            .ok_or(StationFrameError::OutputTooSmall {
                required: ethernet_end,
            })?
            .first_chunk::<ETHERNET_HEADER_LEN>()
        else {
            return Err(StationFrameError::EthernetFrameTooShort);
        };
        let [_, _, _, _, _, _, source @ .., _, _] = ethernet;
        let mut plan = plan_data_encapsulation_with_he_control(
            DataInterfaceRole::Station,
            self.bssid,
            source,
            ethernet,
            priority,
            self.peer_qos,
            false,
            he_control,
        )
        .map_err(|_| StationFrameError::HeControlRequiresQos)?;
        plan.set_protected(true);

        // Only these two fixed MAC header geometries are admitted by the
        // station plan; CCMP and LLC/SNAP have fixed lengths.
        let headroom = if self.peer_qos {
            const {
                crate::data::IEEE80211_QOS_DATA_HEADER_LEN + CCMP_HEADER_LEN + LLC_SNAP_HEADER_LEN
                    - ETHERNET_HEADER_LEN
            }
        } else {
            const {
                crate::data::IEEE80211_LEGACY_DATA_HEADER_LEN
                    + CCMP_HEADER_LEN
                    + LLC_SNAP_HEADER_LEN
                    - ETHERNET_HEADER_LEN
            }
        };
        let frame_offset = ethernet_offset.checked_sub(headroom).ok_or(
            StationFrameError::EthernetHeadroomTooSmall {
                required: headroom,
                available: ethernet_offset,
            },
        )?;
        // Retain the complete admitted frame borrow for its length and the
        // prefix write. FrameParts writes only its own bytes, leaving the
        // original Ethernet payload at the same boundary.
        let frame = storage.get_mut(frame_offset..ethernet_end).ok_or(
            StationFrameError::OutputTooSmall {
                required: ethernet_end,
            },
        )?;
        let length = frame.len();
        plan.header[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        let header: &[u8] = if self.peer_qos {
            &plan.header
        } else {
            plan.header
                .split_at(const { crate::data::IEEE80211_LEGACY_DATA_HEADER_LEN })
                .0
        };
        crate::encoding::FrameParts([header, &self.ccmp_header, &plan.llc_snap])
            .write(frame)
            .map_err(|required| StationFrameError::OutputTooSmall { required })?;
        Ok(EncodedStaFrame {
            offset: frame_offset,
            length,
        })
    }

    /// Coalesce two Ethernet frames into one protected A-MSDU in the first
    /// frame's allocation.
    ///
    /// The first Ethernet payload is moved forward inside `storage`; the
    /// second frame is copied behind it. The returned MPDU starts at the same
    /// offset as [`Self::encode_in_place`], so backend metadata can remain
    /// immediately before it. The caller may release the second frame as soon
    /// as this method returns successfully.
    ///
    /// SOURCE(esp32s31): complete `libnet80211[ieee80211_output.o]::ieee80211_encap_amsdu`. Its `ieee80211_encap_amsdu+0xa8` branch uses `memmove` to grow the
    /// first cache ESF in place; `.L950` copies the following ESF body into
    /// that allocation and calls `libnet80211[ieee80211_output.o]::ieee80211_recycle_cache_eb` immediately.
    /// Thus vendor A-MSDU construction copies between netstack owners; only
    /// the resulting MPDU remains referenced through A-MPDU/DMA completion.
    pub fn encode_amsdu_pair_in_place(
        self,
        storage: &mut [u8],
        ethernet_offset: usize,
        ethernet_length: usize,
        second_ethernet: &[u8],
    ) -> Result<EncodedStaFrame, StationFrameError> {
        validate_peer(self.bssid)?;
        let priority = WmmUserPriority::new(self.user_priority)
            .ok_or(StationFrameError::UserPriorityOutOfRange)?;
        if !self.peer_qos {
            return Err(StationFrameError::AmsduRequiresQos);
        }
        let ethernet_end = ethernet_offset
            .checked_add(ethernet_length)
            .ok_or(StationFrameError::LengthOverflow)?;
        if ethernet_length < ETHERNET_HEADER_LEN || second_ethernet.len() < ETHERNET_HEADER_LEN {
            return Err(StationFrameError::EthernetFrameTooShort);
        }
        if storage.len() < ethernet_end {
            return Err(StationFrameError::OutputTooSmall {
                required: ethernet_end,
            });
        }

        let Some((first_header, first_payload)) = storage
            .get(ethernet_offset..ethernet_end)
            .ok_or(StationFrameError::OutputTooSmall {
                required: ethernet_end,
            })?
            .split_first_chunk::<ETHERNET_HEADER_LEN>()
        else {
            return Err(StationFrameError::EthernetFrameTooShort);
        };
        let first_payload_length = first_payload.len();
        let first_header = *first_header;
        let [_, _, _, _, _, _, source @ .., _, _] = first_header;
        let required =
            sta_protected_amsdu_pair_frame_length(ethernet_length, second_ethernet.len())?;
        let frame_offset = ethernet_offset
            .checked_sub(STA_PROTECTED_QOS_ETHERNET_HEADROOM)
            .ok_or(StationFrameError::EthernetHeadroomTooSmall {
                required: STA_PROTECTED_QOS_ETHERNET_HEADROOM,
                available: ethernet_offset,
            })?;
        let frame_end = frame_offset
            .checked_add(required)
            .ok_or(StationFrameError::LengthOverflow)?;
        if storage.len() < frame_end {
            return Err(StationFrameError::OutputTooSmall {
                required: frame_end,
            });
        }

        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            self.bssid,
            source,
            first_header,
            priority,
            true,
            false,
        );
        plan.set_protected(true);
        plan.set_amsdu(true);

        // A station A-MSDU always owns the complete fixed-size QoS header.
        let mut header = plan.header;
        header[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        const FIRST_PREFIX_LENGTH: usize = crate::data::IEEE80211_QOS_DATA_HEADER_LEN
            + CCMP_HEADER_LEN
            + AMSDU_SUBFRAME_HEADER_LEN
            + LLC_SNAP_HEADER_LEN;
        let first_prefix_length = FIRST_PREFIX_LENGTH;
        storage.copy_within(
            ethernet_offset
                .checked_add(ETHERNET_HEADER_LEN)
                .ok_or(StationFrameError::LengthOverflow)?..ethernet_end,
            frame_offset
                .checked_add(first_prefix_length)
                .ok_or(StationFrameError::LengthOverflow)?,
        );

        // Disjoint slices retain the moved first payload while the two
        // fragment plans write only its prefix and the following subframe.
        let frame =
            storage
                .get_mut(frame_offset..frame_end)
                .ok_or(StationFrameError::OutputTooSmall {
                    required: frame_end,
                })?;
        let (first_prefix, body) = frame.split_at_mut(first_prefix_length);
        let (_, following) = body.split_at_mut(first_payload_length);
        let first_msdu_length = LLC_SNAP_HEADER_LEN
            .checked_add(first_payload_length)
            .ok_or(StationFrameError::LengthOverflow)?;
        crate::encoding::FrameParts([
            header.as_slice(),
            &self.ccmp_header,
            &first_header[..12],
            &(first_msdu_length as u16).to_be_bytes(),
            &crate::data::RFC1042_LLC_SNAP_PREFIX,
            &first_header[12..14],
        ])
        .write(first_prefix)
        .map_err(|required| StationFrameError::OutputTooSmall { required })?;
        let first_subframe_length = AMSDU_SUBFRAME_HEADER_LEN
            .checked_add(first_msdu_length)
            .ok_or(StationFrameError::LengthOverflow)?;
        let padding = [0; 3];
        let first_padding = match first_subframe_length % 4 {
            0 => 0,
            1 => 3,
            2 => 2,
            _ => 1,
        };
        let mut reader = oer_wire::Reader::new(second_ethernet);
        let second_addresses = reader
            .array::<12>()
            .map_err(|_| StationFrameError::EthernetFrameTooShort)?;
        let second_type = reader
            .array::<2>()
            .map_err(|_| StationFrameError::EthernetFrameTooShort)?;
        let second_payload = reader.remaining();
        let second_msdu_length = LLC_SNAP_HEADER_LEN
            .checked_add(second_payload.len())
            .ok_or(StationFrameError::LengthOverflow)?;
        crate::encoding::FrameParts([
            oer_wire::Reader::new(&padding)
                .take(first_padding)
                .map_err(|_| StationFrameError::OutputTooSmall {
                    required: first_padding,
                })?,
            second_addresses,
            &(second_msdu_length as u16).to_be_bytes(),
            &crate::data::RFC1042_LLC_SNAP_PREFIX,
            second_type,
            second_payload,
        ])
        .write(following)
        .map_err(|required| StationFrameError::OutputTooSmall { required })?;
        Ok(EncodedStaFrame {
            offset: frame_offset,
            length: required,
        })
    }
}

const fn ethernet_header(
    destination: [u8; 6],
    source: [u8; 6],
    ether_type: u16,
) -> [u8; ETHERNET_HEADER_LEN] {
    [
        destination[0],
        destination[1],
        destination[2],
        destination[3],
        destination[4],
        destination[5],
        source[0],
        source[1],
        source[2],
        source[3],
        source[4],
        source[5],
        ether_type.to_be_bytes()[0],
        ether_type as u8,
    ]
}

//! Protected QoS A-MSDU sizing and encoding from complete Ethernet frames.

use super::*;

/// One protected QoS A-MSDU prepared for backend CCMP encryption.
///
/// Every borrowed element is a complete Ethernet-II frame. The encoder emits
/// one outer To-DS QoS MPDU and converts each Ethernet frame to the IEEE
/// 802.11 A-MSDU subframe form (DA, SA, big-endian MSDU length, RFC1042
/// LLC/SNAP body and non-final four-byte padding). The returned length stops
/// before the hardware-owned CCMP MIC and FCS, exactly like
/// [`StaProtectedDataFrame`].
///
/// SOURCE: IEEE 802.11 A-MSDU wire format, cross-checked against the inverse
/// iterator in `data::AmsduSubframes`. The 3,839-byte ceiling is the baseline
/// The smaller standard A-MSDU length used by the current bounded encoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaProtectedAmsduFrame<'a> {
    pub source: [u8; 6],
    pub bssid: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub user_priority: u8,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub ethernet_frames: &'a [&'a [u8]],
}

/// Returns the encoded protected QoS MPDU length for an A-MSDU.
///
/// The result excludes the hardware-owned CCMP MIC and FCS, matching
/// [`StaProtectedAmsduFrame::encode`]. Keeping this calculation beside the
/// encoder lets a bounded A-MPDU owner check its negotiated byte ceiling
/// before consuming a sequence number or CCMP packet number.
pub fn sta_protected_amsdu_frame_length(
    ethernet_frames: &[&[u8]],
) -> Result<usize, StationFrameError> {
    let body = crate::encoding::AmsduBody::checked(ethernet_frames, HT_AMSDU_BASELINE_MAX_LEN)
        .map_err(amsdu_error)?;
    let length = body
        .length()
        .map_err(|length| amsdu_error(crate::encoding::AmsduError::TooLong(length)))?;
    crate::data::IEEE80211_QOS_DATA_HEADER_LEN
        .checked_add(CCMP_HEADER_LEN)
        .and_then(|prefix| prefix.checked_add(length))
        .ok_or(StationFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })
}

/// Encoded protected-MPDU length for exactly two Ethernet MSDUs.
///
/// This is the non-mutating admission half of
/// [`StaProtectedEthernetFrame::encode_amsdu_pair_in_place`]. It allows a
/// bounded DMA owner to check both the 3,839-byte negotiated A-MSDU class and
/// its allocation capacity before consuming sequence and CCMP numbers.
pub fn sta_protected_amsdu_pair_frame_length(
    first_ethernet_length: usize,
    second_ethernet_length: usize,
) -> Result<usize, StationFrameError> {
    if first_ethernet_length < ETHERNET_HEADER_LEN || second_ethernet_length < ETHERNET_HEADER_LEN {
        return Err(StationFrameError::EthernetFrameTooShort);
    }
    let first_subframe = first_ethernet_length
        .checked_add(LLC_SNAP_HEADER_LEN)
        .ok_or(StationFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })?;
    let second_subframe = second_ethernet_length
        .checked_add(LLC_SNAP_HEADER_LEN)
        .ok_or(StationFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })?;
    let first_padding =
        4_usize
            .checked_sub(first_subframe & 3)
            .ok_or(StationFrameError::AmsduTooLong {
                length: usize::MAX,
                maximum: HT_AMSDU_BASELINE_MAX_LEN,
            })?
            & 3;
    let amsdu_length = first_subframe
        .checked_add(first_padding)
        .and_then(|length| length.checked_add(second_subframe))
        .ok_or(StationFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })?;
    if amsdu_length > HT_AMSDU_BASELINE_MAX_LEN {
        return Err(StationFrameError::AmsduTooLong {
            length: amsdu_length,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        });
    }
    crate::data::IEEE80211_QOS_DATA_HEADER_LEN
        .checked_add(CCMP_HEADER_LEN)
        .and_then(|length| length.checked_add(amsdu_length))
        .ok_or(StationFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        })
}

fn amsdu_error(error: crate::encoding::AmsduError) -> StationFrameError {
    match error {
        crate::encoding::AmsduError::Empty => StationFrameError::NoAmsduFrames,
        crate::encoding::AmsduError::ShortEthernet => StationFrameError::EthernetFrameTooShort,
        crate::encoding::AmsduError::TooLong(length) => StationFrameError::AmsduTooLong {
            length,
            maximum: HT_AMSDU_BASELINE_MAX_LEN,
        },
    }
}

impl<'a> StaProtectedAmsduFrame<'a> {
    fn plan(
        &self,
    ) -> Result<
        (
            [u8; crate::data::IEEE80211_QOS_DATA_HEADER_LEN],
            crate::encoding::AmsduBody<'a>,
        ),
        StationFrameError,
    > {
        validate_peer(self.bssid)?;
        let priority = WmmUserPriority::new(self.user_priority)
            .ok_or(StationFrameError::UserPriorityOutOfRange)?;
        let Some(first) = self.ethernet_frames.first().copied() else {
            return Err(StationFrameError::NoAmsduFrames);
        };
        let Some(&first_header) = first.first_chunk::<ETHERNET_HEADER_LEN>() else {
            return Err(StationFrameError::EthernetFrameTooShort);
        };
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            self.bssid,
            self.source,
            first_header,
            priority,
            true,
            false,
        );
        plan.set_protected(true);
        plan.set_amsdu(true);
        plan.header[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        let body =
            crate::encoding::AmsduBody::checked(self.ethernet_frames, HT_AMSDU_BASELINE_MAX_LEN)
                .map_err(amsdu_error)?;
        Ok((plan.header, body))
    }

    /// Refresh the MAC and CCMP header of an already encoded A-MSDU.
    ///
    /// The caller must retain the body produced by [`Self::encode`] for the
    /// same `ethernet_frames`, source and BSSID. This bounded operation is
    /// intended for a statically owned TX slot whose plaintext body was not
    /// changed by on-the-fly hardware CCMP. It owns no DMA pointer and cannot
    /// change the encoded length.
    ///
    /// SOURCE(esp32s31): `libpp[pp.o]::ppResortTxAMPDU` retains the complete
    /// CCMP-ready MPDU across a missing BlockAck bit and changes only retry
    /// metadata.
    ///
    /// SOURCE\[HIL_OPEN_HT40_AMSDU_BODY_REUSE_2026_07_29]: the
    /// production PSRAM/PSRAM image reused the body for more than 8,300
    /// accepted WPA2 HT40 MCS7 SGI aggregates; preparation fell from 768 us
    /// to 167 us and five-second samples sustained 102.8..109.7 Mbit/s.
    pub fn refresh_header(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        let (header, body) = self.plan()?;
        let body_length = body
            .length()
            .map_err(|length| amsdu_error(crate::encoding::AmsduError::TooLong(length)))?;
        let required = header
            .len()
            .checked_add(CCMP_HEADER_LEN)
            .and_then(|length| length.checked_add(body_length))
            .ok_or(StationFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(StationFrameError::OutputTooSmall { required });
        }
        crate::encoding::FrameParts([header.as_slice(), &self.ccmp_header])
            .write(output)
            .map_err(|required| StationFrameError::OutputTooSmall { required })?;
        Ok(required)
    }

    pub fn encode(self, output: &mut [u8]) -> Result<usize, StationFrameError> {
        let (header, body) = self.plan()?;
        crate::encoding::FrameParts([header.as_slice(), &self.ccmp_header])
            .write_amsdu(body, output)
            .map_err(|required| StationFrameError::OutputTooSmall { required })
    }
}

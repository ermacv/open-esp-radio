//! Protected QoS A-MSDU sizing and encoding from borrowed MSDUs.

use super::*;
use crate::data::MsduParts;

/// One protected QoS A-MSDU prepared for backend CCMP encryption.
///
/// The encoder emits one outer To-DS QoS MPDU and each borrowed MSDU in the
/// IEEE 802.11 A-MSDU subframe form (DA, SA, big-endian MSDU length, RFC1042
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
    pub msdus: &'a [MsduParts<'a>],
}

/// Returns the encoded protected QoS MPDU length for an A-MSDU.
///
/// The result excludes the hardware-owned CCMP MIC and FCS, matching
/// [`StaProtectedAmsduFrame::encode`]. Keeping this calculation beside the
/// encoder lets a bounded A-MPDU owner check its negotiated byte ceiling
/// before consuming a sequence number or CCMP packet number.
pub fn sta_protected_amsdu_frame_length(
    msdus: &[MsduParts<'_>],
) -> Result<usize, StationFrameError> {
    let body = crate::encoding::AmsduBody::checked(msdus, HT_AMSDU_BASELINE_MAX_LEN)
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

fn amsdu_error(error: crate::encoding::AmsduError) -> StationFrameError {
    match error {
        crate::encoding::AmsduError::Empty => StationFrameError::NoAmsduFrames,
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
        let Some(first) = self.msdus.first().copied() else {
            return Err(StationFrameError::NoAmsduFrames);
        };
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::Station,
            self.bssid,
            self.source,
            first.header(),
            priority,
            true,
            false,
        );
        plan.set_protected(true);
        plan.set_amsdu(true);
        crate::encoding::write_array::<22, _, _>(
            &mut plan.header,
            &self.sequence_number.sequence_control().to_le_bytes(),
        );
        let body = crate::encoding::AmsduBody::checked(self.msdus, HT_AMSDU_BASELINE_MAX_LEN)
            .map_err(amsdu_error)?;
        Ok((plan.header, body))
    }

    /// Refresh the MAC and CCMP header of an already encoded A-MSDU.
    ///
    /// The caller must retain the body produced by [`Self::encode`] for the
    /// same `msdus`, source and BSSID. This bounded operation is
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

//! Role-neutral validated receive view for hardware-deciphered data MPDUs.
//!
//! Peer admission, authorization, duplicate history and reorder policy remain
//! with AP/STA. Once a role admits the public header, both roles must decode
//! the S31 CCMP result and Ethernet payload through this single path.

use oer_esp32s31_ieee80211_mac::rx::{
    RxCcmpDataView, RxError, RxIngressConfig, RxPhyInfo, RxSegment, decode_normalized_rx_metadata,
    view_ccmp_data, view_ccmp_data_fragment, view_normalized_rx_frame,
};
use oer_ieee80211_port::{MacRxMetadata, RxCryptoStatus, RxEvidence};
use oer_ieee80211_wire::ccmp::CcmpHeader;
use oer_ieee80211_wire::data::{
    DataDecapError, DataDecapsulation, DataInterfaceRole, decapsulate_data_frames,
};
use oer_ieee80211_wire::fragmentation::{
    OpenDataFragment, OpenDataFragmentError, parse_ccmp_data_fragment, parse_open_data_fragment,
};

use oer_ieee80211_wire::frame::{FrameControlFields, FrameType, QosControl};
use oer_ieee80211_wire::sequence::SequenceControl;

macro_rules! protected_data_view_body {
    ($raw:ident, $data:ident) => {{
        let mpdu = &$data.mpdu[..$data.frame.mpdu.length];
        if mpdu.len() < 24 {
            return Err(RxError::Bounds);
        }
        let mut metadata = decode_normalized_rx_metadata($raw).ok_or(RxError::Metadata)?;
        // `view_ccmp_data` admits only the successful S31 RX state and rejects
        // the dedicated MIC-failure state before exposing plaintext.
        metadata.crypto =
            RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified);
        let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
        let sequence_control = u16::from_le_bytes([mpdu[22], mpdu[23]]);
        let control = FrameControlFields::from_raw(frame_control);
        let tid = if control.qos() && mpdu.len() >= 26 {
            Some(QosControl::from_bytes([mpdu[24], mpdu[25]]).tid())
        } else {
            None
        };
        Ok(ProtectedDataRxView {
            raw: $raw,
            mpdu,
            frame_control,
            sequence_control,
            tid,
            retry: control.retry(),
            ccmp_header: $data.frame.ccmp_header,
            payload_offset: $data.frame.payload_offset,
            payload_length: $data.frame.payload_length,
            metadata,
        })
    }};
}

#[derive(Clone, Copy, Debug)]
pub struct ProtectedDataRxView<'frame> {
    pub raw: &'frame [u8],
    pub mpdu: &'frame [u8],
    pub frame_control: u16,
    pub sequence_control: u16,
    pub tid: Option<u8>,
    pub retry: bool,
    pub ccmp_header: CcmpHeader,
    payload_offset: usize,
    payload_length: usize,
    metadata: MacRxMetadata<RxPhyInfo>,
}

/// Validate the chip RX/CCMP state and recover the role-independent public
/// ordering identity. No peer state is read or mutated here.
#[inline(always)]
pub fn view_protected_data(
    segment: RxSegment<'_>,
    ingress: RxIngressConfig,
) -> Result<ProtectedDataRxView<'_>, RxError> {
    let data = view_ccmp_data(&segment, ingress)?;
    protected_data_view_hot(segment.buffer, data)
}

#[inline(never)]
fn protected_data_view<'frame>(
    raw: &'frame [u8],
    data: RxCcmpDataView<'frame>,
) -> Result<ProtectedDataRxView<'frame>, RxError> {
    protected_data_view_body!(raw, data)
}

oer_esp32s31_hal::place_rx_hot_path! {
/// Decode the normalized metadata which every ordinary protected MPDU needs.
///
/// Keep the original PSRAM function present for the cold fragment path so this
/// controlled experiment does not shift all later cached code by its size.
#[inline(never)]
fn protected_data_view_hot<'frame>(
    raw: &'frame [u8],
    data: RxCcmpDataView<'frame>,
) -> Result<ProtectedDataRxView<'frame>, RxError> {
    protected_data_view_body!(raw, data)
}}

/// Hardware-authenticated plaintext view of one fragmented CCMP data MPDU.
///
/// Construction requires the fragment-only low-level CCMP validator, so an
/// ordinary protected frame cannot accidentally enter reassembly and a
/// fragment cannot enter ordinary LLC decapsulation.
#[derive(Clone, Copy, Debug)]
pub struct ProtectedDataFragmentRxView<'frame> {
    pub raw: &'frame [u8],
    pub mpdu: &'frame [u8],
    pub fragment: OpenDataFragment<'frame>,
    pub ccmp_header: CcmpHeader,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtectedDataFragmentRxError {
    Radio(RxError),
    Fragment(OpenDataFragmentError),
}

/// Validate S31 hardware MIC success and parse one CCMP data fragment for the
/// selected role. Replay state is deliberately untouched.
#[inline(always)]
pub fn view_protected_data_fragment(
    segment: RxSegment<'_>,
    ingress: RxIngressConfig,
    role: DataInterfaceRole,
) -> Result<ProtectedDataFragmentRxView<'_>, ProtectedDataFragmentRxError> {
    let data =
        view_ccmp_data_fragment(&segment, ingress).map_err(ProtectedDataFragmentRxError::Radio)?;
    let view =
        protected_data_view(segment.buffer, data).map_err(ProtectedDataFragmentRxError::Radio)?;
    let payload_end = view
        .payload_offset
        .checked_add(view.payload_length)
        .ok_or(ProtectedDataFragmentRxError::Radio(RxError::Bounds))?;
    let payload = view
        .mpdu
        .get(view.payload_offset..payload_end)
        .ok_or(ProtectedDataFragmentRxError::Radio(RxError::Bounds))?;
    let fragment = parse_ccmp_data_fragment(role, view.mpdu, payload, view.ccmp_header)
        .map_err(ProtectedDataFragmentRxError::Fragment)?;
    let mut metadata = view.metadata;
    metadata.amsdu = RxEvidence::ProtocolValidated(false);
    Ok(ProtectedDataFragmentRxView {
        raw: view.raw,
        mpdu: view.mpdu,
        fragment,
        ccmp_header: view.ccmp_header,
        metadata,
    })
}

pub struct ProtectedDataDecapsulation<'frame> {
    pub frames: DataDecapsulation<'frame>,
    pub raw: &'frame [u8],
    pub amsdu: bool,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

impl<'frame> ProtectedDataRxView<'frame> {
    /// Apply only the portable STA/AP address mapping after role admission.
    #[inline(always)]
    pub fn decapsulate(
        self,
        role: DataInterfaceRole,
    ) -> Result<ProtectedDataDecapsulation<'frame>, DataDecapError> {
        let frames =
            decapsulate_data_frames(role, self.mpdu, self.payload_offset, self.payload_length)?;
        let amsdu = frames.is_amsdu();
        let mut metadata = self.metadata;
        metadata.amsdu = RxEvidence::ProtocolValidated(amsdu);
        Ok(ProtectedDataDecapsulation {
            frames,
            raw: self.raw,
            amsdu,
            metadata,
        })
    }
}

/// Validated plaintext view of one complete unprotected data MPDU.
#[derive(Clone, Copy, Debug)]
pub struct UnprotectedDataRxView<'frame> {
    pub raw: &'frame [u8],
    pub mpdu: &'frame [u8],
    pub frame_control: u16,
    pub sequence_control: u16,
    pub tid: Option<u8>,
    pub retry: bool,
    payload_offset: usize,
    payload_length: usize,
    metadata: MacRxMetadata<RxPhyInfo>,
}

/// Validate one contiguous Open-network data MPDU without accepting a
/// protected, fragmented, truncated or non-data unit.
#[inline(always)]
pub fn view_unprotected_data(
    segment: RxSegment<'_>,
    ingress: RxIngressConfig,
) -> Result<UnprotectedDataRxView<'_>, RxError> {
    let normalized = view_normalized_rx_frame(&segment, ingress)?;
    let mpdu = normalized.mpdu;
    if normalized.logical_length != mpdu.len() || mpdu.len() < 24 {
        return Err(RxError::Bounds);
    }
    let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    let control = FrameControlFields::from_raw(frame_control);
    if control.kind() != FrameType::Data || !matches!(control.subtype(), 0 | 8) {
        return Err(RxError::Ignored);
    }
    let sequence_control = u16::from_le_bytes([mpdu[22], mpdu[23]]);
    if control.more_fragments()
        || control.protected()
        || control.order()
        || SequenceControl::from_bytes(sequence_control.to_le_bytes()).fragment() != 0
    {
        return Err(RxError::Unsupported);
    }
    let qos = control.qos();
    let mut payload_offset = 24 + usize::from(control.ds() == 3) * 6;
    if qos {
        payload_offset += 2;
    }
    if mpdu.len() < payload_offset {
        return Err(RxError::Bounds);
    }
    let tid = qos.then(|| {
        QosControl::from_bytes([mpdu[payload_offset - 2], mpdu[payload_offset - 1]]).tid()
    });
    let mut metadata = normalized.metadata;
    metadata.crypto = RxEvidence::ProtocolValidated(RxCryptoStatus::Unprotected);
    Ok(UnprotectedDataRxView {
        raw: segment.buffer,
        mpdu,
        frame_control,
        sequence_control,
        tid,
        retry: control.retry(),
        payload_offset,
        payload_length: mpdu.len() - payload_offset,
        metadata,
    })
}

impl<'frame> UnprotectedDataRxView<'frame> {
    #[inline(always)]
    pub fn decapsulate(
        self,
        role: DataInterfaceRole,
    ) -> Result<ProtectedDataDecapsulation<'frame>, DataDecapError> {
        let frames =
            decapsulate_data_frames(role, self.mpdu, self.payload_offset, self.payload_length)?;
        let amsdu = frames.is_amsdu();
        let mut metadata = self.metadata;
        metadata.amsdu = RxEvidence::ProtocolValidated(amsdu);
        Ok(ProtectedDataDecapsulation {
            frames,
            raw: self.raw,
            amsdu,
            metadata,
        })
    }
}

/// Validated Open-network view of one fragmented data MPDU.
///
/// Unlike [`UnprotectedDataRxView`], this value cannot be directly
/// decapsulated. Its fragment token must cross the fixed-capacity reassembly
/// owner before any Ethernet payload is exposed.
#[derive(Clone, Copy, Debug)]
pub struct UnprotectedDataFragmentRxView<'frame> {
    pub raw: &'frame [u8],
    pub mpdu: &'frame [u8],
    pub fragment: OpenDataFragment<'frame>,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnprotectedDataFragmentRxError {
    Radio(RxError),
    Fragment(OpenDataFragmentError),
}

/// Validate normalized S31 receive state and strictly parse one Open data
/// fragment for the selected STA/AP address mapping.
#[inline(always)]
pub fn view_unprotected_data_fragment(
    segment: RxSegment<'_>,
    ingress: RxIngressConfig,
    role: DataInterfaceRole,
) -> Result<UnprotectedDataFragmentRxView<'_>, UnprotectedDataFragmentRxError> {
    let normalized = view_normalized_rx_frame(&segment, ingress)
        .map_err(UnprotectedDataFragmentRxError::Radio)?;
    let mpdu = normalized.mpdu;
    if normalized.logical_length != mpdu.len() {
        return Err(UnprotectedDataFragmentRxError::Radio(RxError::Bounds));
    }
    let fragment =
        parse_open_data_fragment(role, mpdu).map_err(UnprotectedDataFragmentRxError::Fragment)?;
    let mut metadata = normalized.metadata;
    metadata.crypto = RxEvidence::ProtocolValidated(RxCryptoStatus::Unprotected);
    metadata.amsdu = RxEvidence::ProtocolValidated(false);
    Ok(UnprotectedDataFragmentRxView {
        raw: segment.buffer,
        mpdu,
        fragment,
        metadata,
    })
}

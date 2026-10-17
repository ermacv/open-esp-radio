//! Ordinary STA/AP data framing and explicit receive retry history.
//!
//! Encoders are stateless; `duplicate` retains per-peer receive history.
//! This is the live, chip-independent owner of the finite encapsulation
//! policy. Raw ESF, descriptor,
//! node, key, and interface accesses deliberately remain outside this module.
//! Encapsulation takes a checked `WmmUserPriority`; a basic data plan is
//! infallible. Requesting hardware HE-Control can fail only when the resulting
//! frame is not individual QoS data, reported as `HeControlRequiresQos`.

use crate::qos::{WmmAccessCategory, WmmUserPriority};

pub const ETHERNET_HEADER_LEN: usize = 14;
pub const IEEE80211_LEGACY_DATA_HEADER_LEN: usize = 24;
pub const IEEE80211_QOS_DATA_HEADER_LEN: usize = 26;
pub const IEEE80211_HE_CONTROL_LEN: usize = 4;
pub const LLC_SNAP_HEADER_LEN: usize = 8;

const IEEE80211_DATA: u8 = 0x08;
const IEEE80211_QOS_DATA: u8 = 0x88;
const IEEE80211_TO_DS: u8 = 0x01;
const IEEE80211_FROM_DS: u8 = 0x02;
const IEEE80211_ORDER: u8 = 0x80;
const IEEE80211_QOS_AMSDU_PRESENT: u8 = 0x80;
const QOS_NO_ACK_POLICY: u8 = 0x20;
/// RFC 1042 LLC/SNAP prefix before the EtherType.
pub const RFC1042_LLC_SNAP_PREFIX: [u8; 6] = [0xaa, 0xaa, 0x03, 0, 0, 0];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataInterfaceRole {
    Station,
    AccessPoint,
}

pub mod duplicate;
pub use duplicate::RxDuplicateFilter;

/// HE-Control policy for an ordinary QoS data MPDU.
///
/// A backend that explicitly supports hardware insertion keeps CCMP
/// immediately after the 26-byte QoS header and accounts separately for the
/// four bytes inserted on air. Keeping this capability explicit prevents an
/// ordinary HT QoS frame from advertising a field absent on air.
///
/// SOURCE(esp32s31): complete `libnet80211[ieee80211_he.o]::ieee80211_encap_esfbuf_htc` sets Order and descriptor HTC metadata;
/// `libpp[hal_mac_ctl.o]::hal_he_set_htc` writes the per-queue HTC
/// word and its software-select bit. Complete `libpp[pp_he.o]::ppCalSubFrameLength` adds four bytes when DMA metadata byte seven bit zero
/// is set. `HIL_VENDOR_HE_CONTROL_INSERTION_2026_07_30` captured vendor DMA
/// with no placeholder, metadata byte seven equal to one, and an intact CCMP
/// header following the hardware-inserted HE-Control field on air.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DataHeControl {
    #[default]
    Disabled,
    HardwareGeneratedBufferStatusReport,
}

impl DataHeControl {
    /// Bytes inserted into the on-air MPDU by MAC hardware.
    ///
    /// These bytes are not present in the chip-independent encoded frame.
    pub const fn inserted_air_len(self) -> usize {
        match self {
            Self::Disabled => 0,
            Self::HardwareGeneratedBufferStatusReport => IEEE80211_HE_CONTROL_LEN,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataEncapPlan {
    /// Complete DMA-resident 802.11 header. A hardware-generated HE-Control
    /// field is inserted on air and is deliberately absent here.
    pub header: [u8; IEEE80211_QOS_DATA_HEADER_LEN],
    pub header_len: u8,
    pub llc_snap: [u8; LLC_SNAP_HEADER_LEN],
    pub descriptor_multicast: bool,
    /// Standard WMM category; hardware queue and packet-type encoding belong to the chip.
    pub access_category: WmmAccessCategory,
    pub he_control: DataHeControl,
}

impl DataEncapPlan {
    /// Complete DMA prefix before plaintext/CCMP payload.
    pub const fn dma_header_len(self) -> usize {
        self.header_len as usize
    }
}

/// Bounded 802.11-to-Ethernet transform for one ordinary STA/AP MSDU.
///
/// The plan borrows no DMA storage. After [`decapsulate_data`] succeeds, the
/// caller owns a complete Ethernet frame in its output buffer and may return
/// the source RX descriptor to the radio. This is the copy-owned counterpart
/// of the slot/token boundary retained by the production RX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataDecapPlan {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload_offset: usize,
    pub payload_length: usize,
    pub ethernet_length: usize,
}

/// Borrowed Ethernet-II frame represented without an intermediate contiguous
/// scratch allocation.
///
/// Receive integrations can copy these four parts directly into their final
/// owned network-queue slot. The view never outlives the validated MPDU or
/// A-MSDU subframe from which its payload was borrowed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EthernetFrameParts<'payload> {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload: &'payload [u8],
}

impl EthernetFrameParts<'_> {
    pub const fn length(self) -> usize {
        ETHERNET_HEADER_LEN + self.payload.len()
    }

    pub fn copy_to(self, output: &mut [u8]) -> Result<usize, DataDecapError> {
        let required = self.length();
        if output.len() < required {
            return Err(DataDecapError::OutputTooSmall { required });
        }
        output[..6].copy_from_slice(&self.destination);
        output[6..12].copy_from_slice(&self.source);
        output[12..14].copy_from_slice(&self.ether_type.to_be_bytes());
        output[14..required].copy_from_slice(self.payload);
        Ok(required)
    }
}

/// One Ethernet MSDU borrowed from a validated 802.11 A-MSDU payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmsduSubframe<'a> {
    pub destination: [u8; 6],
    pub source: [u8; 6],
    pub ether_type: u16,
    pub payload: &'a [u8],
}

/// Allocation-free iterator over the subframes of one received A-MSDU.
///
/// Construction rejects the entire aggregate when its first destination is
/// `AA:AA:03:00:00:00`, the RFC 1042 LLC/SNAP prefix. An attacker can otherwise
/// flip the unauthenticated A-MSDU-present bit of an ordinary QoS MSDU and
/// reinterpret its LLC header as a subframe destination (CVE-2020-24588).
/// No subframe is exposed before this check, including on the CCMP-verified
/// path: integrity evidence alone does not establish negotiated SPP A-MSDU.
/// This follows the [Linux aggregation-attack mitigation][mitigation].
///
/// [mitigation]: https://git.kernel.org/linus/2b8a1fee3488c602aca8bea004a087e60806a5cf
///
/// SOURCE: IEEE 802.11 A-MSDU subframe format (DA, SA, big-endian MSDU
/// length, LLC/SNAP MSDU and four-byte padding). The need for this path was
/// confirmed by the reviewed source promoted at commit `f233006`, whose
/// `indicate_multi_received_frame` joins a received MPDU split across Wi-Fi
/// DMA descriptors before handing it to the upper data path.
#[derive(Clone, Debug)]
pub struct AmsduSubframes<'a> {
    remaining: &'a [u8],
    failed: bool,
}

/// Role-neutral Ethernet views recovered from one validated data MPDU.
///
/// This iterator is the common STA/AP boundary for ordinary MSDU and A-MSDU
/// decapsulation. Address admission, peer authorization and duplicate
/// history remain with the role-specific receive owner; once admitted, both
/// roles must interpret the data payload identically.
#[derive(Clone, Debug)]
pub struct DataDecapsulation<'a> {
    frames: DataDecapsulationFrames<'a>,
    amsdu: bool,
}

#[derive(Clone, Debug)]
enum DataDecapsulationFrames<'a> {
    Single(Option<EthernetFrameParts<'a>>),
    Aggregate(AmsduSubframes<'a>),
}

impl DataDecapsulation<'_> {
    pub const fn is_amsdu(&self) -> bool {
        self.amsdu
    }
}

impl<'a> Iterator for DataDecapsulation<'a> {
    type Item = Result<EthernetFrameParts<'a>, DataDecapError>;

    #[inline(always)]
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.frames {
            DataDecapsulationFrames::Single(frame) => frame.take().map(Ok),
            DataDecapsulationFrames::Aggregate(frames) => frames.next().map(|frame| {
                frame.map(|frame| EthernetFrameParts {
                    destination: frame.destination,
                    source: frame.source,
                    ether_type: frame.ether_type,
                    payload: frame.payload,
                })
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataDecapError {
    Truncated,
    NotData,
    Fragmented,
    RoleMismatch,
    AmsduUnsupported,
    /// The first A-MSDU destination aliases an ordinary LLC/SNAP header.
    InvalidAmsduDestination,
    InvalidLlcSnap,
    OutputTooSmall {
        required: usize,
    },
}

impl<'a> Iterator for AmsduSubframes<'a> {
    type Item = Result<AmsduSubframe<'a>, DataDecapError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.remaining.is_empty() {
            return None;
        }
        if self.remaining.len() < ETHERNET_HEADER_LEN {
            self.failed = true;
            return Some(Err(DataDecapError::Truncated));
        }
        let msdu_length = usize::from(u16::from_be_bytes([self.remaining[12], self.remaining[13]]));
        let subframe_length = match ETHERNET_HEADER_LEN.checked_add(msdu_length) {
            Some(length)
                if msdu_length >= LLC_SNAP_HEADER_LEN && length <= self.remaining.len() =>
            {
                length
            }
            _ => {
                self.failed = true;
                return Some(Err(DataDecapError::Truncated));
            }
        };
        let llc = &self.remaining[ETHERNET_HEADER_LEN..ETHERNET_HEADER_LEN + LLC_SNAP_HEADER_LEN];
        if llc[..RFC1042_LLC_SNAP_PREFIX.len()] != RFC1042_LLC_SNAP_PREFIX {
            self.failed = true;
            return Some(Err(DataDecapError::InvalidLlcSnap));
        }
        let mut destination = [0; 6];
        destination.copy_from_slice(&self.remaining[..6]);
        let mut source = [0; 6];
        source.copy_from_slice(&self.remaining[6..12]);
        let subframe = AmsduSubframe {
            destination,
            source,
            ether_type: u16::from_be_bytes([llc[6], llc[7]]),
            payload: &self.remaining[ETHERNET_HEADER_LEN + LLC_SNAP_HEADER_LEN..subframe_length],
        };

        if subframe_length == self.remaining.len() {
            self.remaining = &[];
        } else {
            let padded_length = match subframe_length.checked_add(3) {
                Some(length) => length & !3,
                None => {
                    self.failed = true;
                    return Some(Err(DataDecapError::Truncated));
                }
            };
            if padded_length > self.remaining.len() {
                self.failed = true;
                return Some(Err(DataDecapError::Truncated));
            }
            self.remaining = &self.remaining[padded_length..];
        }
        Some(Ok(subframe))
    }
}

/// Plan a data MPDU from a checked user priority.
///
/// A raw priority cannot enter the encoder:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_mac::data::{DataInterfaceRole, plan_data_encapsulation};
/// let _plan = plan_data_encapsulation(
///     DataInterfaceRole::Station, [2; 6], [4; 6], [0; 14], 8, true, false,
/// );
/// ```
///
/// Fixed protocol priorities need no runtime validation:
///
/// ```
/// use oer_ieee80211_mac::data::{DataInterfaceRole, plan_data_encapsulation};
/// use oer_ieee80211_mac::qos::{WmmAccessCategory, WmmUserPriority};
/// let plan = plan_data_encapsulation(
///     DataInterfaceRole::Station, [2; 6], [4; 6], [0; 14],
///     WmmUserPriority::UP7, true, false,
/// );
/// assert_eq!(plan.access_category, WmmAccessCategory::Voice);
/// ```
pub const fn plan_data_encapsulation(
    role: DataInterfaceRole,
    bssid: [u8; 6],
    interface_mac: [u8; 6],
    ethernet: [u8; ETHERNET_HEADER_LEN],
    priority: WmmUserPriority,
    peer_qos: bool,
    no_ack_policy: bool,
) -> DataEncapPlan {
    let [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, e0, e1] = ethernet;
    let destination = [d0, d1, d2, d3, d4, d5];
    let source = [s0, s1, s2, s3, s4, s5];
    let descriptor_multicast = matches!(role, DataInterfaceRole::AccessPoint) && d0 & 1 != 0;
    let qos = peer_qos && !descriptor_multicast;
    let mut header = [0_u8; IEEE80211_QOS_DATA_HEADER_LEN];
    header[0] = if qos {
        IEEE80211_QOS_DATA
    } else {
        IEEE80211_DATA
    };
    match role {
        DataInterfaceRole::Station => {
            header[1] = IEEE80211_TO_DS;
            copy_six(&mut header, 4, bssid);
            copy_six(&mut header, 10, source);
            copy_six(&mut header, 16, destination);
        }
        DataInterfaceRole::AccessPoint => {
            header[1] = IEEE80211_FROM_DS;
            copy_six(&mut header, 4, destination);
            copy_six(&mut header, 10, interface_mac);
            copy_six(&mut header, 16, source);
        }
    }
    if qos {
        header[24] = priority.value() | if no_ack_policy { QOS_NO_ACK_POLICY } else { 0 };
    }
    DataEncapPlan {
        header,
        header_len: if qos {
            IEEE80211_QOS_DATA_HEADER_LEN as u8
        } else {
            IEEE80211_LEGACY_DATA_HEADER_LEN as u8
        },
        llc_snap: [0xaa, 0xaa, 0x03, 0, 0, 0, e0, e1],
        descriptor_multicast,
        access_category: priority.access_category(),
        he_control: DataHeControl::Disabled,
    }
}

/// Hardware-generated HE-Control requires an individual QoS data MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeControlRequiresQos;

/// Plan one data MPDU whose HE-Control bytes are inserted by MAC hardware.
///
/// # Errors
///
/// [`HeControlRequiresQos`] when insertion is requested for a non-QoS frame,
/// including an AP's group-addressed frame even when the peer supports QoS.
///
/// SOURCE(esp32s31): complete
/// `libnet80211[ieee80211_he.o]::ieee80211_encap_esfbuf_htc`
/// sets byte-one bit seven but does not extend or move the DMA header.
/// Complete `libpp[pp_he.o]::ppCalSubFrameLength` accounts for the
/// inserted bytes through DMA metadata byte seven bit zero; that chip-specific
/// metadata remains outside this IEEE 802.11 encoder.
#[allow(clippy::too_many_arguments)]
pub const fn plan_data_encapsulation_with_he_control(
    role: DataInterfaceRole,
    bssid: [u8; 6],
    interface_mac: [u8; 6],
    ethernet: [u8; ETHERNET_HEADER_LEN],
    priority: WmmUserPriority,
    peer_qos: bool,
    no_ack_policy: bool,
    he_control: DataHeControl,
) -> Result<DataEncapPlan, HeControlRequiresQos> {
    let mut plan = plan_data_encapsulation(
        role,
        bssid,
        interface_mac,
        ethernet,
        priority,
        peer_qos,
        no_ack_policy,
    );
    if matches!(
        he_control,
        DataHeControl::HardwareGeneratedBufferStatusReport
    ) {
        if plan.header_len != IEEE80211_QOS_DATA_HEADER_LEN as u8 {
            return Err(HeControlRequiresQos);
        }
        // The chip-specific DMA owner separately marks the four-byte hardware
        // insertion; the encoded QoS header itself remains 26 bytes.
        plan.header[1] |= IEEE80211_ORDER;
    }
    plan.he_control = he_control;
    Ok(plan)
}

/// Validate one ordinary STA/AP data MPDU and locate its Ethernet payload.
///
/// `payload_offset` and `payload_length` describe the decrypted LLC/SNAP view
/// produced by the MAC crate. For protected frames the offset is after the
/// retained CCMP header; a hardware-consumed MIC is therefore naturally
/// excluded by `payload_length`.
#[inline(always)]
pub fn plan_data_decapsulation(
    role: DataInterfaceRole,
    mpdu: &[u8],
    payload_offset: usize,
    payload_length: usize,
) -> Result<DataDecapPlan, DataDecapError> {
    let header = data_header(role, mpdu)?;
    if header
        .qos()
        .is_some_and(|qos| qos & u16::from(IEEE80211_QOS_AMSDU_PRESENT) != 0)
    {
        return Err(DataDecapError::AmsduUnsupported);
    }
    let payload = data_payload(header, payload_offset, payload_length)?;
    let parts = single_payload(header, payload)?;
    Ok(DataDecapPlan {
        destination: parts.destination,
        source: parts.source,
        ether_type: parts.ether_type,
        payload_offset: payload_offset + LLC_SNAP_HEADER_LEN,
        payload_length: parts.payload.len(),
        ethernet_length: parts.length(),
    })
}

fn data_header(
    role: DataInterfaceRole,
    mpdu: &[u8],
) -> Result<crate::frame::DecodedHeader<'_>, DataDecapError> {
    let header = crate::frame::DecodedHeader::decode(mpdu).ok_or(DataDecapError::Truncated)?;
    let control = header.control();
    if !control.is_data() {
        return Err(DataDecapError::NotData);
    }
    if control.more_fragments() || header.sequence_control() & 15 != 0 {
        return Err(DataDecapError::Fragmented);
    }
    if !matches!(
        (role, control.ds()),
        (DataInterfaceRole::Station, 2) | (DataInterfaceRole::AccessPoint, 1)
    ) {
        return Err(DataDecapError::RoleMismatch);
    }
    Ok(header)
}

fn data_payload(
    header: crate::frame::DecodedHeader<'_>,
    offset: usize,
    length: usize,
) -> Result<&[u8], DataDecapError> {
    let end = offset
        .checked_add(length)
        .ok_or(DataDecapError::Truncated)?;
    if offset < header.frame().len() - header.body().len() {
        return Err(DataDecapError::Truncated);
    }
    header
        .frame()
        .get(offset..end)
        .ok_or(DataDecapError::Truncated)
}

fn single_payload<'a>(
    header: crate::frame::DecodedHeader<'_>,
    payload: &'a [u8],
) -> Result<EthernetFrameParts<'a>, DataDecapError> {
    let (llc, payload) = payload
        .split_first_chunk::<LLC_SNAP_HEADER_LEN>()
        .ok_or(DataDecapError::Truncated)?;
    if llc[..RFC1042_LLC_SNAP_PREFIX.len()] != RFC1042_LLC_SNAP_PREFIX {
        return Err(DataDecapError::InvalidLlcSnap);
    }
    let (destination, source) = match header.control().ds() {
        2 => (header.receiver(), header.address3()),
        1 => (header.address3(), header.transmitter()),
        _ => return Err(DataDecapError::RoleMismatch),
    };
    Ok(EthernetFrameParts {
        destination,
        source,
        ether_type: u16::from_be_bytes([llc[6], llc[7]]),
        payload,
    })
}

/// Validate the outer QoS data frame and borrow its complete A-MSDU payload.
pub fn amsdu_subframes<'a>(
    role: DataInterfaceRole,
    mpdu: &'a [u8],
    payload_offset: usize,
    payload_length: usize,
) -> Result<AmsduSubframes<'a>, DataDecapError> {
    let header = data_header(role, mpdu)?;
    if !header
        .qos()
        .is_some_and(|qos| qos & u16::from(IEEE80211_QOS_AMSDU_PRESENT) != 0)
    {
        return Err(DataDecapError::AmsduUnsupported);
    }
    let payload = data_payload(header, payload_offset, payload_length)?;
    aggregate_payload(payload)
}

fn aggregate_payload(payload: &[u8]) -> Result<AmsduSubframes<'_>, DataDecapError> {
    let (first, _) = payload
        .split_first_chunk::<{ ETHERNET_HEADER_LEN + LLC_SNAP_HEADER_LEN }>()
        .ok_or(DataDecapError::Truncated)?;
    if first.first_chunk::<6>() == Some(&RFC1042_LLC_SNAP_PREFIX) {
        return Err(DataDecapError::InvalidAmsduDestination);
    }
    Ok(AmsduSubframes {
        remaining: payload,
        failed: false,
    })
}

/// Validate one role-addressed data MPDU and expose all contained Ethernet
/// frames without allocation or chip/runtime policy. Malformed later A-MSDU
/// subframes remain errors at their iterator position.
pub fn decapsulate_data_frames<'a>(
    role: DataInterfaceRole,
    mpdu: &'a [u8],
    payload_offset: usize,
    payload_length: usize,
) -> Result<DataDecapsulation<'a>, DataDecapError> {
    let header = data_header(role, mpdu)?;
    decapsulate_payload(
        header,
        data_payload(header, payload_offset, payload_length)?,
    )
}

/// The receive admission owner has already checked this header and located
/// the decrypted payload. Do not parse header geometry or commit replay again.
pub(crate) fn decapsulate_payload<'a>(
    header: crate::frame::DecodedHeader<'_>,
    payload: &'a [u8],
) -> Result<DataDecapsulation<'a>, DataDecapError> {
    let amsdu = header
        .qos()
        .is_some_and(|qos| qos & u16::from(IEEE80211_QOS_AMSDU_PRESENT) != 0);
    let frames = if amsdu {
        DataDecapsulationFrames::Aggregate(aggregate_payload(payload)?)
    } else {
        DataDecapsulationFrames::Single(Some(single_payload(header, payload)?))
    };
    Ok(DataDecapsulation { frames, amsdu })
}

/// Copy one validated A-MSDU subframe into ordinary Ethernet-II storage.
pub fn decapsulate_amsdu_subframe(
    subframe: AmsduSubframe<'_>,
    ethernet: &mut [u8],
) -> Result<usize, DataDecapError> {
    let required = ETHERNET_HEADER_LEN
        .checked_add(subframe.payload.len())
        .ok_or(DataDecapError::Truncated)?;
    if ethernet.len() < required {
        return Err(DataDecapError::OutputTooSmall { required });
    }
    ethernet[..6].copy_from_slice(&subframe.destination);
    ethernet[6..12].copy_from_slice(&subframe.source);
    ethernet[12..14].copy_from_slice(&subframe.ether_type.to_be_bytes());
    ethernet[14..required].copy_from_slice(subframe.payload);
    Ok(required)
}

/// Copy one validated MSDU into caller-owned Ethernet storage.
///
/// SOURCE\[HIL_OPEN_HE20_RX_RING_STARVATION_2026_07_29]: the complete post-CCMP
/// receive path executed from PSRAM plateaued at 63.1..65.3 Mbit/s while the
/// MAC reported an additional raw interrupt bit under load. Code placement is
/// deliberately left to the final image/linker policy rather than embedded in
/// this hardware-independent protocol crate.
#[inline(never)]
pub fn decapsulate_data(
    role: DataInterfaceRole,
    mpdu: &[u8],
    payload_offset: usize,
    payload_length: usize,
    ethernet: &mut [u8],
) -> Result<DataDecapPlan, DataDecapError> {
    let plan = plan_data_decapsulation(role, mpdu, payload_offset, payload_length)?;
    if ethernet.len() < plan.ethernet_length {
        return Err(DataDecapError::OutputTooSmall {
            required: plan.ethernet_length,
        });
    }
    ethernet[..6].copy_from_slice(&plan.destination);
    ethernet[6..12].copy_from_slice(&plan.source);
    ethernet[12..14].copy_from_slice(&plan.ether_type.to_be_bytes());
    ethernet[14..plan.ethernet_length]
        .copy_from_slice(&mpdu[plan.payload_offset..plan.payload_offset + plan.payload_length]);
    Ok(plan)
}

const fn copy_six(output: &mut [u8; IEEE80211_QOS_DATA_HEADER_LEN], at: usize, value: [u8; 6]) {
    let mut index = 0;
    while index != value.len() {
        output[at + index] = value[index];
        index += 1;
    }
}

#[cfg(test)]
mod tests;

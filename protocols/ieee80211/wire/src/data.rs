//! Ordinary STA/AP data framing and explicit receive retry history.
//!
//! Encoders are stateless; `duplicate` retains per-peer receive history.
//! This is the live, chip-independent owner of the finite encapsulation
//! policy. Raw ESF, descriptor,
//! node, key, and interface accesses deliberately remain outside this module.
//! Encapsulation takes a checked `WmmUserPriority`; a basic data plan is
//! infallible. Requesting hardware HE-Control can fail only when the resulting
//! frame is not individual QoS data, reported as `HeControlRequiresQos`.
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
        // A byte slice holds at most isize::MAX bytes, so adding the 14-byte
        // header cannot reach usize::MAX.
        ETHERNET_HEADER_LEN.saturating_add(self.payload.len())
    }

    pub fn copy_to(self, output: &mut [u8]) -> Result<usize, DataDecapError> {
        write_ethernet(
            output,
            self.length(),
            ethernet_header(self.destination, self.source, self.ether_type),
            self.payload,
        )
    }
}

/// The Ethernet-II header of `destination`, `source` and `ether_type`.
const fn ethernet_header(
    destination: [u8; 6],
    source: [u8; 6],
    ether_type: u16,
) -> [u8; ETHERNET_HEADER_LEN] {
    let [d0, d1, d2, d3, d4, d5] = destination;
    let [s0, s1, s2, s3, s4, s5] = source;
    let [e0, e1] = ether_type.to_be_bytes();
    [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, e0, e1]
}

/// Write `header` and `payload` to the start of `output`, `required` bytes in
/// all, or refuse an output shorter than that before writing anything.
fn write_ethernet(
    output: &mut [u8],
    required: usize,
    header: [u8; ETHERNET_HEADER_LEN],
    payload: &[u8],
) -> Result<usize, DataDecapError> {
    let too_small = DataDecapError::OutputTooSmall { required };
    let (output_header, rest) = output
        .split_first_chunk_mut::<ETHERNET_HEADER_LEN>()
        .ok_or(too_small)?;
    let (output_payload, _) = rest.split_at_mut_checked(payload.len()).ok_or(too_small)?;
    *output_header = header;
    output_payload.copy_from_slice(payload);
    Ok(required)
}

/// The EtherType after an RFC 1042 LLC/SNAP header, or `None` for another
/// header.
pub(crate) const fn rfc1042_ether_type(llc: [u8; LLC_SNAP_HEADER_LEN]) -> Option<u16> {
    let [p0, p1, p2, p3, p4, p5, e0, e1] = llc;
    let [r0, r1, r2, r3, r4, r5] = RFC1042_LLC_SNAP_PREFIX;
    if p0 == r0 && p1 == r1 && p2 == r2 && p3 == r3 && p4 == r4 && p5 == r5 {
        Some(u16::from_be_bytes([e0, e1]))
    } else {
        None
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
        let subframe = self.split_subframe();
        self.failed = subframe.is_err();
        Some(subframe)
    }
}

impl<'a> AmsduSubframes<'a> {
    /// Split the next subframe and its padding off `remaining`; on an error
    /// `remaining` is unchanged.
    fn split_subframe(&mut self) -> Result<AmsduSubframe<'a>, DataDecapError> {
        let (header, body) = self
            .remaining
            .split_first_chunk::<ETHERNET_HEADER_LEN>()
            .ok_or(DataDecapError::Truncated)?;
        let [
            d0,
            d1,
            d2,
            d3,
            d4,
            d5,
            s0,
            s1,
            s2,
            s3,
            s4,
            s5,
            length0,
            length1,
        ] = *header;
        let msdu_length = u16::from_be_bytes([length0, length1]);
        let (msdu, rest) = body
            .split_at_checked(usize::from(msdu_length))
            .ok_or(DataDecapError::Truncated)?;
        let (llc, payload) = msdu
            .split_first_chunk::<LLC_SNAP_HEADER_LEN>()
            .ok_or(DataDecapError::Truncated)?;
        let ether_type = rfc1042_ether_type(*llc).ok_or(DataDecapError::InvalidLlcSnap)?;
        let subframe = AmsduSubframe {
            destination: [d0, d1, d2, d3, d4, d5],
            source: [s0, s1, s2, s3, s4, s5],
            ether_type,
            payload,
        };
        self.remaining = if rest.is_empty() {
            rest
        } else {
            // A non-final subframe is padded to a multiple of four bytes;
            // only the low two bits of its length matter, so the wrapping
            // arithmetic is exact.
            let padding = usize::from(msdu_length)
                .wrapping_add(ETHERNET_HEADER_LEN)
                .wrapping_neg()
                & 3;
            let (_, next) = rest
                .split_at_checked(padding)
                .ok_or(DataDecapError::Truncated)?;
            next
        };
        Ok(subframe)
    }
}

/// Plan a data MPDU from a checked user priority.
///
/// A raw priority cannot enter the encoder:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_wire::data::{DataInterfaceRole, plan_data_encapsulation};
/// let _plan = plan_data_encapsulation(
///     DataInterfaceRole::Station, [2; 6], [4; 6], [0; 14], 8, true, false,
/// );
/// ```
///
/// Fixed protocol priorities need no runtime validation:
///
/// ```
/// use oer_ieee80211_wire::data::{DataInterfaceRole, plan_data_encapsulation};
/// use oer_ieee80211_wire::qos::{WmmAccessCategory, WmmUserPriority};
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
    let (direction, addresses) = match role {
        DataInterfaceRole::Station => (IEEE80211_TO_DS, [bssid, source, destination]),
        DataInterfaceRole::AccessPoint => (IEEE80211_FROM_DS, [destination, interface_mac, source]),
    };
    let frame_control = [
        if qos {
            IEEE80211_QOS_DATA
        } else {
            IEEE80211_DATA
        },
        direction,
    ];
    let qos_control = if qos {
        [
            priority.value() | if no_ack_policy { QOS_NO_ACK_POLICY } else { 0 },
            0,
        ]
    } else {
        [0, 0]
    };
    DataEncapPlan {
        header: data_header_bytes(frame_control, addresses, qos_control),
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

/// A QoS-length data header: frame control, a zero duration, three
/// addresses, a zero sequence control and the QoS control.
const fn data_header_bytes(
    frame_control: [u8; 2],
    [address1, address2, address3]: [[u8; 6]; 3],
    qos_control: [u8; 2],
) -> [u8; IEEE80211_QOS_DATA_HEADER_LEN] {
    let [f0, f1] = frame_control;
    let [a0, a1, a2, a3, a4, a5] = address1;
    let [b0, b1, b2, b3, b4, b5] = address2;
    let [c0, c1, c2, c3, c4, c5] = address3;
    let [q0, q1] = qos_control;
    [
        f0, f1, 0, 0, a0, a1, a2, a3, a4, a5, b0, b1, b2, b3, b4, b5, c0, c1, c2, c3, c4, c5, 0, 0,
        q0, q1,
    ]
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
    single_decapsulation(role, mpdu, payload_offset, payload_length).map(|(plan, _)| plan)
}

/// [`plan_data_decapsulation`] with the Ethernet view it locates.
#[inline(always)]
fn single_decapsulation(
    role: DataInterfaceRole,
    mpdu: &[u8],
    payload_offset: usize,
    payload_length: usize,
) -> Result<(DataDecapPlan, EthernetFrameParts<'_>), DataDecapError> {
    let header = data_header(role, mpdu)?;
    if header
        .qos()
        .is_some_and(|qos| qos & u16::from(IEEE80211_QOS_AMSDU_PRESENT) != 0)
    {
        return Err(DataDecapError::AmsduUnsupported);
    }
    let payload = data_payload(header, payload_offset, payload_length)?;
    let parts = single_payload(header, payload)?;
    #[expect(
        clippy::expect_used,
        reason = "data_payload found the payload, which begins with the LLC/SNAP header, within the MPDU"
    )]
    let ethernet_payload_offset = payload_offset
        .checked_add(LLC_SNAP_HEADER_LEN)
        .expect("the LLC/SNAP header lies within the MPDU");
    let plan = DataDecapPlan {
        destination: parts.destination,
        source: parts.source,
        ether_type: parts.ether_type,
        payload_offset: ethernet_payload_offset,
        payload_length: parts.payload.len(),
        ethernet_length: parts.length(),
    };
    Ok((plan, parts))
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
    #[expect(
        clippy::expect_used,
        reason = "DecodedHeader::decode keeps the body as a suffix of the frame"
    )]
    let header_length = header
        .frame()
        .len()
        .checked_sub(header.body().len())
        .expect("the body is a suffix of the frame");
    if offset < header_length {
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
    let ether_type = rfc1042_ether_type(*llc).ok_or(DataDecapError::InvalidLlcSnap)?;
    let (destination, source) = match header.control().ds() {
        2 => (header.receiver(), header.address3()),
        1 => (header.address3(), header.transmitter()),
        _ => return Err(DataDecapError::RoleMismatch),
    };
    Ok(EthernetFrameParts {
        destination,
        source,
        ether_type,
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
    write_ethernet(
        ethernet,
        required,
        ethernet_header(subframe.destination, subframe.source, subframe.ether_type),
        subframe.payload,
    )
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
    let (plan, parts) = single_decapsulation(role, mpdu, payload_offset, payload_length)?;
    parts.copy_to(ethernet)?;
    Ok(plan)
}

#[cfg(test)]
mod tests;

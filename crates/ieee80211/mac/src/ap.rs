//! Allocation-free AP association and power-save frame transforms.
//!
//! These pure transforms form the source-owned AP protocol boundary.
//! Association ownership, deferred
//! queues and wakeups stay with the radio owner.
//! Management decoding splits the fixed header and each fixed body as arrays
//! before accessing fields; variable information elements remain borrowed slices.

use crate::{
    block_ack::{BlockAckAction, parse_block_ack_action},
    ccmp::CCMP_HEADER_LEN,
    channel::{Band, Channel},
    data::{DataInterfaceRole, ETHERNET_HEADER_LEN, plan_data_encapsulation},
    ht::{
        HT_CAPABILITY_IE_LEN, HT_OPERATION_IE_LEN, HtPeerCapabilities, ht_capability_ie_for_peer,
        ht_operation_ie, ht_peer_capabilities,
    },
    protection::ApBssProtection,
    qos::WmmUserPriority,
    security::{ApSecurityPolicy, LinkProtection},
    sequence::SequenceNumber,
    station::SAE_AUTHENTICATION_ALGORITHM,
    station_power_save::STA_NULL_DATA_FRAME_LEN,
};

pub mod probe;
pub mod profile;
use profile::Advertisement;

const AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN: usize = 51;
pub const AP_ASSOCIATION_RESPONSE_BODY_LEN: usize =
    AP_LEGACY_ASSOCIATION_RESPONSE_BODY_LEN + HT_CAPABILITY_IE_LEN + HT_OPERATION_IE_LEN;
pub const AP_AUTHENTICATION_RESPONSE_LEN: usize = 30;
pub const AP_PEER_DISCONNECT_LEN: usize = MANAGEMENT_HEADER_LEN + 2;
pub const AP_ASSOCIATION_RESPONSE_LEN: usize = 24 + AP_ASSOCIATION_RESPONSE_BODY_LEN;

const MANAGEMENT_HEADER_LEN: usize = 24;
const OPEN_SYSTEM_AUTHENTICATION_ALGORITHM: u16 = 0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApAssociationResponseError {
    MissingAssociationId,
    OutputTooSmall { required: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPeerDisconnectKind {
    Disassociation,
    Deauthentication,
}

/// Invalid AP data-frame construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApDataFrameError {
    InvalidAccessPoint,
    InvalidPeer,
    InvalidUserPriority,
    EthernetFrameTooShort,
    NoAmsduFrames,
    AmsduTooLong { length: usize, maximum: usize },
    OutputTooSmall { required: usize },
}

/// Baseline HT Max A-MSDU Length selected while HT Capabilities bit 11 is
/// clear. Every AP frame codec below is fenced to the advertised class.
pub const AP_AMSDU_BASELINE_MAX_LEN: usize = 3_839;

/// One unprotected 802.11 data MPDU sent by an AP to its client.
///
/// The initial AP uses this for EAPOL before the controlled port opens. The
/// frame shape is portable; descriptor metadata and queue selection remain in
/// the chip TX owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApDataFrame<'payload> {
    pub access_point: [u8; 6],
    pub destination: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub ether_type: u16,
    pub payload: &'payload [u8],
}

impl ApDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if self.access_point[0] & 1 != 0 || self.access_point == [0; 6] {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        let mut ethernet = [0; ETHERNET_HEADER_LEN];
        ethernet[..6].copy_from_slice(&self.destination);
        ethernet[6..12].copy_from_slice(&self.access_point);
        ethernet[12..14].copy_from_slice(&self.ether_type.to_be_bytes());
        let plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            ethernet,
            WmmUserPriority::UP7,
            false,
            false,
        );
        let header_len = usize::from(plan.header_len);
        let required = header_len
            .checked_add(plan.llc_snap.len())
            .and_then(|length| length.checked_add(self.payload.len()))
            .ok_or(ApDataFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(ApDataFrameError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        let llc_end = header_len + plan.llc_snap.len();
        frame[header_len..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(self.payload);
        Ok(required)
    }
}

/// One plaintext Ethernet-II MPDU for an explicitly Open AP epoch.
///
/// Unlike [`ApDataFrame`], which models an AP-originated EAPOL payload, this
/// codec preserves the complete caller-owned Ethernet header and validates
/// the destination against the selected unicast/group peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApUnprotectedDataFrame<'frame> {
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub more_data: bool,
    pub ethernet: &'frame [u8],
}

impl ApUnprotectedDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if self.access_point[0] & 1 != 0 || self.access_point == [0; 6] {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        if self.ethernet.len() < ETHERNET_HEADER_LEN {
            return Err(ApDataFrameError::EthernetFrameTooShort);
        }
        let mut ethernet_header = [0; ETHERNET_HEADER_LEN];
        ethernet_header.copy_from_slice(&self.ethernet[..ETHERNET_HEADER_LEN]);
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            ethernet_header,
            WmmUserPriority::UP0,
            false,
            false,
        );
        if plan.header[4..10] != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        if self.more_data {
            plan.header[1] |= 0x20;
        }
        let header_len = usize::from(plan.header_len);
        let required = header_len
            .checked_add(plan.llc_snap.len())
            .and_then(|length| length.checked_add(self.ethernet.len() - ETHERNET_HEADER_LEN))
            .ok_or(ApDataFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(ApDataFrameError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        let llc_end = header_len + plan.llc_snap.len();
        frame[header_len..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(&self.ethernet[ETHERNET_HEADER_LEN..]);
        Ok(required)
    }
}

/// One protected Ethernet-II frame sent from an AP to its authorized peer.
///
/// The caller owns the pairwise packet number. This codec merely places the
/// supplied CCMP header in the DMA image; the backend encrypts the
/// plaintext LLC/SNAP payload and appends the MIC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApProtectedDataFrame<'frame> {
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub user_priority: u8,
    pub peer_qos: bool,
    /// Set the IEEE 802.11 More Data bit for a frame released from an AP
    /// power-save queue while further traffic remains buffered.
    pub more_data: bool,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub ethernet: &'frame [u8],
}

pub const AP_PROTECTED_QOS_ETHERNET_OVERHEAD: usize =
    26 + CCMP_HEADER_LEN + 8 - ETHERNET_HEADER_LEN;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodedApFrame {
    pub offset: usize,
    pub length: usize,
}

impl ApProtectedDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if self.access_point[0] & 1 != 0 || self.access_point == [0; 6] {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        let priority = WmmUserPriority::new(self.user_priority)
            .ok_or(ApDataFrameError::InvalidUserPriority)?;
        if self.ethernet.len() < ETHERNET_HEADER_LEN {
            return Err(ApDataFrameError::EthernetFrameTooShort);
        }
        // The initial AP owns one pairwise slot. Group-addressed Ethernet
        // needs the distinct GTK packet-number owner and is rejected by the
        // chip engine before this codec is called.
        let mut ethernet_header = [0; ETHERNET_HEADER_LEN];
        ethernet_header.copy_from_slice(&self.ethernet[..ETHERNET_HEADER_LEN]);
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            ethernet_header,
            priority,
            self.peer_qos,
            false,
        );
        if plan.header[4..10] != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        plan.header[1] |= 0x40;
        if self.more_data {
            plan.header[1] |= 0x20;
        }
        let header_len = usize::from(plan.header_len);
        let required = header_len
            .checked_add(CCMP_HEADER_LEN)
            .and_then(|length| length.checked_add(plan.llc_snap.len()))
            .and_then(|length| length.checked_add(self.ethernet.len() - ETHERNET_HEADER_LEN))
            .ok_or(ApDataFrameError::OutputTooSmall {
                required: usize::MAX,
            })?;
        if output.len() < required {
            return Err(ApDataFrameError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        let ccmp_end = header_len + CCMP_HEADER_LEN;
        frame[header_len..ccmp_end].copy_from_slice(&self.ccmp_header);
        let llc_end = ccmp_end + plan.llc_snap.len();
        frame[ccmp_end..llc_end].copy_from_slice(&plan.llc_snap);
        frame[llc_end..required].copy_from_slice(&self.ethernet[ETHERNET_HEADER_LEN..]);
        Ok(required)
    }
}

/// One AP-originated QoS A-MSDU backed by a finite slice of Ethernet frames.
///
/// `ccmp_header = None` selects an Open MPDU. `Some` selects the plaintext
/// CCMP DMA image; hardware encrypts the A-MSDU body and appends its MIC. All
/// subframes must target the exact unicast receiver carried by the outer
/// From-DS header. This stricter production contract prevents a scheduler
/// from coalescing traffic from different AP peer queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAmsduFrame<'a> {
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub user_priority: u8,
    pub more_data: bool,
    pub ccmp_header: Option<[u8; CCMP_HEADER_LEN]>,
    pub ethernet_frames: &'a [&'a [u8]],
}

/// Return the complete DMA-resident MPDU length for a bounded AP A-MSDU.
///
/// The result excludes the hardware-owned CCMP MIC and FCS. `protected`
/// controls only the retained eight-byte CCMP header; the A-MSDU body has the
/// same IEEE 802.11 representation for Open and CCMP epochs.
pub fn ap_amsdu_frame_length(
    ethernet_frames: &[&[u8]],
    protected: bool,
) -> Result<usize, ApDataFrameError> {
    let body = crate::encoding::AmsduBody::checked(ethernet_frames, AP_AMSDU_BASELINE_MAX_LEN)
        .map_err(amsdu_error)?;
    let length = body
        .length()
        .map_err(|length| amsdu_error(crate::encoding::AmsduError::TooLong(length)))?;
    Ok(crate::data::IEEE80211_QOS_DATA_HEADER_LEN
        + if protected { CCMP_HEADER_LEN } else { 0 }
        + length)
}

fn amsdu_error(error: crate::encoding::AmsduError) -> ApDataFrameError {
    match error {
        crate::encoding::AmsduError::Empty => ApDataFrameError::NoAmsduFrames,
        crate::encoding::AmsduError::ShortEthernet => ApDataFrameError::EthernetFrameTooShort,
        crate::encoding::AmsduError::TooLong(length) => ApDataFrameError::AmsduTooLong {
            length,
            maximum: AP_AMSDU_BASELINE_MAX_LEN,
        },
    }
}

impl ApAmsduFrame<'_> {
    /// Encode every Ethernet input into one From-DS QoS A-MSDU.
    ///
    /// Validation and capacity checks complete before the output is mutated.
    /// This lets a packet-number owner first encode with a placeholder CCMP
    /// header, then consume and patch the real PN only after full admission.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if self.access_point[0] & 1 != 0 || self.access_point == [0; 6] {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        if self.peer[0] & 1 != 0 || self.peer == [0; 6] {
            return Err(ApDataFrameError::InvalidPeer);
        }
        let priority = WmmUserPriority::new(self.user_priority)
            .ok_or(ApDataFrameError::InvalidUserPriority)?;
        let Some(first) = self.ethernet_frames.first().copied() else {
            return Err(ApDataFrameError::NoAmsduFrames);
        };
        for ethernet in self.ethernet_frames.iter().copied() {
            if ethernet.len() < ETHERNET_HEADER_LEN {
                return Err(ApDataFrameError::EthernetFrameTooShort);
            }
            if ethernet[..6] != self.peer {
                return Err(ApDataFrameError::InvalidPeer);
            }
        }
        let body =
            crate::encoding::AmsduBody::checked(self.ethernet_frames, AP_AMSDU_BASELINE_MAX_LEN)
                .map_err(amsdu_error)?;

        let Some(&first_header) = first.first_chunk::<ETHERNET_HEADER_LEN>() else {
            return Err(ApDataFrameError::EthernetFrameTooShort);
        };
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            first_header,
            priority,
            true,
            false,
        );
        if plan.header[4..10] != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        if self.ccmp_header.is_some() {
            plan.header[1] |= 0x40;
        }
        if self.more_data {
            plan.header[1] |= 0x20;
        }
        plan.header[24] |= 0x80;

        let mut header = plan.header;
        header[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        crate::encoding::FrameParts([
            header.as_slice(),
            self.ccmp_header
                .as_ref()
                .map_or(&[], |header| header.as_slice()),
        ])
        .write_amsdu(body, output)
        .map_err(|required| ApDataFrameError::OutputTooSmall { required })
    }
}

impl ApProtectedDataFrame<'_> {
    /// Convert an Ethernet-II frame in its owned allocation to an AP-originated
    /// protected QoS MPDU without moving the payload.
    pub fn encode_in_place(
        self,
        storage: &mut [u8],
        ethernet_offset: usize,
        ethernet_length: usize,
    ) -> Result<EncodedApFrame, ApDataFrameError> {
        if !self.peer_qos {
            return Err(ApDataFrameError::InvalidUserPriority);
        }
        let priority = WmmUserPriority::new(self.user_priority)
            .ok_or(ApDataFrameError::InvalidUserPriority)?;
        if ethernet_length < ETHERNET_HEADER_LEN {
            return Err(ApDataFrameError::EthernetFrameTooShort);
        }
        let ethernet_end = ethernet_offset.checked_add(ethernet_length).ok_or(
            ApDataFrameError::OutputTooSmall {
                required: usize::MAX,
            },
        )?;
        if storage.len() < ethernet_end {
            return Err(ApDataFrameError::OutputTooSmall {
                required: ethernet_end,
            });
        }
        let Some(&ethernet_header) =
            storage[ethernet_offset..ethernet_end].first_chunk::<ETHERNET_HEADER_LEN>()
        else {
            return Err(ApDataFrameError::EthernetFrameTooShort);
        };
        let [destination @ .., _, _, _, _, _, _, _, _] = ethernet_header;
        if destination != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            ethernet_header,
            priority,
            true,
            false,
        );
        plan.header[1] |= 0x40;
        if self.more_data {
            plan.header[1] |= 0x20;
        }
        let header_len = usize::from(plan.header_len);
        let prefix_len = header_len + CCMP_HEADER_LEN + plan.llc_snap.len();
        let headroom = prefix_len - ETHERNET_HEADER_LEN;
        let frame_offset = ethernet_offset
            .checked_sub(headroom)
            .ok_or(ApDataFrameError::OutputTooSmall { required: headroom })?;
        let frame_length = ethernet_length + headroom;
        let frame = &mut storage[frame_offset..ethernet_end];
        frame[..header_len].copy_from_slice(&plan.header[..header_len]);
        frame[22..24].copy_from_slice(&self.sequence_number.sequence_control().to_le_bytes());
        let ccmp_end = header_len + CCMP_HEADER_LEN;
        frame[header_len..ccmp_end].copy_from_slice(&self.ccmp_header);
        frame[ccmp_end..prefix_len].copy_from_slice(&plan.llc_snap);
        Ok(EncodedApFrame {
            offset: frame_offset,
            length: frame_length,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApManagementRequest<'a> {
    Probe {
        peer: [u8; 6],
        ssid: &'a [u8],
    },
    OpenAuthentication {
        peer: [u8; 6],
    },
    /// An SAE Authentication frame: a Commit (transaction 1) or a Confirm
    /// (transaction 2) with its status and the body after the status.
    SaeAuthentication {
        peer: [u8; 6],
        transaction: u16,
        status: u16,
        body: &'a [u8],
    },
    Association {
        peer: [u8; 6],
        security: ApAssociationSecurityObservation<'a>,
        /// Highest legacy rate shared with the B/G ERP rate set advertised
        /// by this AP, in 500-kbit/s units. Zero means no common rate.
        maximum_legacy_rate_500kbps: u8,
        /// Complete one-stream HT receive facts supplied by the peer.
        ht_capabilities: Option<HtPeerCapabilities>,
        /// The peer supplied WMM information or HT, both of which imply QoS
        /// data framing for this profile.
        qos_supported: bool,
        /// Capability Information Short Preamble.
        short_preamble: bool,
    },
    Disassociation {
        peer: [u8; 6],
        reason: u16,
    },
    Deauthentication {
        peer: [u8; 6],
        reason: u16,
    },
    BlockAck {
        peer: [u8; 6],
        action: BlockAckAction,
    },
    /// The association check of a peer protecting its management frames.
    SaQuery {
        peer: [u8; 6],
        query: crate::management_protection::SaQuery,
    },
}

/// Exact on-air security facts from one Association Request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAssociationSecurityObservation<'a> {
    pub privacy: bool,
    pub rsn_ie: Option<&'a [u8]>,
    pub rsn_ie_count: u8,
    pub rsnxe: Option<&'a [u8]>,
    pub rsnxe_count: u8,
    /// A legacy WPA vendor IE (00:50:f2:01) was present. It is never an
    /// acceptable substitute for RSN and makes a mixed request invalid.
    pub legacy_wpa_present: bool,
    /// At least one IE header or payload was truncated. Absence inferred from
    /// a malformed tail is never an Open-security proof.
    pub malformed_elements: bool,
}

/// Parse requests addressed to this AP, including broadcast probe discovery.
///
/// Beacons, responses, foreign BSS traffic and unsupported authentication
/// algorithms are intentionally ignored rather than being promoted into AP
/// state transitions.
pub fn parse_ap_management_request<'a>(
    profile: &Advertisement,
    frame: &'a [u8],
    access_point: [u8; 6],
) -> Option<ApManagementRequest<'a>> {
    let (header, body) = frame.split_first_chunk::<MANAGEMENT_HEADER_LEN>()?;
    let [
        fc0,
        fc1,
        _duration0,
        _duration1,
        d0,
        d1,
        d2,
        d3,
        d4,
        d5,
        p0,
        p1,
        p2,
        p3,
        p4,
        p5,
        b0,
        b1,
        b2,
        b3,
        b4,
        b5,
        sc0,
        _sc1,
    ] = *header;
    let destination = [d0, d1, d2, d3, d4, d5];
    let peer = [p0, p1, p2, p3, p4, p5];
    let bssid = [b0, b1, b2, b3, b4, b5];
    let frame_control = u16::from_le_bytes([fc0, fc1]);
    if frame_control & 0x000c != 0 {
        return None;
    }
    let subtype = (frame_control >> 4) & 0x0f;
    if subtype == 4 {
        if frame_control & 0x470f != 0
            || sc0 & 0x0f != 0
            || peer[0] & 1 != 0
            || peer == [0; 6]
            || (destination != access_point && destination != [0xff; 6])
            || (bssid != access_point && bssid != [0xff; 6])
        {
            return None;
        }
        return Some(ApManagementRequest::Probe {
            peer,
            ssid: probe::ssid(body)?,
        });
    }
    if frame_control & 0x030f != 0
        || peer[0] & 1 != 0
        || peer == [0; 6]
        || destination != access_point
        || bssid != access_point
    {
        return None;
    }
    // Parsing is not authentication. The caller must verify the pairwise
    // integrity and replay state before acting on a protected request.
    let protected = frame_control & 0x4000 != 0;
    if protected && !matches!(subtype, 10 | 12 | 13) {
        return None;
    }
    let management_body = if protected {
        body.split_first_chunk::<CCMP_HEADER_LEN>()?.1
    } else {
        body
    };
    match subtype {
        11 => {
            let (&[a0, a1, t0, t1, s0, s1], body) = management_body.split_first_chunk()?;
            let algorithm = u16::from_le_bytes([a0, a1]);
            let transaction = u16::from_le_bytes([t0, t1]);
            let status = u16::from_le_bytes([s0, s1]);
            match algorithm {
                OPEN_SYSTEM_AUTHENTICATION_ALGORITHM => (transaction == 1 && status == 0)
                    .then_some(ApManagementRequest::OpenAuthentication { peer }),
                SAE_AUTHENTICATION_ALGORITHM => Some(ApManagementRequest::SaeAuthentication {
                    peer,
                    transaction,
                    status,
                    body,
                }),
                _ => None,
            }
        }
        0 => {
            let (&[c0, c1, _listen0, _listen1], information_elements) =
                management_body.split_first_chunk()?;
            let capabilities = u16::from_le_bytes([c0, c1]);
            let ht_capabilities = ht_peer_capabilities(information_elements);
            Some(ApManagementRequest::Association {
                peer,
                security: association_security_observation(
                    information_elements,
                    capabilities & 0x0010 != 0,
                ),
                maximum_legacy_rate_500kbps: maximum_ap_legacy_rate(profile, information_elements),
                ht_capabilities,
                qos_supported: ht_capabilities.is_some() || supports_wmm(information_elements),
                short_preamble: capabilities & 0x0020 != 0,
            })
        }
        10 | 12 => {
            let (reason, _) = management_body.split_first_chunk()?;
            let reason = u16::from_le_bytes(*reason);
            if subtype == 10 {
                Some(ApManagementRequest::Disassociation { peer, reason })
            } else {
                Some(ApManagementRequest::Deauthentication { peer, reason })
            }
        }
        13 => {
            if let Some(query) = crate::management_protection::SaQuery::parse(management_body) {
                Some(ApManagementRequest::SaQuery { peer, query })
            } else {
                Some(ApManagementRequest::BlockAck {
                    peer,
                    action: parse_block_ack_action(management_body)?,
                })
            }
        }
        _ => None,
    }
}

/// Management frames an access point originates after association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApManagementSubtype {
    Action,
    Disassociation,
    Deauthentication,
}

impl ApManagementSubtype {
    const fn frame_control(self) -> u16 {
        match self {
            Self::Action => 0x00d0,
            Self::Disassociation => 0x00a0,
            Self::Deauthentication => 0x00c0,
        }
    }

    /// A robust subtype in a management header, without authenticating it.
    pub fn parse(frame: &[u8]) -> Option<Self> {
        let control = u16::from_le_bytes([*frame.first()?, *frame.get(1)?]);
        if control & 0x030f != 0 {
            return None;
        }
        match control & 0x00f0 {
            0x00d0 => Some(Self::Action),
            0x00a0 => Some(Self::Disassociation),
            0x00c0 => Some(Self::Deauthentication),
            _ => None,
        }
    }

    /// Whether management frame protection applies to this body.
    pub fn is_robust(self, body: &[u8]) -> bool {
        match self {
            Self::Action => body
                .first()
                .copied()
                .is_some_and(crate::management_protection::is_robust_action_category),
            Self::Disassociation | Self::Deauthentication => true,
        }
    }
}

/// One unprotected AP-originated management frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApManagementFrame<'a> {
    pub subtype: ApManagementSubtype,
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub body: &'a [u8],
}

impl ApManagementFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApAssociationResponseError> {
        let required = MANAGEMENT_HEADER_LEN.checked_add(self.body.len()).ok_or(
            ApAssociationResponseError::OutputTooSmall {
                required: usize::MAX,
            },
        )?;
        if output.len() < required {
            return Err(ApAssociationResponseError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            self.subtype.frame_control(),
            self.access_point,
            self.peer,
            self.sequence_number,
        );
        frame[MANAGEMENT_HEADER_LEN..].copy_from_slice(self.body);
        Ok(required)
    }
}

/// A pairwise-protected AP management MPDU. The lower MAC encrypts the
/// body and appends the CCMP MIC; it receives an explicit key selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApProtectedManagementFrame<'a> {
    pub subtype: ApManagementSubtype,
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub body: &'a [u8],
}

impl ApProtectedManagementFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApAssociationResponseError> {
        let body_offset = MANAGEMENT_HEADER_LEN + CCMP_HEADER_LEN;
        let required = body_offset.checked_add(self.body.len()).ok_or(
            ApAssociationResponseError::OutputTooSmall {
                required: usize::MAX,
            },
        )?;
        if output.len() < required {
            return Err(ApAssociationResponseError::OutputTooSmall { required });
        }
        let frame = &mut output[..required];
        frame.fill(0);
        write_management_header(
            frame,
            self.subtype.frame_control() | 0x4000,
            self.access_point,
            self.peer,
            self.sequence_number,
        );
        frame[MANAGEMENT_HEADER_LEN..body_offset].copy_from_slice(&self.ccmp_header);
        frame[body_offset..].copy_from_slice(self.body);
        Ok(required)
    }
}

fn supports_wmm(bytes: &[u8]) -> bool {
    let mut remaining = bytes;
    while remaining.len() >= 2 {
        let length = usize::from(remaining[1]);
        let Some(record) = remaining.get(..length.saturating_add(2)) else {
            return false;
        };
        if remaining[0] == 221 && length >= 6 && record[2..6] == [0x00, 0x50, 0xf2, 0x02] {
            return true;
        }
        remaining = &remaining[record.len()..];
    }
    false
}

fn association_security_observation(
    bytes: &[u8],
    privacy: bool,
) -> ApAssociationSecurityObservation<'_> {
    let mut remaining = bytes;
    let mut observation = ApAssociationSecurityObservation {
        privacy,
        rsn_ie: None,
        rsn_ie_count: 0,
        rsnxe: None,
        rsnxe_count: 0,
        legacy_wpa_present: false,
        malformed_elements: false,
    };
    while !remaining.is_empty() {
        if remaining.len() < 2 {
            observation.malformed_elements = true;
            break;
        }
        let length = usize::from(remaining[1]);
        let Some(record) = remaining.get(..length.saturating_add(2)) else {
            observation.malformed_elements = true;
            break;
        };
        if remaining[0] == 48 {
            observation.rsn_ie_count = observation.rsn_ie_count.saturating_add(1);
            observation.rsn_ie.get_or_insert(record);
        }
        if remaining[0] == 244 {
            observation.rsnxe_count = observation.rsnxe_count.saturating_add(1);
            observation.rsnxe.get_or_insert(record);
        }
        if remaining[0] == 221 && length >= 4 && record[2..6] == [0x00, 0x50, 0xf2, 0x01] {
            observation.legacy_wpa_present = true;
        }
        remaining = &remaining[record.len()..];
    }
    observation
}

fn maximum_ap_legacy_rate(profile: &Advertisement, bytes: &[u8]) -> u8 {
    let mut maximum = 0;
    let mut remaining = bytes;
    while let Some((&id, tail)) = remaining.split_first() {
        let Some((&length, payload)) = tail.split_first() else {
            break;
        };
        let length = usize::from(length);
        let Some(value) = payload.get(..length) else {
            break;
        };
        if id == 1 || id == 50 {
            for encoded in value {
                let rate = encoded & 0x7f;
                if profile.legacy_rates.supports(rate) {
                    maximum = maximum.max(rate);
                }
            }
        }
        remaining = &payload[length..];
    }
    maximum
}

/// Encode the response to one successful or rejected Open System request.
pub fn write_open_authentication_response(
    output: &mut [u8],
    access_point: [u8; 6],
    peer: [u8; 6],
    status: u16,
    management_sequence: SequenceNumber,
) -> Result<usize, ApAssociationResponseError> {
    if output.len() < AP_AUTHENTICATION_RESPONSE_LEN {
        return Err(ApAssociationResponseError::OutputTooSmall {
            required: AP_AUTHENTICATION_RESPONSE_LEN,
        });
    }
    let frame = &mut output[..AP_AUTHENTICATION_RESPONSE_LEN];
    frame.fill(0);
    write_management_header(frame, 0x00b0, access_point, peer, management_sequence);
    frame[26..28].copy_from_slice(&2_u16.to_le_bytes());
    frame[28..30].copy_from_slice(&status.to_le_bytes());
    Ok(AP_AUTHENTICATION_RESPONSE_LEN)
}

/// Encode one SAE Authentication frame to `peer`: a Commit or Confirm, or
/// the refusal of one, whose `body` follows the status.
pub fn write_sae_authentication(
    output: &mut [u8],
    access_point: [u8; 6],
    peer: [u8; 6],
    transaction: u16,
    status: u16,
    body: &[u8],
    management_sequence: SequenceNumber,
) -> Result<usize, ApAssociationResponseError> {
    let required = AP_AUTHENTICATION_RESPONSE_LEN + body.len();
    if output.len() < required {
        return Err(ApAssociationResponseError::OutputTooSmall { required });
    }
    let frame = &mut output[..required];
    frame.fill(0);
    write_management_header(frame, 0x00b0, access_point, peer, management_sequence);
    frame[24..26].copy_from_slice(&SAE_AUTHENTICATION_ALGORITHM.to_le_bytes());
    frame[26..28].copy_from_slice(&transaction.to_le_bytes());
    frame[28..30].copy_from_slice(&status.to_le_bytes());
    frame[AP_AUTHENTICATION_RESPONSE_LEN..].copy_from_slice(body);
    Ok(required)
}

/// Encode an association response with capability privacy matching the exact
/// AP mode and the BSS's current protection.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame writer keeps each independently reviewed 802.11 field explicit at its boundary"
)]
pub fn write_ht_association_response_frame_for_security(
    profile: &Advertisement,
    output: &mut [u8],
    access_point: [u8; 6],
    peer: [u8; 6],
    status: u16,
    association_id: u16,
    management_sequence: SequenceNumber,
    channel: Channel,
    peer_ht: Option<HtPeerCapabilities>,
    security: ApSecurityPolicy,
    protection: ApBssProtection,
) -> Result<usize, ApAssociationResponseError> {
    let (frame, _) = output
        .split_first_chunk_mut::<AP_ASSOCIATION_RESPONSE_LEN>()
        .ok_or(ApAssociationResponseError::OutputTooSmall {
            required: AP_ASSOCIATION_RESPONSE_LEN,
        })?;
    frame.fill(0);
    write_management_header(frame, 0x0010, access_point, peer, management_sequence);
    let Some(body) =
        frame[MANAGEMENT_HEADER_LEN..].first_chunk_mut::<AP_ASSOCIATION_RESPONSE_BODY_LEN>()
    else {
        return Err(ApAssociationResponseError::OutputTooSmall {
            required: AP_ASSOCIATION_RESPONSE_LEN,
        });
    };
    let body_len = write_ht_association_response(
        profile,
        body,
        status,
        association_id,
        channel,
        peer_ht,
        protection,
    )?;
    if security == ApSecurityPolicy::Open {
        body[..2].copy_from_slice(
            &profile
                .capabilities(security.link_protection())
                .to_le_bytes(),
        );
    }
    Ok(MANAGEMENT_HEADER_LEN + body_len)
}

/// Encode one AP-originated peer teardown frame.
pub fn write_ap_peer_disconnect(
    output: &mut [u8],
    access_point: [u8; 6],
    peer: [u8; 6],
    kind: ApPeerDisconnectKind,
    reason: u16,
    management_sequence: SequenceNumber,
) -> Result<usize, ApAssociationResponseError> {
    if output.len() < AP_PEER_DISCONNECT_LEN {
        return Err(ApAssociationResponseError::OutputTooSmall {
            required: AP_PEER_DISCONNECT_LEN,
        });
    }
    let frame = &mut output[..AP_PEER_DISCONNECT_LEN];
    frame.fill(0);
    let frame_control = match kind {
        ApPeerDisconnectKind::Disassociation => 0x00a0,
        ApPeerDisconnectKind::Deauthentication => 0x00c0,
    };
    write_management_header(
        frame,
        frame_control,
        access_point,
        peer,
        management_sequence,
    );
    frame[MANAGEMENT_HEADER_LEN..AP_PEER_DISCONNECT_LEN].copy_from_slice(&reason.to_le_bytes());
    Ok(AP_PEER_DISCONNECT_LEN)
}

fn write_management_header(
    frame: &mut [u8],
    frame_control: u16,
    access_point: [u8; 6],
    peer: [u8; 6],
    management_sequence: SequenceNumber,
) {
    frame[..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&peer);
    frame[10..16].copy_from_slice(&access_point);
    frame[16..22].copy_from_slice(&access_point);
    frame[22..24].copy_from_slice(&management_sequence.sequence_control().to_le_bytes());
}

/// Build the finite AP HT association response body: its length, which
/// is [`AP_ASSOCIATION_RESPONSE_BODY_LEN`] in the 2.4 GHz band and shorter
/// in the 5 GHz band, where the BSS has no ERP element.
pub fn write_ht_association_response(
    profile: &Advertisement,
    body: &mut [u8; AP_ASSOCIATION_RESPONSE_BODY_LEN],
    status: u16,
    association_id: u16,
    channel: Channel,
    peer_ht: Option<HtPeerCapabilities>,
    protection: ApBssProtection,
) -> Result<usize, ApAssociationResponseError> {
    if status == 0 && association_id & 0x3fff == 0 {
        return Err(ApAssociationResponseError::MissingAssociationId);
    }
    body.fill(0);
    body[..2].copy_from_slice(&profile.capabilities(LinkProtection::Ccmp).to_le_bytes());
    body[2..4].copy_from_slice(&status.to_le_bytes());
    let encoded_association_id = if status == 0 {
        0xc000 | (association_id & 0x3fff)
    } else {
        0
    };
    body[4..6].copy_from_slice(&encoded_association_id.to_le_bytes());
    let mut offset = 6;
    let mut put = |bytes: &[u8]| {
        body[offset..offset + bytes.len()].copy_from_slice(bytes);
        offset += bytes.len();
    };
    put(&[1, 8]);
    put(profile.legacy_rates.supported());
    put(&[50, 4]);
    put(profile.legacy_rates.extended());
    if channel.band() == Band::Ghz2_4 {
        put(&[42, 1, protection.erp_information()]);
    }
    put(&profile.wmm.element());
    put(&ht_capability_ie_for_peer(profile.ht, channel, peer_ht));
    put(&ht_operation_ie(channel, protection.ht));
    Ok(offset)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPowerSaveObservation {
    Sleeping { peer: [u8; 6] },
    Active { peer: [u8; 6] },
    PsPoll { peer: [u8; 6], association_id: u16 },
}

/// Parse the RX-derived power-save edge used by the migrated AP owner.
///
/// Association validation is intentionally separate: the AP peer table must
/// confirm that `peer` currently owns the reported association ID.
pub fn observe_ap_power_save(frame: &[u8]) -> Option<ApPowerSaveObservation> {
    if frame.len() < 2 {
        return None;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    let frame_type = (frame_control >> 2) & 3;
    let subtype = (frame_control >> 4) & 0x0f;

    if frame_type == 1 && subtype == 10 && frame.len() >= 16 {
        let raw_association_id = u16::from_le_bytes([frame[2], frame[3]]);
        let association_id = raw_association_id & 0x3fff;
        if raw_association_id & 0xc000 != 0xc000 || association_id == 0 {
            return None;
        }
        return Some(ApPowerSaveObservation::PsPoll {
            peer: frame[10..16].try_into().ok()?,
            association_id,
        });
    }

    if frame.len() < 24 {
        return None;
    }
    let to_ds = frame_control & 0x0100 != 0;
    let from_ds = frame_control & 0x0200 != 0;
    if frame_type != 2 || !to_ds || from_ds {
        return None;
    }
    let peer = frame[10..16].try_into().ok()?;
    if frame_control & 0x1000 != 0 {
        Some(ApPowerSaveObservation::Sleeping { peer })
    } else {
        Some(ApPowerSaveObservation::Active { peer })
    }
}

/// Parse an AP power-save edge while binding PS-Poll Receiver Address to the
/// active BSSID. Protected data admission validates its address mapping in the
/// data owner; the unprotected control frame needs this explicit check before
/// its TA/AID tuple can reserve buffered traffic.
pub fn observe_ap_power_save_for_access_point(
    frame: &[u8],
    access_point: [u8; 6],
) -> Option<ApPowerSaveObservation> {
    let observation = observe_ap_power_save(frame)?;
    if matches!(observation, ApPowerSaveObservation::PsPoll { .. })
        && frame.get(4..10) != Some(access_point.as_slice())
    {
        return None;
    }
    Some(observation)
}

/// Parse the exact legacy Null Data frame used by an associated station to
/// publish a power-management transition.
///
/// Unlike ordinary payload data, Null Data has no Ethernet body whose
/// successful decapsulation can establish the peer identity. Admission is
/// therefore deliberately complete at this boundary: the MPDU must be the
/// exact 24-byte To-DS Null Data geometry emitted by `StaNullDataFrame`, both
/// BSSID address fields must name the active AP, the transmitter must be a
/// valid unicast address, and fragmentation is rejected. Only the Retry and
/// Power Management flag bits may vary.
pub fn observe_ap_null_data_power_save_for_access_point(
    frame: &[u8],
    access_point: [u8; 6],
) -> Option<ApPowerSaveObservation> {
    const NULL_DATA_TO_DS_FRAME_CONTROL: u16 = 0x0148;
    const RETRY: u16 = 0x0800;
    const POWER_MANAGEMENT: u16 = 0x1000;

    if frame.len() != STA_NULL_DATA_FRAME_LEN {
        return None;
    }
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    if frame_control & !(RETRY | POWER_MANAGEMENT) != NULL_DATA_TO_DS_FRAME_CONTROL
        || frame[4..10] != access_point
        || frame[16..22] != access_point
        || frame[22] & 0x0f != 0
    {
        return None;
    }
    let peer: [u8; 6] = frame[10..16].try_into().ok()?;
    if peer == [0; 6] || peer == [0xff; 6] || peer[0] & 1 != 0 {
        return None;
    }
    if frame_control & POWER_MANAGEMENT != 0 {
        Some(ApPowerSaveObservation::Sleeping { peer })
    } else {
        Some(ApPowerSaveObservation::Active { peer })
    }
}

#[cfg(test)]
mod tests;

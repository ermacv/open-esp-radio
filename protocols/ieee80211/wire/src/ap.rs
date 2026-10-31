//! Allocation-free AP association and power-save frame transforms.
//!
//! These pure transforms form the source-owned AP protocol boundary.
//! Association ownership, deferred
//! queues and wakeups stay with the radio owner.
//! Management decoding splits the fixed header and each fixed body as arrays
//! before accessing fields; variable information elements remain borrowed slices.

use crate::data::{MsduHeader, MsduParts};
use crate::frame::{FrameControlFields, FrameType};

use crate::{
    block_ack::{BlockAckAction, parse_block_ack_action},
    ccmp::CCMP_HEADER_LEN,
    channel::{Band, Channel},
    data::{DataInterfaceRole, plan_data_encapsulation},
    ht::{
        HT_CAPABILITY_IE_LEN, HT_OPERATION_IE_LEN, HtPeerCapabilities, ht_capability_ie_for_peer,
        ht_operation_ie, ht_peer_capabilities,
    },
    protection::ApBssProtection,
    qos::WmmUserPriority,
    security::{BssSecurityElements, LinkProtection},
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
    LengthOverflow,
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
        if crate::address::AddressFlags::from_bytes([self.access_point[0]]).group()
            || self.access_point == [0; 6]
        {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        let plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            MsduHeader {
                destination: self.destination,
                source: self.access_point,
                ether_type: self.ether_type,
            },
            WmmUserPriority::UP7,
            false,
            false,
        );
        let mut header = plan.header;
        crate::encoding::write_array::<22, _, _>(
            &mut header,
            &self.sequence_number.sequence_control().to_le_bytes(),
        );
        let mut reader = oer_wire::Reader::new(&header);
        let header = reader.take(usize::from(plan.header_len)).map_err(|_| {
            ApDataFrameError::OutputTooSmall {
                required: usize::from(plan.header_len),
            }
        })?;
        crate::encoding::FrameParts([header, &plan.llc_snap, self.payload])
            .write(output)
            .map_err(|required| ApDataFrameError::OutputTooSmall { required })
    }
}

/// One plaintext MSDU for an explicitly Open AP epoch.
///
/// Unlike [`ApDataFrame`], which models an AP-originated EAPOL payload, this
/// codec carries the MSDU's own source and validates its destination against
/// the selected unicast/group peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApUnprotectedDataFrame<'frame> {
    pub access_point: [u8; 6],
    pub peer: [u8; 6],
    pub sequence_number: SequenceNumber,
    pub more_data: bool,
    pub msdu: MsduParts<'frame>,
}

impl ApUnprotectedDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if crate::address::AddressFlags::from_bytes([self.access_point[0]]).group()
            || self.access_point == [0; 6]
        {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        let payload = self.msdu.payload;
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            self.msdu.header(),
            WmmUserPriority::UP0,
            false,
            false,
        );
        if plan.header[4..10] != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        if self.more_data {
            plan.set_more_data(true);
        }
        let mut header = plan.header;
        crate::encoding::write_array::<22, _, _>(
            &mut header,
            &self.sequence_number.sequence_control().to_le_bytes(),
        );
        let mut reader = oer_wire::Reader::new(&header);
        let header = reader.take(usize::from(plan.header_len)).map_err(|_| {
            ApDataFrameError::OutputTooSmall {
                required: usize::from(plan.header_len),
            }
        })?;
        crate::encoding::FrameParts([header, &plan.llc_snap, payload])
            .write(output)
            .map_err(|required| ApDataFrameError::OutputTooSmall { required })
    }
}

/// One protected MSDU sent from an AP to its authorized peer.
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
    pub msdu: MsduParts<'frame>,
}

impl ApProtectedDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if crate::address::AddressFlags::from_bytes([self.access_point[0]]).group()
            || self.access_point == [0; 6]
        {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        let priority = WmmUserPriority::new(self.user_priority)
            .ok_or(ApDataFrameError::InvalidUserPriority)?;
        // The initial AP owns one pairwise slot. A group-addressed MSDU
        // needs the distinct GTK packet-number owner and is rejected by the
        // chip engine before this codec is called.
        let payload = self.msdu.payload;
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            self.msdu.header(),
            priority,
            self.peer_qos,
            false,
        );
        if plan.header[4..10] != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        plan.set_protected(true);
        if self.more_data {
            plan.set_more_data(true);
        }
        let mut header = plan.header;
        crate::encoding::write_array::<22, _, _>(
            &mut header,
            &self.sequence_number.sequence_control().to_le_bytes(),
        );
        let mut reader = oer_wire::Reader::new(&header);
        let header = reader.take(usize::from(plan.header_len)).map_err(|_| {
            ApDataFrameError::OutputTooSmall {
                required: usize::from(plan.header_len),
            }
        })?;
        crate::encoding::FrameParts([header, &self.ccmp_header, &plan.llc_snap, payload])
            .write(output)
            .map_err(|required| ApDataFrameError::OutputTooSmall { required })
    }
}

/// One AP-originated QoS A-MSDU backed by a finite slice of MSDUs.
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
    pub msdus: &'a [MsduParts<'a>],
}

/// Return the complete DMA-resident MPDU length for a bounded AP A-MSDU.
///
/// The result excludes the hardware-owned CCMP MIC and FCS. `protected`
/// controls only the retained eight-byte CCMP header; the A-MSDU body has the
/// same IEEE 802.11 representation for Open and CCMP epochs.
pub fn ap_amsdu_frame_length(
    msdus: &[MsduParts<'_>],
    protected: bool,
) -> Result<usize, ApDataFrameError> {
    let body = crate::encoding::AmsduBody::checked(msdus, AP_AMSDU_BASELINE_MAX_LEN)
        .map_err(amsdu_error)?;
    let length = body
        .length()
        .map_err(|length| amsdu_error(crate::encoding::AmsduError::TooLong(length)))?;
    crate::data::IEEE80211_QOS_DATA_HEADER_LEN
        .checked_add(if protected { CCMP_HEADER_LEN } else { 0 })
        .and_then(|prefix| prefix.checked_add(length))
        .ok_or(ApDataFrameError::AmsduTooLong {
            length: usize::MAX,
            maximum: AP_AMSDU_BASELINE_MAX_LEN,
        })
}

fn amsdu_error(error: crate::encoding::AmsduError) -> ApDataFrameError {
    match error {
        crate::encoding::AmsduError::Empty => ApDataFrameError::NoAmsduFrames,
        crate::encoding::AmsduError::TooLong(length) => ApDataFrameError::AmsduTooLong {
            length,
            maximum: AP_AMSDU_BASELINE_MAX_LEN,
        },
    }
}

impl ApAmsduFrame<'_> {
    /// Encode every MSDU into one From-DS QoS A-MSDU.
    ///
    /// Validation and capacity checks complete before the output is mutated.
    /// This lets a packet-number owner first encode with a placeholder CCMP
    /// header, then consume and patch the real PN only after full admission.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if crate::address::AddressFlags::from_bytes([self.access_point[0]]).group()
            || self.access_point == [0; 6]
        {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        if crate::address::AddressFlags::from_bytes([self.peer[0]]).group() || self.peer == [0; 6] {
            return Err(ApDataFrameError::InvalidPeer);
        }
        let priority = WmmUserPriority::new(self.user_priority)
            .ok_or(ApDataFrameError::InvalidUserPriority)?;
        let Some(first) = self.msdus.first().copied() else {
            return Err(ApDataFrameError::NoAmsduFrames);
        };
        if self.msdus.iter().any(|msdu| msdu.destination != self.peer) {
            return Err(ApDataFrameError::InvalidPeer);
        }
        let body = crate::encoding::AmsduBody::checked(self.msdus, AP_AMSDU_BASELINE_MAX_LEN)
            .map_err(amsdu_error)?;

        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            self.access_point,
            self.access_point,
            first.header(),
            priority,
            true,
            false,
        );
        if plan.header[4..10] != self.peer {
            return Err(ApDataFrameError::InvalidPeer);
        }
        if self.ccmp_header.is_some() {
            plan.set_protected(true);
        }
        if self.more_data {
            plan.set_more_data(true);
        }
        plan.set_amsdu(true);

        let mut header = plan.header;
        crate::encoding::write_array::<22, _, _>(
            &mut header,
            &self.sequence_number.sequence_control().to_le_bytes(),
        );
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
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn parse_ap_management_request<'a>(
    profile: &Advertisement,
    frame: &'a [u8],
    access_point: [u8; 6],
) -> Option<ApManagementRequest<'a>> {
    let mut reader = oer_wire::Reader::new(frame);
    let header = reader.array::<MANAGEMENT_HEADER_LEN>().ok()?;
    let body = reader.remaining();
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
    if FrameControlFields::from_raw(frame_control).kind() != FrameType::Management {
        return None;
    }
    let subtype = FrameControlFields::from_raw(frame_control).subtype();
    if subtype == 4 {
        if (FrameControlFields::from_raw(frame_control).version() != 0
            || FrameControlFields::from_raw(frame_control).kind() != FrameType::Management
            || FrameControlFields::from_raw(frame_control).ds() != 0
            || FrameControlFields::from_raw(frame_control).more_fragments()
            || FrameControlFields::from_raw(frame_control).protected())
            || crate::sequence::SequenceControl::from_bytes((sc0 as u16).to_le_bytes()).fragment()
                != 0
            || crate::address::AddressFlags::from_bytes([peer[0]]).group()
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
    if (FrameControlFields::from_raw(frame_control).version() != 0
        || FrameControlFields::from_raw(frame_control).kind() != FrameType::Management
        || FrameControlFields::from_raw(frame_control).ds() != 0)
        || crate::address::AddressFlags::from_bytes([peer[0]]).group()
        || peer == [0; 6]
        || destination != access_point
        || bssid != access_point
    {
        return None;
    }
    // Parsing is not authentication. The caller must verify the pairwise
    // integrity and replay state before acting on a protected request.
    let protected = FrameControlFields::from_raw(frame_control).protected();
    if protected && !matches!(subtype, 10 | 12 | 13) {
        return None;
    }
    let management_body = if protected {
        {
            let mut reader = oer_wire::Reader::new(body);
            reader.take(CCMP_HEADER_LEN).ok()?;
            reader.remaining()
        }
    } else {
        body
    };
    match subtype {
        11 => {
            let mut reader = oer_wire::Reader::new(management_body);
            let &[a0, a1, t0, t1, s0, s1] = reader.array().ok()?;
            let body = reader.remaining();
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
            let mut reader = oer_wire::Reader::new(management_body);
            let &[c0, c1, _listen0, _listen1] = reader.array().ok()?;
            let information_elements = reader.remaining();
            let capabilities = u16::from_le_bytes([c0, c1]);
            let ht_capabilities = ht_peer_capabilities(information_elements);
            Some(ApManagementRequest::Association {
                peer,
                security: association_security_observation(
                    information_elements,
                    crate::management::CapabilityInformation::from_bytes(
                        capabilities.to_le_bytes(),
                    )
                    .privacy(),
                ),
                maximum_legacy_rate_500kbps: maximum_ap_legacy_rate(profile, information_elements),
                ht_capabilities,
                qos_supported: ht_capabilities.is_some() || supports_wmm(information_elements),
                short_preamble: crate::management::CapabilityInformation::from_bytes(
                    capabilities.to_le_bytes(),
                )
                .short_preamble(),
            })
        }
        10 | 12 => {
            let mut reader = oer_wire::Reader::new(management_body);
            let reason = reader.array().ok()?;
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

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
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
        let mut reader = oer_wire::Reader::new(frame);
        let control = reader.array().ok()?;
        let control = u16::from_le_bytes(*control);
        if FrameControlFields::from_raw(control).version() != 0
            || FrameControlFields::from_raw(control).kind() != FrameType::Management
            || FrameControlFields::from_raw(control).ds() != 0
        {
            return None;
        }
        match FrameControlFields::from_raw(control).subtype() {
            13 => Some(Self::Action),
            10 => Some(Self::Disassociation),
            12 => Some(Self::Deauthentication),
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
        let header = management_header(
            self.subtype.frame_control(),
            self.access_point,
            self.peer,
            self.sequence_number,
        );
        crate::encoding::FrameParts([header.as_slice(), self.body])
            .write(output)
            .map_err(|required| ApAssociationResponseError::OutputTooSmall { required })
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
        let header = management_header(
            FrameControlFields::from_raw(self.subtype.frame_control())
                .with_protected(true)
                .raw(),
            self.access_point,
            self.peer,
            self.sequence_number,
        );
        crate::encoding::FrameParts([header.as_slice(), &self.ccmp_header, self.body])
            .write(output)
            .map_err(|required| ApAssociationResponseError::OutputTooSmall { required })
    }
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
fn supports_wmm(bytes: &[u8]) -> bool {
    let mut remaining = oer_wire::Reader::new(bytes);
    while let [_, length, ..] = *remaining.remaining() {
        let length = usize::from(length);
        let Some(record) = length
            .checked_add(2)
            .and_then(|record_len| remaining.take(record_len).ok())
        else {
            return false;
        };
        if length >= 6 && matches!(record, [221, _, 0x00, 0x50, 0xf2, 0x02, ..]) {
            return true;
        }
    }
    false
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
fn association_security_observation(
    bytes: &[u8],
    privacy: bool,
) -> ApAssociationSecurityObservation<'_> {
    let mut remaining = oer_wire::Reader::new(bytes);
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
        let [id, length, ..] = *remaining.remaining() else {
            observation.malformed_elements = true;
            break;
        };
        let length = usize::from(length);
        let Some(record) = length
            .checked_add(2)
            .and_then(|record_len| remaining.take(record_len).ok())
        else {
            observation.malformed_elements = true;
            break;
        };
        if id == 48 {
            observation.rsn_ie_count = observation.rsn_ie_count.saturating_add(1);
            observation.rsn_ie.get_or_insert(record);
        }
        if id == 244 {
            observation.rsnxe_count = observation.rsnxe_count.saturating_add(1);
            observation.rsnxe.get_or_insert(record);
        }
        if id == 221 && length >= 4 && matches!(record, [_, _, 0x00, 0x50, 0xf2, 0x01, ..]) {
            observation.legacy_wpa_present = true;
        }
    }
    observation
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
fn maximum_ap_legacy_rate(profile: &Advertisement, bytes: &[u8]) -> u8 {
    let mut maximum = 0;
    let mut remaining = oer_wire::Reader::new(bytes);
    while let Ok(&[id, length]) = remaining.array::<2>() {
        let Ok(value) = remaining.take(usize::from(length)) else {
            break;
        };
        if id == 1 || id == 50 {
            for encoded in value {
                let rate = crate::phy::SupportedRate::from_bytes([*encoded]).rate();
                if profile.legacy_rates.supports(rate) {
                    maximum = maximum.max(rate);
                }
            }
        }
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
    let header = management_header(0x00b0, access_point, peer, management_sequence);
    crate::encoding::FrameParts([
        header.as_slice(),
        &0_u16.to_le_bytes(),
        &2_u16.to_le_bytes(),
        &status.to_le_bytes(),
    ])
    .write(output)
    .map_err(|required| ApAssociationResponseError::OutputTooSmall { required })
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
    let header = management_header(0x00b0, access_point, peer, management_sequence);
    crate::encoding::FrameParts([
        header.as_slice(),
        &SAE_AUTHENTICATION_ALGORITHM.to_le_bytes(),
        &transaction.to_le_bytes(),
        &status.to_le_bytes(),
        body,
    ])
    .write(output)
    .map_err(|required| ApAssociationResponseError::OutputTooSmall { required })
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
    security: BssSecurityElements,
    protection: ApBssProtection,
) -> Result<usize, ApAssociationResponseError> {
    let (frame, _) = output
        .split_first_chunk_mut::<AP_ASSOCIATION_RESPONSE_LEN>()
        .ok_or(ApAssociationResponseError::OutputTooSmall {
            required: AP_ASSOCIATION_RESPONSE_LEN,
        })?;
    frame.fill(0);
    crate::encoding::write_array::<0, _, _>(
        frame,
        &management_header(0x0010, access_point, peer, management_sequence),
    );
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
    if security.link_protection() == LinkProtection::Open {
        crate::encoding::write_array::<0, _, _>(
            body,
            &profile
                .capabilities(security.link_protection())
                .to_le_bytes(),
        );
    }
    MANAGEMENT_HEADER_LEN
        .checked_add(body_len)
        .ok_or(ApAssociationResponseError::LengthOverflow)
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
    let frame_control = match kind {
        ApPeerDisconnectKind::Disassociation => 0x00a0,
        ApPeerDisconnectKind::Deauthentication => 0x00c0,
    };
    let header = management_header(frame_control, access_point, peer, management_sequence);
    crate::encoding::FrameParts([header.as_slice(), &reason.to_le_bytes()])
        .write(output)
        .map_err(|required| ApAssociationResponseError::OutputTooSmall { required })
}

fn management_header(
    frame_control: u16,
    access_point: [u8; 6],
    peer: [u8; 6],
    management_sequence: SequenceNumber,
) -> [u8; MANAGEMENT_HEADER_LEN] {
    let mut frame = [0; MANAGEMENT_HEADER_LEN];
    crate::encoding::write_array::<0, _, _>(&mut frame, &frame_control.to_le_bytes());
    crate::encoding::write_array::<4, _, _>(&mut frame, &peer);
    crate::encoding::write_array::<10, _, _>(&mut frame, &access_point);
    crate::encoding::write_array::<16, _, _>(&mut frame, &access_point);
    crate::encoding::write_array::<22, _, _>(
        &mut frame,
        &management_sequence.sequence_control().to_le_bytes(),
    );
    frame
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
    if status == 0
        && crate::management::AssociationId::from_bytes(association_id.to_le_bytes()).aid() == 0
    {
        return Err(ApAssociationResponseError::MissingAssociationId);
    }

    let encoded_association_id = if status == 0 {
        u16::from_le_bytes(
            crate::management::AssociationId::from_bytes(association_id.to_le_bytes())
                .with_marker(oer_wire::U8::constant::<3>())
                .to_bytes(),
        )
    } else {
        0
    };
    let ht_capability = ht_capability_ie_for_peer(profile.ht, channel, peer_ht);
    let ht_operation = ht_operation_ie(channel, protection.ht);
    let erp = [42, 1, protection.erp_information()];
    crate::encoding::FrameParts([
        &profile.capabilities(LinkProtection::Ccmp).to_le_bytes(),
        &status.to_le_bytes(),
        &encoded_association_id.to_le_bytes(),
        &[1, 8],
        profile.legacy_rates.supported(),
        &[50, 4],
        profile.legacy_rates.extended(),
        if channel.band() == Band::Ghz2_4 {
            &erp
        } else {
            &[]
        },
        &profile.wmm.element(),
        &ht_capability,
        &ht_operation,
    ])
    .write(body)
    .map_err(|required| ApAssociationResponseError::OutputTooSmall { required })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPowerSaveObservation {
    Sleeping { peer: [u8; 6] },
    Active { peer: [u8; 6] },
    PsPoll { peer: [u8; 6], association_id: u16 },
}

/// Parse the RX-derived power-save edge the AP owner uses.
///
/// Association validation is intentionally separate: the AP peer table must
/// confirm that `peer` currently owns the reported association ID.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn observe_ap_power_save(frame: &[u8]) -> Option<ApPowerSaveObservation> {
    observe_power_save(frame).map(|(observation, _)| observation)
}

/// A power-save edge and the Receiver Address of the frame that carried it.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
fn observe_power_save(frame: &[u8]) -> Option<(ApPowerSaveObservation, [u8; 6])> {
    let mut reader = oer_wire::Reader::new(frame);
    let &[fc0, fc1] = reader.array().ok()?;
    let frame_control = u16::from_le_bytes([fc0, fc1]);
    let frame_type = FrameControlFields::from_raw(frame_control)
        .kind()
        .encoding() as u16;
    let subtype = FrameControlFields::from_raw(frame_control).subtype();

    // PS-Poll: Frame Control, AID, Receiver and Transmitter Address.
    if frame_type == 1
        && subtype == 10
        && let Ok(ps_poll) = oer_wire::Reader::new(frame).array::<16>()
    {
        let [_, _, a0, a1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5] = *ps_poll;
        let raw_association_id = u16::from_le_bytes([a0, a1]);
        let association_id =
            crate::management::AssociationId::from_bytes(raw_association_id.to_le_bytes()).aid();
        if crate::management::AssociationId::from_bytes(raw_association_id.to_le_bytes())
            .marker()
            .get()
            != 3
            || association_id == 0
        {
            return None;
        }
        return Some((
            ApPowerSaveObservation::PsPoll {
                peer: [t0, t1, t2, t3, t4, t5],
                association_id,
            },
            [r0, r1, r2, r3, r4, r5],
        ));
    }

    let mut reader = oer_wire::Reader::new(frame);
    let header = reader.array::<24>().ok()?;
    let [
        _,
        _,
        _,
        _,
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
        ..,
    ] = *header;
    let to_ds = FrameControlFields::from_raw(frame_control).to_ds();
    let from_ds = FrameControlFields::from_raw(frame_control).from_ds();
    if frame_type != 2 || !to_ds || from_ds {
        return None;
    }
    let peer = [t0, t1, t2, t3, t4, t5];
    let receiver = [r0, r1, r2, r3, r4, r5];
    if FrameControlFields::from_raw(frame_control).power_management() {
        Some((ApPowerSaveObservation::Sleeping { peer }, receiver))
    } else {
        Some((ApPowerSaveObservation::Active { peer }, receiver))
    }
}

/// Parse an AP power-save edge while binding PS-Poll Receiver Address to the
/// active BSSID. Protected data admission validates its address mapping in the
/// data owner; the unprotected control frame needs this explicit check before
/// its TA/AID tuple can reserve buffered traffic.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn observe_ap_power_save_for_access_point(
    frame: &[u8],
    access_point: [u8; 6],
) -> Option<ApPowerSaveObservation> {
    let (observation, receiver) = observe_power_save(frame)?;
    if matches!(observation, ApPowerSaveObservation::PsPoll { .. }) && receiver != access_point {
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
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn observe_ap_null_data_power_save_for_access_point(
    frame: &[u8],
    access_point: [u8; 6],
) -> Option<ApPowerSaveObservation> {
    const NULL_DATA_TO_DS_FRAME_CONTROL: u16 = 0x0148;

    let Ok(
        &[
            c0,
            c1,
            _,
            _,
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
            sequence0,
            _,
        ],
    ) = <&[u8; STA_NULL_DATA_FRAME_LEN]>::try_from(frame)
    else {
        return None;
    };
    let frame_control = u16::from_le_bytes([c0, c1]);
    if FrameControlFields::from_raw(frame_control)
        .with_retry(false)
        .with_power_management(false)
        .raw()
        != NULL_DATA_TO_DS_FRAME_CONTROL
        || [r0, r1, r2, r3, r4, r5] != access_point
        || [b0, b1, b2, b3, b4, b5] != access_point
        || crate::sequence::SequenceControl::from_bytes((sequence0 as u16).to_le_bytes()).fragment()
            != 0
    {
        return None;
    }
    let peer = [t0, t1, t2, t3, t4, t5];
    if peer == [0; 6]
        || peer == [0xff; 6]
        || crate::address::AddressFlags::from_bytes([peer[0]]).group()
    {
        return None;
    }
    if FrameControlFields::from_raw(frame_control).power_management() {
        Some(ApPowerSaveObservation::Sleeping { peer })
    } else {
        Some(ApPowerSaveObservation::Active { peer })
    }
}

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;

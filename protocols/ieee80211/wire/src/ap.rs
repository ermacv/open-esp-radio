//! Allocation-free AP association and power-save frame transforms.
//!
//! These pure transforms form the source-owned AP protocol boundary.
//! Association ownership, deferred
//! queues and wakeups stay with the radio owner.
//! Management decoding splits the fixed header and each fixed body as arrays
//! before accessing fields; variable information elements remain borrowed slices.

use crate::data::{MsduHeader, MsduParts, PeerQos};
use crate::element::ElementId;
use crate::frame::{
    ControlSubtype, FrameControlFields, FrameType, ManagementSubtype, QosAckPolicy,
};
use crate::management::{AidField, AssociationId};
use crate::management::{AuthenticationAlgorithm, AuthenticationTransaction};
use crate::status::{ReasonCode, StatusCode};
use oer_protocols_ieee802_wire::{EtherType, Eui48, IndividualAddress};

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
    pub access_point: Eui48,
    pub destination: Eui48,
    pub sequence_number: SequenceNumber,
    pub ether_type: EtherType,
    pub payload: &'payload [u8],
}

impl ApDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        if IndividualAddress::for_interface(self.access_point).is_err() {
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
            PeerQos::NonQos,
            QosAckPolicy::Normal,
        );
        let mut header = plan.header;
        crate::encoding::write_array::<22, _, _>(
            &mut header,
            &self.sequence_number.sequence_control().to_bytes(),
        );
        let mut reader = oer_portable_wire::Reader::new(&header);
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

/// One protected MSDU sent from an AP to its authorized peer.
///
/// The caller owns the pairwise packet number. This codec merely places the
/// supplied CCMP header in the DMA image; the backend encrypts the
/// plaintext LLC/SNAP payload and appends the MIC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApProtectedDataFrame<'frame> {
    pub access_point: Eui48,
    pub peer: Eui48,
    pub sequence_number: SequenceNumber,
    pub user_priority: WmmUserPriority,
    pub peer_qos: crate::data::PeerQos,
    /// `Buffered` for a frame released from an AP power-save queue while
    /// further traffic remains buffered.
    pub more_data: crate::data::MoreData,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub msdu: MsduParts<'frame>,
}

impl ApProtectedDataFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        let planned = ProtectedDataPlan::new(
            self.access_point,
            self.peer,
            self.sequence_number,
            self.user_priority,
            self.peer_qos,
            self.more_data,
            self.msdu.header(),
        )?;
        let header = planned.header()?;
        crate::encoding::FrameParts([
            header,
            &self.ccmp_header,
            &planned.llc_snap,
            self.msdu.payload,
        ])
        .write(output)
        .map_err(|required| ApDataFrameError::OutputTooSmall { required })
    }
}

/// The prefix of one protected MSDU an AP sends to its authorized peer or
/// to its BSS, written in place in front of the MSDU's payload: the MAC
/// header, a reserved CCMP header the transmitter fills from the packet
/// number its caller numbers (IEEE Std 802.11-2020, 12.5.3.2), and the RFC
/// 1042 LLC/SNAP header of the MSDU's EtherType.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApProtectedDataPrefix {
    pub access_point: Eui48,
    pub peer: Eui48,
    pub sequence_number: SequenceNumber,
    pub user_priority: WmmUserPriority,
    pub peer_qos: crate::data::PeerQos,
    pub more_data: crate::data::MoreData,
    pub msdu: MsduHeader,
}

impl ApProtectedDataPrefix {
    /// The prefix's length: the MAC header, the CCMP header and LLC/SNAP.
    ///
    /// # Errors
    ///
    /// The addresses or the priority are not an AP data frame's.
    pub fn prefix_len(self) -> Result<usize, ApDataFrameError> {
        let planned = self.planned()?;
        Ok(usize::from(planned.plan.header_len).saturating_add(PROTECTED_PREFIX_TAIL))
    }

    /// Write the prefix into `output`, whose length is [`Self::prefix_len`]: the
    /// octets right before the payload. Its length.
    ///
    /// # Errors
    ///
    /// The addresses or the priority are not an AP data frame's, or
    /// `output` is not the prefix's length; nothing is written then.
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApDataFrameError> {
        let planned = self.planned()?;
        let header = planned.header()?;
        let required = header.len().saturating_add(PROTECTED_PREFIX_TAIL);
        if output.len() != required {
            return Err(ApDataFrameError::OutputTooSmall { required });
        }
        crate::encoding::FrameParts([header, &[0; CCMP_HEADER_LEN], &planned.llc_snap])
            .write(output)
            .map_err(|required| ApDataFrameError::OutputTooSmall { required })
    }

    fn planned(self) -> Result<ProtectedDataPlan, ApDataFrameError> {
        ProtectedDataPlan::new(
            self.access_point,
            self.peer,
            self.sequence_number,
            self.user_priority,
            self.peer_qos,
            self.more_data,
            self.msdu,
        )
    }
}

/// What a protected data MPDU's prefix holds after its MAC header: the CCMP
/// header and LLC/SNAP.
const PROTECTED_PREFIX_TAIL: usize = CCMP_HEADER_LEN + crate::data::LLC_SNAP_HEADER_LEN;

/// The planned MAC header and LLC/SNAP header of one protected AP data
/// MPDU, its Sequence Control written.
struct ProtectedDataPlan {
    plan: crate::data::DataEncapPlan,
    llc_snap: [u8; crate::data::LLC_SNAP_HEADER_LEN],
}

impl ProtectedDataPlan {
    fn new(
        access_point: Eui48,
        peer: Eui48,
        sequence_number: SequenceNumber,
        user_priority: WmmUserPriority,
        peer_qos: crate::data::PeerQos,
        more_data: crate::data::MoreData,
        msdu: MsduHeader,
    ) -> Result<Self, ApDataFrameError> {
        if IndividualAddress::for_interface(access_point).is_err() {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        let priority = user_priority;
        let mut plan = plan_data_encapsulation(
            DataInterfaceRole::AccessPoint,
            access_point,
            access_point,
            msdu,
            priority,
            peer_qos,
            QosAckPolicy::Normal,
        );
        if plan.header[4..10] != peer.octets() {
            return Err(ApDataFrameError::InvalidPeer);
        }
        plan.set_protected();
        if more_data == crate::data::MoreData::Buffered {
            plan.set_more_data();
        }
        crate::encoding::write_array::<22, _, _>(
            &mut plan.header,
            &sequence_number.sequence_control().to_bytes(),
        );
        let llc_snap = plan.llc_snap;
        Ok(Self { plan, llc_snap })
    }

    fn header(&self) -> Result<&[u8], ApDataFrameError> {
        let mut reader = oer_portable_wire::Reader::new(&self.plan.header);
        reader.take(usize::from(self.plan.header_len)).map_err(|_| {
            ApDataFrameError::OutputTooSmall {
                required: usize::from(self.plan.header_len),
            }
        })
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
    pub access_point: Eui48,
    pub peer: Eui48,
    pub sequence_number: SequenceNumber,
    pub user_priority: WmmUserPriority,
    pub more_data: crate::data::MoreData,
    pub ccmp_header: Option<[u8; CCMP_HEADER_LEN]>,
    pub msdus: &'a [MsduParts<'a>],
}

/// Return the complete DMA-resident MPDU length for a bounded AP A-MSDU.
///
/// The result excludes the hardware-owned CCMP MIC and FCS. `protection`
/// controls only the retained eight-byte CCMP header; the A-MSDU body has the
/// same IEEE 802.11 representation for Open and CCMP epochs.
pub fn ap_amsdu_frame_length(
    msdus: &[MsduParts<'_>],
    protection: crate::security::LinkProtection,
) -> Result<usize, ApDataFrameError> {
    let body = crate::encoding::AmsduBody::checked(msdus, AP_AMSDU_BASELINE_MAX_LEN)
        .map_err(amsdu_error)?;
    let length = body
        .length()
        .map_err(|length| amsdu_error(crate::encoding::AmsduError::TooLong(length)))?;
    crate::data::IEEE80211_QOS_DATA_HEADER_LEN
        .checked_add(match protection {
            crate::security::LinkProtection::Open => 0,
            crate::security::LinkProtection::Ccmp => CCMP_HEADER_LEN,
        })
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
        if IndividualAddress::for_interface(self.access_point).is_err() {
            return Err(ApDataFrameError::InvalidAccessPoint);
        }
        if IndividualAddress::for_interface(self.peer).is_err() {
            return Err(ApDataFrameError::InvalidPeer);
        }
        let priority = self.user_priority;
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
            PeerQos::Qos,
            QosAckPolicy::Normal,
        );
        if plan.header[4..10] != self.peer.octets() {
            return Err(ApDataFrameError::InvalidPeer);
        }
        if self.ccmp_header.is_some() {
            plan.set_protected();
        }
        if self.more_data == crate::data::MoreData::Buffered {
            plan.set_more_data();
        }
        plan.set_amsdu();

        let mut header = plan.header;
        crate::encoding::write_array::<22, _, _>(
            &mut header,
            &self.sequence_number.sequence_control().to_bytes(),
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
        peer: Eui48,
        ssid: &'a [u8],
    },
    OpenAuthentication {
        peer: Eui48,
    },
    /// An SAE Authentication frame: a Commit (transaction 1) or a Confirm
    /// (transaction 2) with its status and the body after the status.
    SaeAuthentication {
        peer: Eui48,
        transaction: AuthenticationTransaction,
        status: StatusCode,
        body: &'a [u8],
    },
    Association {
        peer: Eui48,
        security: ApAssociationSecurityObservation<'a>,
        /// Highest legacy rate shared with the B/G ERP rate set advertised
        /// by this AP, in 500-kbit/s units. Zero means no common rate.
        maximum_legacy_rate_500kbps: u8,
        /// Complete one-stream HT receive facts supplied by the peer.
        ht_capabilities: Option<HtPeerCapabilities>,
        /// A QoS STA: the peer supplied WMM information or HT, both of which
        /// imply QoS data framing for this profile.
        qos: crate::data::PeerQos,
        /// The shortest DSSS preamble the peer receives: short when its
        /// Capability Information sets Short Preamble (IEEE Std
        /// 802.11-2020, 9.4.1.4).
        preamble: crate::phy::DsssPreamble,
    },
    Disassociation {
        peer: Eui48,
        reason: ReasonCode,
    },
    Deauthentication {
        peer: Eui48,
        reason: ReasonCode,
    },
    BlockAck {
        peer: Eui48,
        action: BlockAckAction,
    },
    /// The association check of a peer protecting its management frames.
    SaQuery {
        peer: Eui48,
        query: crate::management_protection::SaQuery,
    },
}

/// Exact on-air security facts from one Association Request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAssociationSecurityObservation<'a> {
    /// The request's Capability Information field.
    pub capability: crate::management::CapabilityInformation,
    /// The RSN element (IEEE Std 802.11-2020, 9.4.2.24).
    pub rsn_ie: crate::scan::ElementOccurrence<'a>,
    /// The RSN Extension element (IEEE Std 802.11-2020, 9.4.2.241).
    pub rsnxe: crate::scan::ElementOccurrence<'a>,
    /// A legacy WPA vendor IE (00:50:f2:01). It is never an acceptable
    /// substitute for RSN and makes a mixed request invalid.
    pub legacy_wpa: crate::scan::ElementPresence,
    /// Whether an IE header or payload was truncated. Absence inferred from
    /// a malformed tail is never an Open-security proof.
    pub information_elements: crate::scan::ElementList,
}

impl ApAssociationSecurityObservation<'_> {
    /// The Privacy subfield of the request's Capability Information.
    pub const fn privacy(&self) -> bool {
        self.capability.privacy()
    }
}

/// Parse requests addressed to this AP, including broadcast probe discovery.
///
/// Beacons, responses, foreign BSS traffic and unsupported authentication
/// algorithms are intentionally ignored rather than being promoted into AP
/// state transitions.
pub fn parse_ap_management_request<'a>(
    profile: &Advertisement,
    frame: &'a [u8],
    access_point: Eui48,
) -> Option<ApManagementRequest<'a>> {
    let mut reader = oer_portable_wire::Reader::new(frame);
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
        sc1,
    ] = *header;
    let destination = Eui48::new([d0, d1, d2, d3, d4, d5]);
    let peer = Eui48::new([p0, p1, p2, p3, p4, p5]);
    let bssid = Eui48::new([b0, b1, b2, b3, b4, b5]);
    let frame_control = FrameControlFields::from_bytes([fc0, fc1]);
    if frame_control.kind() != FrameType::Management {
        return None;
    }
    let subtype = frame_control.management_subtype();
    if subtype == ManagementSubtype::ProbeRequest {
        if (frame_control.version() != 0
            || frame_control.kind() != FrameType::Management
            || frame_control.ds() != 0
            || frame_control.more_fragments()
            || frame_control.protected())
            || crate::sequence::SequenceControl::from_bytes([sc0, sc1]).fragment() != 0
            || IndividualAddress::for_interface(peer).is_err()
            || (destination != access_point && destination != Eui48::BROADCAST)
            || (bssid != access_point && bssid != Eui48::BROADCAST)
        {
            return None;
        }
        return Some(ApManagementRequest::Probe {
            peer,
            ssid: probe::ssid(body)?,
        });
    }
    if (frame_control.version() != 0
        || frame_control.kind() != FrameType::Management
        || frame_control.ds() != 0)
        || IndividualAddress::for_interface(peer).is_err()
        || destination != access_point
        || bssid != access_point
    {
        return None;
    }
    // Parsing is not authentication. The caller must verify the pairwise
    // integrity and replay state before acting on a protected request.
    let protected = frame_control.protected();
    if protected
        && !matches!(
            subtype,
            ManagementSubtype::Disassociation
                | ManagementSubtype::Deauthentication
                | ManagementSubtype::Action
        )
    {
        return None;
    }
    let management_body = if protected {
        {
            let mut reader = oer_portable_wire::Reader::new(body);
            reader.take(CCMP_HEADER_LEN).ok()?;
            reader.remaining()
        }
    } else {
        body
    };
    match subtype {
        ManagementSubtype::Authentication => {
            let mut reader = oer_portable_wire::Reader::new(management_body);
            let fields = crate::management::AuthenticationFields::from_bytes(reader.read().ok()?);
            let body = reader.remaining();
            let algorithm = fields.algorithm();
            let transaction = fields.transaction();
            let status = fields.status_code();
            match algorithm {
                AuthenticationAlgorithm::OPEN_SYSTEM => (transaction
                    == AuthenticationTransaction::OPEN_REQUEST
                    && status == StatusCode::SUCCESS)
                    .then_some(ApManagementRequest::OpenAuthentication { peer }),
                AuthenticationAlgorithm::SAE => Some(ApManagementRequest::SaeAuthentication {
                    peer,
                    transaction,
                    status,
                    body,
                }),
                _ => None,
            }
        }
        ManagementSubtype::AssociationRequest => {
            let mut reader = oer_portable_wire::Reader::new(management_body);
            let capabilities =
                crate::management::CapabilityInformation::from_bytes(reader.read().ok()?);
            let _listen_interval: [u8; 2] = reader.read().ok()?;
            let information_elements = reader.remaining();
            let ht_capabilities = ht_peer_capabilities(information_elements);
            Some(ApManagementRequest::Association {
                peer,
                security: association_security_observation(information_elements, capabilities),
                maximum_legacy_rate_500kbps: maximum_ap_legacy_rate(profile, information_elements),
                ht_capabilities,
                qos: if ht_capabilities.is_some() || supports_wmm(information_elements) {
                    crate::data::PeerQos::Qos
                } else {
                    crate::data::PeerQos::NonQos
                },
                preamble: if capabilities.short_preamble() {
                    crate::phy::DsssPreamble::Short
                } else {
                    crate::phy::DsssPreamble::Long
                },
            })
        }
        ManagementSubtype::Disassociation | ManagementSubtype::Deauthentication => {
            let mut reader = oer_portable_wire::Reader::new(management_body);
            let reason = crate::management::ReasonCodeField::from_bytes(reader.read().ok()?).code();
            if subtype == ManagementSubtype::Disassociation {
                Some(ApManagementRequest::Disassociation { peer, reason })
            } else {
                Some(ApManagementRequest::Deauthentication { peer, reason })
            }
        }
        ManagementSubtype::Action => {
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
    const fn frame_control(self) -> FrameControlFields {
        macro_rules! of {
            ($subtype:ident) => {
                match FrameControlFields::management(ManagementSubtype::$subtype) {
                    Some(fields) => fields,
                    None => panic!("a named management subtype fits its field"),
                }
            };
        }
        const ACTION: FrameControlFields = of!(Action);
        const DISASSOCIATION: FrameControlFields = of!(Disassociation);
        const DEAUTHENTICATION: FrameControlFields = of!(Deauthentication);
        const {
            if ACTION.to_bytes()[0] != 0xd0
                || DISASSOCIATION.to_bytes()[0] != 0xa0
                || DEAUTHENTICATION.to_bytes()[0] != 0xc0
            {
                panic!("a named management subtype has its Frame Control field");
            }
        }
        match self {
            Self::Action => ACTION,
            Self::Disassociation => DISASSOCIATION,
            Self::Deauthentication => DEAUTHENTICATION,
        }
    }

    /// A robust subtype in a management header, without authenticating it.
    pub fn parse(frame: &[u8]) -> Option<Self> {
        let mut reader = oer_portable_wire::Reader::new(frame);
        let control = reader.array().ok()?;
        let control = FrameControlFields::from_bytes(*control);
        if control.version() != 0 || control.kind() != FrameType::Management || control.ds() != 0 {
            return None;
        }
        match control.management_subtype() {
            ManagementSubtype::Action => Some(Self::Action),
            ManagementSubtype::Disassociation => Some(Self::Disassociation),
            ManagementSubtype::Deauthentication => Some(Self::Deauthentication),
            _ => None,
        }
    }

    /// Whether management frame protection applies to this body.
    pub fn is_robust(self, body: &[u8]) -> bool {
        match self {
            Self::Action => body
                .first()
                .copied()
                .map(crate::action::ActionCategory::new)
                .is_some_and(crate::action::ActionCategory::is_robust),
            Self::Disassociation | Self::Deauthentication => true,
        }
    }
}

/// One unprotected AP-originated management frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApManagementFrame<'a> {
    pub subtype: ApManagementSubtype,
    pub access_point: Eui48,
    pub peer: Eui48,
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
    pub access_point: Eui48,
    pub peer: Eui48,
    pub sequence_number: SequenceNumber,
    pub ccmp_header: [u8; CCMP_HEADER_LEN],
    pub body: &'a [u8],
}

impl ApProtectedManagementFrame<'_> {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, ApAssociationResponseError> {
        let header = management_header(
            self.subtype.frame_control().with_protected(true),
            self.access_point,
            self.peer,
            self.sequence_number,
        );
        crate::encoding::FrameParts([header.as_slice(), &self.ccmp_header, self.body])
            .write(output)
            .map_err(|required| ApAssociationResponseError::OutputTooSmall { required })
    }
}

fn supports_wmm(bytes: &[u8]) -> bool {
    let mut remaining = oer_portable_wire::Reader::new(bytes);
    while let [id, length, ..] = *remaining.remaining() {
        let length = usize::from(length);
        let Some(record) = length
            .checked_add(2)
            .and_then(|record_len| remaining.take(record_len).ok())
        else {
            return false;
        };
        if ElementId::new(id) == ElementId::VENDOR_SPECIFIC
            && length >= 6
            && matches!(record, [_, _, 0x00, 0x50, 0xf2, 0x02, ..])
        {
            return true;
        }
    }
    false
}

fn association_security_observation(
    bytes: &[u8],
    capability: crate::management::CapabilityInformation,
) -> ApAssociationSecurityObservation<'_> {
    let mut remaining = oer_portable_wire::Reader::new(bytes);
    let mut observation = ApAssociationSecurityObservation {
        capability,
        rsn_ie: crate::scan::ElementOccurrence::Absent,
        rsnxe: crate::scan::ElementOccurrence::Absent,
        legacy_wpa: crate::scan::ElementPresence::Absent,
        information_elements: crate::scan::ElementList::Complete,
    };
    while !remaining.is_empty() {
        let [id, length, ..] = *remaining.remaining() else {
            observation.information_elements = crate::scan::ElementList::Truncated;
            break;
        };
        let length = usize::from(length);
        let Some(record) = length
            .checked_add(2)
            .and_then(|record_len| remaining.take(record_len).ok())
        else {
            observation.information_elements = crate::scan::ElementList::Truncated;
            break;
        };
        let id = ElementId::new(id);
        if id == ElementId::RSN {
            observation.rsn_ie = observation.rsn_ie.and(record);
        }
        if id == ElementId::RSNX {
            observation.rsnxe = observation.rsnxe.and(record);
        }
        if id == ElementId::VENDOR_SPECIFIC
            && length >= 4
            && matches!(record, [_, _, 0x00, 0x50, 0xf2, 0x01, ..])
        {
            observation.legacy_wpa = crate::scan::ElementPresence::Present;
        }
    }
    observation
}

fn maximum_ap_legacy_rate(profile: &Advertisement, bytes: &[u8]) -> u8 {
    let mut maximum = 0;
    let mut remaining = oer_portable_wire::Reader::new(bytes);
    while let Ok(&[id, length]) = remaining.array::<2>() {
        let Ok(value) = remaining.take(usize::from(length)) else {
            break;
        };
        if matches!(
            ElementId::new(id),
            ElementId::SUPPORTED_RATES | ElementId::EXTENDED_SUPPORTED_RATES
        ) {
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
    access_point: Eui48,
    peer: Eui48,
    status: StatusCode,
    management_sequence: SequenceNumber,
) -> Result<usize, ApAssociationResponseError> {
    let header = management_header(
        AUTHENTICATION_FRAME_CONTROL,
        access_point,
        peer,
        management_sequence,
    );
    crate::encoding::FrameParts([
        header.as_slice(),
        // Open System, the second frame of its exchange.
        &crate::management::AuthenticationFields::of(
            AuthenticationAlgorithm::OPEN_SYSTEM,
            AuthenticationTransaction::OPEN_RESPONSE,
            status,
        )
        .to_bytes(),
    ])
    .write(output)
    .map_err(|required| ApAssociationResponseError::OutputTooSmall { required })
}

/// Encode one SAE Authentication frame to `peer`: a Commit or Confirm, or
/// the refusal of one, whose `body` follows the status.
pub fn write_sae_authentication(
    output: &mut [u8],
    access_point: Eui48,
    peer: Eui48,
    transaction: AuthenticationTransaction,
    status: StatusCode,
    body: &[u8],
    management_sequence: SequenceNumber,
) -> Result<usize, ApAssociationResponseError> {
    let header = management_header(
        AUTHENTICATION_FRAME_CONTROL,
        access_point,
        peer,
        management_sequence,
    );
    crate::encoding::FrameParts([
        header.as_slice(),
        &crate::management::AuthenticationFields::of(
            AuthenticationAlgorithm::SAE,
            transaction,
            status,
        )
        .to_bytes(),
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
    access_point: Eui48,
    peer: Eui48,
    status: StatusCode,
    association_id: Option<AssociationId>,
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
    let Some((header, rest)) = frame.split_first_chunk_mut::<MANAGEMENT_HEADER_LEN>() else {
        return Err(ApAssociationResponseError::OutputTooSmall {
            required: AP_ASSOCIATION_RESPONSE_LEN,
        });
    };
    *header = management_header(
        ASSOCIATION_RESPONSE_FRAME_CONTROL,
        access_point,
        peer,
        management_sequence,
    );
    let Some(body) = rest.first_chunk_mut::<AP_ASSOCIATION_RESPONSE_BODY_LEN>() else {
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
            &profile.capabilities(security.link_protection()).to_bytes(),
        );
    }
    MANAGEMENT_HEADER_LEN
        .checked_add(body_len)
        .ok_or(ApAssociationResponseError::LengthOverflow)
}

/// Encode one AP-originated peer teardown frame.
pub fn write_ap_peer_disconnect(
    output: &mut [u8],
    access_point: Eui48,
    peer: Eui48,
    kind: ApPeerDisconnectKind,
    reason: ReasonCode,
    management_sequence: SequenceNumber,
) -> Result<usize, ApAssociationResponseError> {
    let frame_control = match kind {
        ApPeerDisconnectKind::Disassociation => ApManagementSubtype::Disassociation,
        ApPeerDisconnectKind::Deauthentication => ApManagementSubtype::Deauthentication,
    }
    .frame_control();
    let header = management_header(frame_control, access_point, peer, management_sequence);
    crate::encoding::FrameParts([
        header.as_slice(),
        &crate::management::ReasonCodeField::of(reason).to_bytes(),
    ])
    .write(output)
    .map_err(|required| ApAssociationResponseError::OutputTooSmall { required })
}

/// Association Response, management subtype 1 (IEEE Std 802.11-2020, 9.2.4.1.3, Table 9-1).
const ASSOCIATION_RESPONSE_FRAME_CONTROL: FrameControlFields =
    FrameControlFields::from_bytes([0x10, 0x00]);
/// Authentication, management subtype 11 (IEEE Std 802.11-2020, 9.2.4.1.3, Table 9-1).
const AUTHENTICATION_FRAME_CONTROL: FrameControlFields =
    FrameControlFields::from_bytes([0xb0, 0x00]);

fn management_header(
    frame_control: FrameControlFields,
    access_point: Eui48,
    peer: Eui48,
    management_sequence: SequenceNumber,
) -> [u8; MANAGEMENT_HEADER_LEN] {
    let mut frame = [0; MANAGEMENT_HEADER_LEN];
    crate::encoding::write_array::<0, _, _>(&mut frame, &frame_control.to_bytes());
    crate::encoding::write_array::<4, _, _>(&mut frame, &peer.octets());
    crate::encoding::write_array::<10, _, _>(&mut frame, &access_point.octets());
    crate::encoding::write_array::<16, _, _>(&mut frame, &access_point.octets());
    crate::encoding::write_array::<22, _, _>(
        &mut frame,
        &management_sequence.sequence_control().to_bytes(),
    );
    frame
}

/// Build the finite AP HT association response body: its length, which
/// is [`AP_ASSOCIATION_RESPONSE_BODY_LEN`] in the 2.4 GHz band and shorter
/// in the 5 GHz band, where the BSS has no ERP element.
pub fn write_ht_association_response(
    profile: &Advertisement,
    body: &mut [u8; AP_ASSOCIATION_RESPONSE_BODY_LEN],
    status: StatusCode,
    association_id: Option<AssociationId>,
    channel: Channel,
    peer_ht: Option<HtPeerCapabilities>,
    protection: ApBssProtection,
) -> Result<usize, ApAssociationResponseError> {
    // The AID field: the association ID with both marker bits set, or zero
    // when the association is refused (IEEE Std 802.11-2020, 9.4.1.8).
    let encoded_association_id = if status == StatusCode::SUCCESS {
        association_id
            .ok_or(ApAssociationResponseError::MissingAssociationId)?
            .field()
    } else {
        AidField::from_bytes([0; 2])
    };
    let ht_capability = ht_capability_ie_for_peer(profile.ht, channel, peer_ht);
    let ht_operation = ht_operation_ie(channel, protection.ht);
    let erp = [ElementId::ERP.value(), 1, protection.erp_information()];
    crate::encoding::FrameParts([
        &profile.capabilities(LinkProtection::Ccmp).to_bytes(),
        &crate::management::StatusCodeField::of(status).to_bytes(),
        &encoded_association_id.to_bytes(),
        &[ElementId::SUPPORTED_RATES.value(), 8],
        profile.legacy_rates.supported(),
        &[ElementId::EXTENDED_SUPPORTED_RATES.value(), 4],
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
    Sleeping {
        peer: Eui48,
    },
    Active {
        peer: Eui48,
    },
    PsPoll {
        peer: Eui48,
        association_id: AidField,
    },
}

/// Parse the RX-derived power-save edge the AP owner uses.
///
/// Association validation is intentionally separate: the AP peer table must
/// confirm that `peer` currently owns the reported association ID.
pub fn observe_ap_power_save(frame: &[u8]) -> Option<ApPowerSaveObservation> {
    observe_power_save(frame).map(|(observation, _)| observation)
}

/// A power-save edge and the Receiver Address of the frame that carried it.
fn observe_power_save(frame: &[u8]) -> Option<(ApPowerSaveObservation, Eui48)> {
    let mut reader = oer_portable_wire::Reader::new(frame);
    let &[fc0, fc1] = reader.array().ok()?;
    let frame_control = FrameControlFields::from_bytes([fc0, fc1]);
    let frame_type = frame_control.kind();
    let subtype = frame_control.control_subtype();

    // PS-Poll: Frame Control, AID, Receiver and Transmitter Address.
    if frame_type == FrameType::Control
        && subtype == ControlSubtype::PsPoll
        && let Ok(ps_poll) = oer_portable_wire::Reader::new(frame).array::<16>()
    {
        let [_, _, a0, a1, r0, r1, r2, r3, r4, r5, t0, t1, t2, t3, t4, t5] = *ps_poll;
        let field = AidField::from_bytes([a0, a1]);
        if field.marker().get() != 3 || field.aid().get() == 0 {
            return None;
        }
        return Some((
            ApPowerSaveObservation::PsPoll {
                peer: Eui48::new([t0, t1, t2, t3, t4, t5]),
                association_id: field,
            },
            Eui48::new([r0, r1, r2, r3, r4, r5]),
        ));
    }

    let mut reader = oer_portable_wire::Reader::new(frame);
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
    let to_ds = frame_control.to_ds();
    let from_ds = frame_control.from_ds();
    if frame_type != FrameType::Data || !to_ds || from_ds {
        return None;
    }
    let peer = Eui48::new([t0, t1, t2, t3, t4, t5]);
    let receiver = Eui48::new([r0, r1, r2, r3, r4, r5]);
    if frame_control.power_management() {
        Some((ApPowerSaveObservation::Sleeping { peer }, receiver))
    } else {
        Some((ApPowerSaveObservation::Active { peer }, receiver))
    }
}

/// Parse an AP power-save edge while binding PS-Poll Receiver Address to the
/// active BSSID. Protected data admission validates its address mapping in the
/// data owner; the unprotected control frame needs this explicit check before
/// its TA/AID tuple can reserve buffered traffic.
pub fn observe_ap_power_save_for_access_point(
    frame: &[u8],
    access_point: Eui48,
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
pub fn observe_ap_null_data_power_save_for_access_point(
    frame: &[u8],
    access_point: Eui48,
) -> Option<ApPowerSaveObservation> {
    const NULL_DATA_TO_DS_FRAME_CONTROL: FrameControlFields =
        FrameControlFields::from_bytes([0x48, 0x01]);

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
            sequence1,
        ],
    ) = <&[u8; STA_NULL_DATA_FRAME_LEN]>::try_from(frame)
    else {
        return None;
    };
    let frame_control = FrameControlFields::from_bytes([c0, c1]);
    if frame_control.with_retry(false).with_power_management(false) != NULL_DATA_TO_DS_FRAME_CONTROL
        || Eui48::new([r0, r1, r2, r3, r4, r5]) != access_point
        || Eui48::new([b0, b1, b2, b3, b4, b5]) != access_point
        || crate::sequence::SequenceControl::from_bytes([sequence0, sequence1]).fragment() != 0
    {
        return None;
    }
    let peer = Eui48::new([t0, t1, t2, t3, t4, t5]);
    if IndividualAddress::for_interface(peer).is_err() {
        return None;
    }
    if frame_control.power_management() {
        Some(ApPowerSaveObservation::Sleeping { peer })
    } else {
        Some(ApPowerSaveObservation::Active { peer })
    }
}

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;

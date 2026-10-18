//! Protected AP data receive frontier.
//!
//! DMA storage remains borrowed only for this synchronous dispatch. A sink
//! must copy or transfer every Ethernet view before returning, after which the
//! runtime may recycle the descriptor.

use crate::security::ApPairwiseRxCandidate;

use oer_esp32s31_ieee80211::protected_data_rx::{
    ProtectedDataDecapsulation, ProtectedDataFragmentRxError, ProtectedDataRxView,
    UnprotectedDataFragmentRxError, UnprotectedDataRxView, view_protected_data,
    view_protected_data_fragment, view_unprotected_data, view_unprotected_data_fragment,
};

use oer_esp32s31_ieee80211_mac::rx::{
    PUBLIC_HEADER_SIZE, RxError, RxIngressConfig, RxPhyInfo, RxSegment,
    ampdu::{
        RxBlockAckMpduKey, RxBlockAckRequestKey, rx_block_ack_mpdu_key, rx_block_ack_request_key,
    },
    view_normalized_rx_frame,
};

use oer_ieee80211_mac::{
    ccmp::{CcmpHeader, CcmpKeyId, CcmpReplayError, CcmpReplayLane},
    data::{DataDecapError, DataInterfaceRole, EthernetFrameParts, RxDuplicateFilter},
    fragmentation::{
        OPEN_DATA_FRAGMENT_TIMEOUT, OPEN_DATA_REASSEMBLY_CAPACITY, OpenDataDefragmentation,
        OpenDataDefragmenter, OpenDataFragmentError, OpenDataFragmentPreflight,
        OpenDataUnfragmentedAdmission, parse_ccmp_data_identity, parse_open_data_identity,
    },
    security::LinkProtection,
};

use oer_ieee80211_ap::AP_MAX_CLIENTS;

use oer_ieee80211_softmac::MacRxMetadata;

const OPEN_FRAGMENT_CONTEXTS: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApRxConfig {
    pub access_point: [u8; 6],
    pub ingress: RxIngressConfig,
    pub security: LinkProtection,
}

#[derive(Clone, Copy, Debug)]
pub struct ApRxEvent<'frame> {
    pub frame: EthernetFrameParts<'frame>,
    pub raw: &'frame [u8],
    pub amsdu: bool,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

pub trait ApRxSink {
    fn publish(&mut self, event: ApRxEvent<'_>);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApRxError {
    Radio(RxError),
    Data(DataDecapError),
    SecurityModeMismatch,
    PeerQosMismatch,
    PairwiseKeyId(u8),
    Replay(CcmpReplayError),
    KeyGenerationMismatch,
    Fragment(OpenDataFragmentError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApRxDispatch {
    Data { ethernet_frames: u8, amsdu: bool },
    FragmentBuffered { expired: u8, evicted: bool },
    Duplicate,
    ForeignPeer,
    Unauthorized,
    Rejected(ApRxError),
}

/// Value-only request handed from the ordered AP RX dispatcher to the AP key
/// owner. A protected request is created only after S31 hardware has reported
/// successful CCMP integrity verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApRxAdmissionRequest {
    peer: [u8; 6],
    lane: CcmpReplayLane,
    ccmp_header: Option<CcmpHeader>,
    operation: ApRxAdmissionOperation,
}

/// Minimal request for the complete WPA2 pairwise ordinary-data leaf.
///
/// Fragment preparation/commit state cannot be represented by this type, so
/// the saturated path does not carry or branch on the general operation enum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApOrdinaryPairwiseRxRequest {
    peer: [u8; 6],
    lane: CcmpReplayLane,
    ccmp_header: CcmpHeader,
}

impl ApOrdinaryPairwiseRxRequest {
    pub(crate) const fn new(peer: [u8; 6], lane: CcmpReplayLane, ccmp_header: CcmpHeader) -> Self {
        Self {
            peer,
            lane,
            ccmp_header,
        }
    }

    pub const fn peer(self) -> [u8; 6] {
        self.peer
    }

    pub const fn lane(self) -> CcmpReplayLane {
        self.lane
    }

    pub const fn ccmp_header(self) -> CcmpHeader {
        self.ccmp_header
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ApRxAdmissionOperation {
    Ordinary,
    AuthorizeFragment,
    PrepareFragment,
    CommitFragment(ApRxPreparedReplay),
}

impl ApRxAdmissionRequest {
    pub(crate) const fn new(
        peer: [u8; 6],
        lane: CcmpReplayLane,
        ccmp_header: Option<CcmpHeader>,
    ) -> Self {
        Self {
            peer,
            lane,
            ccmp_header,
            operation: ApRxAdmissionOperation::Ordinary,
        }
    }

    fn authorize_fragment(peer: [u8; 6], lane: CcmpReplayLane, header: CcmpHeader) -> Self {
        Self {
            peer,
            lane,
            ccmp_header: Some(header),
            operation: ApRxAdmissionOperation::AuthorizeFragment,
        }
    }

    fn prepare_fragment(peer: [u8; 6], lane: CcmpReplayLane, header: CcmpHeader) -> Self {
        Self {
            peer,
            lane,
            ccmp_header: Some(header),
            operation: ApRxAdmissionOperation::PrepareFragment,
        }
    }

    fn commit_fragment(prepared: ApRxPreparedReplay) -> Self {
        Self {
            peer: prepared.peer,
            lane: prepared.lane,
            ccmp_header: Some(prepared.ccmp_header),
            operation: ApRxAdmissionOperation::CommitFragment(prepared),
        }
    }

    pub const fn peer(self) -> [u8; 6] {
        self.peer
    }

    pub const fn lane(self) -> CcmpReplayLane {
        self.lane
    }

    pub const fn ccmp_header(self) -> Option<CcmpHeader> {
        self.ccmp_header
    }

    pub(crate) const fn operation(self) -> ApRxAdmissionOperation {
        self.operation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ApRxPreparedReplay {
    pub(crate) peer: [u8; 6],
    pub(crate) lane: CcmpReplayLane,
    pub(crate) ccmp_header: CcmpHeader,
    pub(crate) owner: ApRxDuplicateOwner,
    pub(crate) candidate: ApRxPreparedCandidate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ApRxPreparedCandidate {
    Hardware(ApPairwiseRxCandidate),
    /// Source tests can model the external two-phase callback without forging
    /// a hardware-key binding. Production engine admission always rejects it.
    #[cfg(test)]
    Model,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApRxAdmissionOutcome {
    Authorized(ApRxDuplicateOwner),
    Prepared(ApRxPreparedReplay),
    Unauthorized,
    Rejected(ApRxError),
    /// Replay detection rejected an ordinary MPDU of this exact owner. The
    /// dispatcher still applies duplicate removal, which precedes replay
    /// detection in IEEE 802.11 receive processing.
    Replay {
        owner: ApRxDuplicateOwner,
        error: CcmpReplayError,
    },
}

/// Exact bounded duplicate-filter ownership for one AP association.
///
/// AIDs are allocated from `1..=AP_MAX_CLIENTS`, so the owner resolves to a
/// pre-existing slot before CCMP replay admission can commit. The epoch makes
/// reuse of the same AID (including same-address reassociation) reset history
/// instead of inheriting retry fingerprints from its predecessor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApRxDuplicateOwner {
    slot: u8,
    association_epoch: u32,
    key_generation: u32,
}

impl ApRxDuplicateOwner {
    pub(crate) fn new(association_id: u16, association_epoch: u32) -> Option<Self> {
        let slot = association_id.checked_sub(1)?;
        if usize::from(slot) >= AP_MAX_CLIENTS {
            return None;
        }
        Some(Self {
            slot: u8::try_from(slot).ok()?,
            association_epoch,
            key_generation: 0,
        })
    }

    pub(crate) const fn with_key_generation(mut self, key_generation: u32) -> Self {
        self.key_generation = key_generation;
        self
    }

    const fn slot(self) -> usize {
        self.slot as usize
    }

    const fn fragmentation_epoch(self) -> u64 {
        (self.association_epoch as u64) << 32 | self.key_generation as u64
    }
}

/// Unforgeable result of consulting the live AP controlled-port and key
/// owner. Its constructors stay within the chip AP crate so an integration
/// cannot accidentally authorize WPA2 RX with a Boolean or generation zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApRxAdmission {
    outcome: ApRxAdmissionOutcome,
}

impl ApRxAdmission {
    pub(crate) const fn authorized(owner: ApRxDuplicateOwner) -> Self {
        Self {
            outcome: ApRxAdmissionOutcome::Authorized(owner),
        }
    }

    pub(crate) const fn unauthorized() -> Self {
        Self {
            outcome: ApRxAdmissionOutcome::Unauthorized,
        }
    }

    pub(crate) const fn prepared(prepared: ApRxPreparedReplay) -> Self {
        Self {
            outcome: ApRxAdmissionOutcome::Prepared(prepared),
        }
    }

    pub(crate) const fn rejected(error: ApRxError) -> Self {
        Self {
            outcome: ApRxAdmissionOutcome::Rejected(error),
        }
    }

    pub(crate) const fn replayed(owner: ApRxDuplicateOwner, error: CcmpReplayError) -> Self {
        Self {
            outcome: ApRxAdmissionOutcome::Replay { owner, error },
        }
    }

    pub(crate) const fn authorized_owner(self) -> Option<ApRxDuplicateOwner> {
        match self.outcome {
            ApRxAdmissionOutcome::Authorized(owner) => Some(owner),
            ApRxAdmissionOutcome::Prepared(_)
            | ApRxAdmissionOutcome::Unauthorized
            | ApRxAdmissionOutcome::Rejected(_)
            | ApRxAdmissionOutcome::Replay { .. } => None,
        }
    }
}

struct ApPeerDuplicateState {
    address: [u8; 6],
    owner: ApRxDuplicateOwner,
    filter: RxDuplicateFilter,
}

enum ApDataRxView<'frame> {
    Open(UnprotectedDataRxView<'frame>),
    Wpa2Personal(ProtectedDataRxView<'frame>),
}

impl<'frame> ApDataRxView<'frame> {
    fn mpdu(&self) -> &'frame [u8] {
        match self {
            Self::Open(data) => data.mpdu,
            Self::Wpa2Personal(data) => data.mpdu,
        }
    }

    const fn ordering(&self) -> (bool, u16, Option<u8>) {
        match self {
            Self::Open(data) => (data.retry, data.sequence_control, data.tid),
            Self::Wpa2Personal(data) => (data.retry, data.sequence_control, data.tid),
        }
    }

    const fn ccmp_header(&self) -> Option<CcmpHeader> {
        match self {
            Self::Open(_) => None,
            Self::Wpa2Personal(data) => Some(data.ccmp_header),
        }
    }

    fn decapsulate(self) -> Result<ProtectedDataDecapsulation<'frame>, DataDecapError> {
        match self {
            Self::Open(data) => data.decapsulate(DataInterfaceRole::AccessPoint),
            Self::Wpa2Personal(data) => data.decapsulate(DataInterfaceRole::AccessPoint),
        }
    }
}

/// Independent duplicate history for every admitted AP peer.
pub struct ApRxDispatcher {
    config: ApRxConfig,
    duplicates: [Option<ApPeerDuplicateState>; AP_MAX_CLIENTS],
    fragments: OpenDataDefragmenter<OPEN_FRAGMENT_CONTEXTS, OPEN_DATA_REASSEMBLY_CAPACITY>,
    fragment_admission_active: bool,
}

impl ApRxDispatcher {
    pub const fn new(config: ApRxConfig) -> Self {
        Self {
            config,
            duplicates: [const { None }; AP_MAX_CLIENTS],
            fragments: OpenDataDefragmenter::new(OPEN_DATA_FRAGMENT_TIMEOUT),
            fragment_admission_active: false,
        }
    }

    /// Begin a new AP epoch without moving the per-peer duplicate table
    /// through an executor stack.
    pub fn reset(&mut self, config: ApRxConfig) {
        self.config = config;
        self.duplicates.fill_with(|| None);
        self.fragments.clear();
        self.fragment_admission_active = false;
    }

    /// Release duplicate ownership when the AP peer close transaction reaches
    /// its terminal edge. A later AID reuse is safe even if this explicit
    /// cleanup is skipped because [`ApRxDuplicateOwner`] replaces the
    /// exact slot on epoch mismatch.
    pub fn forget_peer(&mut self, peer: [u8; 6]) -> bool {
        let fragmented = self.fragments.forget_transmitter(peer) != 0;
        let Some(index) = self
            .duplicates
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|state| state.address == peer))
        else {
            return fragmented;
        };
        self.duplicates[index] = None;
        true
    }

    /// Revoke every incomplete Open MSDU at an AP stop/reset edge.
    pub fn clear_open_fragmentation(&mut self) -> usize {
        self.fragment_admission_active = false;
        self.fragments.clear()
    }

    /// Return whether an ordinary dispatch can borrow its Ethernet payload
    /// directly from the current staging frame. Reassembled payloads live in
    /// the fragment owner and must take the adapter's copying slow path.
    pub fn may_publish_in_place(&self, segment: RxSegment<'_>) -> bool {
        // This is an immutable path-selection hint, not protocol admission.
        // The dispatcher below still performs the complete normalized view,
        // security and bounds validation. Avoid repeating that full parser on
        // every ordinary in-order MPDU merely to inspect three public header
        // bits.
        segment
            .buffer
            .get(PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 24)
            .is_some_and(|mpdu| !fragmented_mpdu(mpdu))
    }

    /// Return whether the immutable public header selects the complete
    /// ordinary WPA2 QoS-data leaf.
    ///
    /// This is only a path-selection hint. The selected leaf still performs
    /// normalized hardware/CCMP validation, AP address validation, controlled
    /// port admission, replay, duplicate filtering and Ethernet decapsulation.
    /// Any fragment epoch, A-MSDU or non-QoS subtype stays on the complete AP
    /// role graph.
    pub fn may_dispatch_ordinary_pairwise(&self, segment: RxSegment<'_>) -> bool {
        if self.config.security != LinkProtection::Ccmp || self.fragment_admission_active {
            return false;
        }
        let Some(mpdu) = segment.buffer.get(PUBLIC_HEADER_SIZE..) else {
            return false;
        };
        let Some(frame_control) = mpdu
            .get(..2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        else {
            return false;
        };
        // Type=Data, subtype=QoS Data, Protected=1.
        if frame_control & 0x00fc != 0x0088 || frame_control & 0x4000 == 0 {
            return false;
        }
        if fragmented_mpdu(mpdu) {
            return false;
        }
        let qos_control_offset = 24 + usize::from(frame_control & 0x0300 == 0x0300) * 6;
        mpdu.get(qos_control_offset)
            .is_some_and(|control| control & 0x80 == 0)
    }

    /// Extract the same public ordering key used by connected-station RX.
    /// Peer authorization and active-agreement lookup remain with the AP
    /// owner because multiple stations can use the same TID concurrently.
    #[inline(always)]
    pub fn reorder_key(&self, segment: RxSegment<'_>) -> Option<RxBlockAckMpduKey> {
        if self.config.security == LinkProtection::Open {
            return None;
        }
        rx_block_ack_mpdu_key(segment.buffer, self.config.access_point, None)
    }

    /// Classify a BlockAckReq a client addressed to this AP.
    ///
    /// Agreement state still decides whether the named peer and TID have a
    /// reorder window to move.
    ///
    /// SOURCE: `libnet80211.a::hostap_recv_ctl` dispatches control subtype 8
    /// to `ieee80211_recv_bar`, as `sta_recv_ctl` does for the station.
    pub fn block_ack_request_key(&self, segment: RxSegment<'_>) -> Option<RxBlockAckRequestKey> {
        if self.config.security == LinkProtection::Open {
            return None;
        }
        rx_block_ack_request_key(segment.buffer, self.config.access_point, None)
    }

    #[inline(never)]
    pub fn dispatch<S, A>(&mut self, segment: RxSegment<'_>, admit: A, sink: &mut S) -> ApRxDispatch
    where
        S: ApRxSink,
        A: FnMut(ApRxAdmissionRequest) -> ApRxAdmission,
    {
        self.dispatch_inner(segment, None, admit, sink)
    }

    /// Dispatch with the runtime timestamp used for bounded fragment expiry.
    #[inline(never)]
    pub fn dispatch_at<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        now: oer_time::Instant,
        admit: A,
        sink: &mut S,
    ) -> ApRxDispatch
    where
        S: ApRxSink,
        A: FnMut(ApRxAdmissionRequest) -> ApRxAdmission,
    {
        self.dispatch_inner(segment, Some(now), admit, sink)
    }

    /// Dispatch the common complete WPA2 pairwise MPDU without entering the
    /// fragment/open/general role graph.
    ///
    /// The adapter selects this only after immutable public-header preflight
    /// has proved one ordinary current-buffer publication. This function still
    /// performs the complete hardware-CCMP, AP-address, controlled-port,
    /// replay-generation, duplicate and Ethernet validation. If a previous
    /// fragment activated the exceptional ownership graph, it falls back
    /// before mutating any state.
    #[inline(never)]
    pub fn try_dispatch_ordinary_pairwise<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        admit: A,
        sink: &mut S,
    ) -> Option<ApRxDispatch>
    where
        S: ApRxSink,
        A: FnMut(ApOrdinaryPairwiseRxRequest) -> ApRxAdmission,
    {
        if self.config.security != LinkProtection::Ccmp || self.fragment_admission_active {
            return None;
        }
        let data = match view_protected_data(segment, self.config.ingress) {
            Ok(data) => data,
            Err(error) => {
                return Some(ApRxDispatch::Rejected(ApRxError::Radio(error)));
            }
        };
        Some(self.dispatch_ordinary_pairwise_view(data, admit, sink))
    }

    #[inline(never)]
    fn dispatch_ordinary_pairwise_view<S, A>(
        &mut self,
        data: ProtectedDataRxView<'_>,
        mut admit: A,
        sink: &mut S,
    ) -> ApRxDispatch
    where
        S: ApRxSink,
        A: FnMut(ApOrdinaryPairwiseRxRequest) -> ApRxAdmission,
    {
        if data.mpdu[4..10] != self.config.access_point {
            return ApRxDispatch::ForeignPeer;
        }
        let peer: [u8; 6] = data.mpdu[10..16]
            .try_into()
            .expect("validated 802.11 address width");
        if data.ccmp_header.key_id() != CcmpKeyId::PAIRWISE {
            return ApRxDispatch::Rejected(ApRxError::PairwiseKeyId(
                data.ccmp_header.key_id().value(),
            ));
        }
        let retry = data.retry;
        let sequence_control = data.sequence_control;
        let tid = data.tid;
        let ccmp_header = data.ccmp_header;
        let data = match data.decapsulate(DataInterfaceRole::AccessPoint) {
            Ok(data) => data,
            Err(error) => return rejected_data(error),
        };
        let lane = tid.map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
        let duplicate_owner =
            match admit(ApOrdinaryPairwiseRxRequest::new(peer, lane, ccmp_header)).outcome {
                ApRxAdmissionOutcome::Authorized(owner) => owner,
                ApRxAdmissionOutcome::Replay { owner, error } => {
                    return self.replay_rejection(peer, owner, error, retry, sequence_control, tid);
                }
                ApRxAdmissionOutcome::Prepared(_) => {
                    return ApRxDispatch::Rejected(ApRxError::KeyGenerationMismatch);
                }
                ApRxAdmissionOutcome::Unauthorized => {
                    return ApRxDispatch::Unauthorized;
                }
                ApRxAdmissionOutcome::Rejected(error) => {
                    return ApRxDispatch::Rejected(error);
                }
            };
        self.bind_duplicate_owner(peer, duplicate_owner);
        let duplicates = self.duplicates[duplicate_owner.slot()]
            .as_mut()
            .map(|state| &mut state.filter)
            .expect("bound duplicate owner materializes its exact slot");
        if duplicates.is_duplicate(retry, sequence_control, tid) {
            return ApRxDispatch::Duplicate;
        }
        let mut frames = data.frames;
        let amsdu = data.amsdu;
        let mut count = 0_u8;
        for frame in &mut frames {
            let frame = match frame {
                Ok(frame) => frame,
                Err(error) => return rejected_data(error),
            };
            sink.publish(ApRxEvent {
                frame,
                raw: data.raw,
                amsdu,
                metadata: data.metadata,
            });
            count = count.saturating_add(1);
        }
        ApRxDispatch::Data {
            ethernet_frames: count,
            amsdu,
        }
    }

    // Keep the open/fragment/reassembly graph out of ordinary pairwise
    // callers. Without this boundary LLVM duplicates the complete exceptional
    // graph into every admission/sink monomorph, making the common AP leaf an
    // order of magnitude larger than its actual fast body.
    #[inline(never)]
    fn dispatch_inner<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        now: Option<oer_time::Instant>,
        mut admit: A,
        sink: &mut S,
    ) -> ApRxDispatch
    where
        S: ApRxSink,
        A: FnMut(ApRxAdmissionRequest) -> ApRxAdmission,
    {
        let normalized = match view_normalized_rx_frame(&segment, self.config.ingress) {
            Ok(frame) => frame,
            Err(error) => {
                return ApRxDispatch::Rejected(ApRxError::Radio(error));
            }
        };
        let Some(frame_control) = normalized
            .mpdu
            .get(..2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
        else {
            return ApRxDispatch::Rejected(ApRxError::Radio(RxError::Bounds));
        };
        let protected = frame_control & 0x4000 != 0;
        let fragmented = fragmented_mpdu(normalized.mpdu);
        if protected != (self.config.security == LinkProtection::Ccmp) {
            return ApRxDispatch::Rejected(ApRxError::SecurityModeMismatch);
        }
        if fragmented {
            self.fragment_admission_active = true;
            return match self.config.security {
                LinkProtection::Open => self.dispatch_open_fragment(segment, now, &mut admit, sink),
                LinkProtection::Ccmp => {
                    self.dispatch_protected_fragment(segment, now, &mut admit, sink)
                }
            };
        }
        let data = match self.config.security {
            LinkProtection::Open => {
                view_unprotected_data(segment, self.config.ingress).map(ApDataRxView::Open)
            }
            LinkProtection::Ccmp => {
                view_protected_data(segment, self.config.ingress).map(ApDataRxView::Wpa2Personal)
            }
        };
        let data = match data {
            Ok(data) => data,
            Err(error) => {
                return ApRxDispatch::Rejected(ApRxError::Radio(error));
            }
        };
        let mpdu = data.mpdu();
        if mpdu.len() < 24 || mpdu[4..10] != self.config.access_point {
            return ApRxDispatch::ForeignPeer;
        }
        let peer: [u8; 6] = mpdu[10..16]
            .try_into()
            .expect("validated 802.11 address width");
        let (retry, sequence_control, tid) = data.ordering();
        let ccmp_header = data.ccmp_header();
        if let Some(header) = ccmp_header
            && header.key_id() != CcmpKeyId::PAIRWISE
        {
            return ApRxDispatch::Rejected(ApRxError::PairwiseKeyId(header.key_id().value()));
        }
        let data = match data.decapsulate() {
            Ok(data) => data,
            Err(error) => return rejected_data(error),
        };
        let lane = tid.map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
        let preauthorized = if self.fragment_admission_active
            && let Some(header) = ccmp_header
        {
            match admit(ApRxAdmissionRequest::authorize_fragment(peer, lane, header)).outcome {
                ApRxAdmissionOutcome::Authorized(owner) => {
                    self.bind_duplicate_owner(peer, owner);
                    let identity = match parse_ccmp_data_identity(
                        DataInterfaceRole::AccessPoint,
                        mpdu,
                        header,
                    ) {
                        Ok(identity) => identity,
                        Err(error) => {
                            return ApRxDispatch::Rejected(ApRxError::Fragment(error));
                        }
                    };
                    match self.fragments.admit_unfragmented_in_epoch(
                        identity,
                        owner.fragmentation_epoch(),
                        retry,
                        now,
                    ) {
                        Ok(OpenDataUnfragmentedAdmission::Admitted { .. }) => {}
                        Ok(OpenDataUnfragmentedAdmission::Duplicate { .. }) => {
                            return ApRxDispatch::Duplicate;
                        }
                        Err(error) => {
                            return ApRxDispatch::Rejected(ApRxError::Fragment(error));
                        }
                    }
                    Some(owner)
                }
                ApRxAdmissionOutcome::Unauthorized => {
                    return ApRxDispatch::Unauthorized;
                }
                ApRxAdmissionOutcome::Rejected(error) => {
                    return ApRxDispatch::Rejected(error);
                }
                ApRxAdmissionOutcome::Replay { error, .. } => {
                    return ApRxDispatch::Rejected(ApRxError::Replay(error));
                }
                ApRxAdmissionOutcome::Prepared(_) => {
                    return ApRxDispatch::Rejected(ApRxError::KeyGenerationMismatch);
                }
            }
        } else {
            None
        };
        let duplicate_owner =
            match admit(ApRxAdmissionRequest::new(peer, lane, ccmp_header)).outcome {
                ApRxAdmissionOutcome::Authorized(owner) => owner,
                ApRxAdmissionOutcome::Replay { owner, error } => {
                    return self.replay_rejection(peer, owner, error, retry, sequence_control, tid);
                }
                ApRxAdmissionOutcome::Prepared(_) => {
                    return ApRxDispatch::Rejected(ApRxError::KeyGenerationMismatch);
                }
                ApRxAdmissionOutcome::Unauthorized => {
                    return ApRxDispatch::Unauthorized;
                }
                ApRxAdmissionOutcome::Rejected(error) => {
                    return ApRxDispatch::Rejected(error);
                }
            };
        if preauthorized.is_some_and(|owner| owner != duplicate_owner) {
            self.fragments.forget_transmitter(peer);
            return ApRxDispatch::Rejected(ApRxError::KeyGenerationMismatch);
        }
        self.bind_duplicate_owner(peer, duplicate_owner);
        if self.fragment_admission_active && ccmp_header.is_none() {
            let identity = match parse_open_data_identity(DataInterfaceRole::AccessPoint, mpdu) {
                Ok(identity) => identity,
                Err(error) => {
                    return ApRxDispatch::Rejected(ApRxError::Fragment(error));
                }
            };
            match self.fragments.admit_unfragmented_in_epoch(
                identity,
                duplicate_owner.fragmentation_epoch(),
                retry,
                now,
            ) {
                Ok(OpenDataUnfragmentedAdmission::Admitted { .. }) => {}
                Ok(OpenDataUnfragmentedAdmission::Duplicate { .. }) => {
                    return ApRxDispatch::Duplicate;
                }
                Err(error) => {
                    return ApRxDispatch::Rejected(ApRxError::Fragment(error));
                }
            }
        }
        let duplicates = self.duplicate_filter(peer, duplicate_owner);
        if duplicates.is_duplicate(retry, sequence_control, tid) {
            return ApRxDispatch::Duplicate;
        }
        let mut frames = data.frames;
        let amsdu = data.amsdu;
        let mut count = 0_u8;
        for frame in &mut frames {
            let frame = match frame {
                Ok(frame) => frame,
                Err(error) => return rejected_data(error),
            };
            sink.publish(ApRxEvent {
                frame,
                raw: data.raw,
                amsdu,
                metadata: data.metadata,
            });
            count = count.saturating_add(1);
        }
        ApRxDispatch::Data {
            ethernet_frames: count,
            amsdu,
        }
    }

    fn dispatch_open_fragment<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        now: Option<oer_time::Instant>,
        admit: &mut A,
        sink: &mut S,
    ) -> ApRxDispatch
    where
        S: ApRxSink,
        A: FnMut(ApRxAdmissionRequest) -> ApRxAdmission,
    {
        let Some(now) = now else {
            return ApRxDispatch::Rejected(ApRxError::Fragment(
                OpenDataFragmentError::ClockUnavailable,
            ));
        };
        let view = match view_unprotected_data_fragment(
            segment,
            self.config.ingress,
            DataInterfaceRole::AccessPoint,
        ) {
            Ok(view) => view,
            Err(UnprotectedDataFragmentRxError::Radio(error)) => {
                return ApRxDispatch::Rejected(ApRxError::Radio(error));
            }
            Err(UnprotectedDataFragmentRxError::Fragment(error)) => {
                return ApRxDispatch::Rejected(ApRxError::Fragment(error));
            }
        };
        let identity = view.fragment.identity();
        if identity.receiver_address() != self.config.access_point {
            return ApRxDispatch::ForeignPeer;
        }
        let peer = identity.transmitter_address();
        let lane = identity
            .tid()
            .map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
        let duplicate_owner = match admit(ApRxAdmissionRequest::new(peer, lane, None)).outcome {
            ApRxAdmissionOutcome::Authorized(owner) => owner,
            ApRxAdmissionOutcome::Prepared(_) => {
                return ApRxDispatch::Rejected(ApRxError::KeyGenerationMismatch);
            }
            ApRxAdmissionOutcome::Unauthorized => {
                return ApRxDispatch::Unauthorized;
            }
            ApRxAdmissionOutcome::Rejected(error) => {
                return ApRxDispatch::Rejected(error);
            }
            ApRxAdmissionOutcome::Replay { error, .. } => {
                return ApRxDispatch::Rejected(ApRxError::Replay(error));
            }
        };
        self.bind_duplicate_owner(peer, duplicate_owner);
        if view.fragment.fragment_number() == 0
            && self
                .duplicate_filter(peer, duplicate_owner)
                .is_known_duplicate(
                    view.fragment.retry(),
                    view.fragment.sequence_control(),
                    identity.tid(),
                )
        {
            // Fragment zero shares the ordinary MPDU's Sequence Control
            // value. Consult the exact association duplicate owner only at
            // this edge so Retry cannot manufacture a new fragment train by
            // toggling More Fragments on an already accepted ordinary MPDU.
            // Later fragments remain with the reassembler's own history, and
            // no fragment mutates ordinary history.
            return ApRxDispatch::Duplicate;
        }
        let raw = view.raw;
        let metadata = view.metadata;
        match self.fragments.ingest_in_epoch(
            view.fragment,
            duplicate_owner.fragmentation_epoch(),
            now,
            |data| {
                sink.publish(ApRxEvent {
                    frame: data.ethernet_frame(),
                    raw,
                    amsdu: false,
                    metadata,
                });
            },
        ) {
            Ok(OpenDataDefragmentation::Buffered { expired, evicted }) => {
                ApRxDispatch::FragmentBuffered {
                    expired,
                    evicted: evicted.is_some(),
                }
            }
            Ok(OpenDataDefragmentation::Duplicate { .. }) => ApRxDispatch::Duplicate,
            Ok(OpenDataDefragmentation::Complete { .. }) => ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            },
            Err(error) => ApRxDispatch::Rejected(ApRxError::Fragment(error)),
        }
    }

    fn dispatch_protected_fragment<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        now: Option<oer_time::Instant>,
        admit: &mut A,
        sink: &mut S,
    ) -> ApRxDispatch
    where
        S: ApRxSink,
        A: FnMut(ApRxAdmissionRequest) -> ApRxAdmission,
    {
        let Some(now) = now else {
            return ApRxDispatch::Rejected(ApRxError::Fragment(
                OpenDataFragmentError::ClockUnavailable,
            ));
        };
        let view = match view_protected_data_fragment(
            segment,
            self.config.ingress,
            DataInterfaceRole::AccessPoint,
        ) {
            Ok(view) => view,
            Err(ProtectedDataFragmentRxError::Radio(error)) => {
                return ApRxDispatch::Rejected(ApRxError::Radio(error));
            }
            Err(ProtectedDataFragmentRxError::Fragment(error)) => {
                return ApRxDispatch::Rejected(ApRxError::Fragment(error));
            }
        };
        let identity = view.fragment.identity();
        if identity.receiver_address() != self.config.access_point {
            return ApRxDispatch::ForeignPeer;
        }
        if view.ccmp_header.key_id() != CcmpKeyId::PAIRWISE {
            return ApRxDispatch::Rejected(ApRxError::PairwiseKeyId(
                view.ccmp_header.key_id().value(),
            ));
        }
        let peer = identity.transmitter_address();
        let lane = identity
            .tid()
            .map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
        let owner = match admit(ApRxAdmissionRequest::authorize_fragment(
            peer,
            lane,
            view.ccmp_header,
        ))
        .outcome
        {
            ApRxAdmissionOutcome::Authorized(owner) => owner,
            ApRxAdmissionOutcome::Unauthorized => {
                return ApRxDispatch::Unauthorized;
            }
            ApRxAdmissionOutcome::Rejected(error) => {
                return ApRxDispatch::Rejected(error);
            }
            ApRxAdmissionOutcome::Replay { error, .. } => {
                return ApRxDispatch::Rejected(ApRxError::Replay(error));
            }
            ApRxAdmissionOutcome::Prepared(_) => {
                return ApRxDispatch::Rejected(ApRxError::KeyGenerationMismatch);
            }
        };
        if view.fragment.fragment_number() == 0
            && self.is_known_ordinary_duplicate(
                peer,
                owner,
                view.fragment.retry(),
                view.fragment.sequence_control(),
                identity.tid(),
            )
        {
            // Consult only the exact association/PTK generation already
            // authorized above. This must precede duplicate-owner binding,
            // replay preparation and fragment preflight: all three can mutate
            // state, while an ordinary MPDU Retry is a read-only rejection.
            return ApRxDispatch::Duplicate;
        }
        self.bind_duplicate_owner(peer, owner);
        let fragment = view.fragment;
        let more_fragments = fragment.more_fragments();
        let raw = view.raw;
        let metadata = view.metadata;
        let epoch = owner.fragmentation_epoch();
        let admission = match self.fragments.preflight_in_epoch(fragment, epoch, now) {
            Ok(OpenDataFragmentPreflight::Duplicate { .. }) => {
                return ApRxDispatch::Duplicate;
            }
            Ok(OpenDataFragmentPreflight::Admitted(admission)) => admission,
            Err(error) => {
                return ApRxDispatch::Rejected(ApRxError::Fragment(error));
            }
        };
        let prepared = match admit(ApRxAdmissionRequest::prepare_fragment(
            peer,
            lane,
            view.ccmp_header,
        ))
        .outcome
        {
            ApRxAdmissionOutcome::Prepared(prepared) if prepared.owner == owner => prepared,
            ApRxAdmissionOutcome::Prepared(_) => {
                return ApRxDispatch::Rejected(ApRxError::KeyGenerationMismatch);
            }
            ApRxAdmissionOutcome::Unauthorized => {
                return ApRxDispatch::Unauthorized;
            }
            ApRxAdmissionOutcome::Rejected(error) => {
                return ApRxDispatch::Rejected(error);
            }
            ApRxAdmissionOutcome::Replay { error, .. } => {
                return ApRxDispatch::Rejected(ApRxError::Replay(error));
            }
            ApRxAdmissionOutcome::Authorized(_) => {
                return ApRxDispatch::Rejected(ApRxError::KeyGenerationMismatch);
            }
        };

        if more_fragments {
            let outcome = match admission
                .ingest(|_| unreachable!("More Fragments cannot complete one MSDU"))
            {
                Ok(outcome) => outcome,
                Err(error) => {
                    return ApRxDispatch::Rejected(ApRxError::Fragment(error));
                }
            };
            let commit = match admit(ApRxAdmissionRequest::commit_fragment(prepared)).outcome {
                ApRxAdmissionOutcome::Authorized(committed) if committed == owner => Ok(()),
                ApRxAdmissionOutcome::Rejected(error) => Err(error),
                ApRxAdmissionOutcome::Replay { error, .. } => Err(ApRxError::Replay(error)),
                _ => Err(ApRxError::KeyGenerationMismatch),
            };
            if let Err(error) = commit {
                self.fragments.discard(identity, epoch);
                return ApRxDispatch::Rejected(error);
            }
            return match outcome {
                OpenDataDefragmentation::Buffered { expired, evicted } => {
                    ApRxDispatch::FragmentBuffered {
                        expired,
                        evicted: evicted.is_some(),
                    }
                }
                _ => unreachable!("More Fragments produces only a buffered admission"),
            };
        }

        let outcome = admission.ingest(|data| {
            match admit(ApRxAdmissionRequest::commit_fragment(prepared)).outcome {
                ApRxAdmissionOutcome::Authorized(committed) if committed == owner => {}
                ApRxAdmissionOutcome::Rejected(error) => return Err(error),
                ApRxAdmissionOutcome::Replay { error, .. } => {
                    return Err(ApRxError::Replay(error));
                }
                _ => return Err(ApRxError::KeyGenerationMismatch),
            }
            sink.publish(ApRxEvent {
                frame: data.ethernet_frame(),
                raw,
                amsdu: false,
                metadata,
            });
            Ok::<(), ApRxError>(())
        });
        match outcome {
            Ok(OpenDataDefragmentation::Complete { value: Ok(()), .. }) => ApRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            },
            Ok(OpenDataDefragmentation::Complete {
                value: Err(error), ..
            }) => {
                self.fragments.discard(identity, epoch);
                ApRxDispatch::Rejected(error)
            }
            Ok(_) => unreachable!("final fragment produces one completion"),
            Err(error) => ApRxDispatch::Rejected(ApRxError::Fragment(error)),
        }
    }

    /// Test-only admission shim. Production callers cannot bypass the live
    /// controlled-port and key-generation owner with a Boolean.
    #[cfg(test)]
    pub fn dispatch_protected<S, A>(
        &mut self,
        segment: RxSegment<'_>,
        is_authorized: A,
        sink: &mut S,
    ) -> ApRxDispatch
    where
        S: ApRxSink,
        A: FnMut([u8; 6]) -> Option<ApRxDuplicateOwner>,
    {
        let mut is_authorized = is_authorized;
        self.dispatch(
            segment,
            |request| {
                if let Some(owner) = is_authorized(request.peer()) {
                    ApRxAdmission::authorized(owner)
                } else {
                    ApRxAdmission::unauthorized()
                }
            },
            sink,
        )
    }

    #[inline(always)]
    fn bind_duplicate_owner(&mut self, peer: [u8; 6], owner: ApRxDuplicateOwner) {
        let index = owner.slot();
        let current_matches = self.duplicates[index]
            .as_ref()
            .is_some_and(|state| state.address == peer && state.owner == owner);
        if current_matches {
            return;
        }

        // An AID slot or same-address association epoch has changed. Revoke
        // both sides of a possible slot reuse before installing the new
        // duplicate owner, so no retained Open bytes survive a controlled-
        // port/association generation edge even if explicit peer-close
        // cleanup raced or was skipped.
        if let Some(stale) = self.duplicates[index].take() {
            self.fragments.forget_transmitter(stale.address);
        }
        self.fragments.forget_transmitter(peer);
        self.duplicates[index] = Some(ApPeerDuplicateState {
            address: peer,
            owner,
            filter: RxDuplicateFilter::new(),
        });
    }

    #[inline(always)]
    fn duplicate_filter(
        &mut self,
        peer: [u8; 6],
        owner: ApRxDuplicateOwner,
    ) -> &mut RxDuplicateFilter {
        self.bind_duplicate_owner(peer, owner);
        let index = owner.slot();
        self.duplicates[index]
            .as_mut()
            .map(|state| &mut state.filter)
            .expect("exact duplicate owner always materializes its bounded slot")
    }

    /// IEEE 802.11 removes duplicates before replay detection. A Retry of the
    /// MPDU last accepted from this exact owner carries that MPDU's PN, so
    /// its replay rejection is a duplicate, not a replay. History is only
    /// read: a rejected MPDU never becomes the accepted retry fingerprint.
    #[cold]
    fn replay_rejection(
        &self,
        peer: [u8; 6],
        owner: ApRxDuplicateOwner,
        error: CcmpReplayError,
        retry: bool,
        sequence_control: u16,
        tid: Option<u8>,
    ) -> ApRxDispatch {
        if self.is_known_ordinary_duplicate(peer, owner, retry, sequence_control, tid) {
            ApRxDispatch::Duplicate
        } else {
            ApRxDispatch::Rejected(ApRxError::Replay(error))
        }
    }

    #[inline(always)]
    fn is_known_ordinary_duplicate(
        &self,
        peer: [u8; 6],
        owner: ApRxDuplicateOwner,
        retry: bool,
        sequence_control: u16,
        tid: Option<u8>,
    ) -> bool {
        self.duplicates
            .get(owner.slot())
            .and_then(Option::as_ref)
            .filter(|state| state.address == peer && state.owner == owner)
            .is_some_and(|state| {
                state
                    .filter
                    .is_known_duplicate(retry, sequence_control, tid)
            })
    }
}

fn rejected_data(error: DataDecapError) -> ApRxDispatch {
    ApRxDispatch::Rejected(ApRxError::Data(error))
}

fn fragmented_mpdu(mpdu: &[u8]) -> bool {
    let Some(header) = mpdu.get(..24) else {
        return false;
    };
    let frame_control = u16::from_le_bytes([header[0], header[1]]);
    frame_control & 0x000c == 0x0008 && (frame_control & 0x0400 != 0 || header[22] & 0x0f != 0)
}

#[cfg(test)]
mod tests;

//! Connected-station receive dispatch without an executor or platform policy.
//!
//! This module owns the frame-classification and protocol-routing state that
//! used to be duplicated by the ordinary and TX-interleaved HIL receive loops.
//! It deliberately does not log, access unrelated peripherals, enqueue into a
//! network stack or await an executor primitive. Those effects are published
//! through [`ConnectedRxSink`] and belong to the integration runner.

use core::cell::RefCell;

use critical_section::Mutex;
use oer_esp32s31_ieee80211::protected_data_rx::{
    ProtectedDataFragmentRxError, UnprotectedDataFragmentRxError, view_protected_data,
    view_protected_data_fragment, view_unprotected_data, view_unprotected_data_fragment,
};
use oer_esp32s31_ieee80211_dma::rx_ring::RxSegment;
use oer_esp32s31_ieee80211_mac::rx::pool::RxTimes;
use oer_ieee80211_mac::{
    ccmp::{
        CcmpHeader, CcmpKeyId, CcmpReplayError, CcmpReplayLane, CcmpRxReplayCandidate,
        CcmpRxReplayState,
    },
    data::{DataDecapError, DataInterfaceRole, EthernetFrameParts, RxDuplicateFilter},
    extensions::espressif::esp_now::{
        ESP_NOW_ACTION_CATEGORY, ESP_NOW_ORGANIZATION_IDENTIFIER, EspNowVersionError,
        EspNowWireVersion, esp_now_wire_version,
    },
    fragmentation::{
        OPEN_DATA_FRAGMENT_TIMEOUT, OPEN_DATA_REASSEMBLY_CAPACITY, OpenDataDefragmentation,
        OpenDataDefragmenter, OpenDataFragmentError, OpenDataFragmentPreflight,
        OpenDataUnfragmentedAdmission, parse_ccmp_data_identity, parse_open_data_identity,
    },
    management_protection::{SaQuery, is_robust_action_category},
    ndpa::{HeNdpa, HeNdpaError},
    security::LinkProtection,
    station::{StaDisconnect, parse_sta_disconnect},
    station_beacon::{StaBeaconError, StaBeaconObservation, parse_sta_beacon},
    trigger::{TriggerCommonInfo, TriggerParseError, parse_trigger_frame},
    twt::{
        IndividualTwtAction, TwtWireError, is_individual_twt_action_candidate,
        parse_individual_twt_action,
    },
};
use oer_ieee80211_rsn::{EapolKeyFrame, EapolParseError};
use oer_ieee80211_softmac::{
    EspNowPeerId, EspNowReceiveError, EspNowReceivedV1, EspNowReceivedV2, EspNowRxEpoch,
    EspNowRxOutcome, EspNowV2ReceiveError, EspNowV2RxOutcome, MacRxMetadata,
};
#[cfg(test)]
use oer_ieee80211_softmac::{MacRxCryptoStatus, MacRxEvidence};
use static_cell::StaticCell;

use oer_ieee80211_trace::{BeaconDispatch, BeaconVerdict, ControlEventKind};

use oer_esp32s31_ieee80211_mac::{
    rx::ampdu::{
        RxBlockAckMpduKey, RxBlockAckRequestKey, rx_block_ack_mpdu_key, rx_block_ack_request_key,
    },
    rx::{
        PUBLIC_HEADER_SIZE, RxError, RxIngressConfig, RxPhyInfo, decode_normalized_rx_metadata,
        extract_control, extract_management, extract_protected_management,
    },
    tx::ampdu::{BlockAckAction, parse_block_ack_action},
    tx::{HeTriggerScheduledRate, HeTriggerScheduledRateError},
};

use oer_esp32s31_ieee80211::esp_now::normalize_esp_now_rx_metadata;

const TRIGGER_FRAME_CONTROL: u16 = 0x0024;
const NDPA_FRAME_CONTROL: u16 = 0x0054;
const ACTION_FRAME_CONTROL: u16 = 0x00d0;
const PROBE_RESPONSE_FRAME_CONTROL: u16 = 0x0050;
const BEACON_FRAME_CONTROL: u16 = 0x0080;
const DISASSOCIATION_FRAME_CONTROL: u16 = 0x00a0;
const DEAUTHENTICATION_FRAME_CONTROL: u16 = 0x00c0;
const DATA_TYPE_MASK: u16 = 0x000c;
const DATA_TYPE: u16 = 0x0008;
const PROTECTED: u16 = 0x4000;
const QOS_SUBTYPE: u16 = 0x0080;
const TO_FROM_DS: u16 = 0x0300;
const MORE_DATA: u16 = 0x2000;
const MORE_FRAGMENTS: u16 = 0x0400;
const QOS_AMSDU_PRESENT: u8 = 0x80;
const OPEN_FRAGMENT_CONTEXTS: usize = 2;

/// Exact `mcycle` attribution inside the ordinary WPA2 pairwise RX path.
#[cfg(feature = "task-poll-telemetry")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedRxDataCycleProfile {
    pub calls: u32,
    pub completed: u32,
    pub total: u32,
    pub view: u32,
    pub fragment_guard: u32,
    pub decapsulate: u32,
    pub replay: u32,
    pub duplicate: u32,
    pub publish: u32,
}

#[cfg(all(feature = "task-poll-telemetry", target_arch = "riscv32"))]
#[inline(always)]
fn connected_rx_cycle_count() -> u32 {
    riscv::register::mcycle::read() as u32
}

#[cfg(all(feature = "task-poll-telemetry", not(target_arch = "riscv32")))]
#[inline(always)]
fn connected_rx_cycle_count() -> u32 {
    0
}

/// Immutable identity and descriptor-ingress policy for one connected STA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectedRxConfig {
    pub station_address: [u8; 6],
    pub bssid: [u8; 6],
    pub association_id: u16,
    pub ingress: RxIngressConfig,
    pub security: LinkProtection,
    /// Peer-negotiated receive geometry. Open TX may deliberately remain
    /// non-QoS while an HT/WMM AP legitimately sends plaintext QoS Data.
    pub peer_qos: bool,
    /// The association protects its robust management frames.
    pub management_protection: bool,
}

/// Destination class observed before protected-data extraction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxProtection {
    Pairwise,
    Group,
    Other,
}

/// Association-scoped software anti-replay owner for the installed PTK and
/// GTK. It is moved into the one connected RX dispatcher and is never cloned.
/// BlockAck release happens before dispatcher entry, so each per-TID frontier
/// observes sequence-ordered frames even when DMA completion order differed.
#[derive(Debug, Eq, PartialEq)]
pub struct StaCcmpRxReplayEpoch {
    pairwise: CcmpRxReplayState,
    group: CcmpRxReplayState,
    group_key_id: CcmpKeyId,
    group_revision: u32,
}

impl StaCcmpRxReplayEpoch {
    pub fn new(
        pairwise_receive_sequence: [u8; 8],
        group_key_id: u8,
        group_receive_sequence: [u8; 8],
    ) -> Result<Self, StaCcmpRxReplayError> {
        let pairwise = CcmpRxReplayState::from_receive_sequence(pairwise_receive_sequence)
            .map_err(|_| StaCcmpRxReplayError::InvalidPairwiseReceiveSequence)?;
        let group = CcmpRxReplayState::from_receive_sequence(group_receive_sequence)
            .map_err(|_| StaCcmpRxReplayError::InvalidGroupReceiveSequence)?;
        let group_key_id = CcmpKeyId::new(group_key_id)
            .ok_or(StaCcmpRxReplayError::InvalidGroupKeyId(group_key_id))?;
        Ok(Self {
            pairwise,
            group,
            group_key_id,
            group_revision: 0,
        })
    }

    pub const fn group_key_id(&self) -> CcmpKeyId {
        self.group_key_id
    }

    fn prepare(
        &self,
        protection: ConnectedRxProtection,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<StaCcmpRxReplayCandidate, StaCcmpRxReplayError> {
        let lane = match tid {
            Some(tid) => CcmpReplayLane::Tid(tid),
            None => CcmpReplayLane::NonQos,
        };
        match protection {
            ConnectedRxProtection::Pairwise => {
                if header.key_id() != CcmpKeyId::PAIRWISE {
                    return Err(StaCcmpRxReplayError::UnexpectedKeyId {
                        protection,
                        expected: CcmpKeyId::PAIRWISE,
                        observed: header.key_id(),
                    });
                }
                self.pairwise
                    .prepare(lane, header.packet_number())
                    .map(StaCcmpRxReplayCandidate::Pairwise)
                    .map_err(StaCcmpRxReplayError::Replay)
            }
            ConnectedRxProtection::Group => {
                if header.key_id() != self.group_key_id {
                    return Err(StaCcmpRxReplayError::UnexpectedKeyId {
                        protection,
                        expected: self.group_key_id,
                        observed: header.key_id(),
                    });
                }
                self.group
                    .prepare(lane, header.packet_number())
                    .map(StaCcmpRxReplayCandidate::Group)
                    .map_err(StaCcmpRxReplayError::Replay)
            }
            ConnectedRxProtection::Other => Err(StaCcmpRxReplayError::ForeignDestination),
        }
    }

    fn commit(&mut self, candidate: StaCcmpRxReplayCandidate) -> Result<(), StaCcmpRxReplayError> {
        match candidate {
            StaCcmpRxReplayCandidate::Pairwise(candidate) => self
                .pairwise
                .commit(candidate)
                .map_err(StaCcmpRxReplayError::Replay),
            StaCcmpRxReplayCandidate::Group(candidate) => self
                .group
                .commit(candidate)
                .map_err(StaCcmpRxReplayError::Replay),
        }
    }
}

/// Group-only replay state shared with GTK rotation. Pairwise lanes are moved
/// into the unique RX endpoint and therefore need no per-MPDU mutex.
struct StaCcmpSharedGroupReplayEpoch {
    group: CcmpRxReplayState,
    group_key_id: CcmpKeyId,
    group_revision: u32,
}

impl StaCcmpSharedGroupReplayEpoch {
    fn merge_group_frontiers(
        &self,
        group: &mut CcmpRxReplayState,
    ) -> Result<(), StaCcmpRxReplayError> {
        for lane in core::iter::once(CcmpReplayLane::NonQos).chain((0..16).map(CcmpReplayLane::Tid))
        {
            let current = self
                .group
                .highest(lane)
                .ok_or(StaCcmpRxReplayError::Replay(CcmpReplayError::InvalidTid))?;
            let incoming = group
                .highest(lane)
                .ok_or(StaCcmpRxReplayError::Replay(CcmpReplayError::InvalidTid))?;
            if current > incoming {
                let candidate = group
                    .prepare(lane, current)
                    .map_err(StaCcmpRxReplayError::Replay)?;
                group
                    .commit(candidate)
                    .map_err(StaCcmpRxReplayError::Replay)?;
            }
        }
        Ok(())
    }

    fn prepare_group_rotation(
        &self,
        group_key_id: u8,
        group_receive_sequence: [u8; 8],
    ) -> Result<StaCcmpGroupReplayReplacement, StaCcmpRxReplayError> {
        let mut group = CcmpRxReplayState::from_receive_sequence(group_receive_sequence)
            .map_err(|_| StaCcmpRxReplayError::InvalidGroupReceiveSequence)?;
        let group_key_id = CcmpKeyId::new(group_key_id)
            .ok_or(StaCcmpRxReplayError::InvalidGroupKeyId(group_key_id))?;
        if group_key_id == self.group_key_id {
            self.merge_group_frontiers(&mut group)?;
        }
        Ok(StaCcmpGroupReplayReplacement {
            expected_key_id: self.group_key_id,
            expected_revision: self.group_revision,
            group,
            group_key_id,
        })
    }

    fn commit_group_rotation(
        &mut self,
        replacement: StaCcmpGroupReplayReplacement,
    ) -> Result<(), StaCcmpRxReplayError> {
        if self.group_key_id != replacement.expected_key_id
            || self.group_revision != replacement.expected_revision
        {
            return Err(StaCcmpRxReplayError::StaleGroupRotation);
        }
        let revision = self
            .group_revision
            .checked_add(1)
            .ok_or(StaCcmpRxReplayError::GroupRevisionExhausted)?;
        self.group = replacement.group;
        self.group_key_id = replacement.group_key_id;
        self.group_revision = revision;
        Ok(())
    }

    fn prepare_group(
        &self,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<StaCcmpRxReplayCandidate, StaCcmpRxReplayError> {
        if header.key_id() != self.group_key_id {
            return Err(StaCcmpRxReplayError::UnexpectedKeyId {
                protection: ConnectedRxProtection::Group,
                expected: self.group_key_id,
                observed: header.key_id(),
            });
        }
        let lane = match tid {
            Some(tid) => CcmpReplayLane::Tid(tid),
            None => CcmpReplayLane::NonQos,
        };
        self.group
            .prepare(lane, header.packet_number())
            .map(StaCcmpRxReplayCandidate::Group)
            .map_err(StaCcmpRxReplayError::Replay)
    }

    fn commit_group(
        &mut self,
        candidate: StaCcmpRxReplayCandidate,
    ) -> Result<(), StaCcmpRxReplayError> {
        let StaCcmpRxReplayCandidate::Group(candidate) = candidate else {
            return Err(StaCcmpRxReplayError::ForeignDestination);
        };
        self.group
            .commit(candidate)
            .map_err(StaCcmpRxReplayError::Replay)
    }
}

#[derive(Debug, Eq, PartialEq)]
struct StaCcmpGroupReplayReplacement {
    expected_key_id: CcmpKeyId,
    expected_revision: u32,
    group: CcmpRxReplayState,
    group_key_id: CcmpKeyId,
}

/// Static association replay arena shared by the finite connected-control and
/// RX protocol owners.
///
/// The endpoints returned by [`Self::start`] are generation-bound and
/// non-copyable. Group-key replacement first quarantines group publication;
/// pairwise replay remains live. A group RX permit keeps an in-flight
/// publication visible until the synchronous sink callback has returned.
pub struct StaCcmpRxReplayResource {
    state: Mutex<RefCell<StaCcmpRxReplayResourceState>>,
    pairwise_arena: StaticCell<StaCcmpPairwiseReplaySlot>,
}

struct StaCcmpPairwiseReplaySlot {
    resource: &'static StaCcmpRxReplayResource,
    replay: CcmpRxReplayState,
}

struct StaCcmpRxReplayResourceState {
    generation: u32,
    next_rotation_ticket: u32,
    replay: Option<StaCcmpSharedGroupReplayEpoch>,
    pending_rotation: Option<u32>,
    group_publications: u32,
    rx_active: bool,
    control_active: bool,
    group_quarantined: bool,
    pairwise: Option<&'static mut StaCcmpPairwiseReplaySlot>,
    pairwise_initialized: bool,
}

impl StaCcmpRxReplayResource {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(StaCcmpRxReplayResourceState {
                generation: 0,
                next_rotation_ticket: 1,
                replay: None,
                pending_rotation: None,
                group_publications: 0,
                rx_active: false,
                control_active: false,
                group_quarantined: false,
                pairwise: None,
                pairwise_initialized: false,
            })),
            pairwise_arena: StaticCell::new(),
        }
    }

    /// Start one disjoint association epoch in statically located storage.
    #[expect(
        clippy::result_large_err,
        reason = "the no-alloc failure must return the exact affine replay epoch for hardware teardown"
    )]
    pub fn start(
        &'static self,
        replay: StaCcmpRxReplayEpoch,
    ) -> Result<
        (StaCcmpRxReplayRxEndpoint, StaCcmpRxReplayControlEndpoint),
        StaCcmpRxReplayStartFailure,
    > {
        critical_section::with(|cs| {
            let mut state = self.state.borrow(cs).borrow_mut();
            if state.replay.is_some() || state.rx_active || state.control_active {
                return Err(StaCcmpRxReplayStartFailure {
                    error: StaCcmpRxReplayStartError::Busy,
                    replay,
                });
            }
            let Some(generation) = state.generation.checked_add(1) else {
                return Err(StaCcmpRxReplayStartFailure {
                    error: StaCcmpRxReplayStartError::GenerationExhausted,
                    replay,
                });
            };
            state.generation = generation;
            let StaCcmpRxReplayEpoch {
                pairwise,
                group,
                group_key_id,
                group_revision,
            } = replay;
            let pairwise = if state.pairwise_initialized {
                let slot = state
                    .pairwise
                    .take()
                    .expect("an idle replay resource owns its pairwise arena");
                slot.replay = pairwise;
                slot
            } else {
                state.pairwise_initialized = true;
                self.pairwise_arena.init(StaCcmpPairwiseReplaySlot {
                    resource: self,
                    replay: pairwise,
                })
            };
            state.replay = Some(StaCcmpSharedGroupReplayEpoch {
                group,
                group_key_id,
                group_revision,
            });
            state.pending_rotation = None;
            state.group_publications = 0;
            state.rx_active = true;
            state.control_active = true;
            state.group_quarantined = false;
            let generation = state.generation;
            Ok((
                StaCcmpRxReplayRxEndpoint {
                    pairwise: Some(pairwise),
                },
                StaCcmpRxReplayControlEndpoint {
                    resource: self,
                    generation,
                    stopped: false,
                },
            ))
        })
    }

    fn release_if_stopped(state: &mut StaCcmpRxReplayResourceState) {
        if !state.rx_active && !state.control_active && state.group_publications == 0 {
            debug_assert!(state.pairwise.is_some());
            state.replay = None;
            state.pending_rotation = None;
            state.group_quarantined = false;
        }
    }
}

impl Default for StaCcmpRxReplayResource {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaCcmpRxReplayStartError {
    Busy,
    GenerationExhausted,
}

/// Owner-preserving failure to publish an association replay epoch.
///
/// Hardware key teardown still needs the exact replay owner on every rejected
/// start edge, so the input is never consumed into a bare status code.
#[derive(Debug, Eq, PartialEq)]
pub struct StaCcmpRxReplayStartFailure {
    pub error: StaCcmpRxReplayStartError,
    replay: StaCcmpRxReplayEpoch,
}

impl StaCcmpRxReplayStartFailure {
    pub fn into_parts(self) -> (StaCcmpRxReplayStartError, StaCcmpRxReplayEpoch) {
        (self.error, self.replay)
    }
}

/// RX half of one shared replay epoch. It is moved into exactly one connected
/// dispatcher and explicitly stopped after that protocol task is quiescent.
pub struct StaCcmpRxReplayRxEndpoint {
    pairwise: Option<&'static mut StaCcmpPairwiseReplaySlot>,
}

impl core::fmt::Debug for StaCcmpRxReplayRxEndpoint {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("StaCcmpRxReplayRxEndpoint")
            .field("stopped", &self.pairwise.is_none())
            .finish_non_exhaustive()
    }
}

impl PartialEq for StaCcmpRxReplayRxEndpoint {
    fn eq(&self, other: &Self) -> bool {
        match (self.pairwise.as_deref(), other.pairwise.as_deref()) {
            (Some(left), Some(right)) => core::ptr::eq(left, right),
            (None, None) => true,
            _ => false,
        }
    }
}

impl Eq for StaCcmpRxReplayRxEndpoint {}

impl StaCcmpRxReplayRxEndpoint {
    pub const fn vacant() -> Self {
        Self { pairwise: None }
    }

    pub const fn is_live(&self) -> bool {
        self.pairwise.is_some()
    }

    fn resource(&self) -> Result<&'static StaCcmpRxReplayResource, StaCcmpRxReplayError> {
        self.pairwise
            .as_deref()
            .map(|slot| slot.resource)
            .ok_or(StaCcmpRxReplayError::StaleEpoch)
    }

    /// Advance one authenticated pairwise lane in a single replay-owner
    /// transaction. Group publication and GTK rotation retain their split
    /// prepare/commit protocol because they cross a publication boundary.
    #[inline(always)]
    fn commit_pairwise_immediate(
        &mut self,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<(), StaCcmpRxReplayError> {
        if header.key_id() != CcmpKeyId::PAIRWISE {
            return Err(StaCcmpRxReplayError::UnexpectedKeyId {
                protection: ConnectedRxProtection::Pairwise,
                expected: CcmpKeyId::PAIRWISE,
                observed: header.key_id(),
            });
        }
        let lane = tid.map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
        self.pairwise
            .as_deref_mut()
            .ok_or(StaCcmpRxReplayError::StaleEpoch)?
            .replay
            .commit_immediate(lane, header.packet_number())
            .map_err(StaCcmpRxReplayError::Replay)
    }

    fn prepare_pairwise(
        &self,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<StaCcmpRxReplayCandidate, StaCcmpRxReplayError> {
        if header.key_id() != CcmpKeyId::PAIRWISE {
            return Err(StaCcmpRxReplayError::UnexpectedKeyId {
                protection: ConnectedRxProtection::Pairwise,
                expected: CcmpKeyId::PAIRWISE,
                observed: header.key_id(),
            });
        }
        let lane = tid.map_or(CcmpReplayLane::NonQos, CcmpReplayLane::Tid);
        self.pairwise
            .as_deref()
            .ok_or(StaCcmpRxReplayError::StaleEpoch)?
            .replay
            .prepare(lane, header.packet_number())
            .map(StaCcmpRxReplayCandidate::Pairwise)
            .map_err(StaCcmpRxReplayError::Replay)
    }

    fn commit_pairwise(
        &mut self,
        candidate: StaCcmpRxReplayCandidate,
    ) -> Result<(), StaCcmpRxReplayError> {
        let StaCcmpRxReplayCandidate::Pairwise(candidate) = candidate else {
            return Err(StaCcmpRxReplayError::ForeignDestination);
        };
        self.pairwise
            .as_deref_mut()
            .ok_or(StaCcmpRxReplayError::StaleEpoch)?
            .replay
            .commit(candidate)
            .map_err(StaCcmpRxReplayError::Replay)
    }

    fn prepare_candidate(
        &self,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<StaCcmpPreparedRxPublication, StaCcmpRxReplayError> {
        let resource = self.resource()?;
        critical_section::with(|cs| {
            let mut state = resource.state.borrow(cs).borrow_mut();
            if !state.rx_active || self.pairwise.is_none() {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            if state.group_quarantined || state.pending_rotation.is_some() {
                return Err(StaCcmpRxReplayError::GroupRotationInProgress);
            }
            let replay = state
                .replay
                .as_ref()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?;
            let candidate = replay.prepare_group(tid, header)?;
            state.group_publications = state
                .group_publications
                .checked_add(1)
                .ok_or(StaCcmpRxReplayError::PublicationCountExhausted)?;
            Ok(StaCcmpPreparedRxPublication {
                resource,
                generation: state.generation,
                candidate,
                group_gate_armed: true,
            })
        })
    }

    #[cfg(test)]
    fn prepare_publication(
        &mut self,
        protection: ConnectedRxProtection,
        tid: Option<u8>,
        header: CcmpHeader,
    ) -> Result<StaCcmpRxPublicationPermit, StaCcmpRxReplayError> {
        match protection {
            ConnectedRxProtection::Pairwise => {
                let candidate = self.prepare_pairwise(tid, header)?;
                let resource = self.resource()?;
                let generation =
                    critical_section::with(|cs| resource.state.borrow(cs).borrow().generation);
                self.commit_pairwise(candidate)?;
                Ok(StaCcmpRxPublicationPermit {
                    resource,
                    generation,
                    group: false,
                })
            }
            ConnectedRxProtection::Group => self.prepare_candidate(tid, header)?.commit(),
            ConnectedRxProtection::Other => Err(StaCcmpRxReplayError::ForeignDestination),
        }
    }

    pub fn stop(&mut self) -> Result<(), StaCcmpRxReplayError> {
        let resource = self.resource()?;
        critical_section::with(|cs| {
            let mut state = resource.state.borrow(cs).borrow_mut();
            if !state.rx_active || self.pairwise.is_none() {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            if state.group_publications != 0 {
                state.group_quarantined = true;
                return Err(StaCcmpRxReplayError::PublicationInFlight);
            }
            let pairwise = self
                .pairwise
                .take()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?;
            if state.pairwise.is_some() {
                self.pairwise = Some(pairwise);
                return Err(StaCcmpRxReplayError::OwnerUnavailable);
            }
            state.pairwise = Some(pairwise);
            state.rx_active = false;
            state.group_quarantined = true;
            StaCcmpRxReplayResource::release_if_stopped(&mut state);
            Ok(())
        })
    }
}

impl Drop for StaCcmpRxReplayRxEndpoint {
    fn drop(&mut self) {
        let Some(resource) = self.pairwise.as_deref().map(|slot| slot.resource) else {
            return;
        };
        critical_section::with(|cs| {
            let mut state = resource.state.borrow(cs).borrow_mut();
            if state.pairwise.is_none() {
                state.pairwise = self.pairwise.take();
            }
            state.rx_active = false;
            state.group_quarantined = true;
            StaCcmpRxReplayResource::release_if_stopped(&mut state);
        });
    }
}

/// Control half of one shared replay epoch.
pub struct StaCcmpRxReplayControlEndpoint {
    resource: &'static StaCcmpRxReplayResource,
    generation: u32,
    stopped: bool,
}

impl StaCcmpRxReplayControlEndpoint {
    pub fn prepare_group_rotation(
        &self,
        group_key_id: u8,
        group_receive_sequence: [u8; 8],
    ) -> Result<StaCcmpPreparedGroupRotation, StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation != self.generation || !state.control_active || self.stopped {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            if !state.rx_active {
                return Err(StaCcmpRxReplayError::RxStopped);
            }
            if state.group_quarantined || state.pending_rotation.is_some() {
                return Err(StaCcmpRxReplayError::GroupRotationInProgress);
            }
            let replacement = state
                .replay
                .as_ref()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?
                .prepare_group_rotation(group_key_id, group_receive_sequence)?;
            let ticket = state.next_rotation_ticket;
            state.next_rotation_ticket = match state.next_rotation_ticket.checked_add(1) {
                Some(next) => next,
                None => {
                    state.group_quarantined = true;
                    return Err(StaCcmpRxReplayError::GroupRotationTicketExhausted);
                }
            };
            Ok(StaCcmpPreparedGroupRotation {
                generation: self.generation,
                ticket,
                replacement,
            })
        })
    }

    pub fn begin_group_rotation(
        &self,
        mut prepared: StaCcmpPreparedGroupRotation,
    ) -> Result<StaCcmpInstallingGroupRotation, StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if prepared.generation != self.generation
                || state.generation != self.generation
                || !state.control_active
            {
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            if !state.rx_active {
                return Err(StaCcmpRxReplayError::RxStopped);
            }
            if state.group_publications != 0 {
                return Err(StaCcmpRxReplayError::PublicationInFlight);
            }
            if state.group_quarantined || state.pending_rotation.is_some() {
                return Err(StaCcmpRxReplayError::GroupRotationInProgress);
            }
            let replay = state
                .replay
                .as_ref()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?;
            if replay.group_key_id != prepared.replacement.expected_key_id
                || replay.group_revision != prepared.replacement.expected_revision
            {
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            if replay.group_key_id == prepared.replacement.group_key_id {
                // Group RX may have advanced one or more lane frontiers after
                // prepare returned. Refresh the same-KeyID maximum only after
                // the publication gate is known to be clear, while retaining
                // the prepared revision check above so a completed rotation
                // still makes this candidate stale.
                replay.merge_group_frontiers(&mut prepared.replacement.group)?;
            }
            state.pending_rotation = Some(prepared.ticket);
            Ok(StaCcmpInstallingGroupRotation { prepared })
        })
    }

    pub fn commit_group_rotation(
        &self,
        installing: StaCcmpInstallingGroupRotation,
    ) -> Result<(), StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if installing.prepared.generation != self.generation
                || state.generation != self.generation
            {
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            if state.pending_rotation != Some(installing.prepared.ticket) || !state.control_active {
                state.group_quarantined = true;
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            if !state.rx_active || state.group_publications != 0 {
                state.group_quarantined = true;
                return Err(if state.rx_active {
                    StaCcmpRxReplayError::PublicationInFlight
                } else {
                    StaCcmpRxReplayError::RxStopped
                });
            }
            state
                .replay
                .as_mut()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?
                .commit_group_rotation(installing.prepared.replacement)?;
            state.pending_rotation = None;
            Ok(())
        })
    }

    /// Abort after hardware restored the exact old GTK. Replay state was not
    /// changed by prepare/begin, so clearing the gate republishes one coherent
    /// old key+frontier epoch.
    pub fn abort_group_rotation(
        &self,
        installing: StaCcmpInstallingGroupRotation,
    ) -> Result<(), StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if installing.prepared.generation != self.generation
                || state.generation != self.generation
            {
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            if state.pending_rotation != Some(installing.prepared.ticket) {
                state.group_quarantined = true;
                return Err(StaCcmpRxReplayError::StaleGroupRotation);
            }
            state.pending_rotation = None;
            Ok(())
        })
    }

    /// Keep group RX permanently closed after rollback could not re-establish
    /// either complete key+replay epoch. Pairwise replay remains intact until
    /// outer disconnect teardown quiesces the association.
    pub fn quarantine_group_rotation(&self, installing: StaCcmpInstallingGroupRotation) {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation == self.generation
                && state.pending_rotation == Some(installing.prepared.ticket)
            {
                state.pending_rotation = None;
            }
            if state.generation == self.generation {
                state.group_quarantined = true;
            }
        });
    }

    pub fn stop(&mut self) -> Result<(), StaCcmpRxReplayError> {
        let result = critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation != self.generation || !state.control_active {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            state.control_active = false;
            if state.pending_rotation.is_some() {
                state.group_quarantined = true;
            }
            StaCcmpRxReplayResource::release_if_stopped(&mut state);
            Ok(())
        });
        if result.is_ok() {
            self.stopped = true;
        }
        result
    }
}

impl Drop for StaCcmpRxReplayControlEndpoint {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation == self.generation {
                state.control_active = false;
                state.group_quarantined = true;
                StaCcmpRxReplayResource::release_if_stopped(&mut state);
            }
        });
    }
}

/// Prepared replay candidate. It has no side effect and becomes stale after
/// any other candidate begins.
pub struct StaCcmpPreparedGroupRotation {
    generation: u32,
    ticket: u32,
    replacement: StaCcmpGroupReplayReplacement,
}

/// Group-publication gate held across the hardware replacement transaction.
pub struct StaCcmpInstallingGroupRotation {
    prepared: StaCcmpPreparedGroupRotation,
}

struct StaCcmpRxPublicationPermit {
    resource: &'static StaCcmpRxReplayResource,
    generation: u32,
    group: bool,
}

struct StaCcmpPreparedRxPublication {
    resource: &'static StaCcmpRxReplayResource,
    generation: u32,
    candidate: StaCcmpRxReplayCandidate,
    group_gate_armed: bool,
}

impl StaCcmpPreparedRxPublication {
    fn commit(mut self) -> Result<StaCcmpRxPublicationPermit, StaCcmpRxReplayError> {
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation != self.generation || !state.rx_active {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            let replay = state
                .replay
                .as_mut()
                .ok_or(StaCcmpRxReplayError::OwnerUnavailable)?;
            replay.commit_group(self.candidate)?;
            self.group_gate_armed = false;
            Ok(StaCcmpRxPublicationPermit {
                resource: self.resource,
                generation: self.generation,
                group: true,
            })
        })
    }
}

impl Drop for StaCcmpPreparedRxPublication {
    fn drop(&mut self) {
        if !self.group_gate_armed {
            return;
        }
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation == self.generation {
                state.group_publications = state
                    .group_publications
                    .checked_sub(1)
                    .expect("a prepared group replay candidate decrements exactly once");
                StaCcmpRxReplayResource::release_if_stopped(&mut state);
            }
        });
        self.group_gate_armed = false;
    }
}

impl Drop for StaCcmpRxPublicationPermit {
    fn drop(&mut self) {
        if !self.group {
            return;
        }
        critical_section::with(|cs| {
            let mut state = self.resource.state.borrow(cs).borrow_mut();
            if state.generation == self.generation {
                state.group_publications = state
                    .group_publications
                    .checked_sub(1)
                    .expect("a group publication permit decrements exactly once");
                StaCcmpRxReplayResource::release_if_stopped(&mut state);
            }
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaCcmpRxReplayCandidate {
    Pairwise(CcmpRxReplayCandidate),
    Group(CcmpRxReplayCandidate),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaCcmpRxReplayError {
    InvalidPairwiseReceiveSequence,
    InvalidGroupReceiveSequence,
    InvalidGroupKeyId(u8),
    OwnerUnavailable,
    StaleEpoch,
    StaleGroupRotation,
    GroupRotationInProgress,
    PublicationInFlight,
    PublicationCountExhausted,
    GroupRotationTicketExhausted,
    GroupRevisionExhausted,
    RxStopped,
    ForeignDestination,
    UnexpectedKeyId {
        protection: ConnectedRxProtection,
        expected: CcmpKeyId,
        observed: CcmpKeyId,
    },
    Replay(CcmpReplayError),
}

/// MAC identity admitted for an associated HE control exchange.
///
/// Construction remains private to [`ConnectedRxDispatcher`]: a published
/// value proves that the transmitter was the associated BSSID and that the
/// receiver was either this station or an IEEE group address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AssociatedHeControlIdentity {
    duration: u16,
    receiver_address: [u8; 6],
    transmitter_address: [u8; 6],
}

impl AssociatedHeControlIdentity {
    pub const fn duration(self) -> u16 {
        self.duration
    }

    pub const fn receiver_address(self) -> [u8; 6] {
        self.receiver_address
    }

    pub const fn transmitter_address(self) -> [u8; 6] {
        self.transmitter_address
    }
}

/// One semantic event emitted by the connected frame dispatcher.
///
/// Borrowed frame slices remain valid only for the duration of
/// [`ConnectedRxSink::publish`]. A network adapter must copy or otherwise
/// transfer them into storage that it owns before returning.
#[derive(Clone, Copy, Debug)]
pub enum ConnectedRxEvent<'frame> {
    Beacon {
        observation: StaBeaconObservation,
        metadata: MacRxMetadata<RxPhyInfo>,
        /// The frame's receive timestamp: the raw MAC local time of its
        /// reception.
        stamp: Option<u32>,
    },
    ProbeResponse,
    Trigger {
        identity: AssociatedHeControlIdentity,
        common: TriggerCommonInfo,
        schedule: Result<HeTriggerScheduledRate, HeTriggerScheduledRateError>,
        first_user: Option<[u8; 5]>,
        handoff: Option<oer_time::Instant>,
    },
    Ndpa {
        identity: AssociatedHeControlIdentity,
        dialog_token: u8,
        addressed_to_station: bool,
        handoff: Option<oer_time::Instant>,
    },
    BlockAck {
        action: BlockAckAction,
        body: &'frame [u8],
    },
    /// Strictly decoded individual TWT Setup or Teardown action from the
    /// associated AP. The fixed action body is copied into the control lane.
    IndividualTwt {
        action: IndividualTwtAction,
        body: &'frame [u8],
    },
    /// Strictly decoded plaintext ESP-NOW datagram from one configured peer.
    ///
    /// `received` borrows the normalized MPDU and must be copied by a sink
    /// which retains it after [`ConnectedRxSink::publish`] returns.
    EspNow {
        received: EspNowReceivedV1<'frame>,
        metadata: MacRxMetadata<RxPhyInfo>,
    },
    /// One plaintext EAPOL-Key packet on an otherwise protected station link.
    ///
    /// This event is constructed only by the narrow connected WPA2 admission
    /// path. Ordinary plaintext Data, A-MSDU, foreign addresses and non-EAPOL
    /// LLC payloads never reach a sink.
    UnprotectedEapol {
        source: [u8; 6],
        payload: &'frame [u8],
    },
    PeerDisconnect(StaDisconnect),
    /// Under management frame protection, an unprotected Deauthentication
    /// or Disassociation for a class 2 or 3 frame (reason 6 or 7): the
    /// station confirms its association with an SA Query before it believes
    /// it.
    UnprotectedDisconnect(StaDisconnect),
    /// A robust management frame of an association that protects them:
    /// CCMP-protected when individually addressed, carrying a Management MIC
    /// element when `group`. The sink copies it for control to open.
    ProtectedManagement {
        frame: &'frame [u8],
        group: bool,
    },
    /// One admitted data frame of the access point while the station runs
    /// in power save. A fragmented Open MSDU reaches this edge only after
    /// complete reassembly.
    PowerSaveData(PowerSaveData),
    Ethernet {
        frame: EthernetFrameParts<'frame>,
        raw: &'frame [u8],
        amsdu: bool,
        metadata: MacRxMetadata<RxPhyInfo>,
    },
}

/// A beacon of the associated access point and its receive timestamp, the
/// raw MAC local time of its reception.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceivedBeacon {
    pub observation: StaBeaconObservation,
    pub stamp: Option<u32>,
}

/// Owned connected-station event that may cross the lifetime of one staged
/// RX frame.
///
/// Ethernet payload is intentionally absent: it must be copied or transferred
/// to the network queue while [`ConnectedRxEvent::Ethernet`] is borrowed.
/// BlockAck carries its complete parsed fixed fields, so no management-frame
/// body or C-style context pointer needs to survive dispatch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxControlEvent {
    Beacon(ReceivedBeacon),
    ProbeResponse,
    Trigger {
        identity: AssociatedHeControlIdentity,
        common: TriggerCommonInfo,
        schedule: Result<HeTriggerScheduledRate, HeTriggerScheduledRateError>,
        first_user: Option<[u8; 5]>,
        handoff: Option<oer_time::Instant>,
    },
    Ndpa {
        identity: AssociatedHeControlIdentity,
        dialog_token: u8,
        addressed_to_station: bool,
        handoff: Option<oer_time::Instant>,
    },
    BlockAck(BlockAckAction),
    IndividualTwt(IndividualTwtAction),
    PeerDisconnect(StaDisconnect),
    /// An unprotected disconnect of an association that protects its
    /// management frames; control confirms it with an SA Query.
    UnprotectedDisconnect(StaDisconnect),
    /// An SA Query Action control opened from a protected frame.
    SaQuery(SaQuery),
    PowerSaveData(PowerSaveData),
}

/// The part of an admitted data frame station power management reads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerSaveData {
    /// The frame is group addressed.
    pub group: bool,
    /// The access point buffers more frames for the station.
    pub more_data: bool,
}

impl From<&ConnectedRxControlEvent> for ControlEventKind {
    fn from(event: &ConnectedRxControlEvent) -> Self {
        match event {
            ConnectedRxControlEvent::Beacon(_) => Self::Beacon,
            ConnectedRxControlEvent::ProbeResponse => Self::ProbeResponse,
            ConnectedRxControlEvent::Trigger { .. } => Self::Trigger,
            ConnectedRxControlEvent::Ndpa { .. } => Self::Ndpa,
            ConnectedRxControlEvent::BlockAck(_) => Self::BlockAck,
            ConnectedRxControlEvent::IndividualTwt(_) => Self::IndividualTwt,
            ConnectedRxControlEvent::PeerDisconnect(_) => Self::PeerDisconnect,
            ConnectedRxControlEvent::UnprotectedDisconnect(_) => Self::UnprotectedDisconnect,
            ConnectedRxControlEvent::SaQuery(_) => Self::SaQuery,
            ConnectedRxControlEvent::PowerSaveData(_) => Self::PowerSaveData,
        }
    }
}

impl ConnectedRxEvent<'_> {
    /// Copy only the protocol/control information that is independent of the
    /// staged RX allocation.
    pub const fn control(self) -> Option<ConnectedRxControlEvent> {
        match self {
            Self::Beacon {
                observation, stamp, ..
            } => Some(ConnectedRxControlEvent::Beacon(ReceivedBeacon {
                observation,
                stamp,
            })),
            Self::ProbeResponse => Some(ConnectedRxControlEvent::ProbeResponse),
            Self::Trigger {
                identity,
                common,
                schedule,
                first_user,
                handoff,
            } => Some(ConnectedRxControlEvent::Trigger {
                identity,
                common,
                schedule,
                first_user,
                handoff,
            }),
            Self::Ndpa {
                identity,
                dialog_token,
                addressed_to_station,
                handoff,
            } => Some(ConnectedRxControlEvent::Ndpa {
                identity,
                dialog_token,
                addressed_to_station,
                handoff,
            }),
            Self::BlockAck { action, .. } => Some(ConnectedRxControlEvent::BlockAck(action)),
            Self::IndividualTwt { action, .. } => {
                Some(ConnectedRxControlEvent::IndividualTwt(action))
            }
            Self::EspNow { .. } => None,
            Self::UnprotectedEapol { .. } => None,
            Self::PeerDisconnect(disconnect) => {
                Some(ConnectedRxControlEvent::PeerDisconnect(disconnect))
            }
            // Unauthenticated until control opens or confirms them; the
            // sink copies these into their own coalescing lane.
            Self::UnprotectedDisconnect(_) | Self::ProtectedManagement { .. } => None,
            Self::PowerSaveData(data) => Some(ConnectedRxControlEvent::PowerSaveData(data)),
            Self::Ethernet { .. } => None,
        }
    }
}

/// Integration boundary for network delivery, diagnostics and PAC effects.
pub trait ConnectedRxSink {
    fn publish(&mut self, event: ConnectedRxEvent<'_>);

    #[cfg(feature = "task-poll-telemetry")]
    fn observe_data_cycle_profile(&mut self, _profile: ConnectedRxDataCycleProfile) {}

    /// Return whether station power management currently reads data frames:
    /// only while the station advertises power save.
    ///
    /// Ordinary active-mode traffic must not pay the publication and mailbox
    /// cost for an event with no consumer. Test and simple observer sinks keep
    /// the conservative default so their event contract is unchanged.
    fn wants_power_save_data(&self) -> bool {
        true
    }

    /// Opt in only when the sink copies/reassembles v2 before dispatch returns.
    fn supports_esp_now_v2(&self) -> bool {
        false
    }

    fn publish_esp_now_v2(
        &mut self,
        _received: EspNowReceivedV2<'_>,
        _metadata: MacRxMetadata<RxPhyInfo>,
    ) {
    }
}

/// Closed reason for a frame that reached the connected dispatcher but could
/// not produce a semantic event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxError {
    PublicHeader,
    Rx(RxError),
    Trigger(TriggerParseError),
    Ndpa(HeNdpaError),
    Beacon(StaBeaconError),
    /// The associated BSS's beacon carries a TSF the station cannot follow
    /// (`beacon_tsf_followable`); nothing of the beacon is published.
    UnfollowableBeaconTsf,
    IndividualTwt(TwtWireError),
    EspNow(EspNowReceiveError),
    EspNowV2(EspNowV2ReceiveError),
    EspNowVersion(EspNowVersionError),
    EspNowV2SinkUnavailable,
    Data(DataDecapError),
    Eapol(EapolParseError),
    SecurityModeMismatch,
    PeerQosMismatch,
    CcmpReplay(StaCcmpRxReplayError),
    Fragment(OpenDataFragmentError),
}

/// Result of consuming one independently owned staged RX frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRxDispatch {
    Beacon,
    ProbeResponse,
    Trigger,
    Ndpa,
    BlockAck,
    /// A BlockAckReq moved, or left unchanged, a receive reorder window.
    BlockAckRequest,
    IndividualTwt,
    EspNow {
        peer: EspNowPeerId,
    },
    EspNowDuplicate {
        peer: EspNowPeerId,
    },
    EspNowV2 {
        peer: EspNowPeerId,
    },
    EspNowV2Duplicate {
        peer: EspNowPeerId,
    },
    PeerDisconnect,
    /// A robust management frame went to control to open or verify.
    ProtectedManagement,
    /// An unprotected disconnect went to control for an SA Query.
    UnprotectedDisconnect,
    UnprotectedEapol,
    Data {
        ethernet_frames: u8,
        amsdu: bool,
    },
    FragmentBuffered {
        expired: u8,
        evicted: bool,
    },
    Duplicate,
    Ignored,
    Rejected {
        protection: ConnectedRxProtection,
        error: ConnectedRxError,
    },
}

/// Unique protocol-routing state for one connected station.
pub struct ConnectedRxDispatcher {
    config: ConnectedRxConfig,
    duplicate_filter: RxDuplicateFilter,
    shared_ccmp_replay: StaCcmpRxReplayRxEndpoint,
    owned_ccmp_replay: Option<StaCcmpRxReplayEpoch>,
    esp_now: Option<EspNowRxEpoch>,
    fragments: OpenDataDefragmenter<OPEN_FRAGMENT_CONTEXTS, OPEN_DATA_REASSEMBLY_CAPACITY>,
    fragment_admission_active: bool,
}

enum StaCcmpPreparedReplay {
    Owned(StaCcmpRxReplayCandidate),
    SharedPairwise(StaCcmpRxReplayCandidate),
    SharedGroup(StaCcmpPreparedRxPublication),
}

fn prepare_ccmp_replay(
    shared: &mut StaCcmpRxReplayRxEndpoint,
    owned: &mut Option<StaCcmpRxReplayEpoch>,
    protection: ConnectedRxProtection,
    tid: Option<u8>,
    header: CcmpHeader,
) -> Result<StaCcmpPreparedReplay, StaCcmpRxReplayError> {
    if shared.is_live() {
        return match protection {
            ConnectedRxProtection::Pairwise => shared
                .prepare_pairwise(tid, header)
                .map(StaCcmpPreparedReplay::SharedPairwise),
            ConnectedRxProtection::Group => shared
                .prepare_candidate(tid, header)
                .map(StaCcmpPreparedReplay::SharedGroup),
            ConnectedRxProtection::Other => Err(StaCcmpRxReplayError::ForeignDestination),
        };
    }
    match owned.as_mut() {
        Some(replay) => replay
            .prepare(protection, tid, header)
            .map(StaCcmpPreparedReplay::Owned),
        None => Err(StaCcmpRxReplayError::OwnerUnavailable),
    }
}

fn commit_ccmp_replay(
    shared: &mut StaCcmpRxReplayRxEndpoint,
    owned: &mut Option<StaCcmpRxReplayEpoch>,
    prepared: StaCcmpPreparedReplay,
) -> Result<Option<StaCcmpRxPublicationPermit>, StaCcmpRxReplayError> {
    match prepared {
        StaCcmpPreparedReplay::Owned(candidate) => {
            let Some(replay) = owned.as_mut() else {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            };
            replay.commit(candidate)?;
            Ok(None)
        }
        StaCcmpPreparedReplay::SharedPairwise(candidate) => {
            if !shared.is_live() {
                return Err(StaCcmpRxReplayError::StaleEpoch);
            }
            shared.commit_pairwise(candidate)?;
            Ok(None)
        }
        StaCcmpPreparedReplay::SharedGroup(prepared) => prepared.commit().map(Some),
    }
}

impl ConnectedRxDispatcher {
    /// Empty identity used only while a caller-owned protocol arena is parked.
    ///
    /// Frames must not be dispatched through this value. Call
    /// [`Self::try_reconfigure`] at the connected-epoch start edge before
    /// installing replay or ESP-NOW owners.
    pub const fn unconfigured() -> Self {
        Self::new(ConnectedRxConfig {
            station_address: [0; 6],
            bssid: [0; 6],
            association_id: 0,
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            security: LinkProtection::Ccmp,
            peer_qos: true,
            management_protection: false,
        })
    }

    pub const fn new(config: ConnectedRxConfig) -> Self {
        Self {
            config,
            duplicate_filter: RxDuplicateFilter::new(),
            shared_ccmp_replay: StaCcmpRxReplayRxEndpoint::vacant(),
            owned_ccmp_replay: None,
            esp_now: None,
            fragments: OpenDataDefragmenter::new(OPEN_DATA_FRAGMENT_TIMEOUT),
            fragment_admission_active: false,
        }
    }

    /// Revoke the previous association and install a new immutable identity.
    ///
    /// A replay endpoint that cannot stop keeps the old identity installed and
    /// returns an error, so callers cannot publish a new connected epoch over
    /// an in-flight CCMP publication. ESP-NOW duplicate history, ordinary
    /// duplicate history and incomplete fragment contexts are revoked on every
    /// attempt, including that fail-closed replay error path.
    pub fn try_reconfigure(
        &mut self,
        config: ConnectedRxConfig,
    ) -> Result<(), StaCcmpRxReplayError> {
        let replay = self.stop_ccmp_rx_replay();
        self.stop_esp_now_rx_epoch();
        self.clear_fragmentation();
        self.clear_duplicate_history();
        replay?;
        self.config = config;
        Ok(())
    }

    /// Install the unique replay epoch created from this association's M3
    /// pairwise/group RSC values. A WPA2 dispatcher without this owner rejects
    /// protected data instead of silently accepting PN reuse.
    pub fn install_ccmp_rx_replay(&mut self, replay: StaCcmpRxReplayEpoch) {
        assert_eq!(
            self.config.security,
            LinkProtection::Ccmp,
            "CCMP replay state requires a WPA2 connected epoch"
        );
        self.shared_ccmp_replay = StaCcmpRxReplayRxEndpoint::vacant();
        self.owned_ccmp_replay = Some(replay);
    }

    /// Install the RX half of an association replay resource shared with the
    /// connected group-key control transaction.
    pub fn install_shared_ccmp_rx_replay(&mut self, replay: StaCcmpRxReplayRxEndpoint) {
        assert_eq!(
            self.config.security,
            LinkProtection::Ccmp,
            "CCMP replay state requires a WPA2 connected epoch"
        );
        self.owned_ccmp_replay = None;
        self.shared_ccmp_replay = replay;
    }

    pub const fn ccmp_rx_replay_enabled(&self) -> bool {
        self.shared_ccmp_replay.is_live() || self.owned_ccmp_replay.is_some()
    }

    /// Stop the shared RX endpoint after the protocol task is quiescent.
    pub fn stop_ccmp_rx_replay(&mut self) -> Result<(), StaCcmpRxReplayError> {
        self.owned_ccmp_replay = None;
        if !self.shared_ccmp_replay.is_live() {
            return Ok(());
        }
        self.shared_ccmp_replay.stop()?;
        self.shared_ccmp_replay = StaCcmpRxReplayRxEndpoint::vacant();
        Ok(())
    }

    /// Quarantine a replay endpoint after a proven terminal protocol stop.
    ///
    /// This is deliberately separate from [`Self::stop_ccmp_rx_replay`]: the
    /// normal stop API retains a failed endpoint so its caller may retry. A
    /// terminal task that can no longer carry an in-flight publication must
    /// instead drop that endpoint, whose `Drop` implementation revokes RX and
    /// keeps the shared group lane quarantined.
    pub fn quarantine_ccmp_rx_replay(&mut self) -> bool {
        let owned = self.owned_ccmp_replay.take().is_some();
        let shared = core::mem::replace(
            &mut self.shared_ccmp_replay,
            StaCcmpRxReplayRxEndpoint::vacant(),
        )
        .is_live();
        owned || shared
    }

    /// Attach one already station/channel-qualified ESP-NOW receive epoch.
    ///
    /// Hardware receive-policy ownership remains at the connected composition
    /// boundary. This method only installs portable peer and duplicate state.
    pub fn install_esp_now_rx_epoch(&mut self, epoch: EspNowRxEpoch) {
        assert_eq!(
            epoch.config().station().interface.address,
            self.config.station_address,
            "ESP-NOW RX epoch must belong to the connected station"
        );
        self.esp_now = Some(epoch);
    }

    pub const fn esp_now_rx_epoch(&self) -> Option<&EspNowRxEpoch> {
        self.esp_now.as_ref()
    }

    /// Revoke ordinary MPDU retry fingerprints at an association stop edge.
    pub fn clear_duplicate_history(&mut self) {
        self.duplicate_filter = RxDuplicateFilter::new();
    }

    /// Revoke receive authority and clear all duplicate fingerprints before
    /// this connected owner is returned across a stop/restart boundary.
    pub fn stop_esp_now_rx_epoch(&mut self) -> usize {
        let Some(mut epoch) = self.esp_now.take() else {
            return 0;
        };
        epoch.reset_duplicate_history()
    }

    /// Revoke incomplete Open and CCMP MSDUs at the connected-epoch stop edge.
    pub fn clear_fragmentation(&mut self) -> usize {
        self.fragment_admission_active = false;
        self.fragments.clear()
    }

    pub fn clear_open_fragmentation(&mut self) -> usize {
        self.clear_fragmentation()
    }

    pub const fn config(&self) -> ConnectedRxConfig {
        self.config
    }

    /// Return whether this staged unit can publish an Ethernet frame.
    ///
    /// This deliberately performs only immutable public-header
    /// classification. It does not advance duplicate history or authenticate
    /// payload bytes, so an async integration may acquire network capacity
    /// before the finite dispatch without changing protocol state.
    pub fn may_publish_ethernet(&self, segment: RxSegment<'_>) -> bool {
        public_frame_control(segment.buffer).is_some_and(|frame_control| {
            let expected = match self.config.security {
                LinkProtection::Open => DATA_TYPE,
                LinkProtection::Ccmp => DATA_TYPE | PROTECTED,
            };
            frame_control & (DATA_TYPE_MASK | PROTECTED) == expected
                && (self.config.peer_qos || frame_control & QOS_SUBTYPE == 0)
                && !public_fragmented(segment.buffer, frame_control).unwrap_or(false)
        })
    }

    /// Return whether this Open or CCMP fragment may complete an in-progress
    /// MSDU.
    ///
    /// The async adapter uses this immutable hint only to reserve copying
    /// capacity. The reassembly owner still performs the exact sequence and
    /// identity admission after the wait.
    pub fn may_complete_fragment(&self, segment: RxSegment<'_>) -> bool {
        let Some(frame_control) = public_frame_control(segment.buffer) else {
            return false;
        };
        let expected = match self.config.security {
            LinkProtection::Open => DATA_TYPE,
            LinkProtection::Ccmp => DATA_TYPE | PROTECTED,
        };
        if frame_control & (DATA_TYPE_MASK | PROTECTED) != expected
            || (frame_control & QOS_SUBTYPE != 0 && !self.config.peer_qos)
            || frame_control & MORE_FRAGMENTS != 0
        {
            return false;
        }
        segment
            .buffer
            .get(PUBLIC_HEADER_SIZE + 22)
            .is_some_and(|sequence| sequence & 0x0f != 0)
    }

    /// Compatibility hint retained for Open-only adapters.
    pub fn may_complete_open_fragment(&self, segment: RxSegment<'_>) -> bool {
        self.config.security == LinkProtection::Open && self.may_complete_fragment(segment)
    }

    /// Return whether a protected QoS data unit advertises A-MSDU payload.
    ///
    /// This immutable public-header check lets an async adapter select its
    /// deferred multi-frame publication path without parsing/decrypting the
    /// MPDU twice or mutating duplicate history. Malformed input may select
    /// the conservative path and then be rejected by [`Self::dispatch`].
    pub fn may_publish_amsdu(&self, segment: RxSegment<'_>) -> bool {
        if !self.config.peer_qos {
            return false;
        }
        let raw = segment.buffer;
        let Some(frame_control) = public_frame_control(raw) else {
            return false;
        };
        let expected = match self.config.security {
            LinkProtection::Open => DATA_TYPE | QOS_SUBTYPE,
            LinkProtection::Ccmp => DATA_TYPE | PROTECTED | QOS_SUBTYPE,
        };
        if frame_control & (DATA_TYPE_MASK | PROTECTED | QOS_SUBTYPE) != expected {
            return false;
        }
        // Four-address QoS moves the control field by one address slot.
        let qos_control_offset =
            PUBLIC_HEADER_SIZE + 24 + usize::from(frame_control & TO_FROM_DS == TO_FROM_DS) * 6;
        raw.get(qos_control_offset)
            .is_some_and(|control| control & QOS_AMSDU_PRESENT != 0)
    }

    /// Classify a frame that belongs to a receive BlockAck sequence space.
    ///
    /// Open epochs own no BlockAck/reorder state. In WPA2, group, foreign,
    /// unprotected, non-QoS and fragmented frames remain on the direct
    /// dispatch path; agreement state still decides whether the returned TID
    /// is currently reordered.
    pub fn reorder_key(&self, segment: RxSegment<'_>) -> Option<RxBlockAckMpduKey> {
        if self.config.security == LinkProtection::Open {
            return None;
        }
        rx_block_ack_mpdu_key(
            segment.buffer,
            self.config.station_address,
            Some(self.config.bssid),
        )
    }

    /// Classify a BlockAckReq from the associated AP for this station.
    ///
    /// Agreement state still decides whether the named TID has a reorder
    /// window to move.
    pub fn block_ack_request_key(&self, segment: RxSegment<'_>) -> Option<RxBlockAckRequestKey> {
        if self.config.security == LinkProtection::Open {
            return None;
        }
        rx_block_ack_request_key(
            segment.buffer,
            self.config.station_address,
            Some(self.config.bssid),
        )
    }

    /// Consume one staged S31 frame and publish its connected-station effects.
    ///
    /// The supplied segment is already detached from DMA ownership. This call
    /// is finite and allocation-free; it performs no MMIO and never waits.
    pub fn dispatch(
        &mut self,
        segment: RxSegment<'_>,
        mpdu: &mut [u8],
        _ethernet: &mut [u8],
        sink: &mut dyn ConnectedRxSink,
    ) -> ConnectedRxDispatch {
        self.dispatch_received(segment, mpdu, _ethernet, None, sink)
    }

    /// Dispatch with the executor-clock sample attached by the physical RX
    /// producer at its first completed-frame handoff.
    ///
    /// The timestamp is optional because executor-neutral and synthetic users
    /// cannot manufacture it. Runtime Trigger/NDPA response policy rejects a
    /// missing sample rather than starting a fresh window at mailbox dequeue.
    pub fn dispatch_received(
        &mut self,
        segment: RxSegment<'_>,
        mpdu: &mut [u8],
        _ethernet: &mut [u8],
        times: Option<RxTimes>,
        sink: &mut dyn ConnectedRxSink,
    ) -> ConnectedRxDispatch {
        let handoff = times.map(|times| times.handoff);
        let raw = segment.buffer;
        let Some(frame_control) = public_frame_control(raw) else {
            return rejected(ConnectedRxProtection::Other, ConnectedRxError::PublicHeader);
        };
        let protection = public_protection(raw, frame_control, self.config.station_address)
            .unwrap_or(ConnectedRxProtection::Other);

        match frame_control & 0x00fc {
            BEACON_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => {
                        trace_beacon(BeaconVerdict::Unextracted, None, 0);
                        return rejected(protection, ConnectedRxError::Rx(error));
                    }
                };
                let observation = match parse_sta_beacon(
                    &mpdu[..management.length],
                    self.config.bssid,
                    self.config.association_id,
                ) {
                    Ok(observation) => observation,
                    Err(error) => {
                        trace_beacon(BeaconVerdict::Rejected, None, 0);
                        return rejected(protection, ConnectedRxError::Beacon(error));
                    }
                };
                if !crate::connected_control::beacon_tsf_followable(observation.timestamp_tsf) {
                    trace_beacon(BeaconVerdict::Rejected, None, 0);
                    return rejected(protection, ConnectedRxError::UnfollowableBeaconTsf);
                }
                let Some(metadata) = decode_normalized_rx_metadata(raw) else {
                    trace_beacon(BeaconVerdict::NoMetadata, None, 0);
                    return rejected(protection, ConnectedRxError::Rx(RxError::Metadata));
                };
                let rssi_dbm = match metadata.rssi_dbm {
                    oer_ieee80211_softmac::MacRxEvidence::HardwareObserved(rssi)
                    | oer_ieee80211_softmac::MacRxEvidence::ProtocolValidated(rssi) => Some(rssi),
                    oer_ieee80211_softmac::MacRxEvidence::Unavailable => None,
                };
                trace_beacon(
                    BeaconVerdict::Published,
                    rssi_dbm,
                    observation.timestamp_tsf.as_micros() as u32,
                );
                sink.publish(ConnectedRxEvent::Beacon {
                    observation,
                    metadata,
                    stamp: times.and_then(|times| times.stamp),
                });
                ConnectedRxDispatch::Beacon
            }
            PROBE_RESPONSE_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                if management.length < 24
                    || mpdu[4..10] != self.config.station_address
                    || mpdu[10..16] != self.config.bssid
                    || mpdu[16..22] != self.config.bssid
                {
                    return ConnectedRxDispatch::Ignored;
                }
                sink.publish(ConnectedRxEvent::ProbeResponse);
                ConnectedRxDispatch::ProbeResponse
            }
            TRIGGER_FRAME_CONTROL => {
                let control = match extract_control(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(control) => control,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let trigger = match parse_trigger_frame(&mpdu[..control.length]) {
                    Ok(trigger) => trigger,
                    Err(error) => return rejected(protection, ConnectedRxError::Trigger(error)),
                };
                let Some(identity) = self.associated_he_control_identity(
                    trigger.duration,
                    trigger.receiver_address,
                    trigger.transmitter_address,
                ) else {
                    return ConnectedRxDispatch::Ignored;
                };
                let first_user = trigger.user_info_and_padding.get(..5).map(|bytes| {
                    let mut first_user = [0_u8; 5];
                    first_user.copy_from_slice(bytes);
                    first_user
                });
                sink.publish(ConnectedRxEvent::Trigger {
                    identity,
                    common: trigger.common,
                    schedule: HeTriggerScheduledRate::from_trigger_frame(
                        &trigger,
                        self.config.association_id,
                    ),
                    first_user,
                    handoff,
                });
                ConnectedRxDispatch::Trigger
            }
            NDPA_FRAME_CONTROL => {
                let control = match extract_control(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(control) => control,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let ndpa = match HeNdpa::parse(&mpdu[..control.length]) {
                    Ok(ndpa) => ndpa,
                    Err(error) => return rejected(protection, ConnectedRxError::Ndpa(error)),
                };
                let mut receiver_address = [0_u8; 6];
                receiver_address.copy_from_slice(ndpa.receiver_address());
                let mut transmitter_address = [0_u8; 6];
                transmitter_address.copy_from_slice(ndpa.transmitter_address());
                let Some(identity) = self.associated_he_control_identity(
                    ndpa.duration(),
                    receiver_address,
                    transmitter_address,
                ) else {
                    return ConnectedRxDispatch::Ignored;
                };
                sink.publish(ConnectedRxEvent::Ndpa {
                    identity,
                    dialog_token: ndpa.dialog_token(),
                    addressed_to_station: ndpa.contains_association_id(self.config.association_id),
                    handoff,
                });
                ConnectedRxDispatch::Ndpa
            }
            ACTION_FRAME_CONTROL
                if self.config.management_protection && frame_control & PROTECTED != 0 =>
            {
                self.dispatch_protected_management(segment, protection, frame_control, mpdu, sink)
            }
            DISASSOCIATION_FRAME_CONTROL | DEAUTHENTICATION_FRAME_CONTROL
                if self.config.management_protection && frame_control & PROTECTED != 0 =>
            {
                self.dispatch_protected_management(segment, protection, frame_control, mpdu, sink)
            }
            ACTION_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                if management.length < 24 {
                    return ConnectedRxDispatch::Ignored;
                }
                if self.config.management_protection && is_robust_action_category(mpdu[24]) {
                    return self.dispatch_robust_management(
                        &mpdu[..management.length],
                        frame_control,
                        sink,
                    );
                }
                let body = &mpdu[24..management.length];
                let is_associated_peer_action = mpdu[4..10] == self.config.station_address
                    && mpdu[10..16] == self.config.bssid
                    && mpdu[16..22] == self.config.bssid;
                if is_associated_peer_action && let Some(action) = parse_block_ack_action(body) {
                    if self.config.security == LinkProtection::Open {
                        return ConnectedRxDispatch::Ignored;
                    }
                    sink.publish(ConnectedRxEvent::BlockAck { action, body });
                    return ConnectedRxDispatch::BlockAck;
                }
                if is_associated_peer_action && is_individual_twt_action_candidate(body) {
                    let action = match parse_individual_twt_action(body) {
                        Ok(action) => action,
                        Err(error) => {
                            return rejected(protection, ConnectedRxError::IndividualTwt(error));
                        }
                    };
                    sink.publish(ConnectedRxEvent::IndividualTwt { action, body });
                    return ConnectedRxDispatch::IndividualTwt;
                }

                // Do not turn every vendor Action frame into an ESP-NOW
                // rejection. Once the category/OUI identify ESP-NOW, however,
                // the strict codec owns all remaining bounds, address, BSSID,
                // type and version failures.
                if self.esp_now.is_none() || !is_esp_now_action_candidate(body) {
                    return ConnectedRxDispatch::Ignored;
                }
                let version = match esp_now_wire_version(body) {
                    Ok(version) => version,
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::EspNowVersion(error));
                    }
                };
                match version {
                    EspNowWireVersion::V1 => {
                        let outcome = match self
                            .esp_now
                            .as_mut()
                            .expect("candidate admission checked ESP-NOW epoch presence")
                            .receive_v1(&mpdu[..management.length])
                        {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                return rejected(protection, ConnectedRxError::EspNow(error));
                            }
                        };
                        match outcome {
                            EspNowRxOutcome::Received(received) => {
                                let metadata = decode_normalized_rx_metadata(raw)
                                    .unwrap_or_else(MacRxMetadata::unavailable);
                                let peer = received.peer();
                                let phy_mode = self
                                    .esp_now
                                    .as_ref()
                                    .and_then(|epoch| epoch.peer_phy_mode(peer))
                                    .expect(
                                        "an admitted ESP-NOW peer retains its configured PHY context",
                                    );
                                let metadata =
                                    normalize_esp_now_rx_metadata(phy_mode, metadata).normalized;
                                sink.publish(ConnectedRxEvent::EspNow { received, metadata });
                                ConnectedRxDispatch::EspNow { peer }
                            }
                            EspNowRxOutcome::Duplicate { peer } => {
                                ConnectedRxDispatch::EspNowDuplicate { peer }
                            }
                        }
                    }
                    EspNowWireVersion::V2 => {
                        if !sink.supports_esp_now_v2() {
                            return rejected(protection, ConnectedRxError::EspNowV2SinkUnavailable);
                        }
                        let outcome = match self
                            .esp_now
                            .as_mut()
                            .expect("candidate admission checked ESP-NOW epoch presence")
                            .receive_v2(&mpdu[..management.length])
                        {
                            Ok(outcome) => outcome,
                            Err(error) => {
                                return rejected(protection, ConnectedRxError::EspNowV2(error));
                            }
                        };
                        match outcome {
                            EspNowV2RxOutcome::Received(received) => {
                                let metadata = decode_normalized_rx_metadata(raw)
                                    .unwrap_or_else(MacRxMetadata::unavailable);
                                let peer = received.peer();
                                let phy_mode = self
                                    .esp_now
                                    .as_ref()
                                    .and_then(|epoch| epoch.peer_phy_mode(peer))
                                    .expect(
                                        "an admitted ESP-NOW peer retains its configured PHY context",
                                    );
                                let metadata =
                                    normalize_esp_now_rx_metadata(phy_mode, metadata).normalized;
                                sink.publish_esp_now_v2(received, metadata);
                                ConnectedRxDispatch::EspNowV2 { peer }
                            }
                            EspNowV2RxOutcome::Duplicate { peer } => {
                                ConnectedRxDispatch::EspNowV2Duplicate { peer }
                            }
                        }
                    }
                }
            }
            DISASSOCIATION_FRAME_CONTROL | DEAUTHENTICATION_FRAME_CONTROL => {
                let management = match extract_management(
                    core::slice::from_ref(&segment),
                    self.config.ingress,
                    mpdu,
                ) {
                    Ok(management) => management,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let frame = &mpdu[..management.length];
                if self.config.management_protection {
                    return self.dispatch_robust_management(frame, frame_control, sink);
                }
                let Some(disconnect) =
                    parse_sta_disconnect(frame, self.config.station_address, self.config.bssid)
                else {
                    return ConnectedRxDispatch::Ignored;
                };
                sink.publish(ConnectedRxEvent::PeerDisconnect(disconnect));
                ConnectedRxDispatch::PeerDisconnect
            }
            _ => self.dispatch_data(segment, frame_control, protection, handoff, sink),
        }
    }

    /// Route one robust management frame of an association that protects
    /// them, as the vendor `sta_input` and `sta_recv_mgmt` do: a protected
    /// individually addressed frame and a group-addressed frame go to control
    /// to open or verify; an unprotected individually addressed disconnect
    /// for reason 6 or 7 starts an SA Query; every other unprotected robust
    /// frame is dropped.
    ///
    /// SOURCE: complete pinned `libnet80211[ieee80211_sta.o]::sta_input`
    /// and `sta_recv_mgmt`.
    fn dispatch_protected_management(
        &self,
        segment: RxSegment<'_>,
        protection: ConnectedRxProtection,
        frame_control: u16,
        mpdu: &mut [u8],
        sink: &mut dyn ConnectedRxSink,
    ) -> ConnectedRxDispatch {
        let management = match extract_protected_management(
            core::slice::from_ref(&segment),
            self.config.ingress,
            mpdu,
        ) {
            Ok(management) => management,
            Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
        };
        self.dispatch_robust_management(&mpdu[..management.length], frame_control, sink)
    }

    fn dispatch_robust_management(
        &self,
        frame: &[u8],
        frame_control: u16,
        sink: &mut dyn ConnectedRxSink,
    ) -> ConnectedRxDispatch {
        const CLASS_2_FROM_NONAUTHENTICATED: u16 = 6;
        const CLASS_3_FROM_NONASSOCIATED: u16 = 7;
        if frame.len() < 24
            || frame[10..16] != self.config.bssid
            || frame[16..22] != self.config.bssid
        {
            return ConnectedRxDispatch::Ignored;
        }
        let group = frame[4] & 1 != 0;
        if !group && frame[4..10] != self.config.station_address {
            return ConnectedRxDispatch::Ignored;
        }
        if group || frame_control & PROTECTED != 0 {
            sink.publish(ConnectedRxEvent::ProtectedManagement { frame, group });
            return ConnectedRxDispatch::ProtectedManagement;
        }
        match parse_sta_disconnect(frame, self.config.station_address, self.config.bssid) {
            Some(
                disconnect @ StaDisconnect {
                    reason_code: CLASS_2_FROM_NONAUTHENTICATED | CLASS_3_FROM_NONASSOCIATED,
                    ..
                },
            ) => {
                sink.publish(ConnectedRxEvent::UnprotectedDisconnect(disconnect));
                ConnectedRxDispatch::UnprotectedDisconnect
            }
            _ => ConnectedRxDispatch::Ignored,
        }
    }

    fn associated_he_control_identity(
        &self,
        duration: u16,
        receiver_address: [u8; 6],
        transmitter_address: [u8; 6],
    ) -> Option<AssociatedHeControlIdentity> {
        if transmitter_address != self.config.bssid
            || (receiver_address != self.config.station_address && receiver_address[0] & 1 == 0)
        {
            return None;
        }
        Some(AssociatedHeControlIdentity {
            duration,
            receiver_address,
            transmitter_address,
        })
    }

    fn dispatch_data(
        &mut self,
        segment: RxSegment<'_>,
        public_frame_control: u16,
        protection: ConnectedRxProtection,
        handoff: Option<oer_time::Instant>,
        sink: &mut dyn ConnectedRxSink,
    ) -> ConnectedRxDispatch {
        if public_frame_control & DATA_TYPE_MASK != DATA_TYPE {
            return ConnectedRxDispatch::Ignored;
        }
        let observed_protected = public_frame_control & PROTECTED != 0;
        let expected_protected = self.config.security == LinkProtection::Ccmp;
        let fragmented = public_fragmented(segment.buffer, public_frame_control).unwrap_or(false);
        if expected_protected && !observed_protected {
            return self.dispatch_unprotected_eapol(segment, protection, sink);
        }
        if observed_protected != expected_protected {
            return rejected(protection, ConnectedRxError::SecurityModeMismatch);
        }
        if !fragmented
            && protection == ConnectedRxProtection::Pairwise
            && self.shared_ccmp_replay.is_live()
        {
            return self.dispatch_shared_pairwise_data(
                segment,
                public_frame_control,
                handoff,
                sink,
            );
        }
        if fragmented {
            return match self.config.security {
                LinkProtection::Open => {
                    self.dispatch_open_fragment(segment, protection, handoff, sink)
                }
                LinkProtection::Ccmp => {
                    self.dispatch_protected_fragment(segment, protection, handoff, sink)
                }
            };
        }
        let (mpdu, retry, sequence_control, tid, ccmp_header, data) = match self.config.security {
            LinkProtection::Open => {
                let view = match view_unprotected_data(segment, self.config.ingress) {
                    Ok(view) => view,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let identity = match parse_open_data_identity(DataInterfaceRole::Station, view.mpdu)
                {
                    Ok(identity) => identity,
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::Fragment(error));
                    }
                };
                if identity.transmitter_address() != self.config.bssid
                    || identity.receiver_address() != self.config.station_address
                        && identity.receiver_address()[0] & 1 == 0
                    || protection == ConnectedRxProtection::Other
                {
                    return ConnectedRxDispatch::Ignored;
                }
                match self
                    .fragments
                    .admit_unfragmented(identity, view.retry, handoff)
                {
                    Ok(OpenDataUnfragmentedAdmission::Admitted { .. }) => {}
                    Ok(OpenDataUnfragmentedAdmission::Duplicate { .. }) => {
                        return ConnectedRxDispatch::Duplicate;
                    }
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::Fragment(error));
                    }
                }
                let identity = (view.mpdu, view.retry, view.sequence_control, view.tid);
                let data = match view.decapsulate(DataInterfaceRole::Station) {
                    Ok(data) => data,
                    Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
                };
                (identity.0, identity.1, identity.2, identity.3, None, data)
            }
            LinkProtection::Ccmp => {
                let view = match view_protected_data(segment, self.config.ingress) {
                    Ok(view) => view,
                    Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
                };
                let fragment_identity = match parse_ccmp_data_identity(
                    DataInterfaceRole::Station,
                    view.mpdu,
                    view.ccmp_header,
                ) {
                    Ok(identity) => identity,
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::Fragment(error));
                    }
                };
                if fragment_identity.transmitter_address() != self.config.bssid
                    || fragment_identity.receiver_address() != self.config.station_address
                        && fragment_identity.receiver_address()[0] & 1 == 0
                    || protection == ConnectedRxProtection::Other
                {
                    return ConnectedRxDispatch::Ignored;
                }
                match self
                    .fragments
                    .admit_unfragmented(fragment_identity, view.retry, handoff)
                {
                    Ok(OpenDataUnfragmentedAdmission::Admitted { .. }) => {}
                    Ok(OpenDataUnfragmentedAdmission::Duplicate { .. }) => {
                        return ConnectedRxDispatch::Duplicate;
                    }
                    Err(error) => {
                        return rejected(protection, ConnectedRxError::Fragment(error));
                    }
                }
                let identity = (
                    view.mpdu,
                    view.retry,
                    view.sequence_control,
                    view.tid,
                    view.ccmp_header,
                );
                let data = match view.decapsulate(DataInterfaceRole::Station) {
                    Ok(data) => data,
                    Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
                };
                (
                    identity.0,
                    identity.1,
                    identity.2,
                    identity.3,
                    Some(identity.4),
                    data,
                )
            }
        };
        if mpdu[10..16] != self.config.bssid {
            return ConnectedRxDispatch::Ignored;
        }
        if tid.is_some() && !self.config.peer_qos {
            return rejected(protection, ConnectedRxError::PeerQosMismatch);
        }
        let mut shared_publication = None;
        if let Some(header) = ccmp_header {
            let prepared = match prepare_ccmp_replay(
                &mut self.shared_ccmp_replay,
                &mut self.owned_ccmp_replay,
                protection,
                tid,
                header,
            ) {
                Ok(prepared) => prepared,
                Err(error) => {
                    return self.replay_rejection(protection, error, retry, sequence_control, tid);
                }
            };
            // Hardware MIC verification, complete decapsulation and associated
            // peer admission have all completed. Commit before any Ethernet
            // publication; a malformed authenticated MPDU may burn a PN but
            // can never make it reusable.
            shared_publication = match commit_ccmp_replay(
                &mut self.shared_ccmp_replay,
                &mut self.owned_ccmp_replay,
                prepared,
            ) {
                Ok(permit) => permit,
                Err(error) => return rejected(protection, ConnectedRxError::CcmpReplay(error)),
            };
        }
        if self
            .duplicate_filter
            .is_duplicate(retry, sequence_control, tid)
        {
            return ConnectedRxDispatch::Duplicate;
        }
        if sink.wants_power_save_data() {
            sink.publish(ConnectedRxEvent::PowerSaveData(PowerSaveData {
                group: protection == ConnectedRxProtection::Group,
                more_data: public_frame_control & MORE_DATA != 0,
            }));
        }
        let mut frames = data.frames;
        let amsdu = data.amsdu;
        let mut count = 0_u8;
        for frame in &mut frames {
            let frame = match frame {
                Ok(frame) => frame,
                Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
            };
            sink.publish(ConnectedRxEvent::Ethernet {
                frame,
                raw: data.raw,
                amsdu,
                metadata: data.metadata,
            });
            count = count.saturating_add(1);
        }
        let result = ConnectedRxDispatch::Data {
            ethernet_frames: count,
            amsdu,
        };
        drop(shared_publication);
        result
    }

    oer_esp32s31_ieee80211_dma::place_rx_hot_path! {
      /// Dispatch an ordinary production WPA2 pairwise MPDU without routing
      /// it through the group-publication and fragment-reassembly owner graph.
      #[inline(never)]
      fn dispatch_shared_pairwise_data(
        &mut self,
        segment: RxSegment<'_>,
        public_frame_control: u16,
        handoff: Option<oer_time::Instant>,
        sink: &mut dyn ConnectedRxSink,
      ) -> ConnectedRxDispatch {
        #[cfg(feature = "task-poll-telemetry")]
        let cycle_started = connected_rx_cycle_count();
        let protection = ConnectedRxProtection::Pairwise;
        let view = match view_protected_data(segment, self.config.ingress) {
          Ok(view) => view,
          Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
        };
        #[cfg(feature = "task-poll-telemetry")]
        let cycle_view = connected_rx_cycle_count();
        if self.fragment_admission_active {
            let identity = match parse_ccmp_data_identity(
                DataInterfaceRole::Station,
                view.mpdu,
                view.ccmp_header,
            ) {
                Ok(identity) => identity,
                Err(error) => {
                    return rejected(protection, ConnectedRxError::Fragment(error));
                }
            };
            if identity.transmitter_address() != self.config.bssid
                || identity.receiver_address() != self.config.station_address
            {
                return ConnectedRxDispatch::Ignored;
            }
            match self.fragments.admit_unfragmented(
                identity,
                view.retry,
                handoff,
            ) {
                Ok(OpenDataUnfragmentedAdmission::Admitted { .. }) => {}
                Ok(OpenDataUnfragmentedAdmission::Duplicate { .. }) => {
                    return ConnectedRxDispatch::Duplicate;
                }
                Err(error) => {
                    return rejected(protection, ConnectedRxError::Fragment(error));
                }
            }
        }
        if view.mpdu[10..16] != self.config.bssid {
            return ConnectedRxDispatch::Ignored;
        }
        if view.tid.is_some() && !self.config.peer_qos {
          return rejected(protection, ConnectedRxError::PeerQosMismatch);
        }
        #[cfg(feature = "task-poll-telemetry")]
        let cycle_fragment_guard = connected_rx_cycle_count();
        let retry = view.retry;
        let sequence_control = view.sequence_control;
        let tid = view.tid;
        let ccmp_header = view.ccmp_header;
        let data = match view.decapsulate(DataInterfaceRole::Station) {
          Ok(data) => data,
          Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
        };
        #[cfg(feature = "task-poll-telemetry")]
        let cycle_decapsulate = connected_rx_cycle_count();
        if let Err(error) = self
            .shared_ccmp_replay
            .commit_pairwise_immediate(tid, ccmp_header)
        {
          return self.replay_rejection(protection, error, retry, sequence_control, tid);
        }
        #[cfg(feature = "task-poll-telemetry")]
        let cycle_replay = connected_rx_cycle_count();
        if self
            .duplicate_filter
            .is_duplicate(retry, sequence_control, tid)
        {
          return ConnectedRxDispatch::Duplicate;
        }
        #[cfg(feature = "task-poll-telemetry")]
        let cycle_duplicate = connected_rx_cycle_count();
        if sink.wants_power_save_data() {
            sink.publish(ConnectedRxEvent::PowerSaveData(PowerSaveData {
                group: false,
                more_data: public_frame_control & MORE_DATA != 0,
            }));
        }
        let mut frames = data.frames;
        let amsdu = data.amsdu;
        let mut count = 0_u8;
        for frame in &mut frames {
            let frame = match frame {
                Ok(frame) => frame,
                Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
            };
            sink.publish(ConnectedRxEvent::Ethernet {
                frame,
                raw: data.raw,
                amsdu,
                metadata: data.metadata,
            });
          count = count.saturating_add(1);
        }
        #[cfg(feature = "task-poll-telemetry")]
        {
          let cycle_publish = connected_rx_cycle_count();
          sink.observe_data_cycle_profile(ConnectedRxDataCycleProfile {
            calls: 1,
            completed: 1,
            total: cycle_publish.wrapping_sub(cycle_started),
            view: cycle_view.wrapping_sub(cycle_started),
            fragment_guard: cycle_fragment_guard.wrapping_sub(cycle_view),
            decapsulate: cycle_decapsulate.wrapping_sub(cycle_fragment_guard),
            replay: cycle_replay.wrapping_sub(cycle_decapsulate),
            duplicate: cycle_duplicate.wrapping_sub(cycle_replay),
            publish: cycle_publish.wrapping_sub(cycle_duplicate),
          });
        }
        ConnectedRxDispatch::Data {
            ethernet_frames: count,
            amsdu,
        }
      }
    }

    fn dispatch_open_fragment(
        &mut self,
        segment: RxSegment<'_>,
        protection: ConnectedRxProtection,
        handoff: Option<oer_time::Instant>,
        sink: &mut dyn ConnectedRxSink,
    ) -> ConnectedRxDispatch {
        let Some(now) = handoff else {
            return rejected(
                protection,
                ConnectedRxError::Fragment(OpenDataFragmentError::ClockUnavailable),
            );
        };
        let view = match view_unprotected_data_fragment(
            segment,
            self.config.ingress,
            DataInterfaceRole::Station,
        ) {
            Ok(view) => view,
            Err(UnprotectedDataFragmentRxError::Radio(error)) => {
                return rejected(protection, ConnectedRxError::Rx(error));
            }
            Err(UnprotectedDataFragmentRxError::Fragment(error)) => {
                return rejected(protection, ConnectedRxError::Fragment(error));
            }
        };
        let identity = view.fragment.identity();
        if identity.transmitter_address() != self.config.bssid
            || identity.receiver_address() != self.config.station_address
                && identity.receiver_address()[0] & 1 == 0
            || protection == ConnectedRxProtection::Other
        {
            return ConnectedRxDispatch::Ignored;
        }
        if identity.tid().is_some() && !self.config.peer_qos {
            return rejected(protection, ConnectedRxError::PeerQosMismatch);
        }
        if view.fragment.fragment_number() == 0
            && self.duplicate_filter.is_known_duplicate(
                view.fragment.retry(),
                view.fragment.sequence_control(),
                identity.tid(),
            )
        {
            // Fragment zero shares the ordinary MPDU's Sequence Control
            // value. Consult the association-wide duplicate owner exactly at
            // this edge so a retry cannot turn an already accepted ordinary
            // MPDU into a new fragment train merely by setting More
            // Fragments. Later fragments remain owned by the reassembler's
            // per-fragment history, and no fragment mutates ordinary history.
            return ConnectedRxDispatch::Duplicate;
        }
        let power_save_data = PowerSaveData {
            group: protection == ConnectedRxProtection::Group,
            more_data: u16::from_le_bytes([view.mpdu[0], view.mpdu[1]]) & MORE_DATA != 0,
        };
        let raw = view.raw;
        let metadata = view.metadata;
        self.fragment_admission_active = true;
        match self.fragments.ingest(view.fragment, now, |data| {
            if sink.wants_power_save_data() {
                sink.publish(ConnectedRxEvent::PowerSaveData(power_save_data));
            }
            sink.publish(ConnectedRxEvent::Ethernet {
                frame: data.ethernet_frame(),
                raw,
                amsdu: false,
                metadata,
            });
        }) {
            Ok(OpenDataDefragmentation::Buffered { expired, evicted }) => {
                ConnectedRxDispatch::FragmentBuffered {
                    expired,
                    evicted: evicted.is_some(),
                }
            }
            Ok(OpenDataDefragmentation::Duplicate { .. }) => ConnectedRxDispatch::Duplicate,
            Ok(OpenDataDefragmentation::Complete { .. }) => ConnectedRxDispatch::Data {
                ethernet_frames: 1,
                amsdu: false,
            },
            Err(error) => rejected(protection, ConnectedRxError::Fragment(error)),
        }
    }

    fn dispatch_protected_fragment(
        &mut self,
        segment: RxSegment<'_>,
        protection: ConnectedRxProtection,
        handoff: Option<oer_time::Instant>,
        sink: &mut dyn ConnectedRxSink,
    ) -> ConnectedRxDispatch {
        let Some(now) = handoff else {
            return rejected(
                protection,
                ConnectedRxError::Fragment(OpenDataFragmentError::ClockUnavailable),
            );
        };
        let view = match view_protected_data_fragment(
            segment,
            self.config.ingress,
            DataInterfaceRole::Station,
        ) {
            Ok(view) => view,
            Err(ProtectedDataFragmentRxError::Radio(error)) => {
                return rejected(protection, ConnectedRxError::Rx(error));
            }
            Err(ProtectedDataFragmentRxError::Fragment(error)) => {
                return rejected(protection, ConnectedRxError::Fragment(error));
            }
        };
        let identity = view.fragment.identity();
        if identity.transmitter_address() != self.config.bssid
            || identity.receiver_address() != self.config.station_address
            || protection != ConnectedRxProtection::Pairwise
        {
            return ConnectedRxDispatch::Ignored;
        }
        let tid = identity.tid();
        if tid.is_some() && !self.config.peer_qos {
            return rejected(protection, ConnectedRxError::PeerQosMismatch);
        }
        if view.fragment.fragment_number() == 0
            && self.duplicate_filter.is_known_duplicate(
                view.fragment.retry(),
                view.fragment.sequence_control(),
                tid,
            )
        {
            // Protected and Open fragment zero share the ordinary MPDU's
            // Sequence Control space. Fence an authenticated Retry against
            // the association-wide ordinary history before replay prepare or
            // fragment preflight can advance or mutate either owner.
            return ConnectedRxDispatch::Duplicate;
        }

        let raw = view.raw;
        let metadata = view.metadata;
        let more_data = u16::from_le_bytes([view.mpdu[0], view.mpdu[1]]) & MORE_DATA != 0;
        let fragment = view.fragment;
        let more_fragments = fragment.more_fragments();
        let ccmp_header = view.ccmp_header;
        self.fragment_admission_active = true;
        let (fragments, shared_replay, owned_replay) = (
            &mut self.fragments,
            &mut self.shared_ccmp_replay,
            &mut self.owned_ccmp_replay,
        );
        let admission = match fragments.preflight_in_epoch(fragment, 0, now) {
            Ok(OpenDataFragmentPreflight::Duplicate { .. }) => {
                return ConnectedRxDispatch::Duplicate;
            }
            Ok(OpenDataFragmentPreflight::Admitted(admission)) => admission,
            Err(error) => return rejected(protection, ConnectedRxError::Fragment(error)),
        };
        let prepared =
            match prepare_ccmp_replay(shared_replay, owned_replay, protection, tid, ccmp_header) {
                Ok(prepared) => prepared,
                Err(error) => return rejected(protection, ConnectedRxError::CcmpReplay(error)),
            };

        if more_fragments {
            let outcome =
                admission.ingest(|_| unreachable!("More Fragments cannot complete one MSDU"));
            let outcome = match outcome {
                Ok(outcome) => outcome,
                Err(error) => return rejected(protection, ConnectedRxError::Fragment(error)),
            };
            if let Err(error) = commit_ccmp_replay(shared_replay, owned_replay, prepared) {
                fragments.discard(identity, 0);
                return rejected(protection, ConnectedRxError::CcmpReplay(error));
            }
            return match outcome {
                OpenDataDefragmentation::Buffered { expired, evicted } => {
                    ConnectedRxDispatch::FragmentBuffered {
                        expired,
                        evicted: evicted.is_some(),
                    }
                }
                _ => unreachable!("More Fragments produces only a buffered admission"),
            };
        }

        let outcome = admission.ingest(|data| {
            let publication = commit_ccmp_replay(shared_replay, owned_replay, prepared)?;
            if sink.wants_power_save_data() {
                sink.publish(ConnectedRxEvent::PowerSaveData(PowerSaveData {
                    group: false,
                    more_data,
                }));
            }
            sink.publish(ConnectedRxEvent::Ethernet {
                frame: data.ethernet_frame(),
                raw,
                amsdu: false,
                metadata,
            });
            drop(publication);
            Ok::<(), StaCcmpRxReplayError>(())
        });
        match outcome {
            Ok(OpenDataDefragmentation::Complete { value: Ok(()), .. }) => {
                ConnectedRxDispatch::Data {
                    ethernet_frames: 1,
                    amsdu: false,
                }
            }
            Ok(OpenDataDefragmentation::Complete {
                value: Err(error), ..
            }) => {
                fragments.discard(identity, 0);
                rejected(protection, ConnectedRxError::CcmpReplay(error))
            }
            Ok(_) => unreachable!("final fragment produces one completion"),
            Err(error) => rejected(protection, ConnectedRxError::Fragment(error)),
        }
    }

    fn dispatch_unprotected_eapol(
        &mut self,
        segment: RxSegment<'_>,
        protection: ConnectedRxProtection,
        sink: &mut dyn ConnectedRxSink,
    ) -> ConnectedRxDispatch {
        if protection != ConnectedRxProtection::Pairwise {
            return rejected(protection, ConnectedRxError::SecurityModeMismatch);
        }
        let view = match view_unprotected_data(segment, self.config.ingress) {
            Ok(view) => view,
            Err(error) => return rejected(protection, ConnectedRxError::Rx(error)),
        };
        if view.mpdu.get(4..10) != Some(&self.config.station_address)
            || view.mpdu.get(10..16) != Some(&self.config.bssid)
            || (view.tid.is_some() && !self.config.peer_qos)
        {
            return rejected(protection, ConnectedRxError::SecurityModeMismatch);
        }
        let data = match view.decapsulate(DataInterfaceRole::Station) {
            Ok(data) => data,
            Err(error) => return rejected(protection, ConnectedRxError::Data(error)),
        };
        if data.amsdu {
            return rejected(protection, ConnectedRxError::SecurityModeMismatch);
        }
        let mut frames = data.frames;
        let frame = match frames.next() {
            Some(Ok(frame)) => frame,
            Some(Err(error)) => return rejected(protection, ConnectedRxError::Data(error)),
            None => return rejected(protection, ConnectedRxError::SecurityModeMismatch),
        };
        if frames.next().is_some()
            || frame.destination != self.config.station_address
            || frame.source != self.config.bssid
            || frame.ether_type != 0x888e
        {
            return rejected(protection, ConnectedRxError::SecurityModeMismatch);
        }
        if let Err(error) = EapolKeyFrame::parse(frame.payload) {
            return rejected(protection, ConnectedRxError::Eapol(error));
        }
        sink.publish(ConnectedRxEvent::UnprotectedEapol {
            source: frame.source,
            payload: frame.payload,
        });
        ConnectedRxDispatch::UnprotectedEapol
    }
}

impl ConnectedRxDispatcher {
    /// IEEE 802.11 removes duplicates before replay detection. A Retry of the
    /// MPDU last accepted in the same sequence space carries that MPDU's PN,
    /// so its replay rejection is a duplicate, not a replay. History is only
    /// read: a rejected MPDU never becomes the accepted retry fingerprint.
    #[cold]
    fn replay_rejection(
        &self,
        protection: ConnectedRxProtection,
        error: StaCcmpRxReplayError,
        retry: bool,
        sequence_control: u16,
        tid: Option<u8>,
    ) -> ConnectedRxDispatch {
        if matches!(error, StaCcmpRxReplayError::Replay(_))
            && self
                .duplicate_filter
                .is_known_duplicate(retry, sequence_control, tid)
        {
            return ConnectedRxDispatch::Duplicate;
        }
        rejected(protection, ConnectedRxError::CcmpReplay(error))
    }
}

impl Default for ConnectedRxDispatcher {
    fn default() -> Self {
        Self::unconfigured()
    }
}

fn rejected(protection: ConnectedRxProtection, error: ConnectedRxError) -> ConnectedRxDispatch {
    ConnectedRxDispatch::Rejected { protection, error }
}

fn is_esp_now_action_candidate(body: &[u8]) -> bool {
    if body.first() != Some(&ESP_NOW_ACTION_CATEGORY) {
        return false;
    }
    body.len() < 4 || body[1..4] == ESP_NOW_ORGANIZATION_IDENTIFIER
}

fn public_frame_control(raw: &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes([
        *raw.get(PUBLIC_HEADER_SIZE)?,
        *raw.get(PUBLIC_HEADER_SIZE + 1)?,
    ]))
}

fn public_protection(
    raw: &[u8],
    frame_control: u16,
    station_address: [u8; 6],
) -> Option<ConnectedRxProtection> {
    if frame_control & DATA_TYPE_MASK != DATA_TYPE {
        return Some(ConnectedRxProtection::Other);
    }
    let destination = raw.get(PUBLIC_HEADER_SIZE + 4..PUBLIC_HEADER_SIZE + 10)?;
    if destination.first().is_some_and(|byte| byte & 1 != 0) {
        Some(ConnectedRxProtection::Group)
    } else if destination == station_address {
        Some(ConnectedRxProtection::Pairwise)
    } else {
        Some(ConnectedRxProtection::Other)
    }
}

fn public_fragmented(raw: &[u8], frame_control: u16) -> Option<bool> {
    if frame_control & MORE_FRAGMENTS != 0 {
        return Some(true);
    }
    Some(*raw.get(PUBLIC_HEADER_SIZE + 22)? & 0x0f != 0)
}

#[inline(always)]
fn trace_beacon(verdict: BeaconVerdict, rssi_dbm: Option<i8>, timestamp_tsf_low: u32) {
    oer_trace::emit(&BeaconDispatch {
        verdict,
        rssi_dbm,
        timestamp_tsf_low,
    });
}

#[cfg(test)]
mod tests;

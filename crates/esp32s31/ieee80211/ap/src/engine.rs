//! One bounded AP MAC epoch above DMA/IRQ and below the Embassy actor.

mod management;
mod power_save;
mod rx;
mod tx;

use crate::{
    hardware::ApRuntimeHardware,
    rx::{
        ApOrdinaryPairwiseRxRequest, ApRxAdmission, ApRxAdmissionOperation, ApRxAdmissionRequest,
        ApRxDuplicateOwner, ApRxError, ApRxPreparedCandidate, ApRxPreparedReplay,
    },
    security::{
        ApPairwiseBinding, ApPairwiseKeyStorage, ApSecurity, ApSecurityError, ApSecurityStopReport,
    },
};
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_esp32s31_ieee80211_mac::{
    ap_policy::{configure_ap_receive_policy, disable_ap_receive_policy},
    ap_tsf::AccessPointTsf,
    crypto::CryptoKeyError,
    tx::protection::{BasicRates, BssProtection, HePacketPadding},
};
use oer_ieee80211_mac::protection::ApBssProtection;

use oer_ieee80211_mac::{
    ap::{
        ApAmsduFrame, ApAssociationResponseError, ApDataFrame, ApDataFrameError, ApManagementFrame,
        ApManagementRequest, ApManagementSubtype, ApPeerDisconnectKind, ApPowerSaveObservation,
        ApProtectedDataFrame, ApUnprotectedDataFrame, EncodedApFrame,
        observe_ap_power_save_for_access_point, parse_ap_management_request,
        write_ap_peer_disconnect, write_ht_association_response_frame_for_security,
        write_open_authentication_response, write_sae_authentication,
    },
    beacon::{AP_BEACON_CAPACITY, ApBeaconBuildError, dtim, write_ht_beacon},
    block_ack::{OperationalTxBlockAck, TxBlockAckResponse, TxBlockAckRetry},
    ccmp::{CcmpKeyId, CcmpReplayLane},
    channel::WifiChannel,
    data::{IEEE80211_LEGACY_DATA_HEADER_LEN, IEEE80211_QOS_DATA_HEADER_LEN},
    security::{ApSecurityPolicy, LinkProtection},
    ssid::WifiSsid,
};

use oer_ieee80211_ap::{
    AccessPointService, ApAssociationCapabilities, ApAssociationIdentity, ApBufferedGroupRelease,
    ApBufferedUnicastRelease, ApDownlinkAdmission, ApDownlinkDisposition, ApMlmeAction,
    ApPeerBinding, ApPeerClose, ApPeerCloseKind, ApPeerPhase, ApPeerPowerState, ApPeerStatus,
    ApPowerSaveAction, ApServiceError, ApWpa2Error, ApWpa2Progress, ApWpa2RetryProgress,
    beacon::ApBeacon, pmksa::AP_PMKID_LEN, sae::ApSaeReply,
};

use oer_ieee80211_rsn::{OwnedEapolFrame, Pmk, frames::RsnTxFrame};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApEngineError {
    Beacon(ApBeaconBuildError),
    /// The retained beacon could not be stamped with the current TSF, TIM
    /// and TBTT schedule immediately before publication.
    BeaconPreparation,
    Probe(oer_ieee80211_mac::ap::probe::ResponseError),
    Crypto(CryptoKeyError),
    Security(ApSecurityError),
    Service(ApServiceError),
    Frame(ApAssociationResponseError),
    DataFrame(ApDataFrameError),
    Wpa2(ApWpa2Error),
}

impl From<ApAssociationResponseError> for ApEngineError {
    fn from(error: ApAssociationResponseError) -> Self {
        Self::Frame(error)
    }
}

impl From<ApDataFrameError> for ApEngineError {
    fn from(error: ApDataFrameError) -> Self {
        Self::DataFrame(error)
    }
}

impl From<ApServiceError> for ApEngineError {
    fn from(error: ApServiceError) -> Self {
        Self::Service(error)
    }
}

impl From<ApSecurityError> for ApEngineError {
    fn from(error: ApSecurityError) -> Self {
        Self::Security(error)
    }
}

impl From<ApWpa2Error> for ApEngineError {
    fn from(error: ApWpa2Error) -> Self {
        Self::Wpa2(error)
    }
}

/// An ordinary MPDU's replay rejection keeps its exact duplicate owner, so
/// the dispatcher can remove a retransmitted duplicate first.
fn rejected_ordinary_rx_security(
    owner: crate::rx::ApRxDuplicateOwner,
    error: ApSecurityError,
) -> ApRxAdmission {
    match error {
        ApSecurityError::Replay(error) => ApRxAdmission::replayed(owner, error),
        error => rejected_rx_security(error),
    }
}

fn rejected_rx_security(error: ApSecurityError) -> ApRxAdmission {
    match error {
        ApSecurityError::Replay(error) => ApRxAdmission::rejected(ApRxError::Replay(error)),
        ApSecurityError::SecurityModeMismatch => {
            ApRxAdmission::rejected(ApRxError::SecurityModeMismatch)
        }
        ApSecurityError::Crypto(_)
        | ApSecurityError::PacketNumber(_)
        | ApSecurityError::PairwiseStorageNotEmpty
        | ApSecurityError::PairwiseAlreadyInstalled
        | ApSecurityError::AssociationIdAlreadyInstalled
        | ApSecurityError::WrongPeer => ApRxAdmission::rejected(ApRxError::KeyGenerationMismatch),
    }
}

/// One executor-stamped beacon and the exact group queue prefix it advertised.
///
/// `dtim_group_frames` is nonzero only when this concrete frame carries
/// DTIM-count zero and TIM bitmap-control bit zero. It is still only a
/// publication candidate: the MAC must wait for terminal beacon TX success
/// before handing this count to the caller-owned group queue.
pub struct ApBeaconPublication<'frame> {
    pub frame: &'frame mut [u8],
    pub dtim_group_frames: u16,
}

pub struct ApEngineStartFailure<'storage> {
    pub service: AccessPointService<'storage>,
    pub beacon_storage: &'storage mut [u8; AP_BEACON_CAPACITY],
    pub pairwise_storage: &'storage mut ApPairwiseKeyStorage,
    pub error: ApEngineError,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApManagementOutcome {
    Ignored,
    Response {
        len: usize,
        begin_wpa2: bool,
    },
    PeerRemoved {
        peer: [u8; 6],
    },
    /// An SAE Authentication frame for the SAE responder, which runs outside
    /// the radio loop; its body is the first `len` octets of the output.
    SaeAuthentication {
        peer: [u8; 6],
        transaction: u16,
        status: u16,
        len: usize,
    },
}

pub enum ApWpa2Outcome<const N: usize> {
    None,
    Transmit(RsnTxFrame<N>),
    PeerAuthorized { peer: [u8; 6] },
    DeauthenticatePeer { peer: [u8; 6] },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApProtectedFrame {
    pub length: usize,
    /// `None` is a plaintext Open MPDU and must use the zero-MIC ordinary TX
    /// path. A selector is present only for the WPA2 CCMP path.
    pub hardware_key_selector: Option<u8>,
}

/// Fully encoded A-MSDU whose output geometry and peer/key identity are
/// admitted, but whose monotonic QoS sequence and CCMP PN are not consumed.
///
/// The AP MAC holds this value only across its synchronous TX-protection
/// preflight. No public constructor can bypass the capacity/peer checks.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepared A-MSDU must pass TX admission before commit or be discarded"]
pub(crate) struct ApPreparedAmsduFrame {
    pub(crate) length: usize,
    pub(crate) peer: [u8; 6],
    sequence_number: SequenceNumber,
    hardware_key_selector: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ApDataSequenceSpace {
    NonQos,
    Qos,
}

/// Capacity-admitted ordinary AP data frame before sequence/CCMP commit.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepared AP data must pass TX admission before commit or be discarded"]
pub(crate) struct ApPreparedDataFrame {
    pub(crate) length: usize,
    pub(crate) peer: [u8; 6],
    sequence_number: SequenceNumber,
    sequence_space: ApDataSequenceSpace,
    hardware_key_selector: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAggregateFrame {
    pub encoded: EncodedApFrame,
    pub hardware_key_selector: u8,
    pub sequence_number: SequenceNumber,
}

/// Peer and key-slot identity captured once before an aggregate claims its
/// first DMA lease. Every MPDU validates these O(1) generation-bound owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAggregateBinding {
    peer: ApPeerBinding,
    security: ApPairwiseBinding,
}

impl ApAggregateBinding {
    pub const fn peer(self) -> [u8; 6] {
        self.peer.address()
    }
}

/// Generation-bound AP RX context cached across one transmitter burst.
///
/// The portable peer binding validates slot generation and address in O(1),
/// while the pairwise binding independently validates the installed PTK
/// generation. The public status revision invalidates authorization/QoS
/// snapshots without putting a peer-table scan back on every data MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ApRxPeerBinding {
    peer: ApPeerBinding,
    pairwise: Option<ApPairwiseBinding>,
    duplicate_owner: ApRxDuplicateOwner,
    qos_supported: bool,
    status_revision: u32,
}

pub struct ApEngineStop<'storage> {
    pub service: AccessPointService<'storage>,
    pub beacon_storage: &'storage mut [u8; AP_BEACON_CAPACITY],
    pub pairwise_storage: &'storage mut ApPairwiseKeyStorage,
    pub security: ApSecurityStopReport,
}

/// Value-only observations from one AP ownership epoch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApEngineObservation {
    pub beacons_prepared: u32,
    pub authentication_responses_prepared: u32,
    pub association_responses_prepared: u32,
    /// Successful controlled-port openings during the epoch.
    pub authorized_peers: u32,
    /// Largest number of simultaneously admitted peers during the epoch.
    pub maximum_associated_peers: u8,
    /// Largest number of simultaneously open controlled ports during the epoch.
    pub maximum_authorized_peers: u8,
    pub peer_removals: u32,
    pub authentication_timeouts: u32,
    pub wpa2_response_windows: u32,
    pub wpa2_pending_on_stop: u32,
    pub wpa2_retransmissions: u32,
    pub wpa2_handshake_failures: u32,
    pub wpa2_handshake_timeouts: u32,
    pub inactivity_timeouts: u32,
    pub disassociations_prepared: u32,
    pub deauthentications_prepared: u32,
    pub tx_block_ack_requests_prepared: u32,
    pub tx_block_ack_responses_observed: u32,
    pub tx_block_ack_agreements_operational: u32,
    pub tx_block_ack_responses_rejected: u32,
    pub tx_block_ack_negotiation_timeouts: u32,
}

#[cfg(any(feature = "diagnostics", test))]
#[derive(Clone, Copy)]
enum ApEngineObservationEvent {
    BeaconPrepared,
    AuthenticationResponsePrepared,
    AssociationResponsePrepared { associated_peers: u8 },
    PeerRemoved,
    TxBlockAckResponseObserved,
    TxBlockAckOperational,
    TxBlockAckRejected,
    TxBlockAckRequestPrepared,
    TxBlockAckNegotiationTimeout,
    PeerAuthorized { authorized_peers: u8 },
    Wpa2ResponseWindow,
    Wpa2Retransmission,
    Wpa2HandshakeTimeout,
    AuthenticationTimeout,
    Wpa2HandshakeFailure,
    InactivityTimeout,
    Wpa2PendingOnStop,
    DisassociationPrepared,
    DeauthenticationPrepared,
}

#[cfg(any(feature = "diagnostics", test))]
#[derive(Default)]
struct ApEngineObserver {
    observation: ApEngineObservation,
}

#[cfg(any(feature = "diagnostics", test))]
impl ApEngineObserver {
    fn observe(&mut self, event: ApEngineObservationEvent) {
        let observation = &mut self.observation;
        match event {
            ApEngineObservationEvent::BeaconPrepared => {
                observation.beacons_prepared = observation.beacons_prepared.saturating_add(1);
            }
            ApEngineObservationEvent::AuthenticationResponsePrepared => {
                observation.authentication_responses_prepared = observation
                    .authentication_responses_prepared
                    .saturating_add(1);
            }
            ApEngineObservationEvent::AssociationResponsePrepared { associated_peers } => {
                observation.association_responses_prepared =
                    observation.association_responses_prepared.saturating_add(1);
                observation.maximum_associated_peers =
                    observation.maximum_associated_peers.max(associated_peers);
            }
            ApEngineObservationEvent::PeerRemoved => {
                observation.peer_removals = observation.peer_removals.saturating_add(1);
            }
            ApEngineObservationEvent::TxBlockAckResponseObserved => {
                observation.tx_block_ack_responses_observed = observation
                    .tx_block_ack_responses_observed
                    .saturating_add(1);
            }
            ApEngineObservationEvent::TxBlockAckOperational => {
                observation.tx_block_ack_agreements_operational = observation
                    .tx_block_ack_agreements_operational
                    .saturating_add(1);
            }
            ApEngineObservationEvent::TxBlockAckRejected => {
                observation.tx_block_ack_responses_rejected = observation
                    .tx_block_ack_responses_rejected
                    .saturating_add(1);
            }
            ApEngineObservationEvent::TxBlockAckRequestPrepared => {
                observation.tx_block_ack_requests_prepared =
                    observation.tx_block_ack_requests_prepared.saturating_add(1);
            }
            ApEngineObservationEvent::TxBlockAckNegotiationTimeout => {
                observation.tx_block_ack_negotiation_timeouts = observation
                    .tx_block_ack_negotiation_timeouts
                    .saturating_add(1);
            }
            ApEngineObservationEvent::PeerAuthorized { authorized_peers } => {
                observation.authorized_peers = observation.authorized_peers.saturating_add(1);
                observation.maximum_authorized_peers =
                    observation.maximum_authorized_peers.max(authorized_peers);
            }
            ApEngineObservationEvent::Wpa2ResponseWindow => {
                observation.wpa2_response_windows =
                    observation.wpa2_response_windows.saturating_add(1);
            }
            ApEngineObservationEvent::Wpa2Retransmission => {
                observation.wpa2_retransmissions =
                    observation.wpa2_retransmissions.saturating_add(1);
            }
            ApEngineObservationEvent::Wpa2HandshakeTimeout => {
                observation.wpa2_handshake_timeouts =
                    observation.wpa2_handshake_timeouts.saturating_add(1);
            }
            ApEngineObservationEvent::AuthenticationTimeout => {
                observation.authentication_timeouts =
                    observation.authentication_timeouts.saturating_add(1);
            }
            ApEngineObservationEvent::Wpa2HandshakeFailure => {
                observation.wpa2_handshake_failures =
                    observation.wpa2_handshake_failures.saturating_add(1);
            }
            ApEngineObservationEvent::InactivityTimeout => {
                observation.inactivity_timeouts = observation.inactivity_timeouts.saturating_add(1);
            }
            ApEngineObservationEvent::Wpa2PendingOnStop => {
                observation.wpa2_pending_on_stop =
                    observation.wpa2_pending_on_stop.saturating_add(1);
            }
            ApEngineObservationEvent::DisassociationPrepared => {
                observation.disassociations_prepared =
                    observation.disassociations_prepared.saturating_add(1);
            }
            ApEngineObservationEvent::DeauthenticationPrepared => {
                observation.deauthentications_prepared =
                    observation.deauthentications_prepared.saturating_add(1);
            }
        }
    }
}

/// Active AP policy and hardware-key owner.
///
/// Dropping this value loses the only route back to role-neutral Wi-Fi, so a
/// supervisor can only acknowledge stop after consuming [`stop`](Self::stop).
#[must_use = "an active AP engine must be consumed through stop before radio reuse"]
pub struct ApEngine<'storage> {
    next_probe_response: oer_time::Instant,
    /// Protection carried by the retained beacon/probe-response template.
    advertised_protection: ApBssProtection,
    service: AccessPointService<'storage>,
    beacon: ApBeacon<'storage>,
    security: ApSecurity<'storage>,
    rx_peer: Option<ApRxPeerBinding>,
    channel: WifiChannel,
    /// The single writer of the access-point TSF.
    tsf: AccessPointTsf,
    #[cfg(any(feature = "diagnostics", test))]
    observer: ApEngineObserver,
}

impl<'storage> ApEngine<'storage> {
    // Start failure must return the affine service and caller-owned beacon
    // storage. This no-alloc driver cannot box that rollback value merely to
    // shrink the Result discriminant.
    #[allow(clippy::result_large_err, clippy::too_many_arguments)]
    pub fn start<H: ApRuntimeHardware>(
        hardware: &mut H,
        service: AccessPointService<'storage>,
        beacon_storage: &'storage mut [u8; AP_BEACON_CAPACITY],
        pairwise_storage: &'storage mut ApPairwiseKeyStorage,
        ssid: &WifiSsid,
        channel: WifiChannel,
        beacon_interval_tu: u16,
        dtim_period: u8,
        tsf_epoch: u32,
    ) -> Result<Self, ApEngineStartFailure<'storage>> {
        let link_protection = service.link_protection();
        let advertised_protection = service.bss_protection(is_forty_mhz(channel));
        let beacon_len = match write_ht_beacon(
            &crate::profile::ADVERTISEMENT,
            beacon_storage,
            service.address(),
            ssid,
            oer_ieee80211_mac::channel::Channel::from_wifi_channel(channel),
            beacon_interval_tu,
            dtim_period,
            SequenceNumber::ZERO,
            service.security_policy(),
            advertised_protection,
        ) {
            Ok(len) => len,
            Err(error) => {
                return Err(ApEngineStartFailure {
                    service,
                    beacon_storage,
                    pairwise_storage,
                    error: ApEngineError::Beacon(error),
                });
            }
        };
        let beacon = ApBeacon::from_initialized(beacon_storage, beacon_len, beacon_interval_tu);
        configure_ap_receive_policy(hardware, service.address());
        let security = match link_protection {
            LinkProtection::Open => ApSecurity::open(pairwise_storage),
            LinkProtection::Ccmp => ApSecurity::install_group(
                hardware,
                service
                    .gtk()
                    .expect("WPA2 service mode owns one GTK for this epoch"),
                pairwise_storage,
            ),
        };
        let security = match security {
            Ok(security) => security,
            Err(failure) => {
                disable_ap_receive_policy(hardware);
                return Err(ApEngineStartFailure {
                    service,
                    beacon_storage: beacon.into_storage(),
                    pairwise_storage: failure.storage,
                    error: ApEngineError::Security(failure.error),
                });
            }
        };
        // This is the first irreversible timing edge. Beacon construction and
        // group-key installation have succeeded, so a failed start never
        // leaves an unowned hardware TSF epoch behind.
        let mut tsf = AccessPointTsf::new(tsf_epoch);
        tsf.restart(hardware);
        Ok(Self {
            tsf,
            next_probe_response: oer_time::Instant::EPOCH,
            advertised_protection,
            service,
            beacon,
            security,
            rx_peer: None,
            channel,
            #[cfg(any(feature = "diagnostics", test))]
            observer: ApEngineObserver::default(),
        })
    }

    pub const fn channel(&self) -> WifiChannel {
        self.channel
    }

    pub const fn link_protection(&self) -> LinkProtection {
        self.service.link_protection()
    }

    #[cfg(any(feature = "diagnostics", test))]
    #[inline(always)]
    fn observe(&mut self, event: ApEngineObservationEvent) {
        self.observer.observe(event);
    }

    pub fn prepare_beacon(&mut self, now: oer_time::Instant) -> Option<&mut [u8]> {
        self.prepare_beacon_publication(now)
            .map(|publication| publication.frame)
    }

    /// Prepare one beacon together with the exact DTIM group prefix encoded in
    /// that frame. The returned count must not be released until terminal
    /// beacon publication succeeds.
    pub fn prepare_beacon_publication(
        &mut self,
        now: oer_time::Instant,
    ) -> Option<ApBeaconPublication<'_>> {
        let group_pending = self.service.group_traffic_pending();
        let buffered_group_frames = self.service.buffered_group_frames();
        let unicast_tim_bitmap = self.service.unicast_tim_bitmap().ok()?;
        let unicast_tim_bitmap = unicast_tim_bitmap.partial();
        self.advertise_current_protection().ok()?;
        let management_sequence = self.service.next_management_sequence();
        let beacon =
            self.beacon
                .prepare(now, management_sequence, group_pending, unicast_tim_bitmap);
        if beacon.is_some() {
            #[cfg(any(feature = "diagnostics", test))]
            self.observer
                .observe(ApEngineObservationEvent::BeaconPrepared);
        }
        let frame = beacon?;
        let (tim_offset, dtim_count, _) = dtim(frame)?;
        let group_indicated = dtim_count == 0 && frame.get(tim_offset + 4).copied()? & 1 != 0;
        Some(ApBeaconPublication {
            frame,
            dtim_group_frames: if group_indicated {
                buffered_group_frames
            } else {
                0
            },
        })
    }

    pub const fn next_beacon(&self) -> Option<oer_time::Instant> {
        self.beacon.next_publication()
    }

    pub fn beacon_publication_due(&self, now: oer_time::Instant) -> bool {
        self.beacon.publication_due(now)
    }

    pub fn beacon_publication_lateness(&self, now: oer_time::Instant) -> (u64, oer_time::Duration) {
        self.beacon.publication_lateness(now)
    }

    /// Install the PTK only after message four has been MIC-verified, then
    /// atomically open the portable controlled port.
    pub fn authorize_peer<H: ApRuntimeHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        now: oer_time::Instant,
    ) -> Result<(), ApEngineError> {
        if !self.service.wpa2_authorized(peer)? {
            return Err(ApEngineError::Service(ApServiceError::WrongPeerPhase));
        }
        let ptk = self.service.pending_ptk(peer)?;
        let association_id = self
            .service
            .peer_status(peer)
            .ok_or(ApServiceError::UnknownPeer)?
            .association_id;
        self.security
            .install_pairwise(hardware, peer, association_id, ptk)?;
        self.service.authorize(peer, now)?;
        #[cfg(any(feature = "diagnostics", test))]
        self.observe(ApEngineObservationEvent::PeerAuthorized {
            authorized_peers: self.service.authorized_count(),
        });
        Ok(())
    }

    /// Resolve one received EAPOL-Key frame and publish the pairwise hardware
    /// key before reporting that the controlled port may open.
    pub fn handle_eapol<H: ApRuntimeHardware, const N: usize>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        frame: OwnedEapolFrame<N>,
        now: oer_time::Instant,
    ) -> Result<ApWpa2Outcome<N>, ApEngineError> {
        let Some(status) = self.service.peer_status(peer) else {
            return Ok(ApWpa2Outcome::None);
        };
        if status.phase != ApPeerPhase::Securing {
            // Late/replayed EAPOL is an untrusted peer event. Vendor receive
            // dispatch returns from such state branches; it does not abort
            // the PP task or surrender MAC ownership.
            return Ok(ApWpa2Outcome::None);
        }
        match self.service.on_eapol(peer, frame)? {
            ApWpa2Progress::None => Ok(ApWpa2Outcome::None),
            ApWpa2Progress::Transmit(frame) => Ok(ApWpa2Outcome::Transmit(frame)),
            ApWpa2Progress::AuthorizePeer => {
                self.authorize_peer(hardware, peer, now)?;
                Ok(ApWpa2Outcome::PeerAuthorized { peer })
            }
            ApWpa2Progress::DeauthenticatePeer => Ok(ApWpa2Outcome::DeauthenticatePeer { peer }),
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn observation(&self) -> ApEngineObservation {
        self.observer.observation
    }

    pub const fn service_address(&self) -> [u8; 6] {
        self.service.address()
    }

    #[cfg(test)]
    pub(crate) fn current_data_sequence(&self) -> SequenceNumber {
        self.service.current_data_sequence()
    }

    #[cfg(test)]
    pub(crate) fn current_qos_sequence(&self, peer: [u8; 6], tid: u8) -> Option<SequenceNumber> {
        self.service.current_qos_sequence(peer, tid)
    }

    pub fn peer_status(&self, peer: [u8; 6]) -> Option<ApPeerStatus> {
        self.service.peer_status(peer)
    }

    /// Protection this BSS requires now, from its associated peers.
    pub(crate) fn required_protection(&self) -> ApBssProtection {
        self.service.bss_protection(is_forty_mhz(self.channel))
    }

    /// Replace the protection fields of the retained beacon/probe-response
    /// template when the associated peer set changed them.
    pub(crate) fn advertise_current_protection(&mut self) -> Result<(), ApEngineError> {
        let required = self.required_protection();
        if required != self.advertised_protection {
            self.beacon
                .set_bss_protection(required)
                .map_err(|_| ApEngineError::BeaconPreparation)?;
            self.advertised_protection = required;
        }
        Ok(())
    }

    /// Transmit protection facts of this BSS: the required ERP and HT
    /// protection with the local basic rates and short-preamble capability.
    pub fn bss_protection(&self) -> BssProtection {
        let required = self.required_protection();
        let advertisement = &crate::profile::ADVERTISEMENT;
        BssProtection {
            erp: required.erp,
            ht: required.ht.mode,
            he_txop_rts_threshold: None,
            he_packet_padding: HePacketPadding::None,
            basic_rates: BasicRates::from_rate_elements(
                advertisement.legacy_rates.supported(),
                advertisement.legacy_rates.extended(),
            ),
            short_preamble: advertisement.capabilities(LinkProtection::Open) & (1 << 5) != 0,
        }
    }

    #[inline(always)]
    pub fn is_authorized_peer(&self, peer: [u8; 6]) -> bool {
        self.service.is_authorized(peer)
    }

    pub fn authorized_peer_count(&self) -> u8 {
        self.service.authorized_count()
    }

    pub fn associated_peer_count(&self) -> u8 {
        self.service.associated_count()
    }

    pub fn service_status(&self) -> oer_ieee80211_ap::AccessPointServiceStatus {
        self.service.status()
    }

    pub const fn service_status_revision(&self) -> u32 {
        self.service.status_revision()
    }

    pub fn next_peer_deadline(&self) -> Option<oer_time::Instant> {
        self.service.next_peer_deadline()
    }

    pub fn next_wpa2_retry_deadline(&self) -> Option<oer_time::Instant> {
        self.service.next_wpa2_retry_deadline()
    }

    pub fn observe_wpa2_transmit(
        &mut self,
        peer: [u8; 6],
        retransmission: bool,
        acknowledged: bool,
        now: oer_time::Instant,
    ) -> Result<(), ApEngineError> {
        if self
            .service
            .observe_wpa2_transmit(peer, retransmission, acknowledged, now)?
        {
            #[cfg(any(feature = "diagnostics", test))]
            self.observe(ApEngineObservationEvent::Wpa2ResponseWindow);
        }
        Ok(())
    }

    pub fn take_due_wpa2_retry<const N: usize>(
        &mut self,
        now: oer_time::Instant,
    ) -> Result<ApWpa2RetryProgress<N>, ApEngineError> {
        let progress = self.service.take_due_wpa2_retry(now)?;
        match progress {
            ApWpa2RetryProgress::Transmit { .. } => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::Wpa2Retransmission);
            }
            ApWpa2RetryProgress::Close(_) => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::Wpa2HandshakeTimeout);
            }
            ApWpa2RetryProgress::None => {}
        }
        Ok(progress)
    }

    pub fn observe_peer_activity(
        &mut self,
        peer: [u8; 6],
        now: oer_time::Instant,
    ) -> Result<(), ApEngineError> {
        if let Some(binding) = self.rx_peer
            && binding.peer.address() == peer
            && binding.status_revision == self.service.status_revision()
        {
            self.service
                .observe_bound_data_activity(binding.peer, now)?;
            return Ok(());
        }
        self.service.observe_activity(peer, now)?;
        Ok(())
    }

    pub fn begin_due_peer_close(&mut self, now: oer_time::Instant) -> Option<ApPeerClose> {
        let close = self.service.begin_due_peer_close(now)?;
        match close.kind {
            ApPeerCloseKind::AuthenticationTimeout => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::AuthenticationTimeout);
            }
            ApPeerCloseKind::Wpa2HandshakeTimeout => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::Wpa2HandshakeTimeout);
            }
            ApPeerCloseKind::Wpa2HandshakeFailure => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::Wpa2HandshakeFailure);
            }
            ApPeerCloseKind::InactivityTimeout => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::InactivityTimeout);
            }
            ApPeerCloseKind::AccessPointStop => {}
        }
        Some(close)
    }

    pub fn begin_wpa2_failure_close(
        &mut self,
        peer: [u8; 6],
    ) -> Result<ApPeerClose, ApEngineError> {
        let close = self.service.begin_wpa2_failure_close(peer)?;
        #[cfg(any(feature = "diagnostics", test))]
        self.observe(ApEngineObservationEvent::Wpa2HandshakeFailure);
        Ok(close)
    }

    pub fn begin_stop_peer(&mut self) -> Option<ApPeerClose> {
        if self.service.next_wpa2_retry_deadline().is_some() {
            #[cfg(any(feature = "diagnostics", test))]
            self.observe(ApEngineObservationEvent::Wpa2PendingOnStop);
        }
        self.service.begin_stop_peer()
    }

    pub fn encode_peer_disconnect(
        &mut self,
        close: ApPeerClose,
        kind: ApPeerDisconnectKind,
        reason: u16,
        output: &mut [u8],
    ) -> Result<usize, ApEngineError> {
        let status = self
            .service
            .peer_status(close.peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        if status.phase != ApPeerPhase::Closing {
            return Err(ApServiceError::WrongPeerPhase.into());
        }
        let sequence = self.service.next_management_sequence();
        let length = write_ap_peer_disconnect(
            output,
            self.service.address(),
            close.peer,
            kind,
            reason,
            sequence,
        )?;
        match kind {
            ApPeerDisconnectKind::Disassociation => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::DisassociationPrepared);
            }
            ApPeerDisconnectKind::Deauthentication => {
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::DeauthenticationPrepared);
            }
        }
        Ok(length)
    }

    pub fn complete_peer_close<H: ApRuntimeHardware>(
        &mut self,
        hardware: &mut H,
        close: ApPeerClose,
    ) -> Result<(), ApEngineError> {
        let status = self
            .service
            .peer_status(close.peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        if status.phase != ApPeerPhase::Closing {
            return Err(ApServiceError::WrongPeerPhase.into());
        }
        self.security.clear_peer(hardware, close.peer)?;
        self.service.remove_peer(close.peer)?;
        #[cfg(any(feature = "diagnostics", test))]
        self.observe(ApEngineObservationEvent::PeerRemoved);
        Ok(())
    }

    /// The generation of the access-point TSF: a value or sample of its
    /// timer taken in another generation does not describe it.
    pub const fn tsf_generation(&self) -> oer_ieee80211_lower_mac::TsfGeneration {
        self.tsf.generation()
    }

    pub fn stop<H: ApRuntimeHardware>(mut self, hardware: &mut H) -> ApEngineStop<'storage> {
        // RX admission closes before the outer descriptor epoch is allowed
        // to stop. A composition may have already performed this idempotent
        // pre-quiesce leaf while it still owned the live runner.
        disable_ap_receive_policy(hardware);
        let (security, pairwise_storage) = self.security.stop(hardware);
        self.tsf.stop(hardware);
        ApEngineStop {
            service: self.service,
            beacon_storage: self.beacon.into_storage(),
            pairwise_storage,
            security,
        }
    }
}

const fn is_forty_mhz(channel: WifiChannel) -> bool {
    !matches!(
        channel.width(),
        oer_ieee80211_mac::channel::WifiChannelWidth::Mhz20
    )
}

#[cfg(test)]
mod tests;

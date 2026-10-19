//! Bounded multi-peer AP MLME and security ownership.

use core::fmt;
use oer_ieee80211_mac::security::spp::SppScope;
use oer_ieee80211_mac::sequence::SequenceNumber;

mod block_ack;
mod management;
mod peer;
mod power_save;
mod security;
pub use management::{
    ApGroupManagementError, ApManagementAction, ApManagementProtection, ApManagementRejection,
    ApManagementRx, ApManagementRxProtection,
};
pub use security::{ApAssociationSecurityError, admit_association_security};
mod transmit;
pub use transmit::{ApEapolTransmit, ApEapolTxCompletion, ApEapolTxContext};

// Preserve the existing service-level imports while limits own their definition.
pub use crate::limits::{
    AP_MAX_CLIENTS, AP_TIM_VIRTUAL_BITMAP_OCTETS, AccessPointClientLimit,
    AccessPointClientLimitError,
};

use oer_ieee80211_mac::ap::{ApAssociationSecurityObservation, ApPowerSaveObservation};

use crate::pmksa::{AP_PMKID_LEN, ApPmksa, ApPmksaCache};
use oer_ieee80211_mac::beacon::{TimAssociationId, TimBitmapError, TimVirtualBitmap};
use oer_ieee80211_mac::block_ack::{
    AddbaRequest, BlockAckAction, OperationalTxBlockAck, TxBlockAckOriginator,
    TxBlockAckOriginatorConfig, TxBlockAckOriginatorError, TxBlockAckOriginatorPolicy,
    TxBlockAckResponse, TxBlockAckResponseDisposition, TxBlockAckRetry, next_nonzero_dialog_token,
};
use oer_ieee80211_mac::ht::HtPeerCapabilities;
use oer_ieee80211_mac::protection::{
    ApBssProtection, ErpProtection, HtOperationProtection, HtProtectionMode,
};
use oer_ieee80211_mac::security::{
    ApSecurityPolicy, AssociationAkm, AssociationSecurity, GroupManagementCipher, LinkProtection,
    RsnAssociation, SaePwe, rsn::Akm,
};
use oer_ieee80211_rsn::{
    AssociationSecurityBinding, OwnedEapolFrame, Pmk, Ptk, PtkContext,
    aes::{SoftwareAesKeyWrapError, software_aes128_key_wrap},
    element::{ValidatedRsnElement, validate_rsn_element},
    frames::{
        OwnedAssociationSecurityIes, RSN_PLAIN_KEY_DATA_CAPACITY, RsnFrameError, RsnGtk, RsnIgtk,
        RsnPlainKeyData, RsnTxFrame, build_ap_action_frame,
    },
    retry::{RsnRetry, RsnRetryAction, RsnRetryAlarm, RsnRetryConfig, RsnRetryError},
    state::{
        PtkContext as Wpa2StatePtkContext, RsnApAction, RsnApPhase, RsnApState, RsnStateError,
    },
};

pub const AP_STATUS_SUCCESS: u16 = 0;
pub const AP_STATUS_TOO_MANY_STATIONS: u16 = 17;
pub const AP_STATUS_UNSUPPORTED_RATES: u16 = 18;
pub const AP_STATUS_INVALID_RSN: u16 = 40;
/// Status 53: the station named no PMKID of a security association this
/// access point holds for it.
pub const AP_STATUS_INVALID_PMKID: u16 = 53;
pub const AP_ASSOCIATION_DEADLINE: oer_time::Duration = oer_time::Duration::from_secs(15);
/// AP-owned response window for each four-way-handshake publication.
///
/// This follows the generic hostap authenticator policy: a 100-ms first
/// EAPOL-Key response window, 1-second subsequent windows, and four total
/// publications. An acknowledged Message 1 uses the subsequent window
/// immediately. It is protocol policy, not an ESP32 hardware fact.
pub const AP_WPA2_FIRST_RETRY_INTERVAL: oer_time::Duration = oer_time::Duration::from_millis(100);
pub const AP_WPA2_SUBSEQUENT_RETRY_INTERVAL: oer_time::Duration = oer_time::Duration::from_secs(1);
/// Retransmissions after the original M1 or M3 publication.
pub const AP_WPA2_RETRY_ATTEMPTS: u8 = 3;
pub const AP_TX_BLOCK_ACK_TID: u8 = 0;
/// Bounded AP downlink window used for both negotiation and aggregate
/// admission. AP and STA use the same 32-MPDU production aggregate contract;
/// duplex fairness must be provided by the common MAC transaction scheduler,
/// not by weakening the AP's negotiated BlockAck capability.
pub const AP_TX_BLOCK_ACK_WINDOW: u16 = 32;
pub const AP_TX_BLOCK_ACK_NEGOTIATION_TIMEOUT: oer_time::Duration =
    oer_time::Duration::from_millis(100);

/// Validated inactivity policy for an associated SoftAP peer.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct AccessPointInactiveTimeout(u16);

impl AccessPointInactiveTimeout {
    pub const MIN_SECONDS: u16 = 10;
    pub const MAX_SECONDS: u16 = 3_600;
    pub const DEFAULT_SECONDS: u16 = 300;

    pub const fn new(seconds: u16) -> Result<Self, AccessPointInactiveTimeoutError> {
        if seconds < Self::MIN_SECONDS || seconds > Self::MAX_SECONDS {
            return Err(AccessPointInactiveTimeoutError { seconds });
        }
        Ok(Self(seconds))
    }

    pub const fn seconds(self) -> u16 {
        self.0
    }

    pub const fn duration(self) -> oer_time::Duration {
        oer_time::Duration::from_secs(self.0 as u32)
    }
}

impl Default for AccessPointInactiveTimeout {
    fn default() -> Self {
        Self(Self::DEFAULT_SECONDS)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointInactiveTimeoutError {
    seconds: u16,
}

impl AccessPointInactiveTimeoutError {
    pub const fn seconds(self) -> u16 {
        self.seconds
    }
}

impl fmt::Display for AccessPointInactiveTimeoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "access-point inactivity timeout {} is outside {}..={} seconds",
            self.seconds,
            AccessPointInactiveTimeout::MIN_SECONDS,
            AccessPointInactiveTimeout::MAX_SECONDS,
        )
    }
}

impl core::error::Error for AccessPointInactiveTimeoutError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPeerPhase {
    Authenticated,
    Securing,
    Authorized,
    Closing,
}

/// AP-visible power-management state for one associated peer.
///
/// This state follows the peer's most recently admitted PM bit. It does not
/// imply that a frame has already been moved into or out of the caller-owned
/// downlink queue.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ApPeerPowerState {
    #[default]
    Active,
    Sleeping,
}

/// Whether a newly arrived unicast frame may be transmitted immediately or
/// must remain in the caller-owned AP power-save queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApDownlinkDisposition {
    TransmitNow,
    Buffer,
}

/// Non-reusable identity of one associated AP peer.
///
/// The association ID names a bounded peer-table slot, but that slot can be
/// reused after teardown. The epoch fences retained data and scheduler state
/// from a later association which happens to reuse the same slot or address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAssociationIdentity {
    address: [u8; 6],
    association_id: u16,
    association_epoch: u32,
}

impl ApAssociationIdentity {
    pub const fn new(
        address: [u8; 6],
        association_id: u16,
        association_epoch: u32,
    ) -> Option<Self> {
        if address[0] & 1 != 0
            || association_id == 0
            || association_id as usize > AP_MAX_CLIENTS
            || association_epoch == 0
        {
            return None;
        }
        Some(Self {
            address,
            association_id,
            association_epoch,
        })
    }

    pub const fn address(self) -> [u8; 6] {
        self.address
    }

    pub const fn association_id(self) -> u16 {
        self.association_id
    }

    pub const fn association_epoch(self) -> u32 {
        self.association_epoch
    }
}

/// Ownership decision for one authorized unicast downlink.
///
/// Keeping the generation-bound identity beside the power-save decision lets
/// a caller retain the payload without rescanning the peer table or reducing
/// its owner to a reusable MAC address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApDownlinkAdmission {
    identity: ApAssociationIdentity,
    disposition: ApDownlinkDisposition,
}

impl ApDownlinkAdmission {
    pub const fn identity(self) -> ApAssociationIdentity {
        self.identity
    }

    pub const fn disposition(self) -> ApDownlinkDisposition {
        self.disposition
    }
}

/// Semantic result of one admitted PM-bit or PS-Poll observation.
///
/// `ReleaseOne` reserves exactly one buffered-frame count until
/// [`AccessPointService::complete_buffered_unicast_release`] commits or rolls
/// the reservation back. The token is deliberately non-`Copy`.
#[derive(Debug, Eq, PartialEq)]
pub enum ApPowerSaveAction {
    None,
    StateChanged {
        peer: [u8; 6],
        state: ApPeerPowerState,
        buffered_frames: u16,
    },
    ReleaseOne(ApBufferedUnicastRelease),
}

/// Affine reservation for one caller-owned buffered unicast frame.
#[derive(Debug, Eq, PartialEq)]
pub struct ApBufferedUnicastRelease {
    identity: ApAssociationIdentity,
    more_data: bool,
}

impl ApBufferedUnicastRelease {
    pub const fn peer(&self) -> [u8; 6] {
        self.identity.address()
    }

    pub const fn association_id(&self) -> u16 {
        self.identity.association_id()
    }

    pub const fn identity(&self) -> ApAssociationIdentity {
        self.identity
    }

    /// Value for the 802.11 More Data bit on the released MPDU.
    pub const fn more_data(&self) -> bool {
        self.more_data
    }
}

/// Affine reservation for one caller-owned buffered group frame.
///
/// The AP service owns only the advertised count. The caller must retain the
/// exact multicast/broadcast payload before committing it, then bind the
/// oldest retained payload to this token after a successfully published DTIM
/// beacon. Dropping the token deliberately leaves the release blocked.
#[derive(Debug, Eq, PartialEq)]
pub struct ApBufferedGroupRelease {
    generation: u32,
    more_data: bool,
}

impl ApBufferedGroupRelease {
    /// Value for the 802.11 More Data bit on this group-addressed MPDU.
    pub const fn more_data(&self) -> bool {
        self.more_data
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPeerCloseKind {
    AuthenticationTimeout,
    Wpa2HandshakeFailure,
    Wpa2HandshakeTimeout,
    InactivityTimeout,
    AccessPointStop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApPeerClose {
    pub peer: [u8; 6],
    pub kind: ApPeerCloseKind,
    pub was_associated: bool,
    pub maximum_legacy_rate_500kbps: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApAssociationCapabilities {
    pub maximum_legacy_rate_500kbps: u8,
    /// Capability Information Short Preamble of the Association Request.
    pub short_preamble: bool,
    pub ht: Option<HtPeerCapabilities>,
    pub qos_supported: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApMlmeAction {
    AuthenticationResponse {
        peer: [u8; 6],
        status: u16,
    },
    AssociationResponse {
        peer: [u8; 6],
        status: u16,
        association_id: Option<u16>,
    },
    BeginWpa2 {
        peer: [u8; 6],
    },
    PeerRemoved {
        peer: [u8; 6],
    },
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApServiceError {
    UnknownPeer,
    WrongPeerPhase,
    SecurityModeMismatch,
    AssociationIdMismatch,
    BufferedTrafficOverflow,
    NoBufferedTraffic,
    BufferedReleaseInFlight,
    StaleBufferedRelease,
    Wpa2(RsnStateError),
    BlockAck(TxBlockAckOriginatorError),
}

impl From<TxBlockAckOriginatorError> for ApServiceError {
    fn from(error: TxBlockAckOriginatorError) -> Self {
        Self::BlockAck(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApWpa2Error {
    Service(ApServiceError),
    Frame(RsnFrameError),
    KeyWrap(SoftwareAesKeyWrapError),
    MissingPairwiseKey,
    UnexpectedAction,
    Retry(RsnRetryError),
}

impl From<RsnRetryError> for ApWpa2Error {
    fn from(error: RsnRetryError) -> Self {
        Self::Retry(error)
    }
}

impl From<ApServiceError> for ApWpa2Error {
    fn from(error: ApServiceError) -> Self {
        Self::Service(error)
    }
}

impl From<RsnStateError> for ApWpa2Error {
    fn from(error: RsnStateError) -> Self {
        Self::Service(ApServiceError::Wpa2(error))
    }
}

impl From<RsnFrameError> for ApWpa2Error {
    fn from(error: RsnFrameError) -> Self {
        Self::Frame(error)
    }
}

impl From<SoftwareAesKeyWrapError> for ApWpa2Error {
    fn from(error: SoftwareAesKeyWrapError) -> Self {
        Self::KeyWrap(error)
    }
}

pub enum ApWpa2Progress<const N: usize> {
    None,
    Transmit(ApEapolTransmit<N>),
    AuthorizePeer,
    DeauthenticatePeer,
}

pub enum ApWpa2RetryProgress<const N: usize> {
    None,
    Transmit(ApEapolTransmit<N>),
    Close(ApPeerClose),
}

impl From<RsnStateError> for ApServiceError {
    fn from(error: RsnStateError) -> Self {
        Self::Wpa2(error)
    }
}

struct ApPeer {
    address: [u8; 6],
    association_id: u16,
    association_epoch: u32,
    phase: ApPeerPhase,
    wpa2: Option<RsnApState>,
    /// The PMK of a WPA3 station: from its accepted SAE exchange, or the
    /// cached one its association resumed.
    pmk: Option<Pmk>,
    /// The PMKID of `pmk`.
    pmkid: Option<[u8; AP_PMKID_LEN]>,
    association_security_binding: Option<AssociationSecurityBinding>,
    pending_ptk: Option<Ptk>,
    wpa2_retry: RsnRetry,
    wpa2_retry_alarm: Option<RsnRetryAlarm>,
    maximum_legacy_rate_500kbps: u8,
    short_preamble: bool,
    ht: Option<HtPeerCapabilities>,
    qos_supported: bool,
    /// Independent QoS sequence spaces for this receiver's TIDs. TX BlockAck
    /// and receiver reorder state are peer+TID agreements; sharing these
    /// counters across AP clients creates artificial holes whenever the
    /// scheduler switches peers.
    next_qos_sequences: [SequenceNumber; 8],
    tx_block_ack: TxBlockAckOriginator<1>,
    power_state: ApPeerPowerState,
    buffered_unicast_frames: u16,
    buffered_release_in_flight: bool,
    last_activity: oer_time::Instant,
    deadline: oer_time::Instant,
}

/// Caller-owned storage for all per-client AP protocol and key state.
///
/// This is intentionally separate from [`AccessPointService`]. On embedded
/// targets the table is large enough that constructing or moving it through a
/// cooperative task stack is unsafe; the radio integration gives it a stable
/// static address instead.
pub struct AccessPointPeerStorage {
    peers: [Option<ApPeer>; AP_MAX_CLIENTS],
    /// Independent management replay frontiers, indexed by the owned peer
    /// slots. Keeping them beside the table avoids padding every peer.
    management_replay: [management::ApManagementReplay; AP_MAX_CLIENTS],
    /// The original Message 3 frontier, retained across its retransmissions.
    message3_management_ipns: [[u8; 6]; AP_MAX_CLIENTS],
    generation: u32,
    /// The security associations a WPA3 epoch's stations may resume.
    pmksa: ApPmksaCache,
}

impl AccessPointPeerStorage {
    pub const fn new() -> Self {
        Self {
            peers: [const { None }; AP_MAX_CLIENTS],
            management_replay: [const { management::ApManagementReplay::inactive() };
                AP_MAX_CLIENTS],
            message3_management_ipns: [[0; 6]; AP_MAX_CLIENTS],
            generation: 0,
            pmksa: ApPmksaCache::new(),
        }
    }

    /// Lend this complete table to one AP lifecycle. The affine owner clears
    /// retained peer secrets when returned or dropped; it can move directly
    /// from a retired service into the next service.
    ///
    /// ```compile_fail,E0499
    /// use oer_ieee80211_ap::AccessPointPeerStorage;
    /// fn twice(storage: &mut AccessPointPeerStorage) {
    ///     let first = storage.lend();
    ///     let second = storage.lend();
    ///     drop(first);
    /// }
    /// ```
    ///
    /// ```
    /// use oer_ieee80211_ap::AccessPointPeerStorage;
    /// fn sequential(storage: &mut AccessPointPeerStorage) {
    ///     let first = storage.lend();
    ///     drop(first);
    ///     let second = storage.lend();
    ///     drop(second);
    /// }
    /// ```
    pub fn lend(&mut self) -> AccessPointPeers<'_> {
        AccessPointPeers { storage: self }
    }

    fn clear(&mut self) {
        self.peers.fill_with(|| None);
        self.message3_management_ipns.fill([0; 6]);
        self.management_replay
            .iter_mut()
            .for_each(|replay| *replay = management::ApManagementReplay::inactive());
        self.pmksa = ApPmksaCache::new();
    }
}

/// Exclusive ownership of a caller-placed AP peer table. It has no empty
/// state and never copies the large table onto an executor stack.
///
/// The service returns this owner by value after clearing secrets. Dropping
/// an active service clears them through the same guard; hardware key removal
/// remains the runtime's separate obligation.
///
/// ```compile_fail,E0382
/// use oer_ieee80211_ap::AccessPointPeers;
/// fn duplicate(peers: AccessPointPeers<'_>) {
///     let first = peers;
///     let second = peers;
/// }
/// ```
///
/// ```
/// use oer_ieee80211_ap::AccessPointPeers;
/// fn transfer(peers: AccessPointPeers<'_>) -> AccessPointPeers<'_> { peers }
/// ```
#[must_use = "retain the table owner for the service or its restart bank"]
pub struct AccessPointPeers<'peers> {
    storage: &'peers mut AccessPointPeerStorage,
}

impl Drop for AccessPointPeers<'_> {
    fn drop(&mut self) {
        self.storage.clear();
    }
}

impl Default for AccessPointPeerStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl ApPeer {
    fn status(&self) -> ApPeerStatus {
        ApPeerStatus {
            address: self.address,
            association_id: self.association_id,
            association_epoch: self.association_epoch,
            phase: self.phase,
            maximum_legacy_rate_500kbps: self.maximum_legacy_rate_500kbps,
            short_preamble: self.short_preamble,
            ht: self.ht,
            qos_supported: self.qos_supported,
            tx_block_ack: self.tx_block_ack.operational(AP_TX_BLOCK_ACK_TID),
            tx_block_ack_generation: self.tx_block_ack.generation(AP_TX_BLOCK_ACK_TID),
            power_state: self.power_state,
            buffered_unicast_frames: self.buffered_unicast_frames,
            buffered_release_in_flight: self.buffered_release_in_flight,
            last_activity: self.last_activity,
            deadline: self.deadline,
        }
    }

    fn authenticated(
        address: [u8; 6],
        association_id: u16,
        association_epoch: u32,
        now: oer_time::Instant,
    ) -> Self {
        Self {
            address,
            association_id,
            association_epoch,
            phase: ApPeerPhase::Authenticated,
            wpa2: None,
            pmk: None,
            pmkid: None,
            association_security_binding: None,
            pending_ptk: None,
            wpa2_retry: new_ap_wpa2_retry(),
            wpa2_retry_alarm: None,
            // Authentication precedes rate negotiation. Keep the universally
            // compatible 1-Mbit/s value until Association succeeds.
            maximum_legacy_rate_500kbps: 2,
            short_preamble: false,
            ht: None,
            qos_supported: false,
            next_qos_sequences: [SequenceNumber::ZERO; 8],
            tx_block_ack: new_ap_tx_block_ack(),
            power_state: ApPeerPowerState::Active,
            buffered_unicast_frames: 0,
            buffered_release_in_flight: false,
            last_activity: now,
            deadline: now.saturating_add(AP_ASSOCIATION_DEADLINE),
        }
    }

    const fn association_identity(&self) -> ApAssociationIdentity {
        ApAssociationIdentity {
            address: self.address,
            association_id: self.association_id,
            association_epoch: self.association_epoch,
        }
    }
}

/// O(1) identity of one peer-table generation.
///
/// A slot may be reused after disconnect, so its index alone is not authority.
/// Every bound access validates both generation and address before exposing
/// peer state to a hot data-path operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApPeerBinding {
    index: u8,
    generation: u32,
    address: [u8; 6],
}

impl ApPeerBinding {
    pub const fn address(self) -> [u8; 6] {
        self.address
    }
}

/// The access point's one TX Block Ack TID, its Dialog Tokens skipping zero.
const AP_TX_BLOCK_ACK_POLICY: TxBlockAckOriginatorPolicy = TxBlockAckOriginatorPolicy {
    tids: &[AP_TX_BLOCK_ACK_TID],
    first_dialog_token: 1,
    next_dialog_token: next_nonzero_dialog_token,
};

fn new_ap_tx_block_ack() -> TxBlockAckOriginator<1> {
    match TxBlockAckOriginator::new(
        AP_TX_BLOCK_ACK_POLICY,
        TxBlockAckOriginatorConfig {
            window: AP_TX_BLOCK_ACK_WINDOW,
            negotiation_timeout: AP_TX_BLOCK_ACK_NEGOTIATION_TIMEOUT,
            // Baseline 3,839-byte A-MSDU construction and AP RX decapsulation
            // are both source-owned. The operational agreement still keeps
            // this bit false unless the peer echoes support in its ADDBA
            // response.
            amsdu_tids: 1 << AP_TX_BLOCK_ACK_TID,
        },
    ) {
        Ok(originator) => originator,
        Err(_) => panic!("valid AP TX BlockAck policy"),
    }
}

const fn new_ap_wpa2_retry() -> RsnRetry {
    match RsnRetry::new(RsnRetryConfig {
        first_interval: AP_WPA2_FIRST_RETRY_INTERVAL,
        subsequent_interval: AP_WPA2_SUBSEQUENT_RETRY_INTERVAL,
        attempts: AP_WPA2_RETRY_ATTEMPTS,
    }) {
        Ok(retry) => retry,
        Err(_) => panic!("valid AP WPA2 retry policy"),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApPeerStatus {
    pub address: [u8; 6],
    pub association_id: u16,
    /// Non-reusable identity of this AID assignment. Reauthentication keeps
    /// the bounded AID slot but advances this epoch so receive-side duplicate
    /// history cannot cross association ownership.
    pub association_epoch: u32,
    pub phase: ApPeerPhase,
    pub maximum_legacy_rate_500kbps: u8,
    pub short_preamble: bool,
    pub ht: Option<HtPeerCapabilities>,
    pub qos_supported: bool,
    pub tx_block_ack: Option<OperationalTxBlockAck>,
    /// Generation of the peer's TX Block Ack session; with an operational
    /// `tx_block_ack` it identifies that exact agreement.
    pub tx_block_ack_generation: u32,
    pub power_state: ApPeerPowerState,
    pub buffered_unicast_frames: u16,
    pub buffered_release_in_flight: bool,
    pub last_activity: oer_time::Instant,
    pub deadline: oer_time::Instant,
}

impl ApPeerStatus {
    pub const fn association_identity(self) -> ApAssociationIdentity {
        ApAssociationIdentity {
            address: self.address,
            association_id: self.association_id,
            association_epoch: self.association_epoch,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointServiceStatus {
    pub security: LinkProtection,
    pub client_limit: AccessPointClientLimit,
    pub associated: u8,
    pub authorized: u8,
    pub buffered_group_frames: u16,
    pub peers: [Option<ApPeerStatus>; AP_MAX_CLIENTS],
}

/// Complete portable owner for one AP service epoch.
///
/// `S` is the backend's SPP granularity. Interface-scoped backends require
/// all clients to use one mode; the current profiles advertise no SPP and
/// explicitly refuse Required peers. A future SPP profile on that scope
/// must require Negotiated mode for every client. Association-scoped backends
/// can select independently. No chip identity participates in this decision.
///
/// Dropping this value clears the PMK and GTK through their zeroize-on-drop
/// implementations. A chip runtime still must clear its typed hardware slots
/// before it may classify the corresponding physical owner as stopped.
pub struct AccessPointService<'peers, S: SppScope> {
    spp_scope: core::marker::PhantomData<S>,
    address: [u8; 6],
    security: AccessPointSecurityMaterial,
    group_management: Option<oer_ieee80211_rsn::bip::BipTransmitter>,
    peer_storage: AccessPointPeers<'peers>,
    client_limit: AccessPointClientLimit,
    inactive_timeout: AccessPointInactiveTimeout,
    next_management_sequence: SequenceNumber,
    next_data_sequence: SequenceNumber,
    status_revision: u32,
    associated_count: u8,
    authorized_count: u8,
    buffered_group_frames: u16,
    buffered_group_release_generation: u32,
    buffered_group_release_in_flight: bool,
    operational_tx_block_ack: bool,
}

/// Credential ownership for one AP epoch. Open deliberately has no PMK, GTK
/// or placeholder key bytes that could be installed by a later generic path.
/// A WPA3 epoch has no BSS-wide PMK: each station's comes from SAE, and the
/// peer storage keeps the security associations stations may resume.
pub enum AccessPointSecurityMaterial {
    Open,
    Wpa2Personal { pmk: Pmk, gtk: RsnGtk },
    Wpa3Personal { gtk: RsnGtk, igtk: RsnIgtk },
}

impl<'peers, S: SppScope> AccessPointService<'peers, S> {
    pub fn new(
        address: [u8; 6],
        pmk: Pmk,
        gtk: RsnGtk,
        client_limit: AccessPointClientLimit,
        inactive_timeout: AccessPointInactiveTimeout,
        peer_storage: AccessPointPeers<'peers>,
    ) -> Self {
        Self::with_security(
            address,
            AccessPointSecurityMaterial::Wpa2Personal { pmk, gtk },
            client_limit,
            inactive_timeout,
            peer_storage,
        )
    }

    /// A WPA3-Personal epoch: stations authenticate by SAE, and the IGTK
    /// protects group-addressed management frames.
    pub fn new_wpa3(
        address: [u8; 6],
        gtk: RsnGtk,
        igtk: RsnIgtk,
        client_limit: AccessPointClientLimit,
        inactive_timeout: AccessPointInactiveTimeout,
        peer_storage: AccessPointPeers<'peers>,
    ) -> Self {
        Self::with_security(
            address,
            AccessPointSecurityMaterial::Wpa3Personal { gtk, igtk },
            client_limit,
            inactive_timeout,
            peer_storage,
        )
    }

    pub fn new_open(
        address: [u8; 6],
        client_limit: AccessPointClientLimit,
        inactive_timeout: AccessPointInactiveTimeout,
        peer_storage: AccessPointPeers<'peers>,
    ) -> Self {
        Self::with_security(
            address,
            AccessPointSecurityMaterial::Open,
            client_limit,
            inactive_timeout,
            peer_storage,
        )
    }

    fn with_security(
        address: [u8; 6],
        security: AccessPointSecurityMaterial,
        client_limit: AccessPointClientLimit,
        inactive_timeout: AccessPointInactiveTimeout,
        peer_storage: AccessPointPeers<'peers>,
    ) -> Self {
        peer_storage.storage.clear();
        peer_storage.storage.generation = peer_storage
            .storage
            .generation
            .checked_add(1)
            .expect("AP peer generation space is not reusable");
        let group_management = match &security {
            AccessPointSecurityMaterial::Wpa3Personal { igtk, .. } => {
                Some(oer_ieee80211_rsn::bip::BipTransmitter::new(igtk))
            }
            AccessPointSecurityMaterial::Open
            | AccessPointSecurityMaterial::Wpa2Personal { .. } => None,
        };
        Self {
            spp_scope: core::marker::PhantomData,
            address,
            security,
            group_management,
            peer_storage,
            client_limit,
            inactive_timeout,
            next_management_sequence: SequenceNumber::ZERO,
            next_data_sequence: SequenceNumber::ZERO,
            status_revision: 0,
            associated_count: 0,
            authorized_count: 0,
            buffered_group_frames: 0,
            buffered_group_release_generation: 0,
            buffered_group_release_in_flight: false,
            operational_tx_block_ack: false,
        }
    }

    /// The security this BSS offers.
    pub const fn security_policy(&self) -> ApSecurityPolicy {
        match &self.security {
            AccessPointSecurityMaterial::Open => ApSecurityPolicy::Open,
            AccessPointSecurityMaterial::Wpa2Personal { .. } => ApSecurityPolicy::Wpa2Personal,
            AccessPointSecurityMaterial::Wpa3Personal { .. } => ApSecurityPolicy::Wpa3Personal,
        }
    }

    pub const fn link_protection(&self) -> LinkProtection {
        self.security_policy().link_protection()
    }

    pub const fn address(&self) -> [u8; 6] {
        self.address
    }

    pub const fn client_limit(&self) -> AccessPointClientLimit {
        self.client_limit
    }

    pub fn peer_status(&self, address: [u8; 6]) -> Option<ApPeerStatus> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .find(|peer| peer.address == address)
            .map(ApPeer::status)
    }

    /// Resolve a peer address once at aggregate admission. Subsequent MPDUs
    /// use [`Self::bound_peer_status`] instead of rescanning the table.
    pub fn bind_peer(&self, address: [u8; 6]) -> Option<ApPeerBinding> {
        let index = self.peer_index(address)?;
        self.storage().peers[index].as_ref()?;
        Some(ApPeerBinding {
            index: u8::try_from(index).ok()?,
            generation: self.storage().generation,
            address,
        })
    }

    pub fn bound_peer_status(&self, binding: ApPeerBinding) -> Option<ApPeerStatus> {
        let peer = self.bound_peer(binding)?;
        Some(peer.status())
    }

    pub fn peers(&self) -> impl Iterator<Item = ApPeerStatus> + '_ {
        self.storage().peers.iter().flatten().map(ApPeer::status)
    }

    /// Protection this BSS requires of every transmitter, derived from its
    /// associated peers.
    ///
    /// A peer without ERP-OFDM rates is non-ERP and requires ERP protection;
    /// Barker_Preamble_Mode follows a non-ERP peer that cannot receive short
    /// preambles. A peer without HT Capabilities places the HT BSS in non-HT
    /// mixed mode; otherwise a 20-MHz-only HT peer of a 40-MHz BSS selects
    /// 20-MHz protection. Overlapping legacy BSS detection (Nonmember mode) is
    /// not observed by this service. Authenticated peers are not members.
    pub fn bss_protection(&self, forty_mhz_bss: bool) -> ApBssProtection {
        let mut non_erp_present = false;
        let mut barker_preamble = false;
        let mut non_ht_present = false;
        let mut twenty_mhz_only = false;
        let mut non_greenfield_present = false;
        for peer in self
            .peers()
            .filter(|peer| peer.phase != ApPeerPhase::Authenticated)
        {
            let non_erp = peer.maximum_legacy_rate_500kbps <= 22;
            non_erp_present |= non_erp;
            barker_preamble |= non_erp && !peer.short_preamble;
            match peer.ht {
                None => non_ht_present = true,
                Some(ht) => {
                    twenty_mhz_only |= !ht.supports_40_mhz();
                    non_greenfield_present |= !ht.supports_greenfield();
                }
            }
        }
        let mode = if non_ht_present {
            HtProtectionMode::NonHtMixed
        } else if forty_mhz_bss && twenty_mhz_only {
            HtProtectionMode::TwentyMhz
        } else {
            HtProtectionMode::None
        };
        ApBssProtection {
            non_erp_present,
            erp: ErpProtection::new(non_erp_present, barker_preamble),
            ht: HtOperationProtection {
                mode,
                non_greenfield_present,
            },
        }
    }

    pub const fn has_operational_tx_block_ack(&self) -> bool {
        self.operational_tx_block_ack
    }

    /// Operational downlink Block Ack window of one peer.
    ///
    /// A scheduler collecting a batch for this destination must not wait for
    /// more frames than its agreement admits; a peer without an operational
    /// agreement (or a group destination) receives single MPDUs.
    pub fn operational_tx_block_ack_window(&self, peer: [u8; 6]) -> Option<u16> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .find(|existing| existing.address == peer)
            .and_then(|existing| existing.tx_block_ack.operational(AP_TX_BLOCK_ACK_TID))
            .map(|agreement| agreement.window)
    }

    pub const fn associated_count(&self) -> u8 {
        self.associated_count
    }

    pub const fn authorized_count(&self) -> u8 {
        self.authorized_count
    }

    #[inline(always)]
    pub fn is_authorized(&self, address: [u8; 6]) -> bool {
        self.storage()
            .peers
            .iter()
            .flatten()
            .any(|peer| peer.address == address && peer.phase == ApPeerPhase::Authorized)
    }

    pub fn status(&self) -> AccessPointServiceStatus {
        let mut peers = [None; AP_MAX_CLIENTS];
        for (destination, source) in peers.iter_mut().zip(self.storage().peers.iter()) {
            *destination = source.as_ref().map(ApPeer::status);
        }
        AccessPointServiceStatus {
            security: self.link_protection(),
            client_limit: self.client_limit,
            associated: self.associated_count,
            authorized: self.authorized_count,
            buffered_group_frames: self.buffered_group_frames,
            peers,
        }
    }

    /// Monotonic public peer-table revision for cheap change detection.
    pub const fn status_revision(&self) -> u32 {
        self.status_revision
    }

    /// Refresh summaries on the rare peer-state mutation, keeping scheduler
    /// and link-state queries O(1) on the saturated data path.
    fn revise_status(&mut self) {
        let mut associated = 0_u8;
        let mut authorized = 0_u8;
        let mut operational_tx_block_ack = false;
        for peer in self.storage().peers.iter().flatten() {
            if matches!(peer.phase, ApPeerPhase::Securing | ApPeerPhase::Authorized) {
                associated = associated.saturating_add(1);
            }
            if peer.phase == ApPeerPhase::Authorized {
                authorized = authorized.saturating_add(1);
            }
            operational_tx_block_ack |=
                peer.tx_block_ack.operational(AP_TX_BLOCK_ACK_TID).is_some();
        }
        self.associated_count = associated;
        self.authorized_count = authorized;
        self.operational_tx_block_ack = operational_tx_block_ack;
        self.status_revision = self.status_revision.wrapping_add(1);
    }

    fn checked_peer(&self, peer: [u8; 6]) -> Result<&ApPeer, ApServiceError> {
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        self.storage().peers[index]
            .as_ref()
            .ok_or(ApServiceError::UnknownPeer)
    }

    fn checked_peer_mut(&mut self, peer: [u8; 6]) -> Result<&mut ApPeer, ApServiceError> {
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        self.storage_mut().peers[index]
            .as_mut()
            .ok_or(ApServiceError::UnknownPeer)
    }

    fn peer_index(&self, peer: [u8; 6]) -> Option<usize> {
        self.storage()
            .peers
            .iter()
            .position(|existing| existing.as_ref().is_some_and(|value| value.address == peer))
    }

    fn bound_association(&self, identity: ApAssociationIdentity) -> Option<&ApPeer> {
        let index = usize::from(identity.association_id.checked_sub(1)?);
        self.storage().peers.get(index)?.as_ref().filter(|peer| {
            peer.address == identity.address
                && peer.association_id == identity.association_id
                && peer.association_epoch == identity.association_epoch
        })
    }

    fn bound_association_mut(&mut self, identity: ApAssociationIdentity) -> Option<&mut ApPeer> {
        let index = usize::from(identity.association_id.checked_sub(1)?);
        self.storage_mut()
            .peers
            .get_mut(index)?
            .as_mut()
            .filter(|peer| {
                peer.address == identity.address
                    && peer.association_id == identity.association_id
                    && peer.association_epoch == identity.association_epoch
            })
    }

    /// Validate a retained association identity without scanning the peer
    /// table. This is the queue-generation fence used immediately before a
    /// caller releases old payload ownership into a new radio transaction.
    pub fn association_is_current(&self, identity: ApAssociationIdentity) -> bool {
        self.bound_association(identity)
            .is_some_and(|peer| peer.phase == ApPeerPhase::Authorized)
    }

    /// Return the current authorized state after the same O(1) identity
    /// validation when the caller also needs power-save or BA metadata.
    pub fn bound_authorized_peer_status(
        &self,
        identity: ApAssociationIdentity,
    ) -> Option<ApPeerStatus> {
        let peer = self.bound_association(identity)?;
        (peer.phase == ApPeerPhase::Authorized).then(|| peer.status())
    }

    fn bound_peer(&self, binding: ApPeerBinding) -> Option<&ApPeer> {
        if self.storage().generation != binding.generation {
            return None;
        }
        self.storage()
            .peers
            .get(usize::from(binding.index))?
            .as_ref()
            .filter(|peer| peer.address == binding.address)
    }

    fn bound_peer_mut(&mut self, binding: ApPeerBinding) -> Option<&mut ApPeer> {
        if self.storage().generation != binding.generation {
            return None;
        }
        self.storage_mut()
            .peers
            .get_mut(usize::from(binding.index))?
            .as_mut()
            .filter(|peer| peer.address == binding.address)
    }

    fn advance_peer_generation(&mut self) {
        let generation = self
            .storage()
            .generation
            .checked_add(1)
            .expect("AP peer generation space is not reusable");
        self.storage_mut().generation = generation;
    }

    fn occupied_count(&self) -> u8 {
        self.storage().peers.iter().flatten().count() as u8
    }

    /// End the service epoch, clear per-peer and PMKSA secrets and return
    /// the complete table owner for a later AP materialization. The identity
    /// generation frontier stays with the table across this transition.
    pub fn into_peer_storage(self) -> AccessPointPeers<'peers> {
        let peers = self.peer_storage;
        peers.storage.clear();
        peers
    }

    fn storage(&self) -> &AccessPointPeerStorage {
        self.peer_storage.storage
    }

    fn storage_mut(&mut self) -> &mut AccessPointPeerStorage {
        self.peer_storage.storage
    }
}

#[cfg(test)]
mod tests;

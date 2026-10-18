//! Finite AP protocol/TX transaction above the shared ESP32-S31 descriptor.
//!
//! This owner is the boundary consumed by an executor: RX supplies complete
//! MPDUs, a timer supplies beacon timestamps and IRQ supplies TX progress.
//! It never calls an executor and never reports a publication as transmitted
//! before the descriptor reaches a successful terminal completion.

use crate::{
    ampdu::ApAggregateAdmission,
    engine::{ApEngine, ApEngineError, ApManagementOutcome},
    tx::{
        ApTx, ApTxClass, ApTxConfig, ApTxError, ApTxParked, peer_ht_duplicate_tx_selection,
        peer_ht_rate, peer_legacy_rate,
    },
};

use oer_esp32s31_ieee80211::{
    ordinary_tx::{
        OrdinaryTxOutcome, TX_CCMP_MIC_SIZE, TX_FCS_SIZE, TX_METADATA_SIZE, WifiTxEntropy,
        WifiTxPowerProfile, WifiTxResources,
    },
    tx::{WifiTxProgress, WifiTxWake},
};

use oer_esp32s31_ieee80211_mac::tx::{
    HtDuplicateCertificationRequest, HtDuplicateTxSelection, HtRate, LegacyRate, TxHardware,
};

use oer_ieee80211_mac::{ap::ApPeerDisconnectKind, security::LinkProtection};

use oer_ieee80211_ap::{ApPeerClose, ApPeerPowerState, ApServiceError};

use oer_ieee80211_rsn::frames::RsnTxFrame;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingPublication {
    Beacon {
        dtim_group_frames: u16,
    },
    Authentication,
    ProbeResponse {
        receiver: [u8; 6],
        sequence_control: u16,
        started_at: oer_time::Instant,
    },
    Association {
        peer: [u8; 6],
        begin_wpa2: bool,
    },
    Eapol {
        peer: [u8; 6],
        retransmission: bool,
    },
    Data {
        peer: [u8; 6],
    },
    BlockAckRequest {
        peer: [u8; 6],
    },
    RxBlockAckResponse,
    PeerDisconnect {
        close: ApPeerClose,
        stage: ApPeerDisconnectStage,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPendingPublicationKind {
    Beacon,
    Authentication,
    ProbeResponse,
    Association,
    Eapol,
    Data,
    BlockAckRequest,
    RxBlockAckResponse,
    PeerDisconnect,
}

impl PendingPublication {
    const fn kind(self) -> ApPendingPublicationKind {
        match self {
            Self::Beacon { .. } => ApPendingPublicationKind::Beacon,
            Self::ProbeResponse { .. } => ApPendingPublicationKind::ProbeResponse,
            Self::Authentication => ApPendingPublicationKind::Authentication,
            Self::Association { .. } => ApPendingPublicationKind::Association,
            Self::Eapol { .. } => ApPendingPublicationKind::Eapol,
            Self::Data { .. } => ApPendingPublicationKind::Data,
            Self::BlockAckRequest { .. } => ApPendingPublicationKind::BlockAckRequest,
            Self::RxBlockAckResponse => ApPendingPublicationKind::RxBlockAckResponse,
            Self::PeerDisconnect { .. } => ApPendingPublicationKind::PeerDisconnect,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApPeerDisconnectStage {
    Disassociation,
    Deauthentication,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApMacObservation {
    pub beacons_transmitted: u32,
    pub authentication_responses_transmitted: u32,
    pub association_responses_transmitted: u32,
    pub eapol_frames_transmitted: u32,
    pub data_frames_transmitted: u32,
    pub rx_block_ack_responses_transmitted: u32,
    pub data_tx: ApDataTxObservation,
    /// Fixed MCS32 planner requests evaluated for AP aggregate admissions.
    pub ht_duplicate_tx_requests: u32,
    /// Latest typed selection or rejection; never feeds ordinary rate ranking.
    pub ht_duplicate_tx_selection: HtDuplicateTxSelection,
    /// Disconnect transactions accepted by the hardware TX owner.
    pub disassociations_published: u32,
    pub deauthentications_published: u32,
    /// Disconnect transactions whose terminal completion reported an ACK.
    pub disassociations_acknowledged: u32,
    pub deauthentications_acknowledged: u32,
    pub tx_failures: ApTxFailureObservation,
}

/// Aggregate terminal evidence for AP data-frame TX transactions.
///
/// This is observation only: it consumes the already-decoded ordinary TX
/// outcome and does not participate in retry, rate or queue decisions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApDataTxObservation {
    pub attempts: u32,
    pub retried_frames: u32,
    pub cts_timeout_retries: u32,
    pub ack_timeout_retries: u32,
    pub collision_retries: u32,
    pub maximum_attempts: u8,
    pub minimum_final_rate_kbps: u32,
    pub ack_snr_samples: u32,
    pub minimum_ack_snr_db: i8,
    pub maximum_ack_snr_db: i8,
}

#[cfg(any(feature = "diagnostics", test))]
impl ApDataTxObservation {
    fn observe(&mut self, report: oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxReport) {
        let attempts = report.status.attempts;
        self.attempts = self.attempts.saturating_add(u32::from(attempts));
        self.retried_frames = self.retried_frames.saturating_add(u32::from(attempts > 1));
        self.cts_timeout_retries = self
            .cts_timeout_retries
            .saturating_add(u32::from(report.retries.cts_timeouts));
        self.ack_timeout_retries = self
            .ack_timeout_retries
            .saturating_add(u32::from(report.retries.ack_timeouts));
        self.collision_retries = self
            .collision_retries
            .saturating_add(u32::from(report.retries.collisions));
        self.maximum_attempts = self.maximum_attempts.max(attempts);
        let final_rate_kbps = report.status.final_rate.nominal_kbps();
        if self.minimum_final_rate_kbps == 0 || final_rate_kbps < self.minimum_final_rate_kbps {
            self.minimum_final_rate_kbps = final_rate_kbps;
        }
        if let Some(ack_snr_db) = report.status.ack_snr_db {
            if self.ack_snr_samples == 0 {
                self.minimum_ack_snr_db = ack_snr_db;
                self.maximum_ack_snr_db = ack_snr_db;
            } else {
                self.minimum_ack_snr_db = self.minimum_ack_snr_db.min(ack_snr_db);
                self.maximum_ack_snr_db = self.maximum_ack_snr_db.max(ack_snr_db);
            }
            self.ack_snr_samples = self.ack_snr_samples.saturating_add(1);
        }
    }
}

/// Compact semantic classification of terminal AP TX failures.
///
/// Each counter saturates independently. Discovery ACK misses remain a named
/// subset of the total, rather than being discarded or reported as success.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApTxFailureObservation {
    pub hardware_failures: u8,
    pub hardware_timeouts: u8,
    pub collision_limits: u8,
    pub last_hardware_status: u8,
    /// Subset of hardware_failures: one-shot discovery responses without ACK.
    pub probe_ack_timeouts: u8,
}

/// First failed discovery response in this AP epoch. No packet payload is retained.
#[cfg(any(feature = "diagnostics", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeFailure {
    pub receiver: [u8; 6],
    pub sequence_control: u16,
    pub started_at: oer_time::Instant,
    pub completed_at: oer_time::Instant,
    pub outcome: OrdinaryTxOutcome,
}

/// Diagnostic state owned by the optional AP MAC observer.
///
/// This owner is absent from ordinary builds. It consumes already-decoded TX
/// outcomes and never participates in publication, retry or rate decisions.
#[cfg(any(feature = "diagnostics", test))]
#[derive(Default)]
struct ApMacObserver {
    observation: ApMacObservation,
    first_probe_failure: Option<ProbeFailure>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApTxCompletionAction {
    None,
    /// A concrete DTIM beacon advertising this caller-owned queue prefix
    /// reached terminal hardware publication success.
    DtimGroupRelease {
        advertised_frames: u16,
    },
    BeginWpa2 {
        peer: [u8; 6],
    },
    PublicationFailed,
    PeerDisconnectTerminal {
        close: ApPeerClose,
        stage: ApPeerDisconnectStage,
        acknowledged: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApMacError {
    /// A caller attempted to publish another AP frame while the MAC still
    /// owns one ordinary publication.
    PublicationActive(ApPendingPublicationKind),
    /// The retained A-MPDU path attempted to borrow the ordinary descriptor
    /// while an AP publication still owned it.
    AggregateAdapterUnavailable(ApPendingPublicationKind),
    /// TX service was invoked without a corresponding AP publication.
    ServiceWithoutPublication,
    Engine(ApEngineError),
    Transmit(ApTxError),
}

impl From<ApEngineError> for ApMacError {
    fn from(error: ApEngineError) -> Self {
        Self::Engine(error)
    }
}

impl From<ApTxError> for ApMacError {
    fn from(error: ApTxError) -> Self {
        Self::Transmit(error)
    }
}

/// AP engine and the unique ordinary descriptor used by the current role.
pub struct ApMac<'beacon, 'slot, P, E, T, const BUFFER_SIZE: usize> {
    engine: ApEngine<'beacon>,
    transmit: ApTx<'slot, P, E, T, BUFFER_SIZE>,
    pending: Option<PendingPublication>,
    #[cfg(any(feature = "diagnostics", test))]
    observer: ApMacObserver,
}

/// AP protocol state with no ordinary descriptor or DMA publication owner.
pub struct ApMacParked<'beacon> {
    engine: ApEngine<'beacon>,
    transmit: ApTxParked,
    #[cfg(any(feature = "diagnostics", test))]
    observer: ApMacObserver,
}

/// Quiescent AP MAC owners returned after the ordinary TX transaction parks.
pub struct ApMacParts<'beacon, 'slot, P, E, T, const BUFFER_SIZE: usize> {
    pub engine: ApEngine<'beacon>,
    pub transmit: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
}

impl<'beacon> ApMacParked<'beacon> {
    /// Borrow role-local protocol state without recovering the shared
    /// ordinary-TX capability.
    pub const fn engine(&self) -> &ApEngine<'beacon> {
        &self.engine
    }

    /// Mutate role-local peer/RX state without granting descriptor or MMIO
    /// publication authority.
    pub fn engine_mut(&mut self) -> &mut ApEngine<'beacon> {
        &mut self.engine
    }

    /// Observe the role-local beacon schedule without recovering the shared
    /// ordinary-TX capability.
    pub fn beacon_publication_due(&self, now: oer_time::Instant) -> bool {
        self.engine.beacon_publication_due(now)
    }

    /// Return the current beacon deadline while the physical TX owner is lent
    /// to neither role.
    pub const fn next_beacon(&self) -> Option<oer_time::Instant> {
        self.engine.next_beacon()
    }

    /// Earliest AP protocol deadline which may require a future hardware
    /// publication. This is observation only and grants no MMIO authority.
    pub fn next_control_deadline(&self) -> Option<oer_time::Instant> {
        self.engine
            .next_peer_deadline()
            .into_iter()
            .chain(self.engine.next_wpa2_retry_deadline())
            .chain(self.engine.next_tx_block_ack_deadline())
            .min()
    }

    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.engine.has_operational_tx_block_ack()
    }

    pub fn operational_tx_block_ack_window(&self, peer: [u8; 6]) -> Option<u16> {
        self.engine.operational_tx_block_ack_window(peer)
    }
}

/// The MAC's transmitter clock, the image's monotonic time.
impl<P, E, T: oer_time::Clock, const BUFFER_SIZE: usize> oer_time::Clock
    for ApMac<'_, '_, P, E, T, BUFFER_SIZE>
{
    fn now(&self) -> oer_time::Instant {
        self.transmit.now()
    }
}

impl<P, E, T: oer_time::Timer, const BUFFER_SIZE: usize> oer_time::Timer
    for ApMac<'_, '_, P, E, T, BUFFER_SIZE>
{
    fn wait_until(&self, deadline: oer_time::Instant) -> impl core::future::Future<Output = ()> {
        self.transmit.wait_until(deadline)
    }
}

impl<'beacon, 'slot, P, E, T, const BUFFER_SIZE: usize> ApMac<'beacon, 'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    /// Submitted ordinary work, read before another exchange starts.
    pub fn work(&self) -> oer_ieee80211_softmac::MacTxWork {
        self.transmit.work()
    }

    pub fn new(
        engine: ApEngine<'beacon>,
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: ApTxConfig,
    ) -> Self {
        Self {
            engine,
            transmit: ApTx::new(resources, config),
            pending: None,
            #[cfg(any(feature = "diagnostics", test))]
            observer: ApMacObserver::default(),
        }
    }

    /// Install an explicit MCS32 certification request without adding it to
    /// the AP's ordinary HT rate ranking.
    pub fn with_ht_duplicate_certification_request(
        mut self,
        request: Option<HtDuplicateCertificationRequest>,
    ) -> Self {
        self.transmit
            .set_ht_duplicate_certification_request(request);
        self
    }

    pub const fn engine(&self) -> &ApEngine<'beacon> {
        &self.engine
    }

    pub fn engine_mut(&mut self) -> &mut ApEngine<'beacon> {
        &mut self.engine
    }

    /// Borrow the AP encoder/key owner and the ordinary publication-policy
    /// adapter together at an idle boundary. The shared retained-DMA A-MPDU
    /// owner consumes these two narrow capabilities; it does not receive the
    /// complete AP MAC or peer table.
    pub fn try_aggregate_adapter(
        &mut self,
    ) -> Result<
        (
            &mut ApEngine<'beacon>,
            &mut ApTx<'slot, P, E, T, BUFFER_SIZE>,
        ),
        ApMacError,
    > {
        if let Some(pending) = self.pending {
            return Err(ApMacError::AggregateAdapterUnavailable(pending.kind()));
        }
        self.transmit
            .install_bss_protection(self.engine.bss_protection());
        Ok((&mut self.engine, &mut self.transmit))
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn observation(&self) -> ApMacObservation {
        self.observer.observation
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn first_probe_failure(&self) -> Option<ProbeFailure> {
        self.observer.first_probe_failure
    }

    pub fn pending_publication_kind(&self) -> Option<ApPendingPublicationKind> {
        self.pending.map(PendingPublication::kind)
    }

    pub const fn tx_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub const fn next_beacon(&self) -> Option<oer_time::Instant> {
        self.engine.next_beacon()
    }

    pub fn beacon_publication_due(&self, now: oer_time::Instant) -> bool {
        self.engine.beacon_publication_due(now)
    }

    pub fn beacon_publication_lateness(&self, now: oer_time::Instant) -> (u64, oer_time::Duration) {
        self.engine.beacon_publication_lateness(now)
    }

    pub fn wait_tx_deadline(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.transmit.wait_deadline()
    }

    pub fn publish_beacon<H>(
        &mut self,
        hardware: &mut H,
        now: oer_time::Instant,
    ) -> Result<(), ApMacError>
    where
        H: TxHardware,
    {
        self.require_idle()?;
        let publication = self
            .engine
            .prepare_beacon_publication(now)
            .ok_or(ApMacError::Engine(ApEngineError::BeaconPreparation))?;
        self.transmit
            .start_encoded(hardware, ApTxClass::Beacon, publication.frame)?;
        self.pending = Some(PendingPublication::Beacon {
            dtim_group_frames: publication.dtim_group_frames,
        });
        Ok(())
    }

    pub fn publish_management<H>(
        &mut self,
        hardware: &mut H,
        request: &[u8],
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now: oer_time::Instant,
        scratch: &mut [u8],
    ) -> Result<ApManagementOutcome, ApMacError>
    where
        H: ApRuntimeHardware + TxHardware,
    {
        self.require_idle()?;
        let outcome = self.engine.handle_management(
            hardware,
            request,
            authenticator_nonce,
            initial_replay_counter,
            now,
            scratch,
        )?;
        let ApManagementOutcome::Response { len, begin_wpa2 } = outcome else {
            return Ok(outcome);
        };
        let peer = scratch[4..10]
            .try_into()
            .expect("AP response encoder always writes receiver address");
        let class = if scratch[0] == 0x50 {
            ApTxClass::ProbeResponse
        } else {
            ApTxClass::Management
        };
        self.transmit
            .start_encoded(hardware, class, &scratch[..len])?;
        self.pending = Some(if begin_wpa2 {
            PendingPublication::Association { peer, begin_wpa2 }
        } else if scratch[0] == 0x50 {
            PendingPublication::ProbeResponse {
                receiver: peer,
                sequence_control: u16::from_le_bytes([scratch[22], scratch[23]]),
                started_at: now,
            }
        } else if scratch[0] == 0xb0 {
            PendingPublication::Authentication
        } else {
            PendingPublication::Association { peer, begin_wpa2 }
        });
        Ok(outcome)
    }

    pub fn publish_eapol<H, const N: usize>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        frame: &RsnTxFrame<N>,
        scratch: &mut [u8],
    ) -> Result<(), ApMacError>
    where
        H: TxHardware,
    {
        self.require_idle()?;
        let len = self.engine.encode_eapol(peer, frame, scratch)?;
        self.transmit
            .start_encoded(hardware, ApTxClass::Eapol, &scratch[..len])?;
        self.pending = Some(PendingPublication::Eapol {
            peer,
            retransmission: frame.retransmission(),
        });
        Ok(())
    }

    pub fn publish_rx_block_ack_response<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        body: &[u8],
        scratch: &mut [u8],
    ) -> Result<(), ApMacError> {
        self.require_idle()?;
        let length = self
            .engine
            .encode_rx_block_ack_response(peer, body, scratch)?;
        self.transmit
            .start_encoded(hardware, ApTxClass::Management, &scratch[..length])?;
        self.pending = Some(PendingPublication::RxBlockAckResponse);
        Ok(())
    }

    pub fn publish_tx_block_ack_request<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        now: oer_time::Instant,
        scratch: &mut [u8],
    ) -> Result<bool, ApMacError> {
        self.require_idle()?;
        let Some(length) = self
            .engine
            .prepare_tx_block_ack_request(peer, now, scratch)?
        else {
            return Ok(false);
        };
        self.transmit
            .start_encoded(hardware, ApTxClass::Management, &scratch[..length])?;
        self.pending = Some(PendingPublication::BlockAckRequest { peer });
        Ok(true)
    }

    pub fn peer_ht_rate(&self, peer: [u8; 6]) -> Option<HtRate> {
        let status = self.engine.peer_status(peer)?;
        peer_ht_rate(self.engine.channel(), status.ht?)
    }

    /// Return the independent fixed-request result for one AP peer.
    pub fn peer_ht_duplicate_tx_selection(&self, peer: [u8; 6]) -> Option<HtDuplicateTxSelection> {
        let status = self.engine.peer_status(peer)?;
        Some(peer_ht_duplicate_tx_selection(
            self.engine.channel(),
            status.ht,
            self.transmit.ht_duplicate_certification_request(),
        ))
    }

    /// Resolve one Ethernet lease to the AP peer/rate/BlockAck policy that
    /// must remain stable for the complete aggregate build.
    pub fn aggregate_admission(&mut self, ethernet: &[u8]) -> Option<ApAggregateAdmission> {
        let peer = ethernet
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())?;
        if peer[0] & 1 != 0 {
            return None;
        }
        let (binding, status) = self.engine.bind_aggregate_peer(peer).ok()?;
        if status.power_state != ApPeerPowerState::Active {
            return None;
        }
        let ht_duplicate_tx_selection = peer_ht_duplicate_tx_selection(
            self.engine.channel(),
            status.ht,
            self.transmit.ht_duplicate_certification_request(),
        );
        #[cfg(any(feature = "diagnostics", test))]
        if ht_duplicate_tx_selection.request().is_some() {
            self.observer.observation.ht_duplicate_tx_requests = self
                .observer
                .observation
                .ht_duplicate_tx_requests
                .saturating_add(1);
            self.observer.observation.ht_duplicate_tx_selection = ht_duplicate_tx_selection;
        }
        #[cfg(not(any(feature = "diagnostics", test)))]
        let _ = ht_duplicate_tx_selection;
        let agreement = status.tx_block_ack?;
        let rate = peer_ht_rate(self.engine.channel(), status.ht?)?;
        Some(ApAggregateAdmission::new(
            binding,
            status.association_identity(),
            rate,
            agreement.window,
            status.tx_block_ack_generation,
            agreement.amsdu,
        ))
    }

    pub fn next_tx_block_ack_deadline(&self) -> Option<oer_time::Instant> {
        self.engine.next_tx_block_ack_deadline()
    }

    pub fn expire_tx_block_ack(&mut self, now: oer_time::Instant) -> Result<bool, ApMacError> {
        Ok(self.engine.expire_tx_block_ack(now))
    }

    pub fn publish_ethernet<H>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        ethernet: &[u8],
        scratch: &mut [u8],
    ) -> Result<(), ApMacError>
    where
        H: TxHardware,
    {
        self.publish_ethernet_with_more_data(hardware, peer, ethernet, scratch, false)
    }

    /// Publish one protected network frame with an explicit AP More Data bit.
    pub fn publish_ethernet_with_more_data<H>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        ethernet: &[u8],
        scratch: &mut [u8],
        more_data: bool,
    ) -> Result<(), ApMacError>
    where
        H: TxHardware,
    {
        self.require_idle()?;
        self.transmit
            .install_bss_protection(self.engine.bss_protection());
        let hardware_mic_length = if self.engine.link_protection() == LinkProtection::Open {
            0
        } else {
            TX_CCMP_MIC_SIZE
        };
        // Ordinary TX aligns metadata + MPDU + hardware MIC + FCS to four
        // bytes. Bound the encoder first so the retained descriptor copy and
        // publication cannot reject after sequence/PN admission.
        let payload_capacity =
            (BUFFER_SIZE & !3).saturating_sub(TX_METADATA_SIZE + hardware_mic_length + TX_FCS_SIZE);
        let scratch_capacity = scratch.len().min(payload_capacity);
        let prepared = self.engine.encode_protected_ethernet_with_more_data(
            peer,
            ethernet,
            &mut scratch[..scratch_capacity],
            more_data,
        )?;
        let rate = if peer[0] & 1 != 0 {
            // Group traffic must be decodable by every associated peer. The
            // initial B/G ERP AP advertises 1 Mbit/s as a basic rate; unlike
            // unicast there is no single peer rate negotiation or ACK.
            LegacyRate::Dsss1MLong
        } else {
            self.engine
                .peer_status(peer)
                .map(|status| peer_legacy_rate(status.maximum_legacy_rate_500kbps))
                .ok_or(ApMacError::Engine(ApEngineError::Service(
                    ApServiceError::UnknownPeer,
                )))?
        };
        let encoded = self
            .engine
            .commit_prepared_data(prepared, &mut scratch[..scratch_capacity])?;
        match encoded.hardware_key_selector {
            None => {
                self.transmit.start_unprotected_data_encoded(
                    hardware,
                    &scratch[..encoded.length],
                    peer[0] & 1 != 0,
                    rate,
                )?;
            }
            Some(hardware_key_selector) if peer[0] & 1 != 0 => {
                self.transmit.start_group_protected_encoded(
                    hardware,
                    &scratch[..encoded.length],
                    hardware_key_selector,
                )?;
            }
            Some(hardware_key_selector) => {
                self.transmit.start_protected_encoded(
                    hardware,
                    &scratch[..encoded.length],
                    hardware_key_selector,
                    rate,
                )?;
            }
        }
        self.pending = Some(PendingPublication::Data { peer });
        Ok(())
    }

    /// Publish two ordered unicast Ethernet leases as one QoS A-MSDU when
    /// the peer/security/buffer frontier admits it.
    ///
    /// `Ok(false)` consumes neither sequence nor CCMP PN and leaves both
    /// caller leases untouched, allowing exact ordinary-TX fallback.
    pub fn publish_amsdu_pair<H>(
        &mut self,
        hardware: &mut H,
        first: &[u8],
        second: &[u8],
        scratch: &mut [u8],
    ) -> Result<bool, ApMacError>
    where
        H: TxHardware,
    {
        self.require_idle()?;
        self.transmit
            .install_bss_protection(self.engine.bss_protection());
        let hardware_mic_length = if self.engine.link_protection() == LinkProtection::Open {
            0
        } else {
            TX_CCMP_MIC_SIZE
        };
        let payload_capacity =
            (BUFFER_SIZE & !3).saturating_sub(TX_METADATA_SIZE + hardware_mic_length + TX_FCS_SIZE);
        let scratch_capacity = scratch.len().min(payload_capacity);
        let Some(prepared) = self.engine.encode_amsdu_ethernet_pair(
            first,
            second,
            &mut scratch[..scratch_capacity],
        )?
        else {
            return Ok(false);
        };
        let peer = prepared.peer;
        let rate = self
            .engine
            .peer_status(peer)
            .map(|status| peer_legacy_rate(status.maximum_legacy_rate_500kbps))
            .ok_or(ApMacError::Engine(ApEngineError::Service(
                ApServiceError::UnknownPeer,
            )))?;
        let encoded = self
            .engine
            .commit_prepared_amsdu(prepared, &mut scratch[..scratch_capacity])?;
        match encoded.hardware_key_selector {
            None => self.transmit.start_unprotected_data_encoded(
                hardware,
                &scratch[..encoded.length],
                false,
                rate,
            )?,
            Some(hardware_key_selector) => self.transmit.start_protected_encoded(
                hardware,
                &scratch[..encoded.length],
                hardware_key_selector,
                rate,
            )?,
        };
        self.pending = Some(PendingPublication::Data { peer });
        Ok(true)
    }

    pub fn publish_peer_disconnect<H>(
        &mut self,
        hardware: &mut H,
        close: ApPeerClose,
        stage: ApPeerDisconnectStage,
        scratch: &mut [u8],
    ) -> Result<(), ApMacError>
    where
        H: TxHardware,
    {
        self.require_idle()?;
        let (kind, reason) = match stage {
            ApPeerDisconnectStage::Disassociation => (
                ApPeerDisconnectKind::Disassociation,
                if close.kind == oer_ieee80211_ap::ApPeerCloseKind::InactivityTimeout {
                    4
                } else {
                    2
                },
            ),
            ApPeerDisconnectStage::Deauthentication => (ApPeerDisconnectKind::Deauthentication, 2),
        };
        let length = self
            .engine
            .encode_peer_disconnect(close, kind, reason, scratch)?;
        // Complete vendor `wifi_softap_stop` publishes subtype 0xa0 and then
        // 0xc0 before `cnx_node_leave`; neither call waits for an ACK. Keep our
        // stronger DMA-ownership rule (each transaction reaches a terminal
        // completion before the next one), but distinguish hardware
        // publication from an ACK that a departing peer may never return.
        self.transmit
            .start_encoded(hardware, ApTxClass::Management, &scratch[..length])?;
        #[cfg(any(feature = "diagnostics", test))]
        {
            match stage {
                ApPeerDisconnectStage::Disassociation => {
                    self.observer.observation.disassociations_published = self
                        .observer
                        .observation
                        .disassociations_published
                        .saturating_add(1);
                }
                ApPeerDisconnectStage::Deauthentication => {
                    self.observer.observation.deauthentications_published = self
                        .observer
                        .observation
                        .deauthentications_published
                        .saturating_add(1);
                }
            }
        }
        self.pending = Some(PendingPublication::PeerDisconnect { close, stage });
        Ok(())
    }

    pub fn service_tx<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
        now: oer_time::Instant,
    ) -> Result<(WifiTxProgress, ApTxCompletionAction), ApMacError> {
        if self.pending.is_none() {
            return Err(ApMacError::ServiceWithoutPublication);
        }
        let progress = self.transmit.service(hardware, wake)?;
        if progress == WifiTxProgress::Pending {
            return Ok((progress, ApTxCompletionAction::None));
        }
        let pending = self.pending.take().expect("checked AP publication");
        let outcome = self
            .transmit
            .take_last_outcome()
            .expect("terminal AP TX retains an outcome");
        #[cfg(any(feature = "diagnostics", test))]
        if let PendingPublication::Data { peer } = pending
            && peer[0] & 1 == 0
        {
            // Group traffic has no ACK and deliberately uses the 1-Mbit/s
            // basic rate. Mixing it into per-peer retry/rate evidence makes a
            // healthy broadcast look like a unicast rate-control collapse.
            self.observer.observation.data_tx.observe(outcome.report());
        }
        if !matches!(outcome, OrdinaryTxOutcome::Success(_)) {
            #[cfg(any(feature = "diagnostics", test))]
            if let PendingPublication::ProbeResponse {
                receiver,
                sequence_control,
                started_at,
            } = pending
            {
                self.observer
                    .first_probe_failure
                    .get_or_insert(ProbeFailure {
                        receiver,
                        sequence_control,
                        started_at,
                        completed_at: now,
                        outcome,
                    });
            }
            #[cfg(any(feature = "diagnostics", test))]
            match outcome {
                OrdinaryTxOutcome::HardwareFailure(report) => {
                    if matches!(pending, PendingPublication::ProbeResponse { .. })
                        && report.completion.is_some_and(|completion| {
                            completion.disposition()
                                == oer_esp32s31_ieee80211_mac::tx::TxCompletionDisposition::AckTimeout
                        })
                    {
                        self.observer.observation.tx_failures.probe_ack_timeouts = self
                            .observer
                            .observation
                            .tx_failures
                            .probe_ack_timeouts
                            .saturating_add(1);
                    }
                    self.observer.observation.tx_failures.hardware_failures = self
                        .observer
                        .observation
                        .tx_failures
                        .hardware_failures
                        .saturating_add(1);
                    self.observer.observation.tx_failures.last_hardware_status = report
                        .completion
                        .map(|completion| completion.status())
                        .unwrap_or(0);
                }
                OrdinaryTxOutcome::HardwareTimeout(_) => {
                    self.observer.observation.tx_failures.hardware_timeouts = self
                        .observer
                        .observation
                        .tx_failures
                        .hardware_timeouts
                        .saturating_add(1);
                }
                OrdinaryTxOutcome::CollisionLimit(_) => {
                    self.observer.observation.tx_failures.collision_limits = self
                        .observer
                        .observation
                        .tx_failures
                        .collision_limits
                        .saturating_add(1);
                }
                OrdinaryTxOutcome::Success(_) => unreachable!("checked failed AP publication"),
            }
            return Ok((
                progress,
                match pending {
                    PendingPublication::PeerDisconnect { close, stage } => {
                        ApTxCompletionAction::PeerDisconnectTerminal {
                            close,
                            stage,
                            acknowledged: false,
                        }
                    }
                    PendingPublication::Eapol {
                        peer,
                        retransmission,
                    } => {
                        self.engine
                            .observe_wpa2_transmit(peer, retransmission, false, now)?;
                        ApTxCompletionAction::PublicationFailed
                    }
                    _ => ApTxCompletionAction::PublicationFailed,
                },
            ));
        }
        let action = match pending {
            PendingPublication::Beacon { dtim_group_frames } => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer.observation.beacons_transmitted = self
                        .observer
                        .observation
                        .beacons_transmitted
                        .saturating_add(1);
                }
                if dtim_group_frames == 0 {
                    ApTxCompletionAction::None
                } else {
                    ApTxCompletionAction::DtimGroupRelease {
                        advertised_frames: dtim_group_frames,
                    }
                }
            }
            PendingPublication::ProbeResponse { .. } => ApTxCompletionAction::None,
            PendingPublication::Authentication => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer
                        .observation
                        .authentication_responses_transmitted = self
                        .observer
                        .observation
                        .authentication_responses_transmitted
                        .saturating_add(1);
                }
                ApTxCompletionAction::None
            }
            PendingPublication::Association { peer, begin_wpa2 } => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer.observation.association_responses_transmitted = self
                        .observer
                        .observation
                        .association_responses_transmitted
                        .saturating_add(1);
                }
                if begin_wpa2 {
                    ApTxCompletionAction::BeginWpa2 { peer }
                } else {
                    ApTxCompletionAction::None
                }
            }
            PendingPublication::Eapol {
                peer,
                retransmission,
            } => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer.observation.eapol_frames_transmitted = self
                        .observer
                        .observation
                        .eapol_frames_transmitted
                        .saturating_add(1);
                }
                self.engine
                    .observe_wpa2_transmit(peer, retransmission, true, now)?;
                ApTxCompletionAction::None
            }
            PendingPublication::Data { peer } => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer.observation.data_frames_transmitted = self
                        .observer
                        .observation
                        .data_frames_transmitted
                        .saturating_add(1);
                }
                if peer[0] & 1 == 0 {
                    self.engine.observe_peer_activity(peer, now)?;
                }
                ApTxCompletionAction::None
            }
            PendingPublication::BlockAckRequest { .. } => ApTxCompletionAction::None,
            PendingPublication::RxBlockAckResponse => {
                #[cfg(any(feature = "diagnostics", test))]
                {
                    self.observer.observation.rx_block_ack_responses_transmitted = self
                        .observer
                        .observation
                        .rx_block_ack_responses_transmitted
                        .saturating_add(1);
                }
                ApTxCompletionAction::None
            }
            PendingPublication::PeerDisconnect { close, stage } => {
                #[cfg(any(feature = "diagnostics", test))]
                match stage {
                    ApPeerDisconnectStage::Disassociation => {
                        self.observer.observation.disassociations_acknowledged = self
                            .observer
                            .observation
                            .disassociations_acknowledged
                            .saturating_add(1);
                    }
                    ApPeerDisconnectStage::Deauthentication => {
                        self.observer.observation.deauthentications_acknowledged = self
                            .observer
                            .observation
                            .deauthentications_acknowledged
                            .saturating_add(1);
                    }
                }
                ApTxCompletionAction::PeerDisconnectTerminal {
                    close,
                    stage,
                    acknowledged: true,
                }
            }
        };
        Ok((progress, action))
    }

    fn require_idle(&self) -> Result<(), ApMacError> {
        if let Some(pending) = self.pending {
            Err(ApMacError::PublicationActive(pending.kind()))
        } else {
            Ok(())
        }
    }

    /// Lend the physical ordinary TX owner while preserving all AP-local
    /// protocol, timer and observation state.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
            ApMacParked<'beacon>,
        ),
        Self,
    > {
        let Self {
            engine,
            transmit,
            pending,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
        } = self;
        if pending.is_some() {
            return Err(Self {
                engine,
                transmit,
                pending,
                #[cfg(any(feature = "diagnostics", test))]
                observer,
            });
        }
        match transmit.try_park() {
            Ok((resources, transmit)) => Ok((
                resources,
                ApMacParked {
                    engine,
                    transmit,
                    #[cfg(any(feature = "diagnostics", test))]
                    observer,
                },
            )),
            Err(transmit) => Err(Self {
                engine,
                transmit,
                pending: None,
                #[cfg(any(feature = "diagnostics", test))]
                observer,
            }),
        }
    }

    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        parked: ApMacParked<'beacon>,
    ) -> Self {
        let ApMacParked {
            engine,
            transmit,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
        } = parked;
        Self {
            engine,
            transmit: ApTx::resume(resources, transmit),
            pending: None,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
        }
    }

    /// Recover AP protocol and common TX resources only after TX is idle.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_into_parts(self) -> Result<ApMacParts<'beacon, 'slot, P, E, T, BUFFER_SIZE>, Self> {
        self.try_park().map(|(resources, parked)| {
            let ApMacParked { engine, .. } = parked;
            ApMacParts {
                engine,
                transmit: resources,
            }
        })
    }
}

#[cfg(test)]
mod tests;

use crate::hardware::ApRuntimeHardware;

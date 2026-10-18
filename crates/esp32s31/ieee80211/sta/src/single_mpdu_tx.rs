//! Production owner for one connected-station legacy/HT MPDU transaction.
//!
//! This module owns the executor-independent transaction between an encoded
//! station frame and the ESP32-S31 ordinary TX queue. It deliberately owns no logging,
//! benchmark statistics, NVS state or RTOS adapter. Every PAC borrow ends
//! before a timer is awaited.

use core::future::Future;
use oer_espressif_ieee80211_policy::rate_control::DEFAULT_CONTROL_SCHEDULE;
use oer_ieee80211_mac::sequence::SequenceNumber;

use oer_esp32s31_ieee80211::{
    esp_now::{
        EspNowTxConfig, EspNowTxError, start_esp_now_v1_plaintext, start_esp_now_v2_plaintext,
    },
    ordinary_tx::{
        OrdinaryTxError, OrdinaryTxOwner, OrdinaryTxParked, OrdinaryTxPlan, TX_CCMP_MIC_SIZE,
        TX_METADATA_SIZE,
    },
};

use oer_esp32s31_ieee80211_mac::{
    crypto::{CcmpTxPacketNumberError, StaPairwiseCcmpSlot},
    rate::schedule::{RateScheduleRef, schedule_publication_limit},
    tx::{
        LegacyTxQueue, TxControlFrame, TxError, TxHardware, TxPhyRate,
        protection::{ProtectedPpdu, TxProtectionDecision},
        runtime::{
            OrdinaryRetryError, OrdinaryRetryRatePolicy, WifiTxRuntimePolicy, WifiTxTraffic,
            WifiTxTrafficError, select_schedule_retry_rate,
        },
    },
};

use oer_ieee80211_mac::{
    block_ack::encode_block_ack_request,
    channel::WifiChannel,
    extensions::espressif::esp_now::EspNowRandomValue,
    management::ProbeRequest,
    management_protection::is_robust_action_category,
    qos::WmmUserPriority,
    security::LinkProtection,
    station::{
        StaDataFrame, StaManagementFrame, StaManagementSubtype, StaProtectedDataFrame,
        StaProtectedEthernetFrame, StaProtectedManagementFrame, StaTxSequenceCounters,
        StationFrameError,
    },
    station_power_save::{StaNullDataFrame, StaPowerManagement},
};

use oer_ieee80211_softmac::{
    EspNowPeerId, EspNowProtocol, EspNowSendError, EspNowV2SendError, MacTxPlan, MacTxQueueState,
    interface::BoundVirtualInterface,
};

pub use oer_esp32s31_ieee80211::ordinary_tx::{
    OrdinaryTxOutcome as SingleMpduTxOutcome, OrdinaryTxReport as SingleMpduTxReport,
    TxResetReason, WifiTxEntropy, WifiTxPowerPair, WifiTxPowerProfile, WifiTxResources,
};

use oer_esp32s31_ieee80211::tx::{WifiTxProgress, WifiTxWake};

/// Association-derived inputs for the first ordinary connected-data slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SingleMpduTxConfig {
    pub station_address: [u8; 6],
    pub bssid: [u8; 6],
    /// The SSID the station associated with, which its reachability probes
    /// name.
    pub ssid: oer_ieee80211_mac::ssid::WifiSsid,
    pub peer_qos: bool,
    /// The association protects its robust management frames, so robust
    /// Action frames leave under the pairwise key.
    pub management_protection: bool,
    /// Access category of a prepared retry that does not name its own.
    pub access_category: oer_ieee80211_mac::qos::WmmAccessCategory,
    /// Schedule record of every non-data frame: management, EAPOL and
    /// power-management Null. It selects their rate, retry rates and
    /// publication budget.
    pub control_schedule: RateScheduleRef,
    /// Watchdog applied independently to each hardware publication.
    pub publication_timeout: oer_time::Duration,
}

/// Rate and publication budget of one network data MPDU, selected per frame
/// from the association's ordinary rate schedule.
///
/// SOURCE: `libpp[trc.o]::rcGetSched` selects the data schedule record
/// that `libpp[trc.o]::rcGetRate` walks for the retry ladder, and complete
/// `libpp[trc.o]::rcReachRetryLimit` bounds the MPDU retry counter by record byte `0x08`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaDataTxSelection {
    pub rate: TxPhyRate,
    pub publication_limit: u8,
}

impl SingleMpduTxConfig {
    /// Derive the effective frame geometry for the selected security mode.
    /// The current Open encoder is intentionally non-QoS; keeping a peer's
    /// WMM bit here would consume the wrong sequence space and imply an
    /// unsupported plaintext A-MPDU path.
    pub const fn for_security(mut self, security: LinkProtection) -> Self {
        if matches!(security, LinkProtection::Open) {
            self.peer_qos = false;
        }
        self
    }
}

/// Protocol resources installed at the WPA2-to-connected TX handoff.
///
/// Keeping the key token, all independent sequence spaces and the negotiated
/// publication policy in one value prevents a partial transition from
/// constructing a connected transmitter with mismatched session state.
pub struct ConnectedTxHandoff {
    pub security: ConnectedTxSecurity,
    pub sequences: StaTxSequenceCounters,
    pub config: SingleMpduTxConfig,
}

/// Pairwise TX ownership for one connected station epoch.
pub enum ConnectedTxSecurity {
    Open,
    Wpa2Personal(StaPairwiseCcmpSlot),
}

impl ConnectedTxSecurity {
    pub const fn mode(&self) -> LinkProtection {
        match self {
            Self::Open => LinkProtection::Open,
            Self::Wpa2Personal(_) => LinkProtection::Ccmp,
        }
    }

    pub const fn hardware_key_selector(&self) -> u8 {
        match self {
            Self::Open => 0,
            Self::Wpa2Personal(key) => key.hardware_index(),
        }
    }
}

/// Queue priorities for one unprotected connected Action frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActionTxConfig {
    pub scheduler_priority: u8,
    pub packet_priority: u8,
}

impl ActionTxConfig {
    /// Neutral profile for standards-defined unprotected management actions.
    /// It intentionally retains the ordinary queue-zero priorities without
    /// implying that a vendor-specific wire format is being sent.
    pub const STANDARD_MANAGEMENT: Self = Self {
        scheduler_priority: 1,
        packet_priority: 1,
    };

    /// Profile used by ordinary management frames recovered from the vendor
    /// queue-zero path.
    pub const VENDOR_MANAGEMENT: Self = Self::STANDARD_MANAGEMENT;

    /// Profile retained by the recovered connected RX ADDBA response path.
    pub const RX_ADDBA_RESPONSE: Self = Self {
        scheduler_priority: 0,
        packet_priority: 0,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleMpduTxError {
    Busy,
    EthernetFrameTooShort,
    SecurityModeMismatch,
    PacketNumber(CcmpTxPacketNumberError),
    BufferSizeOverflow,
    DeadlineOverflow,
    ProbeEncode,
    Encode(StationFrameError),
    Tx(TxError),
    Retry(OrdinaryRetryError),
    Traffic(WifiTxTrafficError),
    TrafficSelectionMismatch {
        expected: WifiTxTraffic,
        provided: WifiTxTraffic,
    },
    /// A TID outside the eight QoS user priorities.
    InvalidTid(u8),
    RadioResetRequired(TxResetReason),
    /// A BlockAckReq was planned at a non-legacy rate.
    BlockAckRequestRate,
}

/// Failure before one plaintext ESP-NOW request acquires the connected ordinary-TX
/// transaction. Protocol admission and chip publication remain separate so
/// applications can distinguish a stale peer from a hardware/PHY rejection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SingleMpduEspNowTxError {
    Protocol(EspNowSendError),
    V2Protocol(EspNowV2SendError),
    Backend(EspNowTxError),
}

impl From<EspNowSendError> for SingleMpduEspNowTxError {
    fn from(error: EspNowSendError) -> Self {
        Self::Protocol(error)
    }
}

impl From<EspNowV2SendError> for SingleMpduEspNowTxError {
    fn from(error: EspNowV2SendError) -> Self {
        Self::V2Protocol(error)
    }
}

impl From<EspNowTxError> for SingleMpduEspNowTxError {
    fn from(error: EspNowTxError) -> Self {
        Self::Backend(error)
    }
}

impl From<TxError> for SingleMpduTxError {
    fn from(error: TxError) -> Self {
        Self::Tx(error)
    }
}

impl From<OrdinaryRetryError> for SingleMpduTxError {
    fn from(error: OrdinaryRetryError) -> Self {
        Self::Retry(error)
    }
}

impl From<WifiTxTrafficError> for SingleMpduTxError {
    fn from(error: WifiTxTrafficError) -> Self {
        Self::Traffic(error)
    }
}

impl From<CcmpTxPacketNumberError> for SingleMpduTxError {
    fn from(error: CcmpTxPacketNumberError) -> Self {
        Self::PacketNumber(error)
    }
}

impl From<OrdinaryTxError> for SingleMpduTxError {
    fn from(error: OrdinaryTxError) -> Self {
        match error {
            OrdinaryTxError::Busy => Self::Busy,
            OrdinaryTxError::BufferSizeOverflow => Self::BufferSizeOverflow,
            OrdinaryTxError::DeadlineOverflow => Self::DeadlineOverflow,
            OrdinaryTxError::Tx(error) => Self::Tx(error),
            OrdinaryTxError::Retry(error) => Self::Retry(error),
            OrdinaryTxError::RadioResetRequired(reason) => Self::RadioResetRequired(reason),
            OrdinaryTxError::BlockAckRequestRate => Self::BlockAckRequestRate,
        }
    }
}

/// Unique ordinary-MPDU descriptor, crypto PN and retry owner.
pub struct SingleMpduTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    ordinary: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    security: ConnectedTxSecurity,
    sequences: StaTxSequenceCounters,
    config: SingleMpduTxConfig,
}

/// The transmitter's timer, for phases that wait on its clock.
impl<P, E, T: oer_time::Clock, const BUFFER_SIZE: usize> oer_time::Clock
    for SingleMpduTx<'_, P, E, T, BUFFER_SIZE>
{
    fn now(&self) -> oer_time::Instant {
        self.ordinary.now()
    }
}

impl<P, E, T: oer_time::Timer, const BUFFER_SIZE: usize> oer_time::Timer
    for SingleMpduTx<'_, P, E, T, BUFFER_SIZE>
{
    fn wait_until(&self, deadline: oer_time::Instant) -> impl Future<Output = ()> {
        self.ordinary.wait_until(deadline)
    }
}

/// Opaque station-local state retained while another VIF owns physical TX.
pub struct SingleMpduTxParked {
    ordinary: OrdinaryTxParked,
    handoff: ConnectedTxHandoff,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> SingleMpduTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    /// Ordinary publications, including per-attempt length/rate and retries.
    pub fn work(&self) -> oer_ieee80211_softmac::MacTxWork {
        self.ordinary.work()
    }

    /// Borrow the timer adapter, including its separately owned radio clock.
    pub fn timer(&self) -> &T {
        &self.ordinary.timer
    }

    pub fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        handoff: ConnectedTxHandoff,
    ) -> Self {
        let ConnectedTxHandoff {
            security,
            sequences,
            config,
        } = handoff;
        Self {
            ordinary: OrdinaryTxOwner::new(resources),
            security,
            sequences,
            config,
        }
    }

    /// Resume from the exact opaque station state produced by
    /// [`Self::try_park`].
    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        parked: SingleMpduTxParked,
    ) -> Self {
        let SingleMpduTxParked { ordinary, handoff } = parked;
        let ConnectedTxHandoff {
            security,
            sequences,
            config,
        } = handoff;
        Self {
            ordinary: OrdinaryTxOwner::resume(resources, ordinary),
            security,
            sequences,
            config,
        }
    }

    pub(crate) fn from_ordinary(
        ordinary: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
        security: ConnectedTxSecurity,
        sequences: StaTxSequenceCounters,
        config: SingleMpduTxConfig,
    ) -> Self {
        Self {
            ordinary,
            security,
            sequences,
            config,
        }
    }

    pub const fn active(&self) -> bool {
        self.ordinary.active()
    }

    pub fn queue_state(&self) -> MacTxQueueState {
        self.ordinary.queue_state()
    }

    /// Exact ordinary descriptor lifecycle retained for bounded diagnostics.
    pub fn slot_state(&self) -> oer_esp32s31_ieee80211_mac::tx::TxSlotState {
        self.ordinary.slot_state()
    }

    /// Current ordinary descriptor ownership word retained for diagnostics.
    pub fn descriptor_word0(&self) -> u32 {
        self.ordinary.descriptor_word0()
    }

    pub const fn policy(&self) -> &WifiTxRuntimePolicy {
        self.ordinary.policy()
    }

    pub fn policy_mut(&mut self) -> &mut WifiTxRuntimePolicy {
        self.ordinary.policy_mut()
    }

    pub fn select_network_traffic(
        &self,
        ethernet: &[u8],
    ) -> Result<WifiTxTraffic, WifiTxTrafficError> {
        self.policy()
            .select_network_traffic(ethernet, self.config.peer_qos)
    }

    pub fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        self.ordinary.take_last_outcome()
    }

    pub const fn last_outcome(&self) -> Option<SingleMpduTxOutcome> {
        self.ordinary.last_outcome()
    }

    pub const fn power_profile(&self) -> &P {
        self.ordinary.power()
    }

    /// Protection and control frame for one aggregate PPDU to the BSSID.
    pub fn control_frame_for(&self, ppdu: ProtectedPpdu) -> (TxProtectionDecision, TxControlFrame) {
        self.ordinary.control_frame_for(ppdu)
    }

    pub fn contention_publication(
        &mut self,
        queue: LegacyTxQueue,
    ) -> (
        oer_esp32s31_ieee80211_mac::edca::EdcaContentionParameters,
        u16,
    ) {
        self.ordinary.contention_publication(queue)
    }

    pub fn record_retry_failure(&mut self, queue: LegacyTxQueue) {
        self.ordinary.record_retry_failure(queue);
    }

    pub fn record_success(&mut self, queue: LegacyTxQueue) {
        self.ordinary.record_success(queue);
    }

    pub fn reset_terminal_exchange(&mut self, queue: LegacyTxQueue) {
        self.ordinary.reset_terminal_exchange(queue);
    }

    /// Split an idle connected transmitter back into reusable descriptor
    /// resources and its association-owned key/sequence handoff.
    ///
    /// This is the inverse ownership edge of
    /// [`crate::control_tx::ControlTransmitter::try_into_connected`]. It does
    /// not clear the hardware key: an outer station teardown must consume the
    /// returned token through `StaPairwiseCcmpSlot::clear` using the unique
    /// hardware owner.
    #[allow(clippy::result_large_err)]
    pub fn try_into_parts(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
            ConnectedTxHandoff,
        ),
        Self,
    > {
        let Self {
            ordinary,
            security,
            sequences,
            config,
        } = self;
        match ordinary.try_into_resources() {
            Ok(resources) => Ok((
                resources,
                ConnectedTxHandoff {
                    security,
                    sequences,
                    config,
                },
            )),
            Err(ordinary) => Err(Self {
                ordinary,
                security,
                sequences,
                config,
            }),
        }
    }

    /// Separate idle physical resources from opaque station-local callback,
    /// key and sequence state without losing a terminal TX observation.
    #[allow(clippy::result_large_err)]
    pub fn try_park(
        self,
    ) -> Result<
        (
            WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
            SingleMpduTxParked,
        ),
        Self,
    > {
        let Self {
            ordinary,
            security,
            sequences,
            config,
        } = self;
        match ordinary.try_park() {
            Ok((resources, ordinary)) => Ok((
                resources,
                SingleMpduTxParked {
                    ordinary,
                    handoff: ConnectedTxHandoff {
                        security,
                        sequences,
                        config,
                    },
                },
            )),
            Err(ordinary) => Err(Self {
                ordinary,
                security,
                sequences,
                config,
            }),
        }
    }

    pub fn peek_qos_sequence(&self, tid: u8) -> Option<SequenceNumber> {
        self.sequences.peek_qos(tid)
    }

    pub fn take_protected_metadata(
        &mut self,
        tid: u8,
    ) -> Result<Option<StaProtectedEthernetFrame>, CcmpTxPacketNumberError> {
        let ConnectedTxSecurity::Wpa2Personal(key) = &mut self.security else {
            return Ok(None);
        };
        if self.sequences.peek_qos(tid).is_none() {
            return Ok(None);
        }
        let ccmp_header = key.next_tx_ccmp_header()?;
        let sequence_number = self
            .sequences
            .take_data(Some(tid))
            .expect("validated QoS sequence space remains owned");
        Ok(Some(StaProtectedEthernetFrame {
            bssid: self.config.bssid,
            sequence_number,
            user_priority: tid,
            peer_qos: self.config.peer_qos,
            ccmp_header,
        }))
    }

    pub const fn hardware_key_selector(&self) -> u8 {
        self.security.hardware_key_selector()
    }

    pub const fn link_protection(&self) -> LinkProtection {
        self.security.mode()
    }

    pub const fn config(&self) -> SingleMpduTxConfig {
        self.config
    }

    /// Publish one exact protected MPDU copied from a detached aggregate.
    /// Sequence Control and CCMP PN are already present and must not be
    /// allocated again; only the IEEE Retry bit is added.
    pub fn copy_encoded_retry(&mut self, encoded: &[u8]) -> Result<usize, SingleMpduTxError> {
        if self.link_protection() == LinkProtection::Open {
            return Err(SingleMpduTxError::SecurityModeMismatch);
        }
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let frame_length = encoded.len();
        if frame_length < 2 {
            return Err(SingleMpduTxError::EthernetFrameTooShort);
        }
        let output = self.ordinary.buffer_mut()?;
        let end = TX_METADATA_SIZE
            .checked_add(frame_length)
            .ok_or(SingleMpduTxError::BufferSizeOverflow)?;
        output
            .get_mut(TX_METADATA_SIZE..end)
            .ok_or(SingleMpduTxError::BufferSizeOverflow)?
            .copy_from_slice(encoded);
        output[TX_METADATA_SIZE + 1] |= 0x08;
        Ok(frame_length)
    }

    pub fn start_prepared_encoded_retry_for_category<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame_length: usize,
        hardware_mic_length: usize,
        selection: StaDataTxSelection,
        access_category: oer_ieee80211_mac::qos::WmmAccessCategory,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.link_protection() == LinkProtection::Open || hardware_mic_length == 0 {
            return Err(SingleMpduTxError::SecurityModeMismatch);
        }
        let queue = LegacyTxQueue::from_access_category(access_category);
        self.ordinary
            .start(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category,
                        initial_rate: selection.rate,
                        publication_limit: selection.publication_limit,
                        publication_timeout: self.config.publication_timeout,
                    },
                    hardware_mic_length,
                    hardware_key_selector: self.security.hardware_key_selector(),
                    interface: oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: queue.vendor_data_scheduler_priority(),
                    packet_priority: queue.vendor_data_packet_priority(),
                    priority_count: 1,
                },
            )
            .map_err(Into::into)
    }

    /// Publish a BlockAckReq for `tid` whose BlockAck starts at
    /// `starting_sequence`.
    ///
    /// SOURCE: `libpp[pp.o]::ppFillAMPDUBar` leaves Duration zero and
    /// queues the request on the access category of its TID; `libpp[trc.o]::rcGetSched`
    /// selects row three of `rc11BSchedTbl`, the 1 Mbit/s long-preamble
    /// schedule with 32 publications, and the descriptor solicits a BlockAck
    /// (blobray ).
    pub fn start_block_ack_request<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        tid: u8,
        starting_sequence: SequenceNumber,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let access_category = WmmUserPriority::new(tid)
            .ok_or(SingleMpduTxError::InvalidTid(tid))?
            .access_category();
        let frame = encode_block_ack_request(
            self.config.bssid,
            self.config.station_address,
            tid,
            starting_sequence,
        );
        self.ordinary.buffer_mut()?[TX_METADATA_SIZE..TX_METADATA_SIZE + frame.len()]
            .copy_from_slice(&frame);
        let schedule = DEFAULT_CONTROL_SCHEDULE;
        let queue = LegacyTxQueue::from_access_category(access_category);
        self.ordinary
            .start_with_retry_rate_policy(
                hardware,
                OrdinaryTxPlan {
                    frame_length: frame.len(),
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category,
                        initial_rate: select_schedule_retry_rate(schedule, 0)
                            .expect("the control schedule starts with a legacy rate"),
                        publication_limit: schedule_publication_limit(schedule),
                        publication_timeout: self.config.publication_timeout,
                    },
                    hardware_mic_length: 0,
                    hardware_key_selector: 0,
                    interface: oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: queue.vendor_data_scheduler_priority(),
                    packet_priority: queue.vendor_data_packet_priority(),
                    priority_count: 1,
                },
                OrdinaryRetryRatePolicy::Schedule(schedule),
            )
            .map_err(Into::into)
    }

    /// Exchange and retry ladder of one non-data frame: the association's
    /// control schedule selects its first rate, every retry rate and its
    /// publication budget.
    ///
    /// SOURCE: `libpp[pp.o]::ppTxProtoProc` leaves management frames,
    /// power-management (QoS-)Null and BlockAck control without the data bit,
    /// and `trc_set_per_pkt_rate` sets bit 25 on EAPOL; `libpp[trc.o]::rcGetSched` then
    /// selects trc `+0x68`, `libpp[trc.o]::rcGetRate` walks it and `libpp[trc.o]::rcReachRetryLimit`
    /// bounds it by record byte `0x08`. A unicast probe request additionally
    /// selects `BasicOFDMSched`, which is the connected station's `+0x68`.
    fn control_exchange(
        config: &SingleMpduTxConfig,
    ) -> (MacTxPlan<TxPhyRate>, OrdinaryRetryRatePolicy) {
        let schedule = config.control_schedule;
        (
            MacTxPlan {
                access_category: LegacyTxQueue::Voice.access_category(),
                initial_rate: select_schedule_retry_rate(schedule, 0)
                    .expect("every recovered control schedule starts with a legacy rate"),
                publication_limit: schedule_publication_limit(schedule),
                publication_timeout: config.publication_timeout,
            },
            OrdinaryRetryRatePolicy::Schedule(schedule),
        )
    }

    /// Copy and encode one Ethernet frame, then publish the first DMA attempt.
    pub fn start<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        ethernet: &[u8],
        selection: StaDataTxSelection,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        let traffic = self.select_network_traffic(ethernet)?;
        self.start_with_traffic(hardware, ethernet, traffic, selection)
    }

    /// Publish one classified network MPDU through the matching EDCA queue
    /// and QoS sequence space.
    pub fn start_with_traffic<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        ethernet: &[u8],
        traffic: WifiTxTraffic,
        selection: StaDataTxSelection,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        if ethernet.len() < 14 {
            return Err(SingleMpduTxError::EthernetFrameTooShort);
        }
        let expected = self.select_network_traffic(ethernet)?;
        if traffic != expected {
            return Err(SingleMpduTxError::TrafficSelectionMismatch {
                expected,
                provided: traffic,
            });
        }

        let destination: [u8; 6] = ethernet[..6]
            .try_into()
            .expect("validated Ethernet destination");
        let source = ethernet[6..12]
            .try_into()
            .expect("validated Ethernet source");
        let ether_type = u16::from_be_bytes([ethernet[12], ethernet[13]]);
        let sequence_number = self
            .sequences
            .take_data(self.config.peer_qos.then_some(traffic.tid()))
            .expect("classified TID is a valid sequence space");
        let (frame_length, hardware_mic_length, hardware_key_selector) = {
            let buffer = self.ordinary.buffer_mut()?;
            match &mut self.security {
                ConnectedTxSecurity::Open => (
                    StaDataFrame {
                        source,
                        bssid: self.config.bssid,
                        destination,
                        sequence_number,
                        ether_type,
                        payload: &ethernet[14..],
                    }
                    .encode(&mut buffer[TX_METADATA_SIZE..])
                    .map_err(SingleMpduTxError::Encode)?,
                    0,
                    0,
                ),
                ConnectedTxSecurity::Wpa2Personal(key) => (
                    StaProtectedDataFrame {
                        source,
                        bssid: self.config.bssid,
                        destination,
                        sequence_number,
                        user_priority: traffic.tid(),
                        peer_qos: self.config.peer_qos,
                        ccmp_header: key.next_tx_ccmp_header()?,
                        ether_type,
                        payload: &ethernet[14..],
                    }
                    .encode(&mut buffer[TX_METADATA_SIZE..])
                    .map_err(SingleMpduTxError::Encode)?,
                    TX_CCMP_MIC_SIZE,
                    key.hardware_index(),
                ),
            }
        };
        let queue = traffic.queue();
        self.ordinary
            .start(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange: MacTxPlan {
                        access_category: traffic.access_category,
                        initial_rate: selection.rate,
                        publication_limit: selection.publication_limit,
                        publication_timeout: self.config.publication_timeout,
                    },
                    hardware_mic_length,
                    hardware_key_selector,
                    interface: oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: queue.vendor_data_scheduler_priority(),
                    packet_priority: queue.vendor_data_packet_priority(),
                    priority_count: 1,
                },
            )
            .map_err(Into::into)
    }

    /// Publish a protected EAPOL packet through the connected ordinary-TX
    /// owner. Group-key responses share this transaction with management and
    /// network traffic, so they cannot bypass DMA ownership or IRQ ordering.
    pub fn start_protected_eapol<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        payload: &[u8],
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let ConnectedTxSecurity::Wpa2Personal(key) = &mut self.security else {
            return Err(SingleMpduTxError::SecurityModeMismatch);
        };
        let sequence_number = self
            .sequences
            .take_data(self.config.peer_qos.then_some(0))
            .expect("selected EAPOL sequence-number owner exists");
        let ccmp_header = key.next_tx_ccmp_header()?;
        let frame_length = {
            let buffer = self.ordinary.buffer_mut()?;
            StaProtectedDataFrame {
                source: self.config.station_address,
                bssid: self.config.bssid,
                destination: self.config.bssid,
                sequence_number,
                user_priority: 7,
                peer_qos: self.config.peer_qos,
                ccmp_header,
                ether_type: 0x888e,
                payload,
            }
            .encode(&mut buffer[TX_METADATA_SIZE..])
            .map_err(SingleMpduTxError::Encode)?
        };
        let (exchange, retry_rate_policy) = Self::control_exchange(&self.config);
        self.ordinary
            .start_with_retry_rate_policy(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange,
                    hardware_mic_length: TX_CCMP_MIC_SIZE,
                    hardware_key_selector: key.hardware_index(),
                    interface: oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: LegacyTxQueue::Voice.vendor_data_scheduler_priority(),
                    packet_priority: LegacyTxQueue::Voice.vendor_data_packet_priority(),
                    priority_count: 1,
                },
                retry_rate_policy,
            )
            .map_err(Into::into)
    }

    /// Encode and publish one connected Action management frame.
    ///
    /// Under management frame protection a robust Action frame leaves
    /// CCMP-protected under the pairwise key, as the vendor's
    /// `ieee80211_crypto_encap` protects it; every other Action frame leaves
    /// in plaintext. The same pinned descriptor is shared with network data,
    /// so this method fails while any prior transaction is active. The runner
    /// enforces that control work is started only after the current network
    /// lease has lost hardware ownership.
    pub fn start_action<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        let protected = self.config.management_protection
            && body
                .first()
                .is_some_and(|category| is_robust_action_category(*category));
        self.start_management(
            hardware,
            StaManagementSubtype::Action,
            body,
            protected,
            config,
        )
    }

    /// Encode and publish the Deauthentication of a station leaving its
    /// access point, with `reason_code`.
    ///
    /// A Deauthentication is a robust management frame: under management
    /// frame protection it leaves CCMP-protected under the pairwise key, as
    /// the vendor's `ieee80211_send_mgmt` protects it.
    pub fn start_deauthentication<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        reason_code: u16,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        self.start_management(
            hardware,
            StaManagementSubtype::Deauthentication,
            &reason_code.to_le_bytes(),
            self.config.management_protection,
            ActionTxConfig::VENDOR_MANAGEMENT,
        )
    }

    fn start_management<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        subtype: StaManagementSubtype,
        body: &[u8],
        protected: bool,
        config: ActionTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let (frame_length, hardware_mic_length, hardware_key_selector) = if protected {
            let ConnectedTxSecurity::Wpa2Personal(key) = &mut self.security else {
                return Err(SingleMpduTxError::SecurityModeMismatch);
            };
            let ccmp_header = key.next_tx_ccmp_header()?;
            let sequence_number = self.sequences.take_non_qos();
            let buffer = self.ordinary.buffer_mut()?;
            let frame_length = StaProtectedManagementFrame {
                subtype,
                source: self.config.station_address,
                bssid: self.config.bssid,
                sequence_number,
                ccmp_header,
                body,
            }
            .encode(&mut buffer[TX_METADATA_SIZE..])
            .map_err(SingleMpduTxError::Encode)?;
            (frame_length, TX_CCMP_MIC_SIZE, key.hardware_index())
        } else {
            let sequence_number = self.sequences.take_non_qos();
            let buffer = self.ordinary.buffer_mut()?;
            let frame_length = StaManagementFrame {
                subtype,
                source: self.config.station_address,
                bssid: self.config.bssid,
                sequence_number,
                body,
            }
            .encode(&mut buffer[TX_METADATA_SIZE..])
            .map_err(SingleMpduTxError::Encode)?;
            (frame_length, 0, 0)
        };
        let (exchange, retry_rate_policy) = Self::control_exchange(&self.config);
        self.ordinary
            .start_with_retry_rate_policy(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange,
                    hardware_mic_length,
                    hardware_key_selector,
                    interface: oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: config.scheduler_priority,
                    packet_priority: config.packet_priority,
                    priority_count: 1,
                },
                retry_rate_policy,
            )
            .map_err(Into::into)
    }

    /// Resolve, encode and publish one plaintext ESP-NOW v1 Action MPDU
    /// through the connected station's sole ordinary descriptor.
    ///
    /// `active_channel` and `active_station` must come from the same live
    /// connected owner that supplied this transmitter. The portable protocol
    /// keeps peer and requested-PHY policy typed; the chip backend remains the
    /// only authority which may admit that PHY mode.
    #[allow(clippy::too_many_arguments)]
    pub fn start_esp_now_v1_plaintext<H: TxHardware, const PEERS: usize>(
        &mut self,
        hardware: &mut H,
        protocol: &EspNowProtocol<PEERS>,
        peer: EspNowPeerId,
        random_value: EspNowRandomValue,
        payload: &[u8],
        active_channel: WifiChannel,
        active_station: BoundVirtualInterface,
        config: EspNowTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduEspNowTxError> {
        if self.ordinary.active() {
            return Err(EspNowTxError::Tx(OrdinaryTxError::Busy).into());
        }
        let peer_channel = protocol
            .peers()
            .get(peer)
            .map_err(EspNowSendError::Peer)?
            .channel();
        if peer_channel != active_channel {
            return Err(EspNowTxError::ChannelMismatch {
                prepared: peer_channel,
                active: active_channel,
            }
            .into());
        }
        // Build against a copied sequence frontier. The real shared
        // management/non-QoS counter advances only after the backend creates
        // a live ordinary transaction. Ordinary TX has no fallible step after
        // TxDmaPublication::commit changes ownership and rings the infallible
        // doorbell, so `Ok` is the exact publication edge; PHY, buffer, queue
        // and LR-frontier rejection cannot burn a sequence number.
        let mut next_sequence = *self.sequences.non_qos_mut();
        let prepared = protocol.prepare_v1_tx(peer, &mut next_sequence, random_value, payload)?;
        let result = start_esp_now_v1_plaintext(
            &mut self.ordinary,
            hardware,
            prepared,
            active_channel,
            active_station,
            config,
        )
        .map_err(Into::into);
        if result.is_ok() {
            *self.sequences.non_qos_mut() = next_sequence;
        }
        result
    }

    /// Resolve and publish one plaintext v2 Action MPDU through the same
    /// connected ordinary transaction as v1.
    #[allow(clippy::too_many_arguments)]
    pub fn start_esp_now_v2_plaintext<H: TxHardware, const PEERS: usize>(
        &mut self,
        hardware: &mut H,
        protocol: &EspNowProtocol<PEERS>,
        peer: EspNowPeerId,
        random_value: EspNowRandomValue,
        payload: &[u8],
        active_channel: WifiChannel,
        active_station: BoundVirtualInterface,
        config: EspNowTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduEspNowTxError> {
        if self.ordinary.active() {
            return Err(EspNowTxError::Tx(OrdinaryTxError::Busy).into());
        }
        let peer_channel = protocol
            .peers()
            .get(peer)
            .map_err(EspNowV2SendError::Peer)?
            .channel();
        if peer_channel != active_channel {
            return Err(EspNowTxError::ChannelMismatch {
                prepared: peer_channel,
                active: active_channel,
            }
            .into());
        }
        let mut next_sequence = *self.sequences.non_qos_mut();
        let prepared = protocol.prepare_v2_tx(peer, &mut next_sequence, random_value, payload)?;
        let result = start_esp_now_v2_plaintext(
            &mut self.ordinary,
            hardware,
            prepared,
            active_channel,
            active_station,
            config,
        )
        .map_err(Into::into);
        if result.is_ok() {
            *self.sequences.non_qos_mut() = next_sequence;
        }
        result
    }

    /// Encode and publish one AP reachability Probe Request naming the
    /// association's SSID: to the access point's address when `directed`,
    /// to the broadcast address otherwise (as the destination and the
    /// BSSID).
    ///
    /// SOURCE(esp32s31): complete `libnet80211[wl_cnx.o]::send_ap_probe`
    /// passes the configured SSID (`g_wifi_nvs`) to
    /// `ieee80211_send_probereq` for either address; a hidden-SSID access
    /// point answers only a Probe Request that names its SSID.
    ///
    /// TX completion is not reachability evidence: connected control waits
    /// for a BSSID-validated Probe Response or beacon before cancelling its
    /// bounded probe sequence.
    pub fn start_beacon_probe<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        directed: bool,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        const BASIC_RATES: &[u8] = &[0x82, 0x84, 0x8b, 0x96];

        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let sequence_number = self.sequences.take_non_qos();
        let frame_length = {
            let buffer = self.ordinary.buffer_mut()?;
            let address = if directed {
                self.config.bssid
            } else {
                [0xff; 6]
            };
            ProbeRequest {
                destination: address,
                source: self.config.station_address,
                bssid: address,
                sequence_number,
                ssid: self.config.ssid.as_bytes(),
                supported_rates: BASIC_RATES,
            }
            .encode(&mut buffer[TX_METADATA_SIZE..])
            .map_err(|_| SingleMpduTxError::ProbeEncode)?
        };
        let (exchange, retry_rate_policy) = Self::control_exchange(&self.config);
        self.ordinary
            .start_with_retry_rate_policy(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange,
                    hardware_mic_length: 0,
                    hardware_key_selector: 0,
                    interface: oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: ActionTxConfig::VENDOR_MANAGEMENT.scheduler_priority,
                    packet_priority: ActionTxConfig::VENDOR_MANAGEMENT.packet_priority,
                    priority_count: 1,
                },
                retry_rate_policy,
            )
            .map_err(Into::into)
    }

    /// Encode and publish a station power-management Null Data frame.
    ///
    /// A successful return only means that the MPDU owns the hardware TX
    /// transaction. Callers must wait for [`SingleMpduTxOutcome::Success`]
    /// before treating `PowerSave` as acknowledged by the access point.
    pub fn start_power_management_null<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        if self.ordinary.active() {
            return Err(SingleMpduTxError::Busy);
        }
        let sequence_number = self.sequences.take_non_qos();
        let frame_length = {
            let buffer = self.ordinary.buffer_mut()?;
            StaNullDataFrame {
                station_address: self.config.station_address,
                bssid: self.config.bssid,
                sequence_number,
                power_management,
            }
            .encode(&mut buffer[TX_METADATA_SIZE..])
            .map_err(SingleMpduTxError::Encode)?
        };
        let (exchange, retry_rate_policy) = Self::control_exchange(&self.config);
        self.ordinary
            .start_with_retry_rate_policy(
                hardware,
                OrdinaryTxPlan {
                    frame_length,
                    descriptor_capacity: None,
                    exchange,
                    hardware_mic_length: 0,
                    hardware_key_selector: 0,
                    interface: oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxInterface::Station,
                    scheduler_priority: ActionTxConfig::VENDOR_MANAGEMENT.scheduler_priority,
                    packet_priority: ActionTxConfig::VENDOR_MANAGEMENT.packet_priority,
                    priority_count: 1,
                },
                retry_rate_policy,
            )
            .map_err(Into::into)
    }

    pub fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        self.ordinary.wait_deadline()
    }

    pub fn next_deadline(&self) -> Option<oer_time::Instant> {
        self.ordinary.next_deadline()
    }

    /// Consume one IRQ/deadline edge and retain or release DMA ownership.
    pub fn service<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, SingleMpduTxError> {
        self.ordinary.service(hardware, wake).map_err(Into::into)
    }
}
#[cfg(test)]
mod tests;

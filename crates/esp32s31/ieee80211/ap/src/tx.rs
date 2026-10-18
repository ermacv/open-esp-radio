//! ESP32-S31 ordinary TX ownership specialized for an access-point epoch.
//!
//! Frame construction remains in the portable AP/IEEE 802.11 layers. This
//! owner adds the chip queue, retry, power and descriptor policy and exposes
//! the same finite IRQ/deadline transaction used by STA. It deliberately does
//! not poll, spawn tasks or count a frame as transmitted before completion.

use oer_esp32s31_hal::types::MacInterface;

use oer_esp32s31_ieee80211::{
    ampdu_tx::{AmpduTxRoleAdapter, HtAmpduPublicationInputs, ht_ampdu_publication_config},
    ordinary_tx::{
        OrdinaryTxError, OrdinaryTxInterface, OrdinaryTxOutcome, OrdinaryTxOwner, OrdinaryTxPlan,
        TX_CCMP_MIC_SIZE, TX_METADATA_SIZE, WifiTxEntropy, WifiTxPowerProfile, WifiTxResources,
    },
    tx::{WifiTxProgress, WifiTxWake},
};

use oer_esp32s31_ieee80211_mac::tx::{
    HtAmpduTxConfig, HtChannelWidth, HtDuplicateCertificationRequest, HtDuplicateRate,
    HtDuplicateTxLinkCapabilities, HtDuplicateTxSelection, HtGuardInterval, HtMcs, HtRate,
    LegacyRate, LegacyTxQueue, TxHardware, TxPhyRate,
    protection::{BssProtection, ProtectedPpdu, TxPsdu, TxReceiver},
    runtime::{OrdinaryRetryRatePolicy, select_schedule_retry_rate},
    select_esp32s31_ht_duplicate_tx,
};

use oer_esp32s31_ieee80211_mac::rate::schedule::schedule_publication_limit;
use oer_espressif_ieee80211_policy::rate_control::DEFAULT_CONTROL_SCHEDULE;
use oer_ieee80211_mac::{
    block_ack::BLOCK_ACK_REQUEST_LEN,
    channel::{WifiChannel, WifiChannelWidth},
    ht::HtPeerCapabilities,
    qos::WmmAccessCategory,
};

use oer_ieee80211_softmac::{MacTxPlan, MacTxQueueState};

/// Runtime-independent publication policy for the initial AP implementation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApTxConfig {
    /// Watchdog applied independently to each hardware publication.
    pub publication_timeout: oer_time::Duration,
}

/// Semantic AP frame class translated into the private ESP32-S31 queue plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApTxClass {
    /// Broadcast beacon; never retried for an acknowledgement.
    Beacon,
    /// Authentication, association or deauthentication response.
    Management,
    /// Best-effort discovery response: one attempt, never a retained retry series.
    ProbeResponse,
    /// Unprotected EAPOL exchange during the four-way handshake.
    Eapol,
    /// Pairwise protected Ethernet data after the controlled port opens.
    Data,
    /// GTK-protected multicast/broadcast data. Hardware may report terminal
    /// publication success, but there is no receiver ACK and no retry series.
    GroupData,
}

impl ApTxClass {
    fn publication_limit(self, rate: LegacyRate) -> u8 {
        match self {
            Self::Beacon | Self::GroupData | Self::ProbeResponse => 1,
            Self::Management | Self::Eapol | Self::Data => rate
                .vendor_retry_publication_limit()
                .expect("every AP legacy rate has a recovered Dot11G schedule"),
        }
    }

    const fn queue(self) -> LegacyTxQueue {
        // The recovered vendor management/control path uses VO. EAPOL is
        // deliberately kept on the same bounded pre-data path until the
        // controlled port opens.
        match self {
            Self::Beacon | Self::Management | Self::ProbeResponse | Self::Eapol => {
                LegacyTxQueue::Voice
            }
            Self::Data | Self::GroupData => LegacyTxQueue::BestEffort,
        }
    }

    const fn initial_rate(self) -> LegacyRate {
        match self {
            // The AP advertises an ERP+HT BSS, so every associated peer
            // still supports the mandatory 24 Mbit/s OFDM legacy rate. Keep
            // discovery and controlled-port setup on the maximally compatible
            // 1 Mbit/s path, but do not serialize ordinary data at that rate.
            Self::Data => LegacyRate::Ofdm24M,
            Self::GroupData => LegacyRate::Dsss1MLong,
            Self::Beacon | Self::Management | Self::ProbeResponse | Self::Eapol => {
                LegacyRate::Dsss1MLong
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApTxError {
    FrameTooLarge,
    /// An aggregate MPDU without hardware CCMP cannot be retried alone.
    UnprotectedAggregateMpdu,
    Ordinary(OrdinaryTxError),
}

impl From<OrdinaryTxError> for ApTxError {
    fn from(error: OrdinaryTxError) -> Self {
        Self::Ordinary(error)
    }
}

/// Unique ordinary descriptor owned by one active AP epoch.
#[must_use = "an AP TX owner must be quiesced and recovered before role transition"]
pub struct ApTx<'slot, P, E, T, const BUFFER_SIZE: usize> {
    ordinary: OrdinaryTxOwner<'slot, P, E, T, BUFFER_SIZE>,
    config: ApTxConfig,
    ht_duplicate_certification: Option<HtDuplicateCertificationRequest>,
}

/// The transmitter's timer, for phases that wait on its clock.
impl<P, E, T: oer_time::Clock, const BUFFER_SIZE: usize> oer_time::Clock
    for ApTx<'_, P, E, T, BUFFER_SIZE>
{
    fn now(&self) -> oer_time::Instant {
        self.ordinary.now()
    }
}

impl<P, E, T: oer_time::Timer, const BUFFER_SIZE: usize> oer_time::Timer
    for ApTx<'_, P, E, T, BUFFER_SIZE>
{
    fn wait_until(&self, deadline: oer_time::Instant) -> impl Future<Output = ()> {
        self.ordinary.wait_until(deadline)
    }
}

/// AP-local ordinary-TX policy retained while the physical descriptor owner
/// is lent to the station role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApTxParked {
    config: ApTxConfig,
    ht_duplicate_certification: Option<HtDuplicateCertificationRequest>,
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> ApTx<'slot, P, E, T, BUFFER_SIZE>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    /// Borrow the timer adapter, including its separately owned radio clock.
    pub fn timer(&self) -> &T {
        &self.ordinary.timer
    }

    /// Submitted ordinary work, read before another exchange starts.
    pub fn work(&self) -> oer_ieee80211_softmac::MacTxWork {
        self.ordinary.work()
    }

    pub fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: ApTxConfig,
    ) -> Self {
        Self {
            ordinary: OrdinaryTxOwner::new(resources),
            config,
            ht_duplicate_certification: None,
        }
    }

    /// Install the independent, fixed MCS32 certification request.
    ///
    /// Normal AP rate selection never consults this request as a ranked
    /// candidate. The peer planner reports its typed rejection and retains
    /// the ordinary HT fallback until the S31 formatter contract is reviewed.
    pub fn set_ht_duplicate_certification_request(
        &mut self,
        request: Option<HtDuplicateCertificationRequest>,
    ) {
        self.ht_duplicate_certification = request;
    }

    pub const fn ht_duplicate_certification_request(
        &self,
    ) -> Option<HtDuplicateCertificationRequest> {
        self.ht_duplicate_certification
    }

    pub fn queue_state(&self) -> MacTxQueueState {
        self.ordinary.queue_state()
    }

    pub const fn maximum_ht_aggregate_bytes(&self) -> u16 {
        self.ordinary.policy().ht_ampdu().maximum_aggregate_bytes()
    }

    pub fn install_bss_protection(&mut self, protection: BssProtection) {
        self.ordinary
            .policy_mut()
            .install_bss_protection(protection);
    }

    pub const fn publication_timeout(&self) -> oer_time::Duration {
        self.config.publication_timeout
    }

    /// Build the AP-specific key/interface/power wrapper around the common
    /// HT A-MPDU formatter. Frame retention and BlockAck completion remain in
    /// the role-neutral aggregate owner.
    pub fn ht_ampdu_config(
        &mut self,
        rate: HtRate,
        aggregate_length: u16,
        subframes: u8,
        hardware_key_selector: u8,
    ) -> Option<HtAmpduTxConfig> {
        let queue = LegacyTxQueue::BestEffort;
        let (contention, contention_window) = self.ordinary.contention_publication(queue);
        let data_power = self.ordinary.power().power_pair(rate.power_lookup_code());
        let (_, control) = self.ordinary.control_frame_for(ProtectedPpdu {
            rate: TxPhyRate::Ht(rate),
            receiver: TxReceiver::Individual,
            psdu: TxPsdu::Ampdu {
                length: u32::from(aggregate_length),
            },
        });
        ht_ampdu_publication_config(
            AmpduTxRoleAdapter {
                interface: MacInterface::AccessPoint,
                hardware_key_selector,
            },
            HtAmpduPublicationInputs {
                rate,
                aggregate_length,
                subframes,
                protection_spacing: self.ordinary.policy().ht_ampdu().protection_spacing(),
                data_power_primary: data_power.primary as u8,
                data_power_alternate: data_power.alternate as u8,
                control,
                aifsn: contention.aifsn(),
                contention_window,
                scheduler_priority: queue.vendor_data_scheduler_priority(),
                packet_priority: queue.vendor_data_packet_priority(),
            },
        )
    }

    /// Advance the shared BE contention state before republishing a retained
    /// aggregate. The AP and STA paths must use the same recovered LMAC EDCA
    /// transition: a retry is not a fresh exchange at ECWmin.
    pub(crate) fn record_aggregate_retry_failure(&mut self) {
        self.ordinary
            .record_retry_failure(LegacyTxQueue::BestEffort);
    }

    /// Restore ECWmin after a fully acknowledged aggregate exchange.
    pub(crate) fn record_aggregate_success(&mut self) {
        self.ordinary.record_success(LegacyTxQueue::BestEffort);
    }

    /// Restore ECWmin after a terminal incomplete, collision, or timeout
    /// result before a different MSDU exchange can claim the queue.
    pub fn reset_aggregate_contention(&mut self) {
        self.ordinary
            .reset_terminal_exchange(LegacyTxQueue::BestEffort);
    }

    /// Copy one portable encoded MPDU into the only ordinary DMA slot and
    /// publish it. A successful return means hardware owns the descriptor,
    /// not that the frame reached the air.
    pub fn start_encoded<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        class: ApTxClass,
        frame: &[u8],
    ) -> Result<WifiTxProgress, ApTxError> {
        self.start_encoded_with_key(hardware, class, frame, 0, 0, None)
    }

    /// Publish one plaintext protected MPDU through hardware CCMP.
    pub fn start_protected_encoded<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        hardware_key_selector: u8,
        rate: LegacyRate,
    ) -> Result<WifiTxProgress, ApTxError> {
        self.start_encoded_with_key(
            hardware,
            ApTxClass::Data,
            frame,
            TX_CCMP_MIC_SIZE,
            hardware_key_selector,
            Some(rate),
        )
    }

    /// Publish one plaintext Open-network data MPDU. Zero MIC length is the
    /// authoritative security selector; the key field is ignored by the
    /// already-reviewed ordinary unprotected path used for EAPOL/management.
    pub fn start_unprotected_data_encoded<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        group: bool,
        rate: LegacyRate,
    ) -> Result<WifiTxProgress, ApTxError> {
        self.start_encoded_with_key(
            hardware,
            if group {
                ApTxClass::GroupData
            } else {
                ApTxClass::Data
            },
            frame,
            0,
            0,
            Some(rate),
        )
    }

    /// Publish one GTK-protected group MPDU through the ordinary basic-rate
    /// owner. The transaction has exactly one hardware publication and does
    /// not interpret completion as an acknowledgement.
    pub fn start_group_protected_encoded<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        hardware_key_selector: u8,
    ) -> Result<WifiTxProgress, ApTxError> {
        self.start_encoded_with_key(
            hardware,
            ApTxClass::GroupData,
            frame,
            TX_CCMP_MIC_SIZE,
            hardware_key_selector,
            Some(LegacyRate::Dsss1MLong),
        )
    }

    /// Publish one protected client MPDU taken out of a completed aggregate
    /// whose Block Ack agreement ended. Sequence Control and the CCMP header
    /// are already encoded and are kept; only the IEEE Retry bit is added.
    /// The MPDU follows the AP's ordinary pairwise data exchange.
    pub fn start_aggregate_mpdu_retry<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        encoded: &[u8],
        hardware_mic_length: usize,
        hardware_key_selector: u8,
    ) -> Result<WifiTxProgress, ApTxError> {
        if hardware_mic_length == 0 || encoded.len() < 2 {
            return Err(ApTxError::UnprotectedAggregateMpdu);
        }
        self.copy_frame(encoded)?;
        // Frame Control byte 1, bit 3: the IEEE Retry bit.
        self.ordinary.buffer_mut()?[TX_METADATA_SIZE + 1] |= 0x08;
        self.start_copied(
            hardware,
            ApTxClass::Data,
            encoded.len(),
            hardware_mic_length,
            hardware_key_selector,
            None,
        )
    }

    /// Publish one encoded BlockAckReq on the access category of its TID.
    ///
    /// SOURCE: `libpp[pp.o]::ppFillAMPDUBar` queues the request on the
    /// access category of its TID; `libpp[trc.o]::rcGetSched` selects row three of
    /// `rc11BSchedTbl`, the 1 Mbit/s long-preamble schedule with 32
    /// publications, and the descriptor solicits a BlockAck (blobray
    /// ).
    pub fn start_block_ack_request<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8; BLOCK_ACK_REQUEST_LEN],
        access_category: WmmAccessCategory,
    ) -> Result<WifiTxProgress, ApTxError> {
        self.copy_frame(frame)?;
        let schedule = DEFAULT_CONTROL_SCHEDULE;
        let queue = LegacyTxQueue::from_access_category(access_category);
        Ok(self.ordinary.start_with_retry_rate_policy(
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
                interface: OrdinaryTxInterface::AccessPoint,
                scheduler_priority: queue.vendor_data_scheduler_priority(),
                packet_priority: queue.vendor_data_packet_priority(),
                priority_count: 1,
            },
            OrdinaryRetryRatePolicy::Schedule(schedule),
        )?)
    }

    fn copy_frame(&mut self, frame: &[u8]) -> Result<(), ApTxError> {
        let end = TX_METADATA_SIZE
            .checked_add(frame.len())
            .ok_or(ApTxError::FrameTooLarge)?;
        let buffer = self.ordinary.buffer_mut()?;
        let destination = buffer
            .get_mut(TX_METADATA_SIZE..end)
            .ok_or(ApTxError::FrameTooLarge)?;
        destination.copy_from_slice(frame);
        Ok(())
    }

    fn start_encoded_with_key<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        class: ApTxClass,
        frame: &[u8],
        hardware_mic_length: usize,
        hardware_key_selector: u8,
        rate: Option<LegacyRate>,
    ) -> Result<WifiTxProgress, ApTxError> {
        self.copy_frame(frame)?;
        self.start_copied(
            hardware,
            class,
            frame.len(),
            hardware_mic_length,
            hardware_key_selector,
            rate,
        )
    }

    fn start_copied<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        class: ApTxClass,
        frame_length: usize,
        hardware_mic_length: usize,
        hardware_key_selector: u8,
        rate: Option<LegacyRate>,
    ) -> Result<WifiTxProgress, ApTxError> {
        let queue = class.queue();
        let initial_rate = rate.unwrap_or_else(|| class.initial_rate());
        Ok(self.ordinary.start(
            hardware,
            OrdinaryTxPlan {
                frame_length,
                descriptor_capacity: None,
                exchange: MacTxPlan {
                    access_category: queue.access_category(),
                    initial_rate: TxPhyRate::Legacy(initial_rate),
                    publication_limit: class.publication_limit(initial_rate),
                    publication_timeout: self.config.publication_timeout,
                },
                hardware_mic_length,
                hardware_key_selector,
                interface: OrdinaryTxInterface::AccessPoint,
                scheduler_priority: queue.vendor_data_scheduler_priority(),
                packet_priority: queue.vendor_data_packet_priority(),
                priority_count: 1,
            },
        )?)
    }

    /// Advance an active descriptor from one coalesced IRQ/deadline edge.
    pub fn service<H: TxHardware>(
        &mut self,
        hardware: &mut H,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, ApTxError> {
        Ok(self.ordinary.service(hardware, wake)?)
    }

    pub fn take_last_outcome(&mut self) -> Option<OrdinaryTxOutcome> {
        self.ordinary.take_last_outcome()
    }

    pub fn wait_deadline(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.ordinary.wait_deadline()
    }

    /// Lend the physical ordinary descriptor at an idle role boundary.
    #[allow(clippy::result_large_err)]
    pub fn try_park(
        self,
    ) -> Result<(WifiTxResources<'slot, P, E, T, BUFFER_SIZE>, ApTxParked), Self> {
        let Self {
            ordinary,
            config,
            ht_duplicate_certification,
        } = self;
        match ordinary.try_into_resources() {
            Ok(resources) => Ok((
                resources,
                ApTxParked {
                    config,
                    ht_duplicate_certification,
                },
            )),
            Err(ordinary) => Err(Self {
                ordinary,
                config,
                ht_duplicate_certification,
            }),
        }
    }

    pub fn resume(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        parked: ApTxParked,
    ) -> Self {
        Self {
            ordinary: OrdinaryTxOwner::new(resources),
            config: parked.config,
            ht_duplicate_certification: parked.ht_duplicate_certification,
        }
    }

    /// Recover the common TX capability only when no descriptor is owned by
    /// DMA and the queue is not quarantined for radio reset.
    #[allow(clippy::result_large_err)]
    pub fn try_into_resources(self) -> Result<WifiTxResources<'slot, P, E, T, BUFFER_SIZE>, Self> {
        self.try_park().map(|(resources, _)| resources)
    }
}

/// Translate the portable 500-kbit/s association rate into the exact legacy
/// hardware rate enum. The parser supplies only members of the AP's advertised
/// B/G rate set; the 1-Mbit/s fallback preserves safety for stale peer state.
pub const fn peer_legacy_rate(maximum_rate_500kbps: u8) -> LegacyRate {
    match maximum_rate_500kbps {
        108 => LegacyRate::Ofdm54M,
        96 => LegacyRate::Ofdm48M,
        72 => LegacyRate::Ofdm36M,
        48 => LegacyRate::Ofdm24M,
        36 => LegacyRate::Ofdm18M,
        24 => LegacyRate::Ofdm12M,
        18 => LegacyRate::Ofdm9M,
        12 => LegacyRate::Ofdm6M,
        22 => LegacyRate::Cck11MLong,
        11 => LegacyRate::Cck5M5Long,
        4 => LegacyRate::Dsss2MLong,
        _ => LegacyRate::Dsss1MLong,
    }
}

/// Select the fastest one-stream HT vector admitted by both BSS geometry and
/// the peer's observed receive capabilities.
pub fn peer_ht_rate(channel: WifiChannel, peer: HtPeerCapabilities) -> Option<HtRate> {
    let mcs = HtMcs::from_index(peer.highest_rx_mcs())?;
    let (channel_width, guard_width) = match channel.width() {
        WifiChannelWidth::Mhz20 => (HtChannelWidth::Mhz20, WifiChannelWidth::Mhz20),
        width @ (WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below)
            if peer.supports_40_mhz() =>
        {
            (HtChannelWidth::Mhz40, width)
        }
        WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below => {
            (HtChannelWidth::Mhz20, WifiChannelWidth::Mhz20)
        }
    };
    // The recovered retry table has a complete SGI schedule only for MCS7.
    // Lower peer maxima therefore stay on their complete LGI schedules.
    let guard_interval = if mcs == HtMcs::Mcs7 && peer.supports_short_guard_interval(guard_width) {
        HtGuardInterval::Short400Ns
    } else {
        HtGuardInterval::Long800Ns
    };
    Some(HtRate::new(mcs, guard_interval, channel_width))
}

/// Retain a peer-admitted HT Duplicate candidate without publishing it.
///
/// The returned value is protocol-valid, including peer capability and HT40
/// geometry. It cannot enter [`TxPhyRate`] until the ESP32-S31 queue/rate/power
/// encoding has a reviewed oracle; the explicit selector below reports that
/// hardware rejection without changing the ordinary peer rate.
pub fn peer_ht_duplicate_rate(
    channel: WifiChannel,
    peer: HtPeerCapabilities,
) -> Option<HtDuplicateRate> {
    peer.ht_duplicate_mcs32()?;
    let width = match channel.width() {
        width @ (WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below) => width,
        WifiChannelWidth::Mhz20 => return None,
    };
    let guard_interval = if peer.supports_short_guard_interval(width) {
        HtGuardInterval::Short400Ns
    } else {
        HtGuardInterval::Long800Ns
    };
    Some(HtDuplicateRate::new(guard_interval))
}

/// Evaluate the AP's explicit MCS32 request for one associated peer.
///
/// This is an observation/planning path only. A rejected request leaves
/// [`peer_ht_rate`] in sole ownership of the publishable HT fallback.
pub fn peer_ht_duplicate_tx_selection(
    channel: WifiChannel,
    peer: Option<HtPeerCapabilities>,
    request: Option<HtDuplicateCertificationRequest>,
) -> HtDuplicateTxSelection {
    let Some(peer) = peer else {
        return select_esp32s31_ht_duplicate_tx(
            request,
            HtDuplicateTxLinkCapabilities::new(None, false, false),
        );
    };
    let (channel_width, peer_supports_short_guard_interval) = match channel.width() {
        WifiChannelWidth::Mhz20 => (
            HtChannelWidth::Mhz20,
            peer.supports_short_guard_interval(WifiChannelWidth::Mhz20),
        ),
        width @ (WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below) => (
            HtChannelWidth::Mhz40,
            peer.supports_short_guard_interval(width),
        ),
    };
    select_esp32s31_ht_duplicate_tx(
        request,
        HtDuplicateTxLinkCapabilities::new(
            Some(channel_width),
            peer.supports_ht_duplicate_mcs32(),
            peer_supports_short_guard_interval,
        ),
    )
}

#[cfg(test)]
mod tests;

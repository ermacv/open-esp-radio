#![expect(
    clippy::manual_async_fn,
    reason = "TX adapters keep the executor-neutral borrowed Future contracts explicit"
)]

use super::*;
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_time::Clock as _;

impl<
    B: MaterializedTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> ConnectedControlTx
    for ConnectedTx<'_, '_, B, P, E, T, SLOTS, AMPDU_BUFFER_SIZE, ORDINARY_BUFFER_SIZE>
{
    fn take_last_outcome(&mut self) -> Option<SingleMpduTxOutcome> {
        self.take_last_ordinary_outcome()
    }

    fn has_network_tx_report(&self) -> bool {
        !self.network_power.is_empty()
    }

    fn take_network_tx_report(&mut self) -> NetworkTxPowerReport {
        core::mem::take(&mut self.network_power)
    }

    fn peek_qos_sequence(&self, tid: u8) -> Option<SequenceNumber> {
        self.ordinary.peek_qos_sequence(tid)
    }

    fn start_action<H: oer_esp32s31_ieee80211_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
        body: &[u8],
        config: ActionTxConfig,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary.start_action(hardware, body, config)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(DatapathControlProgress::TxPending)
    }

    fn start_deauthentication<H: oer_esp32s31_ieee80211_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
        reason_code: u16,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary
            .start_deauthentication(hardware, reason_code)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(DatapathControlProgress::TxPending)
    }

    fn start_power_management_null<H: oer_esp32s31_ieee80211_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
        power_management: StaPowerManagement,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary
            .start_power_management_null(hardware, power_management)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(DatapathControlProgress::TxPending)
    }

    fn start_beacon_probe<H: oer_esp32s31_ieee80211_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
        directed: bool,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary.start_beacon_probe(hardware, directed)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(DatapathControlProgress::TxPending)
    }

    fn start_protected_eapol<H: oer_esp32s31_ieee80211_mac::tx::TxHardware>(
        &mut self,
        hardware: &mut H,
        payload: &[u8],
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, SingleMpduTxError> {
        if self.active() {
            return Err(SingleMpduTxError::Busy);
        }
        self.ordinary.start_protected_eapol(hardware, payload)?;
        self.active = ConnectedTxActive::Ordinary;
        Ok(DatapathControlProgress::TxPending)
    }

    fn set_tx_block_ack_agreement(&mut self, tid: u8, agreement: Option<(u16, bool)>) {
        self.set_block_ack_agreement(tid, agreement);
    }

    fn bss_protection(&self) -> oer_esp32s31_ieee80211_mac::tx::protection::BssProtection {
        self.ordinary.policy().protection().bss()
    }

    fn install_bss_protection(
        &mut self,
        protection: oer_esp32s31_ieee80211_mac::tx::protection::BssProtection,
    ) {
        self.ordinary
            .policy_mut()
            .install_bss_protection(protection);
    }

    fn publish_he_trigger_response<H: oer_esp32s31_ieee80211_mac::tx::TxHardware>(
        &mut self,
        _hardware: &mut H,
        request: HeTriggerRuntimeRequest,
    ) -> Result<(), ConnectedHeControlRuntimeRejection> {
        // Recheck at the final owner: control validation and this handoff are
        // separate calls, and neither may extend an expired response window.
        if self.ordinary.now() >= request.response_deadline {
            return Err(ConnectedHeControlRuntimeRejection::MissedResponseWindow);
        }
        if self.he_trigger_based != Some(request.queue_policy) {
            return Err(ConnectedHeControlRuntimeRejection::QueuePolicyMismatch);
        }
        if request.queue_policy.tid().value() != HE_TRIGGER_DATA_TID {
            return Err(ConnectedHeControlRuntimeRejection::QueueTidMismatch);
        }
        if self.active() {
            return Err(ConnectedHeControlRuntimeRejection::TxOwnerBusy);
        }
        if self.standby_error.is_some() {
            return Err(ConnectedHeControlRuntimeRejection::PreparedQueueFaulted);
        }
        let Some(prepared) = self.standby_prepared.as_ref() else {
            return Err(ConnectedHeControlRuntimeRejection::PreparedQueueUnavailable);
        };
        if prepared.traffic.tid() != HE_TRIGGER_DATA_TID
            || prepared.traffic.queue() != LegacyTxQueue::BestEffort
        {
            return Err(ConnectedHeControlRuntimeRejection::QueueTidMismatch);
        }
        if !matches!(self.config.rate, TxPhyRate::He(_)) {
            return Err(ConnectedHeControlRuntimeRejection::UnsupportedQueueFormat);
        }
        if oer_esp32s31_hal::types::MacHeTbLinkReservation::for_queue(
            request.queue_policy.tid_limit(),
            LegacyTxQueue::BestEffort.hardware_index(),
            prepared.original_subframes,
        )
        .is_none()
        {
            return Err(ConnectedHeControlRuntimeRejection::UnsupportedQueueGeometry);
        }

        // This is the single future-oracle attachment point. The retained
        // standby arena still owns every network lease and DMA byte, while the
        // reviewed lower layer can validate MPLEN/BSR. What is not reviewed is
        // the HE-TB PHY-vector/doorbell transition which would bind the parsed
        // RU, GI/LTF, MCS and DATA_LENGTH. Do not publish the HE-SU formatter's
        // vector as a substitute.
        Err(ConnectedHeControlRuntimeRejection::TbPhyPublicationUnverified)
    }

    fn publish_he_ndpa_feedback<H: oer_esp32s31_ieee80211_mac::tx::TxHardware>(
        &mut self,
        _hardware: &mut H,
        request: HeNdpaRuntimeRequest,
    ) -> Result<(), ConnectedHeControlRuntimeRejection> {
        if self.ordinary.now() >= request.response_deadline {
            return Err(ConnectedHeControlRuntimeRejection::MissedResponseWindow);
        }
        if self.active() {
            return Err(ConnectedHeControlRuntimeRejection::TxOwnerBusy);
        }
        Err(ConnectedHeControlRuntimeRejection::NdpaFeedbackPublicationUnverified)
    }
}

impl<
    B: MaterializedTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> crate::roles::station::esp_now_tx::EspNowConnectedTx
    for ConnectedTx<'_, '_, B, P, E, T, SLOTS, AMPDU_BUFFER_SIZE, ORDINARY_BUFFER_SIZE>
{
    fn start_esp_now_v1_plaintext<
        H: oer_esp32s31_ieee80211_mac::tx::TxHardware,
        const PEERS: usize,
    >(
        &mut self,
        hardware: &mut H,
        protocol: &oer_ieee80211_softmac::EspNowProtocol<PEERS>,
        request: &crate::roles::esp_now::mailbox::EspNowOwnedV1Tx,
        active_channel: oer_ieee80211_mac::channel::WifiChannel,
        active_station: oer_ieee80211_softmac::interface::BoundVirtualInterface,
        config: oer_esp32s31_ieee80211::esp_now::EspNowTxConfig,
    ) -> Result<WifiTxProgress, oer_esp32s31_ieee80211_sta::single_mpdu_tx::SingleMpduEspNowTxError>
    {
        if self.active() {
            return Err(
                oer_esp32s31_ieee80211_sta::single_mpdu_tx::SingleMpduEspNowTxError::Backend(
                    oer_esp32s31_ieee80211::esp_now::EspNowTxError::Tx(
                        oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxError::Busy,
                    ),
                ),
            );
        }
        let progress = self.ordinary.start_esp_now_v1_plaintext(
            hardware,
            protocol,
            request.peer(),
            request.random_value(),
            request.payload(),
            active_channel,
            active_station,
            config,
        )?;
        if progress == WifiTxProgress::Pending {
            self.active = ConnectedTxActive::Ordinary;
        }
        Ok(progress)
    }

    fn start_esp_now_v2_plaintext<
        H: oer_esp32s31_ieee80211_mac::tx::TxHardware,
        const PEERS: usize,
    >(
        &mut self,
        hardware: &mut H,
        protocol: &oer_ieee80211_softmac::EspNowProtocol<PEERS>,
        request: crate::roles::esp_now::mailbox::EspNowV2TxRequest<'_>,
        active_channel: oer_ieee80211_mac::channel::WifiChannel,
        active_station: oer_ieee80211_softmac::interface::BoundVirtualInterface,
        config: oer_esp32s31_ieee80211::esp_now::EspNowTxConfig,
    ) -> Result<WifiTxProgress, oer_esp32s31_ieee80211_sta::single_mpdu_tx::SingleMpduEspNowTxError>
    {
        if self.active() {
            return Err(
                oer_esp32s31_ieee80211_sta::single_mpdu_tx::SingleMpduEspNowTxError::Backend(
                    oer_esp32s31_ieee80211::esp_now::EspNowTxError::Tx(
                        oer_esp32s31_ieee80211::ordinary_tx::OrdinaryTxError::Busy,
                    ),
                ),
            );
        }
        let progress = self.ordinary.start_esp_now_v2_plaintext(
            hardware,
            protocol,
            request.peer(),
            request.random_value(),
            request.payload(),
            active_channel,
            active_station,
            config,
        )?;
        if progress == WifiTxProgress::Pending {
            self.active = ConnectedTxActive::Ordinary;
        }
        Ok(progress)
    }
}

/// The ordinary owner's clock, for control waits.
impl<
    B: MaterializedTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> oer_time::Clock
    for ConnectedTx<'_, '_, B, P, E, T, SLOTS, AMPDU_BUFFER_SIZE, ORDINARY_BUFFER_SIZE>
{
    fn now(&self) -> oer_time::Instant {
        self.ordinary.now()
    }
}

impl<
    B: MaterializedTxFrame,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> oer_time::Timer
    for ConnectedTx<'_, '_, B, P, E, T, SLOTS, AMPDU_BUFFER_SIZE, ORDINARY_BUFFER_SIZE>
{
    fn wait_until(&self, deadline: oer_time::Instant) -> impl Future<Output = ()> {
        self.ordinary.wait_until(deadline)
    }
}

impl<
    'slot,
    'ampdu,
    B,
    H,
    P,
    E,
    T,
    SoftwareFrame,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> DatapathNetworkTxService<H, SoftwareFrame, B>
    for ConnectedTx<'slot, 'ampdu, B, P, E, T, SLOTS, AMPDU_BUFFER_SIZE, ORDINARY_BUFFER_SIZE>
where
    B: MaterializedTxFrame,
    H: HtAmpduHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + crate::mac_clock::MacClockReader,
    SoftwareFrame: SoftwareTxFrame,
{
    type Error = AggregateTxError;

    fn start<'a, I>(
        &'a mut self,
        hardware: &'a mut H,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a
    where
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = B> + 'a,
    {
        async move {
            match self.start_request(hardware, frame, network) {
                Ok(progress) => Ok(progress),
                Err(RequestTxError::Busy(_)) => Err(AggregateTxError::ActiveTransaction),
                Err(RequestTxError::Unmaterialized(_)) => {
                    panic!("station aggregate selected without a free DMA credit")
                }
                Err(RequestTxError::Radio(error)) => Err(error),
            }
        }
    }

    fn last_started_frame_count(&self) -> usize {
        self.active_network_frame_count()
    }

    fn wait_deadline(&mut self) -> impl Future<Output = ()> + '_ {
        ConnectedTx::wait_deadline(self)
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move { ConnectedTx::service(self, hardware, wake) }
    }

    fn has_prepared(&self) -> bool {
        self.has_prepared_network_tx()
    }

    fn batch_demand<I>(&self, network: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = B>,
    {
        self.network_batch_demand(network.queue_len())
    }

    fn prepared_frame_count(&self) -> usize {
        self.prepared_network_frame_count()
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_scheduler_phase(&mut self, phase: PreparedTxSchedulerPhase, at_micros: u64) {
        ConnectedTx::mark_prepared_scheduler_phase(self, phase, at_micros);
    }

    fn start_prepared<I>(
        &mut self,
        hardware: &mut H,
        network: &I,
    ) -> Result<WifiTxProgress, Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = B>,
    {
        self.start_prepared_network(hardware, network)
    }

    fn cancel_prepared<I>(&mut self, _network: &I) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = B>,
    {
        self.cancel_prepared_network()
    }

    fn can_prepare(&self) -> bool {
        self.can_prepare_network_tx()
    }

    fn prepare<'a, I>(
        &'a mut self,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
        SoftwareFrame: 'a,
        I: SelectedBurstMaterializer<SoftwareFrame = SoftwareFrame, PhysicalFrame = B> + 'a,
    {
        async move {
            if !self.can_prepare_network_tx() {
                return Ok(());
            }
            self.prepare_request_standby(frame, network)
                .unwrap_or_else(|_| panic!("station standby selected without a free DMA credit"));
            Ok(())
        }
    }
}

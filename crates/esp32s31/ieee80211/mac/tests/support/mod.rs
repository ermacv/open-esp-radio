use crate::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Operation {
    BeginColdHandshake(u32),
    InitializeMacAntenna,
    InitializeHalTail(MacInterruptMask, MacSlowClockCalibration),
    InitializeColdCoex(MacColdCoexPti),
    InitializeCryptoBypass,
    InitializeLastRxBufferTable,
    InitializeTxRxPrefix,
    InitializeTxRxCallbacks(MacDelaySlot),
    InitializeTxRxSuffix,
    InitializeColdReceivePolicy,
    DisablePhyLowRate,
    EnableMacInterrupts(MacInterruptMask),
    ProgramInterfaceAddress(MacInterface, [u8; 6]),
    ApplyStaLinkPolicy([u8; 6]),
    InstallCcmp(MacInterface, u8, MacCcmpKeyIdentity),
    ClearCcmp(u8),
    InitializeHePrefix,
    InitializeTxPower(MacTxPowerTable),
    InitializeHeSuffix,
    SetWifiMacReset(bool),
    InitializeRxBufferPrefix,
    ConfigureRxDescriptorWindow,
    ObserveRxLastDescriptor,
    ObserveRxNextDescriptor,
    ObserveRxWalkerEnabled,
    ObserveRxReloadPending,
    PublishRxDescriptorBase(u32),
    PublishRxWalkerEnable,
    RequestRxReload,
    StopRxWalker,
    ConfigureOpenPromiscuousReceive,
    ForceTxCca,
    DisableTxQueue(u8),
    ReleaseTxCca,
    AcknowledgeTxEvent(u8, MacTxDetachReason),
    Fence,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ColdStartClockEdge {
    SetWifiMacReset(bool),
}

pub(super) type ColdStartClockTrace = Rc<RefCell<Vec<ColdStartClockEdge>>>;

#[derive(Default)]
pub(super) struct MockMmio {
    pub(super) operations: Vec<Operation>,
    pub(super) cold_handshake_result: Option<Result<MacColdStartOutcome, MacColdStartError>>,
    pub(super) cold_start_clock_trace: Option<ColdStartClockTrace>,
    pub(super) tx_completions: [Option<MacTxCompletionObservation>; 4],
    pub(super) tx_timeout_pending: [bool; 4],
    pub(super) tx_collision_pending: [bool; 4],
    pub(super) tx_queue_attached: [bool; 4],
    pub(super) tx_detach_fails: [bool; 4],
    pub(super) tx_control: [Option<oer_esp32s31_hal::types::MacTxControlFrame>; 4],
    pub(super) rx_last_descriptor_low: u32,
    pub(super) rx_next_descriptor_low: u32,
    pub(super) rx_walker_enabled: bool,
    pub(super) rx_reload_pending: bool,
    pub(super) rx_descriptor_base: u32,
    pub(super) ccmp_valid: [bool; 25],
}

impl MockMmio {
    pub(super) fn operations(&self) -> &[Operation] {
        &self.operations
    }

    pub(super) fn record_fence(&mut self) {
        self.operations.push(Operation::Fence);
    }

    pub(super) fn set_tx_timeout_pending(&mut self, queue: u8, pending: bool) {
        self.tx_timeout_pending[usize::from(queue)] = pending;
    }

    pub(super) fn set_tx_collision_pending(&mut self, queue: u8, pending: bool) {
        self.tx_collision_pending[usize::from(queue)] = pending;
    }

    pub(super) fn set_tx_queue_attached(&mut self, queue: u8, attached: bool) {
        self.tx_queue_attached[usize::from(queue)] = attached;
    }

    pub(super) fn set_rx_last_descriptor_address(&mut self, address: u32) {
        self.rx_last_descriptor_low = rx_descriptor_low(address);
    }

    pub(super) fn set_rx_last_descriptor_low(&mut self, address_low: u32) {
        self.rx_last_descriptor_low = address_low;
    }

    pub(super) fn set_rx_next_descriptor_address(&mut self, address: u32) {
        self.rx_next_descriptor_low = rx_descriptor_low(address);
    }

    pub(super) fn set_rx_next_descriptor_low(&mut self, address_low: u32) {
        self.rx_next_descriptor_low = address_low;
    }

    pub(super) fn set_rx_walker_enabled(&mut self, enabled: bool) {
        self.rx_walker_enabled = enabled;
    }

    pub(super) fn set_rx_reload_pending(&mut self, pending: bool) {
        self.rx_reload_pending = pending;
    }

    pub(super) fn rx_descriptor_base(&self) -> u32 {
        self.rx_descriptor_base
    }

    pub(super) fn set_tx_completion(&mut self, queue: u8, completion: MacTxCompletionObservation) {
        self.tx_completions[usize::from(queue)] = Some(completion);
    }

    pub(super) fn record_clock_edge(&self, edge: ColdStartClockEdge) {
        if let Some(trace) = &self.cold_start_clock_trace {
            trace.borrow_mut().push(edge);
        }
    }
}

impl MacResetHardware for MockMmio {
    fn set_wifi_mac_reset(&mut self, asserted: bool) {
        self.record_clock_edge(ColdStartClockEdge::SetWifiMacReset(asserted));
        self.operations.push(Operation::SetWifiMacReset(asserted));
    }
}

impl RxDma for MockMmio {
    fn last_descriptor_low(&mut self) -> u32 {
        self.operations.push(Operation::ObserveRxLastDescriptor);
        self.rx_last_descriptor_low
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.operations.push(Operation::ObserveRxNextDescriptor);
        self.rx_next_descriptor_low
    }

    fn next_descriptor(&mut self) -> oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor {
        oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor::validation(
            self.next_descriptor_low(),
            false,
        )
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        self.record_fence();
        let next = self.next_descriptor_low();
        self.record_fence();
        observed(RxDmaCursorObservation::validation(last, next))
    }

    fn walker_enabled(&mut self) -> bool {
        self.operations.push(Operation::ObserveRxWalkerEnabled);
        self.rx_walker_enabled
    }

    fn reload_pending(&mut self) -> bool {
        self.operations.push(Operation::ObserveRxReloadPending);
        self.rx_reload_pending
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaReloadSettled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        (!self.reload_pending())
            .then(|| settled(oer_esp32s31_ieee80211_mac::rx::RxDmaReloadSettled::validation()))
    }

    fn configure_descriptor_window(&mut self, _: &RxDmaBinding) {
        self.operations.push(Operation::ConfigureRxDescriptorWindow);
    }

    fn write_descriptor_base(&mut self, _: &RxDmaBinding, address: u32) {
        self.rx_descriptor_base = address;
        self.operations
            .push(Operation::PublishRxDescriptorBase(address));
    }

    fn publish_walker_enable(&mut self, _: &RxDmaBinding) {
        self.rx_walker_enabled = true;
        self.operations.push(Operation::PublishRxWalkerEnable);
    }

    fn request_reload(&mut self, _: &RxDmaBinding) {
        self.rx_reload_pending = true;
        self.operations.push(Operation::RequestRxReload);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        self.operations.push(Operation::ObserveRxWalkerEnabled);
        if self.rx_walker_enabled {
            return None;
        }
        self.rx_walker_enabled = true;
        self.operations.push(Operation::PublishRxWalkerEnable);
        self.record_fence();
        self.operations.push(Operation::ObserveRxWalkerEnabled);
        self.rx_walker_enabled
            .then(|| enabled(oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled::validation()))
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.rx_walker_enabled = false;
        self.operations.push(Operation::StopRxWalker);
        self.record_fence();
        self.operations.push(Operation::ObserveRxWalkerEnabled);
        (!self.rx_walker_enabled).then(|| stopped(RxDmaWalkerStopped::validation()))
    }

    fn fence(&mut self) {
        self.record_fence();
    }
}

pub(super) const fn rx_descriptor_low(address: u32) -> u32 {
    if address == 0 { 0 } else { address - DMA_LOW }
}

pub(super) fn confirm_completed_unit_link_release<const COUNT: usize>(
    live: &mut RxRingLive<'_, COUNT>,
    mmio: &mut MockMmio,
    descriptors: &[Descriptor; COUNT],
    descriptor_base: u32,
    last_descriptor_low: u32,
    descriptor_count: usize,
) {
    let last_descriptor_low = rx_descriptor_low(last_descriptor_low);
    assert!(
        !live.observe_completed_unit_link_release(mmio, last_descriptor_low, descriptor_count,),
        "the current LAST descriptor still owns its nonterminal link",
    );
    let tail_index = usize::try_from(
        (last_descriptor_low.wrapping_sub(rx_descriptor_low(descriptor_base))) / DESCRIPTOR_BYTES,
    )
    .unwrap();
    let later_index = (tail_index + 1) % COUNT;
    descriptors[later_index].write_word0(descriptors[later_index].word0() | BIT_30);
    let later_low = rx_descriptor_low(descriptor_base + later_index as u32 * DESCRIPTOR_BYTES);
    mmio.set_rx_last_descriptor_low(later_low);
    assert!(live.observe_completed_unit_link_release(mmio, later_low, descriptor_count,));
}

impl CcmpKeyHardware for MockMmio {
    fn configure_ccmp_association(
        &mut self,
        _interface: oer_esp32s31_hal::types::MacInterface,
        _mode: oer_esp32s31_hal::types::MacSppMode,
    ) {
    }
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        let Some(valid) = self.ccmp_valid.get_mut(usize::from(index)) else {
            return MacKeyInstallOutcome::Rejected;
        };
        if *valid {
            return MacKeyInstallOutcome::Occupied;
        }
        *valid = true;
        self.operations.push(Operation::InstallCcmp(
            MacInterface::Station,
            index,
            identity,
        ));
        MacKeyInstallOutcome::Installed
    }

    fn install_ap_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        let Some(valid) = self.ccmp_valid.get_mut(usize::from(index)) else {
            return MacKeyInstallOutcome::Rejected;
        };
        if *valid {
            return MacKeyInstallOutcome::Occupied;
        }
        *valid = true;
        self.operations.push(Operation::InstallCcmp(
            MacInterface::AccessPoint,
            index,
            identity,
        ));
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        if let Some(valid) = self.ccmp_valid.get_mut(usize::from(index)) {
            *valid = false;
            self.operations.push(Operation::ClearCcmp(index));
        }
    }

    fn ccmp_entry_is_valid(&self, index: u8) -> Option<bool> {
        self.ccmp_valid.get(usize::from(index)).copied()
    }
}

impl StaLinkRxPolicyHardware for MockMmio {
    fn apply_sta_link_policy(&mut self, bssid_address: [u8; 6]) {
        self.operations
            .push(Operation::ApplyStaLinkPolicy(bssid_address));
    }
}

impl MacInterfaceAddressHardware for MockMmio {
    fn program_interface_address(&mut self, interface: MacInterface, address: [u8; 6]) {
        self.operations
            .push(Operation::ProgramInterfaceAddress(interface, address));
    }
}

impl MacColdHandshakeHardware for MockMmio {
    fn begin_cold_handshake(
        &mut self,
        sample_limit: u32,
    ) -> Result<MacColdStartOutcome, MacColdStartError> {
        self.operations
            .push(Operation::BeginColdHandshake(sample_limit));
        self.cold_handshake_result
            .unwrap_or(Err(MacColdStartError::HandshakeTimedOut {
                samples: sample_limit,
                sample_limit,
            }))
    }
}

impl MacSnifferHardware for MockMmio {
    fn configure_open_promiscuous_receive(&mut self) {
        self.operations
            .push(Operation::ConfigureOpenPromiscuousReceive);
    }

    fn disable_open_promiscuous_receive(&mut self) {}
}

impl MacColdCryptoHardware for MockMmio {
    fn initialize_crypto_bypass(&mut self) {
        self.operations.push(Operation::InitializeCryptoBypass);
    }
}

impl MacColdAntennaHardware for MockMmio {
    fn initialize_mac_antenna(&mut self) {
        self.operations.push(Operation::InitializeMacAntenna);
    }
}

impl MacColdHalTailHardware for MockMmio {
    fn initialize_hal_tail(
        &mut self,
        event_mask: MacInterruptMask,
        slow_clock_calibration: MacSlowClockCalibration,
    ) {
        self.operations.push(Operation::InitializeHalTail(
            event_mask,
            slow_clock_calibration,
        ));
    }
}

impl MacColdCoexHardware for MockMmio {
    fn initialize_cold_coex(&mut self, pti: MacColdCoexPti) {
        self.operations.push(Operation::InitializeColdCoex(pti));
    }
}

impl MacColdHeHardware for MockMmio {
    fn initialize_he_prefix(&mut self) {
        self.operations.push(Operation::InitializeHePrefix);
    }

    fn initialize_tx_power(&mut self, table: &MacTxPowerTable) {
        self.operations.push(Operation::InitializeTxPower(*table));
    }

    fn initialize_he_suffix(&mut self) {
        self.operations.push(Operation::InitializeHeSuffix);
    }
}

impl MacColdEnableHardware for MockMmio {
    fn enable_mac_interrupts(&mut self, event_mask: MacInterruptMask) {
        self.operations
            .push(Operation::EnableMacInterrupts(event_mask));
    }
}

impl MacColdLastRxBufferHardware for MockMmio {
    fn initialize_last_rx_buffer_table(&mut self) {
        self.operations.push(Operation::InitializeLastRxBufferTable);
    }
}

impl MacColdTxRxHardware for MockMmio {
    fn initialize_txrx_prefix(&mut self) {
        self.operations.push(Operation::InitializeTxRxPrefix);
    }

    fn initialize_txrx_callbacks(&mut self, delay_slot: MacDelaySlot) {
        self.operations
            .push(Operation::InitializeTxRxCallbacks(delay_slot));
    }

    fn initialize_txrx_suffix(&mut self) {
        self.operations.push(Operation::InitializeTxRxSuffix);
    }
}

impl MacColdRxPolicyHardware for MockMmio {
    fn initialize_cold_receive_policy(&mut self) {
        self.operations.push(Operation::InitializeColdReceivePolicy);
    }
}

impl MacLowRateHardware for MockMmio {
    fn disable_phy_low_rate(&mut self) {
        self.operations.push(Operation::DisablePhyLowRate);
    }
}

impl MacColdRxBufferHardware for MockMmio {
    fn initialize_rx_buffer_prefix(&mut self) {
        self.operations.push(Operation::InitializeRxBufferPrefix);
    }
}

impl TxHardware for MockMmio {
    fn prepare_bound_legacy_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacLegacyTxProgram,
    ) -> bool {
        self.tx_control[usize::from(queue)] = Some(program.control());
        true
    }

    fn prepare_bound_ht_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        program: MacHtTxProgram,
    ) -> bool {
        self.tx_control[usize::from(queue)] = Some(program.control());
        true
    }

    fn prepare_bound_he_tx(
        &mut self,
        _dma: &dyn PreparedTxDma,
        queue: u8,
        program: oer_esp32s31_hal::types::MacHeTxProgram,
    ) -> bool {
        self.tx_control[usize::from(queue)] = Some(program.control());
        true
    }

    fn start_bound_legacy_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.record_fence();
        self.record_fence();
    }

    fn start_bound_ht_tx(&mut self, _dma: &dyn HardwareOwnedTxDma, _queue: u8) {
        self.record_fence();
        self.record_fence();
    }

    fn take_tx_completion(&mut self, queue: u8) -> Option<MacTxCompletionObservation> {
        self.tx_completions[usize::from(queue)].take()
    }

    fn begin_tx_timeout_abort(&mut self, queue: u8) -> bool {
        if !self.tx_timeout_pending[usize::from(queue)] {
            return false;
        }
        self.operations.push(Operation::ForceTxCca);
        self.record_fence();
        true
    }

    fn with_tx_queue_detached<R>(
        &mut self,
        queue: u8,
        expected_descriptor_head: u32,
        reason: MacTxDetachReason,
        detached: impl for<'detached> FnOnce(MacTxQueueDetached<'detached>) -> R,
    ) -> MacTxDetachOutcome<R> {
        let index = usize::from(queue);
        match reason {
            MacTxDetachReason::Collision => {
                if !self.tx_collision_pending[index] {
                    return MacTxDetachOutcome::NoEvent;
                }
                self.operations.push(Operation::DisableTxQueue(queue));
                self.tx_queue_attached[index] = false;
                self.record_fence();
                self.operations
                    .push(Operation::AcknowledgeTxEvent(queue, reason));
                self.tx_collision_pending[index] = false;
            }
            MacTxDetachReason::Timeout => {
                if !self.tx_timeout_pending[index] {
                    return MacTxDetachOutcome::NoEvent;
                }
                self.operations.push(Operation::DisableTxQueue(queue));
                self.tx_queue_attached[index] = false;
                self.operations.push(Operation::ReleaseTxCca);
                self.operations
                    .push(Operation::AcknowledgeTxEvent(queue, reason));
                self.tx_timeout_pending[index] = false;
            }
            MacTxDetachReason::Completed => {
                self.operations.push(Operation::DisableTxQueue(queue));
                self.tx_queue_attached[index] = false;
            }
        }
        self.record_fence();
        if self.tx_detach_fails[index] {
            self.tx_queue_attached[index] = true;
            MacTxDetachOutcome::Failed
        } else {
            MacTxDetachOutcome::Detached(detached(MacTxQueueDetached::new_model(
                expected_descriptor_head,
            )))
        }
    }

    fn take_block_ack_completion(
        &mut self,
        _queue: u8,
    ) -> Option<oer_esp32s31_hal::types::MacHtAmpduCompletionObservation> {
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PlatformOperation {
    MacDelayRandom,
    SlowClockCalibration,
    TxPower(u8),
    CoexPti(MacCoexEvent),
}

#[derive(Default)]
pub(super) struct MockPlatform {
    pub(super) operations: Vec<PlatformOperation>,
}

impl MacDelayEntropy for MockPlatform {
    fn mac_delay_random(&mut self) -> u32 {
        self.operations.push(PlatformOperation::MacDelayRandom);
        7
    }
}

impl MacSlowClockCalibrationSource for MockPlatform {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        self.operations
            .push(PlatformOperation::SlowClockCalibration);
        MacSlowClockCalibration::Unavailable
    }
}

impl MacTxPowerSource for MockPlatform {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair {
        self.operations.push(PlatformOperation::TxPower(rate));
        MacTxPowerPair {
            primary: rate as i8,
            alternate: -(rate as i8),
        }
    }
}

impl MacCoexPtiSource for MockPlatform {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti {
        self.operations.push(PlatformOperation::CoexPti(event));
        MacCoexPti::from_osi_value(match event {
            MacCoexEvent::Event1 => 5,
            MacCoexEvent::Event3 => 7,
            MacCoexEvent::Event10 => 3,
            MacCoexEvent::Event15 => 1,
        })
    }
}

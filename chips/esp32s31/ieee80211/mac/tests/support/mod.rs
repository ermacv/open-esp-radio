use crate::*;
use oer_esp32s31_hal::dma_static::Placed;
pub(super) use oer_esp32s31_hal::ieee80211::dma::rx_dma::model::{
    RxWalkerModel, RxWalkerOperation as Walker,
};
use oer_esp32s31_hal::ieee80211::dma::{
    rx_ring::RxDmaBufferAddresses,
    rx_storage::{PlacedRxDma, RxDmaStorage},
    space::HostDma,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Operation {
    RequestColdHandshake,
    SampleColdHandshake,
    FinishColdHandshake,
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
    /// Not-ready samples before the ready edge; `None` never becomes ready.
    pub(super) cold_handshake_ready_after: Option<u32>,
    pub(super) cold_handshake_samples: u32,
    pub(super) cold_start_clock_trace: Option<ColdStartClockTrace>,
    pub(super) tx_completions: [Option<MacTxCompletionObservation>; 4],
    pub(super) tx_timeout_pending: [bool; 4],
    pub(super) tx_collision_pending: [bool; 4],
    pub(super) tx_queue_attached: [bool; 4],
    pub(super) tx_detach_fails: [bool; 4],
    pub(super) tx_control: [Option<oer_esp32s31_hal::types::MacTxControlFrame>; 4],
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

/// A walker model that behaves as plain registers: a rung doorbell reads
/// pending until the test clears it ([`RxWalkerFixture::set_rx_reload_pending`])
/// and never moves NEXT, so the test plays every hardware cursor move.
pub(super) fn mock_walker() -> RxWalkerModel {
    let mut walker = RxWalkerModel::new();
    walker.reload_latency = u32::MAX;
    walker.lose_reloads = u32::MAX;
    walker
}

/// The register images a test sets on the walker model.
pub(super) trait RxWalkerFixture {
    fn set_rx_last_descriptor_address(&mut self, address: u32);
    fn set_rx_next_descriptor_address(&mut self, address: u32);
    fn set_rx_next_descriptor_low(&mut self, address_low: u32);
    fn set_rx_walker_enabled(&mut self, enabled: bool);
    fn set_rx_reload_pending(&mut self, pending: bool);
    fn rx_reload_pending(&self) -> bool;
}

impl RxWalkerFixture for RxWalkerModel {
    fn set_rx_last_descriptor_address(&mut self, address: u32) {
        self.last_descriptor_low = rx_descriptor_low(address);
    }

    fn set_rx_next_descriptor_address(&mut self, address: u32) {
        self.next_descriptor_low = rx_descriptor_low(address);
    }

    fn set_rx_next_descriptor_low(&mut self, address_low: u32) {
        self.next_descriptor_low = address_low;
    }

    fn set_rx_walker_enabled(&mut self, enabled: bool) {
        self.walker_enabled = enabled;
    }

    fn set_rx_reload_pending(&mut self, pending: bool) {
        self.reload_pending_observations = if pending { u32::MAX } else { 0 };
    }

    fn rx_reload_pending(&self) -> bool {
        self.reload_pending_observations != 0
    }
}

pub(super) const fn rx_descriptor_low(address: u32) -> u32 {
    if address == 0 {
        0
    } else {
        address - oer_esp32s31_hal::layouts::DMA_WINDOW.start
    }
}

/// Prove a completed unit's link release through the product observation:
/// LAST at the unit's tail still owns its link, and LAST at a later
/// terminal-complete descriptor releases it.
pub(super) fn confirm_completed_unit_link_release<const COUNT: usize>(
    live: &mut RxRingLive<'_, COUNT>,
    mmio: &mut RxWalkerModel,
    descriptors: &[RxDescriptor; COUNT],
    descriptor_base: u32,
    last_descriptor_address: u32,
    descriptor_count: usize,
) {
    mmio.set_rx_last_descriptor_address(last_descriptor_address);
    assert!(
        !live.observe_current_completed_unit_link_release(mmio, descriptor_count),
        "the current LAST descriptor still owns its nonterminal link",
    );
    let tail_index =
        usize::try_from((last_descriptor_address - descriptor_base) / DESCRIPTOR_BYTES).unwrap();
    let later_index = (tail_index + 1) % COUNT;
    descriptors[later_index].write_word0(descriptors[later_index].word0().with_rx_done(true));
    mmio.set_rx_last_descriptor_address(descriptor_base + later_index as u32 * DESCRIPTOR_BYTES);
    assert!(live.observe_current_completed_unit_link_release(mmio, descriptor_count));
}

/// One leaked RX DMA arena in the host address space and the layout it
/// published.
pub(super) struct HostRxArena<const COUNT: usize, const BUFFER: usize, const STORAGE: usize> {
    placed: PlacedRxDma<COUNT, BUFFER, STORAGE, HostDma>,
    pub(super) storage: &'static RxDmaStorage<COUNT, BUFFER, STORAGE, HostDma>,
    pub(super) base: u32,
    pub(super) buffers: &'static RxDmaBufferAddresses<COUNT>,
}

impl<const COUNT: usize, const BUFFER: usize, const STORAGE: usize>
    HostRxArena<COUNT, BUFFER, STORAGE>
{
    /// A fresh arena whose buffers hold [`STALE_PAYLOAD`] before its first
    /// ring, as a previous epoch would leave them.
    pub(super) fn new() -> Self {
        let mut storage = Box::new(RxDmaStorage::new());
        for index in 0..COUNT {
            storage.buffer_mut(index).unwrap().fill(STALE_PAYLOAD);
        }
        let placed = RxDmaStorage::place(Placed::host(Box::leak(storage))).unwrap();
        let storage = placed.storage();
        Self {
            placed,
            storage,
            base: placed.descriptor_base(),
            buffers: storage.buffer_addresses(),
        }
    }

    pub(super) fn descriptors(&self) -> &'static [RxDescriptor; COUNT] {
        self.storage.descriptors()
    }

    pub(super) fn address(&self, index: usize) -> u32 {
        self.base + index as u32 * DESCRIPTOR_BYTES
    }

    pub(super) fn prepare(
        &self,
        mmio: &mut RxWalkerModel,
    ) -> Result<RxRingStopped<'static, COUNT>, RxRingError> {
        self.placed.prepare_ring(mmio)
    }

    pub(super) fn start(&self, mmio: &mut RxWalkerModel) -> RxRingLive<'static, COUNT> {
        self.prepare(mmio)
            .unwrap()
            .try_start(mmio)
            .map_err(|(_, error)| error)
            .unwrap()
    }

    /// Model the DMA engine writing `bytes` at the start of the buffer it
    /// published for descriptor `index`.
    pub(super) fn dma_write(&self, live: &RxRingLive<'_, COUNT>, index: usize, bytes: &[u8]) {
        self.storage
            .dma_write(live, self.buffers[index], bytes)
            .unwrap();
    }

    /// The leading and trailing recycle guards of descriptor `index`'s
    /// buffer, as the DMA engine sees them.
    pub(super) fn dma_guards(
        &self,
        live: &RxRingLive<'_, COUNT>,
        index: usize,
    ) -> ([u8; 4], [u8; 4]) {
        let mut leading = [0; 4];
        let mut trailing = [0; 4];
        let address = self.buffers[index];
        self.storage.dma_read(live, address, &mut leading).unwrap();
        self.storage
            .dma_read(live, address + BUFFER as u32, &mut trailing)
            .unwrap();
        (leading, trailing)
    }

    /// Whether descriptor `index`'s buffer carries both recycle guards.
    pub(super) fn guards_restored(&self, live: &RxRingLive<'_, COUNT>, index: usize) -> bool {
        let sentinel = RX_BUFFER_SENTINEL.to_le_bytes();
        self.dma_guards(live, index) == (sentinel, sentinel)
    }
}

/// Bytes a previous epoch left in a host arena's buffers.
pub(super) const STALE_PAYLOAD: u8 = 0x5a;

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
    fn request_cold_handshake(&mut self) {
        self.operations.push(Operation::RequestColdHandshake);
    }

    fn sample_cold_handshake_ready(&mut self) -> bool {
        self.operations.push(Operation::SampleColdHandshake);
        let sample = self.cold_handshake_samples;
        self.cold_handshake_samples += 1;
        self.cold_handshake_ready_after
            .is_some_and(|ready_after| sample >= ready_after)
    }

    fn finish_cold_handshake(&mut self) {
        self.operations.push(Operation::FinishColdHandshake);
    }
}

/// Drive `future` on `clock`, advancing it to the next deadline whenever the
/// future waits.
pub(super) fn run<F: core::future::Future>(
    clock: &oer_time_virtual::VirtualClock,
    future: F,
) -> F::Output {
    let mut future = core::pin::pin!(future);
    loop {
        if let core::task::Poll::Ready(output) = future.as_mut().poll(
            &mut core::task::Context::from_waker(core::task::Waker::noop()),
        ) {
            return output;
        }
        clock
            .advance_to_next_deadline()
            .expect("a pending future waits on the clock");
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

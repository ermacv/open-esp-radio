use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll},
};

use crate::{
    datapath::rx::{dma::StagedRxProducer, staging::StagedRxQueue},
    roles::monitor::rx::MonitorRx,
};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_esp32s31_ieee80211_mac::rx::{
    RxDmaBinding, RxDmaWalkerStopped, RxRingStopped, pool::RxStagePool,
};

use oer_ieee80211_runtime::{MonitorCapturePool, MonitorCaptureResources};

use oer_ieee80211_softmac::{
    MonitorDropReason, MonitorFrame, MonitorPublishOutcome, MonitorSink, WifiConfig,
    WifiMonitorConfig,
    interface::{ChannelContextId, MonitorTapPoint},
};

use std::boxed::Box;

use super::*;

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(core::task::Waker::noop());
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

const RX_TEST_COUNT: usize = 2;
const RX_TEST_BUFFER_SIZE: usize = 128;
const RX_TEST_STORAGE_SIZE: usize = RX_TEST_BUFFER_SIZE + 4;
const RX_TEST_BASE: u32 = 0x2f00_1000;
const RX_TEST_BUFFERS: [u32; RX_TEST_COUNT] = [0x2f00_2000, 0x2f00_2080];

#[derive(Default)]
struct MockRxDma {
    walker: bool,
    fail_enable: bool,
    fail_disable: bool,
    descriptor_base: u32,
    reload_requests: u32,
}

impl RxDma for MockRxDma {
    fn last_descriptor_low(&mut self) -> u32 {
        if self.walker {
            RX_TEST_BASE & 0x000f_ffff
        } else {
            0
        }
    }

    fn next_descriptor_low(&mut self) -> u32 {
        if self.walker {
            (RX_TEST_BASE + 12) & 0x000f_ffff
        } else {
            0
        }
    }

    fn next_descriptor(&mut self) -> oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor {
        oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor::validation(
            self.next_descriptor_low(),
            false,
        )
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaCursorObservation<'confirmation>,
        ) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        self.fence();
        let next = self.next_descriptor_low();
        self.fence();
        observed(oer_esp32s31_ieee80211_mac::rx::RxDmaCursorObservation::validation(last, next))
    }

    fn walker_enabled(&mut self) -> bool {
        self.walker
    }

    fn reload_pending(&mut self) -> bool {
        false
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

    fn configure_descriptor_window(&mut self, _: &RxDmaBinding) {}

    fn write_descriptor_base(&mut self, _: &RxDmaBinding, address: u32) {
        self.descriptor_base = address;
    }

    fn publish_walker_enable(&mut self, _: &RxDmaBinding) {
        self.walker = true;
    }

    fn request_reload(&mut self, _: &RxDmaBinding) {
        self.reload_requests = self.reload_requests.saturating_add(1);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        if self.fail_enable {
            None
        } else {
            self.walker = true;
            Some(enabled(
                oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled::validation(),
            ))
        }
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        if self.fail_disable {
            None
        } else {
            self.walker = false;
            Some(stopped(RxDmaWalkerStopped::validation()))
        }
    }

    fn fence(&mut self) {}
}

#[derive(Default)]
struct FrameObserver {
    frames: u32,
}

impl ScanFrameObserver for FrameObserver {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {
        self.frames = self.frames.saturating_add(1);
    }
}

#[derive(Default)]
struct MonitorObserver {
    frames: u32,
    first_byte: Option<u8>,
    drop_all: bool,
}

impl MonitorSink<oer_esp32s31_ieee80211_mac::rx::RxPhyInfo> for MonitorObserver {
    fn try_publish(
        &mut self,
        frame: MonitorFrame<'_, oer_esp32s31_ieee80211_mac::rx::RxPhyInfo>,
    ) -> MonitorPublishOutcome {
        assert_eq!(frame.tap, MonitorTapPoint::Normalized);
        assert_eq!(frame.channel_context, ChannelContextId::PRIMARY);
        self.frames = self.frames.saturating_add(1);
        self.first_byte = frame.bytes.first().copied();
        if self.drop_all {
            MonitorPublishOutcome::Dropped(MonitorDropReason::Full)
        } else {
            MonitorPublishOutcome::Published
        }
    }
}

fn write_test_beacon(
    storage: &mut ReceiveDmaStorage<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>,
) {
    const FRAME_LENGTH: usize = 43;
    const SIGNAL_LENGTH: usize = FRAME_LENGTH + 4;
    const FRAME_OFFSET: usize = 0x40;

    let mut bytes = [0_u8; RX_TEST_BUFFER_SIZE];
    bytes[0] = (-42_i8) as u8;
    bytes[0x38..0x3c].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    let frame = &mut bytes[FRAME_OFFSET..FRAME_OFFSET + FRAME_LENGTH];
    frame[0] = 0x80;
    frame[10..16].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    frame[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[36..40].copy_from_slice(&[0, 2, b'a', b'p']);
    frame[40..43].copy_from_slice(&[3, 1, 6]);
    storage
        .buffer_mut(0)
        .expect("test RX buffer exists")
        .copy_from_slice(&bytes);
}

fn complete_test_beacon(
    storage: &ReceiveDmaStorage<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>,
) {
    use oer_esp32s31_ieee80211_dma::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};

    const FRAME_LENGTH: usize = 43;
    const SIGNAL_LENGTH: usize = FRAME_LENGTH + 4;
    const FRAME_OFFSET: usize = 0x40;
    const RECEIVED_LENGTH: usize = FRAME_OFFSET + SIGNAL_LENGTH;

    storage.descriptors()[0].write_word0(
        RX_TEST_BUFFER_SIZE as u32 | (RECEIVED_LENGTH as u32) << LENGTH_SHIFT | BIT_30 | BIT_31,
    );
}

fn monitor_plan() -> oer_ieee80211_softmac::WifiStandaloneMonitorPlan {
    WifiConfig::monitor(WifiMonitorConfig::normalized())
        .validate(oer_esp32s31_ieee80211_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES)
        .unwrap()
        .standalone_monitor()
        .unwrap()
}

fn finish_monitor_with_live_handoff(
    monitor: MonitorRx<'_, RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>,
    hardware: &mut MockRxDma,
) {
    let live = monitor
        .into_live()
        .unwrap_or_else(|_| panic!("monitor role must return a live physical ring"));
    assert!(hardware.walker, "logical monitor stop keeps RX DMA live");
    match live.try_stop(hardware) {
        Ok(_halted) => {}
        Err(_) => panic!("test cleanup must stop the extracted physical ring"),
    }
}

#[test]
fn scan_rx_hands_the_exact_live_ring_to_the_next_role() {
    let mut storage =
        ReceiveDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut rx =
        ScanRx::prepare_initial(&mut hardware, &storage, RX_TEST_BASE, &RX_TEST_BUFFERS).unwrap();
    assert_eq!(rx.phase(), RxFrontierPhase::Prepared);

    rx.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);
    let mut table = ScanTable::<4>::new();
    let mut frame = [0_u8; 64];
    let mut observer = FrameObserver::default();
    let monotonic = oer_time_virtual::SkipClock::new();
    let mut context =
        ScanObservationContext::new(
            6,
            crate::mac_clock::MacClockStorage::<
                embassy_sync::blocking_mutex::raw::NoopRawMutex,
                _,
                _,
            >::new(&monotonic)
            .start(crate::mac_clock::FixedCounter(0))
            .snapshot(),
            &mut frame,
            &mut table,
            &mut observer,
        );
    let progress = rx.observe_management(&mut hardware, &mut context).unwrap();
    let release = rx.observe_management(&mut hardware, &mut context).unwrap();

    assert_eq!(progress.completed_descriptors, 1);
    assert_eq!(progress.parsed_management_frames, 1);
    assert_eq!(progress.inserted_records, 1);
    assert_eq!(progress.recycled_descriptors, 0);
    assert!(!progress.service_probe_pending);
    assert_eq!(release.completed_descriptors, 0);
    assert_eq!(release.recycled_descriptors, 0);
    assert_eq!(observer.frames, 1);
    assert_eq!(table.records()[0].ssid_bytes(), b"ap");
    assert_eq!(table.records()[0].channel, 6);

    rx.park().unwrap();
    let live = match rx.into_live() {
        Ok(live) => live,
        Err(_) => panic!("completed scan must expose its live owner"),
    };
    assert!(hardware.walker);
    assert_eq!(live.descriptor_base(), RX_TEST_BASE);
    assert_eq!(live.buffer_addresses(), &RX_TEST_BUFFERS);
    let _halted = live
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test cleanup must stop the physical ring"));
}

#[test]
fn normalized_monitor_borrows_the_mpdu_and_retains_current_last_until_stop() {
    let mut storage =
        ReceiveDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut monitor = MonitorRx::prepare_initial(
        monitor_plan(),
        &mut hardware,
        &storage,
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
    )
    .unwrap();
    monitor.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);

    let mut sink = MonitorObserver::default();
    let progress = monitor.service(&mut hardware, &mut sink).unwrap();
    let release = monitor.service(&mut hardware, &mut sink).unwrap();
    assert_eq!(progress.completed_descriptors, 1);
    assert_eq!(progress.published_frames, 1);
    assert_eq!(progress.dropped_frames, 0);
    assert_eq!(progress.recycled_descriptors, 0);
    assert!(!progress.service_probe_pending);
    assert_eq!(release.completed_descriptors, 0);
    assert_eq!(release.published_frames, 0);
    assert_eq!(release.recycled_descriptors, 0);
    assert_eq!(sink.frames, 1);
    assert_eq!(sink.first_byte, Some(0x80));
    finish_monitor_with_live_handoff(monitor, &mut hardware);
}

#[test]
fn live_scan_ring_is_rebound_to_monitor_without_restarting_the_walker() {
    let mut storage =
        ReceiveDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut scan =
        ScanRx::prepare_initial(&mut hardware, &storage, RX_TEST_BASE, &RX_TEST_BUFFERS).unwrap();
    scan.start(&mut hardware).unwrap();
    scan.park().unwrap();
    let live = scan
        .into_live()
        .unwrap_or_else(|_| panic!("parked scan must return its live ring"));

    let mut monitor = MonitorRx::from_live(monitor_plan(), live, &storage)
        .unwrap_or_else(|_| panic!("monitor must accept the exact live scan ring"));
    assert_eq!(monitor.phase(), RxFrontierPhase::Live);
    monitor.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);

    let mut sink = MonitorObserver::default();
    let progress = monitor.service(&mut hardware, &mut sink).unwrap();
    assert_eq!(progress.published_frames, 1);
    assert_eq!(sink.frames, 1);
    finish_monitor_with_live_handoff(monitor, &mut hardware);
}

#[test]
fn full_monitor_sink_drops_observation_without_backpressuring_the_ring() {
    let mut storage =
        ReceiveDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut monitor = MonitorRx::prepare_initial(
        monitor_plan(),
        &mut hardware,
        &storage,
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
    )
    .unwrap();
    monitor.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);

    let mut sink = MonitorObserver {
        drop_all: true,
        ..MonitorObserver::default()
    };
    let progress = monitor.service(&mut hardware, &mut sink).unwrap();
    let release = monitor.service(&mut hardware, &mut sink).unwrap();
    assert_eq!(progress.published_frames, 0);
    assert_eq!(progress.dropped_frames, 1);
    assert_eq!(progress.full_drops, 1);
    assert_eq!(progress.oversized_drops, 0);
    assert_eq!(progress.filtered_drops, 0);
    assert_eq!(progress.recycled_descriptors, 0);
    assert!(!progress.service_probe_pending);
    assert_eq!(release.completed_descriptors, 0);
    assert_eq!(release.dropped_frames, 0);
    assert_eq!(release.recycled_descriptors, 0);
    assert_eq!(hardware.reload_requests, 0);
    finish_monitor_with_live_handoff(monitor, &mut hardware);
}

#[test]
fn monitor_capture_owns_its_copy_while_current_last_remains_dma_visible() {
    let mut storage =
        ReceiveDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut monitor = MonitorRx::prepare_initial(
        monitor_plan(),
        &mut hardware,
        &storage,
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
    )
    .unwrap();
    let pool = MonitorCapturePool::<64, 2>::new();
    let resources = MonitorCaptureResources::<
        NoopRawMutex,
        oer_esp32s31_ieee80211_mac::rx::RxPhyInfo,
        2,
        64,
        2,
    >::new(&pool);
    let (mut sink, receiver) = resources.split();

    monitor.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);
    let progress = monitor.service(&mut hardware, &mut sink).unwrap();
    let release = monitor.service(&mut hardware, &mut sink).unwrap();

    assert_eq!(progress.published_frames, 1);
    assert_eq!(progress.recycled_descriptors, 0);
    assert!(!progress.service_probe_pending);
    assert_eq!(release.completed_descriptors, 0);
    assert_eq!(release.published_frames, 0);
    assert_eq!(release.recycled_descriptors, 0);
    assert_eq!(hardware.reload_requests, 0);
    assert_eq!(pool.claimed_slots(), 1);
    let captured = receiver.try_receive().expect("async capture owns its copy");
    assert_eq!(captured.bytes().first(), Some(&0x80));
    assert_eq!(
        captured.metadata().channel_context,
        ChannelContextId::PRIMARY
    );
    drop(captured);
    assert_eq!(pool.claimed_slots(), 0);
    finish_monitor_with_live_handoff(monitor, &mut hardware);
}

#[test]
fn monitor_only_wifi_plan_materializes_the_promiscuous_ring_owner() {
    let mut storage =
        ReceiveDmaStorage::<RX_TEST_COUNT, RX_TEST_BUFFER_SIZE, RX_TEST_STORAGE_SIZE>::new();
    write_test_beacon(&mut storage);
    let mut hardware = MockRxDma::default();
    let mut monitor = MonitorRx::prepare_initial(
        monitor_plan(),
        &mut hardware,
        &storage,
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
    )
    .unwrap();
    assert_eq!(monitor.channel_context(), ChannelContextId::PRIMARY);
    monitor.start(&mut hardware).unwrap();
    complete_test_beacon(&storage);
    let mut sink = MonitorObserver::default();
    let progress = monitor.service(&mut hardware, &mut sink).unwrap();
    assert_eq!(progress.published_frames, 1);
    finish_monitor_with_live_handoff(monitor, &mut hardware);
}

#[test]
fn complete_scan_can_start_another_channel_without_rebuilding_the_ring() {
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<
        RX_TEST_COUNT,
        RX_TEST_BUFFER_SIZE,
        RX_TEST_STORAGE_SIZE,
    >::new()));
    let mut hardware = MockRxDma::default();
    let mut rx =
        ScanRx::prepare_initial(&mut hardware, storage, RX_TEST_BASE, &RX_TEST_BUFFERS).unwrap();

    rx.prepare_initial_or_retry(&mut hardware).unwrap();
    assert_eq!(rx.phase(), RxFrontierPhase::Prepared);
    rx.start(&mut hardware).unwrap();
    rx.park().unwrap();
    assert_eq!(rx.phase(), RxFrontierPhase::Live);

    rx.prepare_initial_or_retry(&mut hardware).unwrap();
    assert_eq!(rx.phase(), RxFrontierPhase::Live);
    rx.start(&mut hardware).unwrap();
    assert_eq!(rx.phase(), RxFrontierPhase::Live);
}

#[test]
fn scan_rx_retains_its_typed_phase_across_enable_failure_and_logical_park() {
    let storage = Box::leak(Box::new(ReceiveDmaStorage::<
        RX_TEST_COUNT,
        RX_TEST_BUFFER_SIZE,
        RX_TEST_STORAGE_SIZE,
    >::new()));
    let mut hardware = MockRxDma::default();
    let mut rx =
        ScanRx::prepare_initial(&mut hardware, storage, RX_TEST_BASE, &RX_TEST_BUFFERS).unwrap();

    hardware.fail_enable = true;
    assert_eq!(
        rx.start(&mut hardware),
        Err(RxFrontierError::Ring(RxRingError::Busy))
    );
    assert_eq!(rx.phase(), RxFrontierPhase::Prepared);

    hardware.fail_enable = false;
    rx.start(&mut hardware).unwrap();
    hardware.fail_disable = true;
    rx.park().unwrap();
    assert_eq!(rx.phase(), RxFrontierPhase::Live);
    assert!(hardware.walker, "logical park does not touch walker enable");
}

#[test]
fn running_scan_rx_returns_the_exact_connected_epoch_resources() {
    const STAGE_SLOTS: usize = 1;
    const STAGE_CAPACITY: usize = 64;
    type TestDelay = oer_time_virtual::SkipClock;

    let storage = Box::leak(Box::new(ReceiveDmaStorage::<
        RX_TEST_COUNT,
        RX_TEST_BUFFER_SIZE,
        RX_TEST_STORAGE_SIZE,
    >::new()));
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
        RX_TEST_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    let pool = RxStagePool::<STAGE_SLOTS, STAGE_CAPACITY>::new();
    let queue = StagedRxQueue::<NoopRawMutex, STAGE_SLOTS, STAGE_CAPACITY, STAGE_SLOTS>::new();
    let (sender, _receiver) = queue.split();
    let connected = StagedRxProducer::new(ring, storage, &pool, TestDelay::new(), sender);
    let storage_address = connected.storage() as *const _;

    let mut running = RunningScanRx::from_parked(connected)
        .unwrap_or_else(|_| panic!("running scan must accept the live connected producer"));
    assert_eq!(running.phase(), RxFrontierPhase::Live);
    running.prepare_initial(&mut hardware).unwrap();
    assert_eq!(running.phase(), RxFrontierPhase::Live);

    let parked = running
        .into_parked()
        .unwrap_or_else(|_| panic!("running scan must return the live connected producer"));
    assert_eq!(parked.storage() as *const _, storage_address);
    assert_eq!(parked.ring().descriptor_base(), RX_TEST_BASE);

    let mut running = RunningScanRx::from_parked(parked)
        .unwrap_or_else(|_| panic!("running scan must rebind the live connected producer"));
    running.prepare_initial(&mut hardware).unwrap();
    block_on(running.start(&mut hardware)).unwrap();
    running.park().unwrap();

    let parked = running
        .into_parked()
        .unwrap_or_else(|_| panic!("parked running scan must restore connected resources"));
    assert_eq!(parked.storage() as *const _, storage_address);
    assert_eq!(parked.ring().descriptor_base(), RX_TEST_BASE);
    assert_eq!(
        parked.storage().buffers().as_ptr(),
        storage.buffers().as_ptr()
    );
    assert_eq!(parked.queued_frames(), 0);
}

#[test]
fn running_scan_rx_restarts_the_exact_halted_connected_epoch_resources() {
    const STAGE_SLOTS: usize = 1;
    const STAGE_CAPACITY: usize = 64;
    type TestDelay = oer_time_virtual::SkipClock;

    let storage = Box::leak(Box::new(ReceiveDmaStorage::<
        RX_TEST_COUNT,
        RX_TEST_BUFFER_SIZE,
        RX_TEST_STORAGE_SIZE,
    >::new()));
    let mut hardware = MockRxDma::default();
    let stopped = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        RX_TEST_BASE,
        &RX_TEST_BUFFERS,
        RX_TEST_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let ring = stopped
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    let pool = RxStagePool::<STAGE_SLOTS, STAGE_CAPACITY>::new();
    let queue = StagedRxQueue::<NoopRawMutex, STAGE_SLOTS, STAGE_CAPACITY, STAGE_SLOTS>::new();
    let (sender, _receiver) = queue.split();
    let connected = StagedRxProducer::new(ring, storage, &pool, TestDelay::new(), sender);
    let stopped = connected
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("connected RX ring must stop"));
    assert!(!hardware.walker);

    let mut running = RunningScanRx::from_stopped(stopped);
    assert_eq!(running.phase(), RxFrontierPhase::Halted);
    running.prepare_initial(&mut hardware).unwrap();
    assert_eq!(running.phase(), RxFrontierPhase::Prepared);
    block_on(running.start(&mut hardware)).unwrap();
    assert_eq!(running.phase(), RxFrontierPhase::Live);

    let parked = running
        .into_parked()
        .unwrap_or_else(|_| panic!("restarted scan must return the connected producer"));
    assert_eq!(parked.ring().descriptor_base(), RX_TEST_BASE);
    assert_eq!(
        parked.storage().buffers().as_ptr(),
        storage.buffers().as_ptr()
    );
}

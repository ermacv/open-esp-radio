use core::{
    cell::Cell,
    future::{Future, pending, poll_fn, ready},
    pin::pin,
    task::{Context, Poll, Waker},
};

use crate::{
    datapath::{
        irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, InterruptEpoch},
        rx::dma::ReceiveDmaStorage,
    },
    roles::monitor::rx::MonitorRx,
};

use embassy_futures::block_on;

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_esp32s31_ieee80211_dma::{
    descriptor::{BIT_30, BIT_31, LENGTH_SHIFT},
    rx_ring::RxDmaArenaState,
};

use oer_esp32s31_ieee80211_mac::{
    irq::{EVENT_RX_SUCCESS, MacInterruptRoute},
    rx::{RxDma, RxDmaBinding, RxDmaWalkerStopped, RxPhyInfo},
};

use oer_time_virtual::SkipClock;

use oer_ieee80211_softmac::{
    MonitorDropReason, MonitorFilter, MonitorFrame, MonitorFrameTypeMask, MonitorPublishOutcome,
    MonitorSink, WifiConfig, WifiMonitorConfig,
};

use std::rc::Rc;

use super::*;

const RX_COUNT: usize = 2;
const RX_BUFFER_SIZE: usize = 128;
const RX_STORAGE_SIZE: usize = RX_BUFFER_SIZE + 4;
const RX_BASE: u32 = 0x2f00_1000;
const RX_BUFFERS: [u32; RX_COUNT] = [0x2f00_2000, 0x2f00_2080];

fn restore_test_rx_delivery() {}

#[derive(Default)]
struct Hardware {
    walker: bool,
    promiscuous: bool,
    reloads: u32,
    base_writes: u32,
    disable_busy_count: u8,
    walker_when_dropped: Option<Rc<Cell<Option<bool>>>>,
}

impl MacSnifferHardware for Hardware {
    fn configure_open_promiscuous_receive(&mut self) {
        self.promiscuous = true;
    }

    fn disable_open_promiscuous_receive(&mut self) {
        self.promiscuous = false;
    }
}

impl MacRuntimeStopHardware for Hardware {
    fn request_mac_runtime_stop(&mut self) {}

    fn mac_runtime_active_state(&mut self) -> u8 {
        0
    }

    fn resume_mac_runtime(&mut self) {}
}

impl Drop for Hardware {
    fn drop(&mut self) {
        if let Some(observation) = &self.walker_when_dropped {
            observation.set(Some(self.walker));
        }
    }
}

impl RxDma for Hardware {
    fn last_descriptor_low(&mut self) -> u32 {
        if self.walker {
            RX_BASE & 0x000f_ffff
        } else {
            0
        }
    }

    fn next_descriptor_low(&mut self) -> u32 {
        if self.walker {
            // The fixture services only finite two-descriptor epochs. A
            // service wake is published after the modeled walker consumed
            // the zero-successor tail, so NEXT=0 is the reclaim handshake.
            0
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

    fn write_descriptor_base(&mut self, _: &RxDmaBinding, _: u32) {
        self.base_writes = self.base_writes.saturating_add(1);
    }

    fn publish_walker_enable(&mut self, _: &RxDmaBinding) {
        self.walker = true;
    }

    fn request_reload(&mut self, _: &RxDmaBinding) {
        self.reloads = self.reloads.saturating_add(1);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        self.walker = true;
        Some(enabled(
            oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled::validation(),
        ))
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        if self.disable_busy_count != 0 {
            self.disable_busy_count -= 1;
            return None;
        }
        self.walker = false;
        Some(stopped(RxDmaWalkerStopped::validation()))
    }

    fn fence(&mut self) {}
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RouteError {
    Activate,
    Quiesce,
}

struct Route<'state> {
    active: &'state Cell<bool>,
    event_mask: &'state Cell<u32>,
    fail_activate: bool,
    fail_quiesce: bool,
    fail_quiesce_permanently: bool,
}

struct RuntimeFixture {
    platform: (),
    irq: EmbassyMacIrqRuntime<NoopRawMutex>,
    power: EmbassyPowerIrqRuntime<NoopRawMutex>,
    active: Cell<bool>,
    event_mask: Cell<u32>,
    walker_when_dropped: Rc<Cell<Option<bool>>>,
}

impl RuntimeFixture {
    fn new() -> Self {
        Self {
            platform: (),
            irq: EmbassyMacIrqRuntime::new_with_rx_moderation(restore_test_rx_delivery),
            power: EmbassyPowerIrqRuntime::new(),
            active: Cell::new(false),
            event_mask: Cell::new(0),
            walker_when_dropped: Rc::new(Cell::new(None)),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct RouteBehavior {
    fail_activate: bool,
    fail_quiesce: bool,
    fail_quiesce_permanently: bool,
}

impl MacInterruptRoute for Route<'_> {
    type Platform = ();
    type Setup = u8;
    type Error = RouteError;

    fn activate(
        &mut self,
        _: &Self::Platform,
        setup: Self::Setup,
        event_mask: oer_esp32s31_hal::types::MacInterruptMask,
    ) -> Result<(), (Self::Error, Self::Setup)> {
        if self.fail_activate {
            return Err((RouteError::Activate, setup));
        }
        self.active.set(true);
        self.event_mask.set(event_mask.bits());
        Ok(())
    }

    fn quiesce(&mut self, _: &Self::Platform) -> Result<Self::Setup, Self::Error> {
        if self.fail_quiesce_permanently {
            return Err(RouteError::Quiesce);
        }
        if self.fail_quiesce {
            self.fail_quiesce = false;
            return Err(RouteError::Quiesce);
        }
        self.active.set(false);
        Ok(7)
    }
}

#[derive(Default)]
struct Sink {
    frames: u32,
    full_after: Option<u32>,
}

impl MonitorSink<RxPhyInfo> for Sink {
    fn try_publish(&mut self, frame: MonitorFrame<'_, RxPhyInfo>) -> MonitorPublishOutcome {
        if self.full_after.is_some_and(|limit| self.frames >= limit) {
            return MonitorPublishOutcome::Dropped(MonitorDropReason::Full);
        }
        assert_eq!(frame.bytes.first(), Some(&0x80));
        self.frames = self.frames.saturating_add(1);
        MonitorPublishOutcome::Published
    }
}

fn monitor_plan() -> oer_ieee80211_softmac::WifiStandaloneMonitorPlan {
    monitor_plan_with(WifiMonitorConfig::normalized())
}

fn monitor_plan_with(
    monitor: WifiMonitorConfig,
) -> oer_ieee80211_softmac::WifiStandaloneMonitorPlan {
    WifiConfig::monitor(monitor)
        .validate(oer_esp32s31_ieee80211_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES)
        .unwrap()
        .standalone_monitor()
        .unwrap()
}

fn write_beacon(
    storage: &mut ReceiveDmaStorage<RX_COUNT, RX_BUFFER_SIZE, RX_STORAGE_SIZE>,
    index: usize,
) {
    const FRAME_LENGTH: usize = 43;
    const SIGNAL_LENGTH: usize = FRAME_LENGTH + 4;
    const FRAME_OFFSET: usize = 0x40;
    let mut bytes = [0_u8; RX_BUFFER_SIZE];
    bytes[0] = (-42_i8) as u8;
    bytes[0x38..0x3c].copy_from_slice(
        &(((SIGNAL_LENGTH + 4) as u32) << 16 | SIGNAL_LENGTH as u32).to_le_bytes(),
    );
    bytes[FRAME_OFFSET] = 0x80;
    storage
        .buffer_mut(index)
        .expect("test RX buffer exists")
        .copy_from_slice(&bytes);
}

fn complete_beacon(
    storage: &ReceiveDmaStorage<RX_COUNT, RX_BUFFER_SIZE, RX_STORAGE_SIZE>,
    index: usize,
) {
    const FRAME_LENGTH: usize = 43;
    const SIGNAL_LENGTH: usize = FRAME_LENGTH + 4;
    const FRAME_OFFSET: usize = 0x40;
    const RECEIVED_LENGTH: usize = FRAME_OFFSET + SIGNAL_LENGTH;
    storage.descriptors()[index].write_word0(
        RX_BUFFER_SIZE as u32 | (RECEIVED_LENGTH as u32) << LENGTH_SHIFT | BIT_30 | BIT_31,
    );
}

fn service<'storage, 'runtime>(
    storage: &'storage ReceiveDmaStorage<RX_COUNT, RX_BUFFER_SIZE, RX_STORAGE_SIZE>,
    runtime: &'runtime RuntimeFixture,
    sink: Sink,
    behavior: RouteBehavior,
) -> MonitorService<
    'storage,
    'runtime,
    Hardware,
    Route<'runtime>,
    NoopRawMutex,
    Sink,
    RX_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
> {
    service_with_plan(storage, runtime, sink, behavior, monitor_plan())
}

fn service_with_plan<'storage, 'runtime>(
    storage: &'storage ReceiveDmaStorage<RX_COUNT, RX_BUFFER_SIZE, RX_STORAGE_SIZE>,
    runtime: &'runtime RuntimeFixture,
    sink: Sink,
    behavior: RouteBehavior,
    plan: oer_ieee80211_softmac::WifiStandaloneMonitorPlan,
) -> MonitorService<
    'storage,
    'runtime,
    Hardware,
    Route<'runtime>,
    NoopRawMutex,
    Sink,
    RX_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
> {
    let mut hardware = Hardware {
        walker_when_dropped: Some(runtime.walker_when_dropped.clone()),
        ..Hardware::default()
    };
    let receive =
        MonitorRx::prepare_initial(plan, &mut hardware, storage, RX_BASE, &RX_BUFFERS).unwrap();
    let interrupts = InterruptEpoch::new(
        Route {
            active: &runtime.active,
            event_mask: &runtime.event_mask,
            fail_activate: behavior.fail_activate,
            fail_quiesce: behavior.fail_quiesce,
            fail_quiesce_permanently: behavior.fail_quiesce_permanently,
        },
        7,
        &runtime.irq,
        &runtime.power,
    );
    MonitorService::new(hardware, receive, sink, interrupts, ())
}

#[test]
fn one_irq_epoch_services_durable_rx_then_returns_every_owner() {
    let mut storage = ReceiveDmaStorage::new();
    write_beacon(&mut storage, 0);
    let runtime = RuntimeFixture::new();
    let mut owner = service(
        &storage,
        &runtime,
        Sink::default(),
        RouteBehavior::default(),
    );
    let stop_polls = Cell::new(0_u8);
    let stop = poll_fn(|context| {
        let poll = stop_polls.get();
        if poll == 0 {
            assert!(runtime.irq.is_rx_moderation_active());
            stop_polls.set(1);
            complete_beacon(&storage, 0);
            runtime.irq.publish(EVENT_RX_SUCCESS);
            context.waker().wake_by_ref();
            Poll::Pending
        } else if poll == 1 {
            // Let the monitor's self-posted ownership-confirmation turn run
            // before asking the finite role to stop.
            stop_polls.set(2);
            context.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    });

    let report = block_on(owner.run_until_stopped(stop, &SkipClock::new()))
        .unwrap_or_else(|_| panic!("monitor run must stop cleanly"));
    assert_eq!(report.rx_service_wakes, 1);
    assert_eq!(report.rx_interrupt_posts, 1);
    assert_eq!(report.receive.published_frames, 1);
    assert_eq!(report.receive.recycled_descriptors, 0);
    assert_eq!(
        runtime.event_mask.get(),
        ESP32S31_STANDALONE_MONITOR_INTERRUPT_MASK.bits()
    );
    assert!(runtime.active.get());
    assert!(runtime.irq.is_rx_moderation_active());
    assert_eq!(owner.receive_phase(), RxFrontierPhase::Live);
    assert!(owner.interrupt_active());
    let (mut hardware, receive, sink, _, _) = match owner.try_into_parts() {
        Ok(parts) => parts,
        Err(_) => panic!("stopped monitor owners must be extractable"),
    };
    assert!(hardware.walker);
    assert!(!hardware.promiscuous);
    assert_eq!(hardware.reloads, 0);
    assert_eq!(sink.frames, 1);
    let live = receive
        .into_live()
        .unwrap_or_else(|_| panic!("stopped monitor must return the live physical ring"));
    match live.try_stop(&mut hardware) {
        Ok(_halted) => {}
        Err(_) => panic!("test cleanup must stop the extracted physical ring"),
    }
}

#[test]
fn saturation_is_counted_without_preventing_dma_recycle() {
    let mut storage = ReceiveDmaStorage::new();
    write_beacon(&mut storage, 0);
    write_beacon(&mut storage, 1);
    let runtime = RuntimeFixture::new();
    let mut owner = service(
        &storage,
        &runtime,
        Sink {
            frames: 0,
            full_after: Some(1),
        },
        RouteBehavior::default(),
    );
    let first_poll = Cell::new(true);
    let stop = poll_fn(|context| {
        if first_poll.replace(false) {
            complete_beacon(&storage, 0);
            complete_beacon(&storage, 1);
            runtime.irq.publish(EVENT_RX_SUCCESS);
            context.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    });

    let report = block_on(owner.run_until_stopped(stop, &SkipClock::new()))
        .unwrap_or_else(|_| panic!("monitor saturation must still stop cleanly"));
    assert_eq!(report.receive.completed_descriptors, 2);
    assert_eq!(report.receive.published_frames, 1);
    assert_eq!(report.receive.full_drops, 1);
    assert_eq!(report.receive.recycled_descriptors, 2);
    let (hardware, _, _, _, _) = match owner.try_into_parts() {
        Ok(parts) => parts,
        Err(_) => panic!("stopped monitor owners must be extractable"),
    };
    // Both descriptors completed before service, so discarding this complete
    // software list makes its head null. The vendor append branch republishes
    // the returned chain through RX_DESCRIPTOR_BASE instead of ringing the
    // live-list reload doorbell.
    assert_eq!(hardware.reloads, 0);
    assert_eq!(hardware.base_writes, 2);
}

#[test]
fn bounded_filter_runs_before_the_sink_and_stop_recovers_current_last() {
    let mut storage = ReceiveDmaStorage::new();
    write_beacon(&mut storage, 0);
    let runtime = RuntimeFixture::new();
    let plan = monitor_plan_with(
        WifiMonitorConfig::normalized()
            .with_filter(MonitorFilter::all().frame_types(MonitorFrameTypeMask::DATA)),
    );
    let mut owner = service_with_plan(
        &storage,
        &runtime,
        Sink::default(),
        RouteBehavior::default(),
        plan,
    );
    let stop_polls = Cell::new(0_u8);
    let stop = poll_fn(|context| {
        let poll = stop_polls.get();
        if poll == 0 {
            stop_polls.set(1);
            complete_beacon(&storage, 0);
            runtime.irq.publish(EVENT_RX_SUCCESS);
            context.waker().wake_by_ref();
            Poll::Pending
        } else if poll == 1 {
            stop_polls.set(2);
            context.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    });

    let report = block_on(owner.run_until_stopped(stop, &SkipClock::new()))
        .unwrap_or_else(|_| panic!("filtered monitor run must stop cleanly"));

    assert_eq!(report.receive.completed_descriptors, 1);
    assert_eq!(report.receive.published_frames, 0);
    assert_eq!(report.receive.dropped_frames, 1);
    assert_eq!(report.receive.filtered_drops, 1);
    assert_eq!(report.receive.recycled_descriptors, 0);
    let (_, _, sink, _, _) = owner
        .try_into_parts()
        .unwrap_or_else(|_| panic!("stopped monitor owners must be extractable"));
    assert_eq!(sink.frames, 0);
}

#[test]
fn activation_failure_quiesces_the_route_but_preserves_the_live_ring() {
    let storage = ReceiveDmaStorage::new();
    let runtime = RuntimeFixture::new();
    let mut owner = service(
        &storage,
        &runtime,
        Sink::default(),
        RouteBehavior {
            fail_activate: true,
            ..RouteBehavior::default()
        },
    );

    let failure = match block_on(owner.run_until_stopped(ready(()), &SkipClock::new())) {
        Ok(_) => panic!("route activation must fail"),
        Err(failure) => failure,
    };
    assert_eq!(owner.receive_phase(), RxFrontierPhase::Live);
    assert!(!owner.interrupt_active());
    assert!(matches!(
        failure.error,
        MonitorRunError::Activate(MacInterruptEpochActivateError::Route(RouteError::Activate))
    ));
    block_on(owner.stop(&SkipClock::new()))
        .unwrap_or_else(|_| panic!("caller can retry a transient quiesce failure"));
    assert_eq!(owner.receive_phase(), RxFrontierPhase::Live);
    assert!(!owner.interrupt_active());
}

#[test]
fn logical_stop_does_not_call_physical_irq_quiesce() {
    let storage = ReceiveDmaStorage::new();
    let runtime = RuntimeFixture::new();
    let mut owner = service(
        &storage,
        &runtime,
        Sink::default(),
        RouteBehavior {
            fail_quiesce: true,
            ..RouteBehavior::default()
        },
    );

    block_on(owner.run_until_stopped(ready(()), &SkipClock::new()))
        .unwrap_or_else(|_| panic!("logical stop must not call route quiesce"));
    assert_eq!(owner.receive_phase(), RxFrontierPhase::Live);
    assert!(owner.interrupt_active());
    assert!(runtime.irq.is_rx_moderation_active());
}

#[test]
fn cancelled_run_keeps_owner_available_for_explicit_shutdown() {
    let storage = ReceiveDmaStorage::new();
    let runtime = RuntimeFixture::new();
    let mut owner = service(
        &storage,
        &runtime,
        Sink::default(),
        RouteBehavior::default(),
    );

    {
        let clock = SkipClock::new();
        let mut run = pin!(owner.run_until_stopped(pending(), &clock));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    }

    assert_eq!(owner.receive_phase(), RxFrontierPhase::Live);
    assert!(owner.interrupt_active());
    assert_eq!(
        owner.stopped_hardware_mut().map(|_| ()),
        Err(MonitorStoppedAccessError::RoleActive)
    );
    block_on(owner.stop(&SkipClock::new()))
        .unwrap_or_else(|_| panic!("cancelled monitor must remain recoverable"));
    assert_eq!(owner.receive_phase(), RxFrontierPhase::Live);
    assert!(owner.interrupt_active());
    assert!(runtime.irq.is_rx_moderation_active());
    assert!(owner.stopped_hardware_mut().is_ok());
}

#[test]
fn logical_stop_does_not_request_a_physical_walker_stop() {
    let storage = ReceiveDmaStorage::new();
    let runtime = RuntimeFixture::new();
    let mut owner = service(
        &storage,
        &runtime,
        Sink::default(),
        RouteBehavior::default(),
    );
    {
        let clock = SkipClock::new();
        let mut run = pin!(owner.run_until_stopped(pending(), &clock));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    }
    owner
        .hardware
        .as_mut()
        .expect("monitor hardware owner exists")
        .disable_busy_count = 1;

    block_on(owner.stop(&SkipClock::new()))
        .unwrap_or_else(|_| panic!("logical monitor stop must succeed"));

    assert_eq!(owner.receive_phase(), RxFrontierPhase::Live);
    assert!(owner.interrupt_active());
    assert_eq!(
        owner
            .hardware
            .as_ref()
            .expect("monitor hardware owner exists")
            .disable_busy_count,
        1,
        "logical stop must not call the physical walker-stop primitive"
    );
}

#[test]
fn dropping_a_cancelled_live_service_retains_irq_and_dma_owners() {
    let storage = ReceiveDmaStorage::new();
    let runtime = RuntimeFixture::new();
    {
        let mut owner = service(
            &storage,
            &runtime,
            Sink::default(),
            RouteBehavior::default(),
        );
        {
            let clock = SkipClock::new();
            let mut run = pin!(owner.run_until_stopped(pending(), &clock));
            let mut context = Context::from_waker(Waker::noop());
            assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
        }
        assert!(runtime.active.get());
        drop(owner);
    }
    assert!(runtime.active.get());
    assert_eq!(runtime.walker_when_dropped.get(), None);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn busy_drop_does_not_spin_or_release_hardware_visible_owners() {
    let storage = ReceiveDmaStorage::new();
    let runtime = RuntimeFixture::new();
    {
        let mut owner = service(
            &storage,
            &runtime,
            Sink::default(),
            RouteBehavior::default(),
        );
        {
            let clock = SkipClock::new();
            let mut run = pin!(owner.run_until_stopped(pending(), &clock));
            let mut context = Context::from_waker(Waker::noop());
            assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
        }
        owner
            .hardware
            .as_mut()
            .expect("monitor hardware owner exists")
            .disable_busy_count = 1;
        drop(owner);
    }

    assert!(runtime.active.get());
    assert_eq!(runtime.walker_when_dropped.get(), None);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

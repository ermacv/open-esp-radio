//! The receive runner over host models of every owner it drives.
//!
//! The interrupt line runs through a leaked [`HandlerCell`] and the same
//! line functions the image's handlers call. [`Host::p3`] plays the radio
//! interrupt level: while a routed bank asserts its line, it runs that
//! bank's handler. The doorbell runs its handler synchronously, as the
//! software interrupt preempts the radio executor, and then lets the level
//! serve whatever the handler unmasked. Frames are completed by writing the
//! descriptor word and the walker's LAST/NEXT, as the DMA engine would.

use core::{
    cell::{Cell, RefCell},
    convert::Infallible,
    pin::Pin,
    task::{Context, Waker},
};
use std::{boxed::Box, vec::Vec};

use oer_esp32s31_hal::types::{MacInterruptEvents, MacInterruptObservation};
use oer_esp32s31_ieee80211_dma::{
    descriptor::{BIT_30, BIT_31, LENGTH_SHIFT},
    rx_dma::{
        RxDmaCursorObservation, RxDmaNextDescriptor, RxDmaReloadSettled, RxDmaWalkerEnabled,
        RxDmaWalkerStopped,
        model::{RxWalkerModel, RxWalkerOperation},
    },
    rx_ring::RxDmaArenaState,
    rx_storage::RxDmaStorage,
    space::HostDma,
};
use oer_esp32s31_ieee80211_mac::{
    irq::model::{
        MacInterruptModel, MacInterruptOperation, PowerInterruptModel, PowerInterruptOperation,
    },
    rx::{RxDmaBinding, RxRingError, pool::NetworkRxFrame, pool::RxStageTransactionError},
};
use oer_time_virtual::VirtualClock;

use super::*;
use crate::{
    irq::{run_doorbell, service_mac_line, service_power_line},
    rx::{
        arena::WifiRxArena,
        control::ReceiveStopHandle,
        counters::WifiRxReport,
        frontier::RxFrontierError,
        returns::{HeldRxFrame, RxReturnNotifier},
        sink::{CountingAdmission, CountingSink},
        transaction::{CompletedUnit, CreditPolicy, IngressClass, IngressRoute, Preview},
    },
};

const COUNT: usize = 32;
const BUFFER: usize = 1_664;
const STORAGE: usize = 1_668;
const SLOTS: usize = 32;

type HostEpoch = ActiveEpoch<Moderated, MacInterruptModel, PowerInterruptModel>;
type HostLine = WifiIrqLine<HostEpoch>;
type HostArena = WifiRxArena<COUNT, BUFFER, STORAGE, SLOTS, HostDma, HostEpoch>;
type HostStorage = RxDmaStorage<COUNT, BUFFER, STORAGE, HostDma>;

fn leak<T>(value: T) -> &'static mut T {
    Box::leak(Box::new(value))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    PromiscuousOn,
    PromiscuousOff,
    Activate,
    RoutesEnabled { installed: bool, pending: bool },
    RoutesSilenced,
    RuntimeResume,
    RuntimeStop,
    WalkerStop,
    Ring { released: usize },
    Acknowledge,
    MacLine,
    PowerLine,
}

/// The hardware every host owner shares.
struct Host {
    walker: RefCell<RxWalkerModel>,
    log: RefCell<Vec<Event>>,
    routed: Cell<bool>,
    /// The doorbell rings but its handler never runs.
    deaf: Cell<bool>,
    runtime_active: Cell<u8>,
    cell: HandlerCell<HostLine>,
}

impl Host {
    fn new() -> Self {
        Self {
            walker: RefCell::new(RxWalkerModel::new()),
            log: RefCell::new(Vec::new()),
            routed: Cell::new(false),
            deaf: Cell::new(false),
            runtime_active: Cell::new(0),
            cell: HandlerCell::new(),
        }
    }

    fn record(&self, event: Event) {
        self.log.borrow_mut().push(event);
    }

    fn take_log(&self) -> Vec<Event> {
        core::mem::take(&mut *self.log.borrow_mut())
    }

    fn line<R>(&self, f: impl FnOnce(&mut HostLine) -> R) -> R {
        self.cell.with(f).expect("the line is installed and idle")
    }

    fn mac(&self) -> MacInterruptModel {
        self.line(|line| line.epoch().banks().0.clone())
    }

    fn power(&self) -> PowerInterruptModel {
        self.line(|line| line.epoch().banks().1.clone())
    }

    /// The radio interrupt level: run each routed, asserted bank's handler.
    fn p3(&self) {
        for _ in 0..8 {
            if !self.routed.get() {
                return;
            }
            let Some((mac, power)) = self.cell.with(|line| {
                let (mac, power) = line.epoch().banks();
                (mac.line_asserted(), power.line_asserted())
            }) else {
                return;
            };
            if !mac && !power {
                return;
            }
            if power {
                self.record(Event::PowerLine);
                self.cell.with(service_power_line);
            }
            if mac {
                self.record(Event::MacLine);
                self.cell.with(service_mac_line);
            }
        }
    }
}

struct HostRadio {
    host: &'static Host,
}

impl RxDma for HostRadio {
    fn last_descriptor_low(&mut self) -> u32 {
        self.host.walker.borrow_mut().last_descriptor_low()
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.host.walker.borrow_mut().next_descriptor_low()
    }

    fn next_descriptor(&mut self) -> RxDmaNextDescriptor {
        self.host.walker.borrow_mut().next_descriptor()
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'c> FnOnce(RxDmaCursorObservation<'c>) -> R,
    ) -> R {
        self.host.walker.borrow_mut().with_ordered_cursor(observed)
    }

    fn walker_enabled(&mut self) -> bool {
        self.host.walker.borrow_mut().walker_enabled()
    }

    fn reload_pending(&mut self) -> bool {
        RxDma::reload_pending(&mut *self.host.walker.borrow_mut())
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'c> FnOnce(RxDmaReloadSettled<'c>) -> R,
    ) -> Option<R> {
        self.host
            .walker
            .borrow_mut()
            .try_with_reload_settled(settled)
    }

    fn configure_descriptor_window(&mut self, binding: &RxDmaBinding<'_>) {
        self.host
            .walker
            .borrow_mut()
            .configure_descriptor_window(binding);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32) {
        self.host
            .walker
            .borrow_mut()
            .write_descriptor_base(binding, address);
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>) {
        self.host.walker.borrow_mut().publish_walker_enable(binding);
    }

    fn request_reload(&mut self, binding: &RxDmaBinding<'_>) {
        self.host.walker.borrow_mut().request_reload(binding);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        enabled: impl for<'c> FnOnce(RxDmaWalkerEnabled<'c>) -> R,
    ) -> Option<R> {
        self.host
            .walker
            .borrow_mut()
            .try_with_walker_enabled(binding, enabled)
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        stopped: impl for<'c> FnOnce(RxDmaWalkerStopped<'c>) -> R,
    ) -> Option<R> {
        self.host.record(Event::WalkerStop);
        self.host
            .walker
            .borrow_mut()
            .try_with_walker_stopped(binding, stopped)
    }

    fn fence(&mut self) {
        self.host.walker.borrow_mut().fence();
    }
}

impl MacSnifferHardware for HostRadio {
    fn configure_open_promiscuous_receive(&mut self) {
        self.host.record(Event::PromiscuousOn);
    }

    fn disable_open_promiscuous_receive(&mut self) {
        self.host.record(Event::PromiscuousOff);
    }
}

impl MacRuntimeStopHardware for HostRadio {
    fn request_mac_runtime_stop(&mut self) {
        self.host.record(Event::RuntimeStop);
    }

    fn mac_runtime_active_state(&mut self) -> u8 {
        self.host.runtime_active.get()
    }

    fn resume_mac_runtime(&mut self) {
        self.host.record(Event::RuntimeResume);
    }
}

struct HostRoutes {
    host: &'static Host,
    requests: &'static WifiIrqRequests,
}

impl WifiIrqRoutes for HostRoutes {
    fn enable(&mut self) {
        self.host.record(Event::RoutesEnabled {
            installed: self.host.cell.with(|_| ()).is_some(),
            pending: self.requests.is_pending(),
        });
        self.host.routed.set(true);
    }

    fn silence(&mut self) {
        self.host.record(Event::RoutesSilenced);
        self.host.routed.set(false);
    }
}

struct HostDoorbell {
    host: &'static Host,
    rx: &'static HostStorage,
}

impl RadioDoorbell for HostDoorbell {
    fn ring(&self) {
        self.host.record(Event::Ring {
            released: self.rx.released_buffer_count(),
        });
        if self.host.deaf.get() {
            return;
        }
        run_doorbell(&self.host.cell, || self.host.record(Event::Acknowledge));
        // The level then serves whatever the handler unmasked.
        self.host.p3();
    }
}

/// A doorbell nobody listens to, for requests made before an epoch.
struct Silent;

impl RadioDoorbell for Silent {
    fn ring(&self) {}
}

type Held = RefCell<Vec<NetworkRxFrame<'static, SLOTS, BUFFER>>>;

/// A publisher that keeps every lease until the test releases it.
struct HoldingSink {
    held: &'static Held,
}

impl Publisher<'static, BUFFER, SLOTS> for HoldingSink {
    const DEPTH: usize = SLOTS;

    fn free_capacity(&self) -> usize {
        SLOTS - self.held.borrow().len()
    }

    fn preview(&self, unit: CompletedUnit, _: [u8; 24]) -> Preview {
        self.unclassified_preview(unit)
    }

    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview {
        Preview {
            unit,
            frame_control: None,
            class: IngressClass::Unclassified,
            route: IngressRoute::Malformed,
        }
    }

    fn try_send(
        &self,
        frame: NetworkRxFrame<'static, SLOTS, BUFFER>,
    ) -> Result<(), NetworkRxFrame<'static, SLOTS, BUFFER>> {
        self.held.borrow_mut().push(frame);
        Ok(())
    }
}

/// An admission whose reserve leaves no ordinary credit: every service
/// transaction refuses it before touching the ring.
struct OverReserved;

impl Admission for OverReserved {
    fn credit_policy(&self) -> CreditPolicy {
        CreditPolicy::new(SLOTS)
    }
}

struct Harness {
    host: &'static Host,
    rx: &'static HostStorage,
    base: u32,
    requests: &'static WifiIrqRequests,
    counters: &'static WifiRxCounters,
    stop: ReceiveStopHandle<'static>,
    clock: &'static VirtualClock,
    runner: Pin<Box<dyn Future<Output = Infallible>>>,
    next: usize,
}

fn harness() -> Harness {
    harness_with(|counters| {
        (
            CountingSink::new(counters),
            CountingAdmission::new(counters),
        )
    })
}

fn harness_with<E, M>(owners: impl FnOnce(&'static WifiRxCounters) -> (E, M)) -> Harness
where
    E: Publisher<'static, BUFFER, SLOTS> + 'static,
    M: Admission + 'static,
{
    harness_staged::<SLOTS, E, M>(|counters, _| owners(counters))
}

/// A harness whose arena has `STAGE` staging slots; `owners` also gets the
/// upper layer's return notifier.
fn harness_staged<const STAGE: usize, E, M>(
    owners: impl FnOnce(&'static WifiRxCounters, RxReturnNotifier<'static>) -> (E, M),
) -> Harness
where
    E: Publisher<'static, BUFFER, STAGE> + 'static,
    M: Admission + 'static,
{
    let host: &'static Host = leak(Host::new());
    let arena: &'static mut WifiRxArena<COUNT, BUFFER, STORAGE, STAGE, HostDma, HostEpoch> =
        leak(bytemuck::Zeroable::zeroed());
    let (parts, stop) = arena.bind().expect("a zeroed arena binds");
    let (rx, base) = (parts.rx(), parts.descriptor_base());
    let (requests, counters) = (parts.requests(), parts.counters());
    let (publisher, admission) = owners(counters, parts.returns());
    let (publisher, admission) = (&*leak(publisher), &*leak(admission));
    let clock: &'static VirtualClock = leak(VirtualClock::new());
    let hardware = leak(HostRadio { host });
    let routes = leak(HostRoutes { host, requests });
    let doorbell = &*leak(HostDoorbell { host, rx });
    let activate = move || {
        host.record(Event::Activate);
        ActiveEpoch::for_host_test(MacInterruptModel::new(), PowerInterruptModel::new())
    };
    let runner = Box::pin(receive(
        hardware, activate, parts, &host.cell, routes, doorbell, clock, publisher, admission,
    ));
    Harness {
        host,
        rx,
        base,
        requests,
        counters,
        stop,
        clock,
        runner,
        next: 0,
    }
}

fn rx_success() -> MacInterruptObservation {
    MacInterruptObservation::from_semantic_events(MacInterruptEvents::RX_SUCCESS, false, false)
}

impl Harness {
    /// Run the level and the runner until both are idle, advancing time to
    /// each deadline the runner waits for.
    fn settle(&mut self) {
        for _ in 0..64 {
            self.host.p3();
            let mut context = Context::from_waker(Waker::noop());
            assert!(self.runner.as_mut().poll(&mut context).is_pending());
            self.clock.advance_to_next_deadline();
        }
    }

    /// Complete the units `lengths` describe (one descriptor each, the last
    /// one terminal) at the next descriptors, as the DMA engine would, and
    /// leave the walker past them.
    fn complete_unit(&mut self, lengths: &[u32]) {
        for (step, &length) in lengths.iter().enumerate() {
            let index = (self.next + step) % COUNT;
            let terminal = if step + 1 == lengths.len() { BIT_30 } else { 0 };
            self.rx.descriptors()[index]
                .write_word0(BUFFER as u32 | (length << LENGTH_SHIFT) | terminal | BIT_31);
        }
        self.next = (self.next + lengths.len()) % COUNT;
        let last = (self.next + COUNT - 1) % COUNT;
        self.host
            .walker
            .borrow_mut()
            .set_cursor(self.base, last, Some(self.next));
    }

    /// The MAC latches its RX-success event for the completed units.
    fn rx_event(&self) {
        self.host
            .line(|line| line.epoch_mut().banks_mut().0.latch(rx_success()));
    }

    fn frame(&mut self, length: u32) {
        self.complete_unit(&[length]);
        self.rx_event();
    }

    fn poll_once(&mut self) {
        let mut context = Context::from_waker(Waker::noop());
        assert!(self.runner.as_mut().poll(&mut context).is_pending());
    }

    fn report(&self) -> WifiRxReport {
        self.counters.report()
    }

    fn walker_stops(&self) -> usize {
        self.host
            .walker
            .borrow()
            .operation_count(RxWalkerOperation::Stop)
    }

    /// Bring the epoch up and forget its bring-up log.
    fn started(mut self) -> Self {
        self.settle();
        assert!(self.host.walker.borrow().walker_enabled);
        self.host.take_log();
        self.host.walker.borrow_mut().clear_operations();
        self
    }
}

#[test]
fn routes_enable_after_epoch_activation_and_install() {
    let mut harness = harness();
    // A request left by an earlier epoch is cleared by the install.
    harness.requests.ring(WifiIrqRequest::MaskAll, &Silent);
    harness.settle();
    assert_eq!(
        harness.host.take_log(),
        [
            Event::PromiscuousOn,
            Event::Activate,
            Event::RoutesEnabled {
                installed: true,
                pending: false,
            },
            Event::RuntimeResume,
        ]
    );
    // The walker started between the routes and the runtime resume.
    let walker = harness.host.walker.borrow();
    assert!(walker.walker_enabled);
    assert_eq!(walker.operation_count(RxWalkerOperation::Enable), 1);
    drop(walker);
    // Nothing touched the banks; the stale mask was not applied.
    assert!(harness.host.mac().operations().is_empty());
    assert!(harness.host.mac().rx_delivery_enabled());
    assert!(harness.stop.try_exit().is_none());
}

#[test]
fn p2_drains_then_unmasks_only_through_the_doorbell() {
    let mut harness = harness().started();
    harness.frame(200);
    harness.settle();

    // The staged buffer is released; it stays the walker's LAST descriptor
    // until the next frame, so the second pass rearms nothing and unmasks.
    assert_eq!(
        harness.host.take_log(),
        [
            Event::MacLine,
            Event::Ring { released: 1 },
            Event::Acknowledge
        ]
    );
    // The only bank operations are the MAC handler's and the doorbell's.
    assert!(harness.host.mac().operations().matches(&[
        MacInterruptOperation::Status,
        MacInterruptOperation::MaskRxDelivery,
        MacInterruptOperation::Acknowledge(rx_success()),
        MacInterruptOperation::UnmaskRxDelivery,
    ]));
    assert!(harness.host.power().operations().is_empty());
    assert!(harness.host.mac().rx_delivery_enabled());
    assert!(!harness.requests.is_pending());
    let report = harness.report();
    assert_eq!((report.units, report.bytes), (1, 200));
    assert_eq!((report.mac_irqs, report.doorbell_unmasks), (1, 1));
    assert_eq!(report.service_passes, 2);
    assert_eq!(harness.rx.detached_buffer_count(), 0);

    // The next frame moves the walker on: the first buffer is rearmed.
    harness.frame(300);
    harness.settle();
    assert_eq!(harness.report().units, 2);
    assert_eq!(harness.rx.released_buffer_count(), 1);
    assert_eq!(
        harness.rx.descriptors()[0].word0(),
        oer_esp32s31_ieee80211_dma::descriptor::rx_armed_word(BUFFER as u32).unwrap()
    );
}

#[test]
fn rx_latched_while_masked_fires_after_unmask() {
    let mut harness = harness().started();
    harness.frame(100);
    // The MAC handler masks RX; a second frame completes before the runner
    // drains and latches behind the mask.
    harness.host.p3();
    assert!(!harness.host.mac().rx_delivery_enabled());
    harness.frame(120);
    assert!(!harness.host.mac().line_asserted());
    harness.settle();

    // The doorbell's unmask exposed the latched event: the level served it
    // at once, and the runner drained the second frame and unmasked again.
    let log = harness.host.take_log();
    assert_eq!(log.iter().filter(|e| **e == Event::MacLine).count(), 2);
    let report = harness.report();
    assert_eq!((report.units, report.bytes), (2, 220));
    assert_eq!(report.mac_irqs, 2);
    assert!(report.doorbell_unmasks >= 2);
    assert!(harness.host.mac().rx_delivery_enabled());
    assert!(!harness.host.mac().line_asserted());
}

#[test]
fn released_frames_are_rearmed_before_unmask() {
    let mut harness = harness().started();
    let rings = |harness: &Harness| -> Vec<Event> {
        harness
            .host
            .take_log()
            .into_iter()
            .filter(|event| matches!(event, Event::Ring { .. }))
            .collect()
    };
    // A first burst: its released buffers end at the walker's LAST, so the
    // ring keeps the whole prefix until the walker moves on.
    for _ in 0..16 {
        harness.complete_unit(&[64]);
    }
    harness.rx_event();
    harness.settle();
    assert_eq!(rings(&harness), [Event::Ring { released: 16 }]);

    // A second burst moves LAST past the first: those sixteen are rearmed
    // before the unmask, and only the second burst's wait.
    for _ in 0..15 {
        harness.complete_unit(&[64]);
    }
    harness.rx_event();
    harness.settle();
    assert_eq!(rings(&harness), [Event::Ring { released: 15 }]);
    let armed = oer_esp32s31_ieee80211_dma::descriptor::rx_armed_word(BUFFER as u32).unwrap();
    for descriptor in &harness.rx.descriptors()[..16] {
        assert_eq!(descriptor.word0(), armed);
    }
    let report = harness.report();
    assert_eq!((report.units, report.bytes), (31, 64 * 31));
    assert!(harness.host.mac().rx_delivery_enabled());

    // The walker was not left short: the next frames are received, and
    // the ring wraps over the rearmed descriptors.
    for _ in 0..3 {
        harness.frame(80);
        harness.settle();
    }
    let report = harness.report();
    assert_eq!(report.units, 34);
    assert_eq!(report.service_errors, 0);
}

#[test]
fn probe_pending_keeps_rx_masked() {
    let mut harness = harness().started();
    // The walker stopped at the accepted terminal with NEXT = 0 and the
    // terminal's writeback is not visible yet: no further RX edge is
    // guaranteed, so the pass must probe again with RX still masked.
    harness
        .host
        .walker
        .borrow_mut()
        .set_cursor(harness.base, COUNT - 1, None);
    harness.rx_event();
    harness.host.p3();

    // Four immediate self-wakes, then one timer wait.
    for poll in 1..=IMMEDIATE_CONTINUATIONS {
        harness.poll_once();
        assert_eq!(harness.report().immediate_continuations, poll);
        assert_eq!(harness.clock.next_deadline(), None);
    }
    harness.poll_once();
    let report = harness.report();
    assert_eq!(
        (report.immediate_continuations, report.timed_continuations),
        (IMMEDIATE_CONTINUATIONS, 1)
    );
    let now = oer_time::Clock::now(harness.clock);
    assert_eq!(
        harness.clock.next_deadline(),
        now.checked_add(CONTINUATION_WAIT)
    );

    harness.settle();
    assert!(!harness.host.mac().rx_delivery_enabled());
    assert_eq!(harness.report().doorbell_unmasks, 0);
    assert!(!harness.requests.is_pending());
    assert!(
        !harness
            .host
            .take_log()
            .iter()
            .any(|event| matches!(event, Event::Ring { .. }))
    );

    // The walker moves on: the probe ends and RX is unmasked.
    harness.host.walker.borrow_mut().next_descriptor_low = harness.base & 0x000f_ffff;
    harness.settle();
    assert!(harness.host.mac().rx_delivery_enabled());
    assert_eq!(harness.report().doorbell_unmasks, 1);
}

#[test]
fn install_clears_stale_requests_and_receive_proceeds() {
    let mut harness = harness();
    harness.requests.ring(WifiIrqRequest::MaskAll, &Silent);
    harness.settle();
    harness.frame(90);
    harness.settle();
    assert_eq!(harness.report().units, 1);
    assert!(harness.host.line(|line| !line.masked()));
}

#[test]
fn teardown_is_mask_silence_close_drain_stop_once() {
    let mut harness = harness().started();
    harness.frame(200);
    harness.settle();
    harness.host.take_log();

    harness.stop.request_stop();
    harness.settle();
    assert_eq!(
        harness.host.take_log(),
        [
            Event::Ring { released: 1 },
            Event::Acknowledge,
            Event::RoutesSilenced,
            Event::PromiscuousOff,
            Event::RuntimeStop,
            Event::WalkerStop,
        ]
    );
    assert_eq!(
        harness.stop.try_exit(),
        Some(WifiRxExit {
            cause: WifiRxCause::StopRequested,
            end: WifiRxEnd::Halted,
        })
    );
    // Both banks masked, nothing acknowledged by the mask.
    let mac = harness.host.mac();
    assert!(!mac.rx_delivery_enabled() && !mac.other_sources_enabled());
    assert!(
        harness
            .host
            .power()
            .operations()
            .matches(&[PowerInterruptOperation::MaskAll])
    );
    // The halted ring is held: the arena is neither reusable nor poisoned.
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::Prepared);

    // Parked: nothing runs again.
    harness.settle();
    assert!(harness.host.take_log().is_empty());
    assert_eq!(harness.walker_stops(), 1);
}

#[test]
fn teardown_waits_for_staged_leases_and_the_mac_runtime() {
    let held: &'static Held = leak(RefCell::new(Vec::new()));
    let mut harness =
        harness_with(|counters| (HoldingSink { held }, CountingAdmission::new(counters))).started();
    harness.frame(200);
    harness.settle();
    assert_eq!(harness.rx.detached_buffer_count(), 1);
    harness.host.runtime_active.set(1);
    harness.host.take_log();

    harness.stop.request_stop();
    harness.settle();
    // Masked, silenced and closed, but no stop while a lease is out.
    assert!(!harness.host.take_log().contains(&Event::WalkerStop));
    assert!(harness.stop.try_exit().is_none());

    // The upper owner returns its lease; the MAC runtime is still busy.
    held.borrow_mut().clear();
    harness.settle();
    assert_eq!(harness.walker_stops(), 0);
    harness.host.runtime_active.set(0);
    harness.settle();
    assert_eq!(harness.walker_stops(), 1);
    assert_eq!(
        harness.stop.try_exit().map(|exit| exit.end),
        Some(WifiRxEnd::Halted)
    );
}

#[test]
fn failed_stop_quarantines_and_never_retries() {
    let mut harness = harness().started();
    harness.host.walker.borrow_mut().refuse_stop = true;
    harness.stop.request_stop();
    harness.settle();
    assert_eq!(
        harness.stop.try_exit(),
        Some(WifiRxExit {
            cause: WifiRxCause::StopRequested,
            end: WifiRxEnd::Quarantined {
                reason: WifiRxQuarantine::StopRefused(RxFrontierError::Ring(RxRingError::Busy)),
                walker_stopped: false,
            },
        })
    );
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::ResetRequired);
    harness.settle();
    harness.settle();
    assert_eq!(harness.walker_stops(), 1);
}

#[test]
fn service_fault_masks_and_quarantines() {
    let mut harness =
        harness_with(|counters| (CountingSink::new(counters), OverReserved)).started();
    harness.frame(200);
    harness.settle();
    assert_eq!(
        harness.stop.try_exit(),
        Some(WifiRxExit {
            cause: WifiRxCause::Service(RxStageTransactionError::InvalidCreditReserve {
                reserved: SLOTS,
                pool_slots: SLOTS,
                queue_depth: SLOTS,
            }),
            end: WifiRxEnd::Quarantined {
                reason: WifiRxQuarantine::Fault,
                walker_stopped: true,
            },
        })
    );
    assert_eq!(
        harness.host.take_log(),
        [
            Event::MacLine,
            Event::Ring { released: 0 },
            Event::Acknowledge,
            Event::RoutesSilenced,
            Event::PromiscuousOff,
            Event::RuntimeStop,
            Event::WalkerStop,
        ]
    );
    assert!(harness.host.line(|line| line.masked()));
    // Poisoned before the stop: the arena waits for a radio reset.
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::ResetRequired);
    let report = harness.report();
    assert_eq!((report.service_errors, report.doorbell_unmasks), (1, 0));
}

#[test]
fn a_mask_never_confirmed_quarantines_without_a_stop() {
    let mut harness = harness().started();
    harness.host.deaf.set(true);
    harness.stop.request_stop();
    harness.settle();
    assert_eq!(
        harness.stop.try_exit(),
        Some(WifiRxExit {
            cause: WifiRxCause::StopRequested,
            end: WifiRxEnd::Quarantined {
                reason: WifiRxQuarantine::MaskTimeout,
                walker_stopped: false,
            },
        })
    );
    assert_eq!(
        harness.host.take_log(),
        [
            Event::Ring { released: 0 },
            Event::RoutesSilenced,
            Event::PromiscuousOff,
            Event::RuntimeStop,
        ]
    );
    assert_eq!(harness.walker_stops(), 0);
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn an_occupied_cell_refuses_the_epoch_before_any_route() {
    let mut first = harness().started();
    // A second epoch on the same cell: its line is refused and masked by
    // the executor itself, and its routes are never enabled.
    let cell = &first.host.cell;
    let host: &'static Host = leak(Host::new());
    let arena: &'static mut HostArena = leak(bytemuck::Zeroable::zeroed());
    let (parts, mut stop) = arena.bind().unwrap();
    let counters = parts.counters();
    let mut runner = Box::pin(receive(
        leak(HostRadio { host }),
        || ActiveEpoch::for_host_test(MacInterruptModel::new(), PowerInterruptModel::new()),
        parts,
        cell,
        leak(HostRoutes {
            host,
            requests: leak(WifiIrqRequests::new()),
        }),
        &*leak(HostDoorbell { host, rx: first.rx }),
        &*leak(VirtualClock::<8>::new()),
        &*leak(CountingSink::new(counters)),
        &*leak(CountingAdmission::new(counters)),
    ));
    assert!(
        runner
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    assert_eq!(
        stop.try_exit(),
        Some(WifiRxExit {
            cause: WifiRxCause::LineRefused,
            end: WifiRxEnd::NotStarted,
        })
    );
    assert_eq!(
        host.take_log(),
        [Event::PromiscuousOn, Event::PromiscuousOff]
    );
    // The first epoch still runs.
    first.frame(50);
    first.settle();
    assert_eq!(first.report().units, 1);
}

#[test]
fn report_counts_units_bytes_and_drops() {
    let mut harness = harness().started();
    harness.complete_unit(&[200]);
    harness.complete_unit(&[0]);
    harness.complete_unit(&[1_000]);
    harness.rx_event();
    harness.settle();

    let report = harness.report();
    assert_eq!((report.units, report.bytes), (2, 1_200));
    assert_eq!(
        (
            report.empty_drops,
            report.chained_drops,
            report.oversized_drops,
            report.too_long_drops,
            report.overload_drops,
            report.admission_blocks,
            report.stage_capacity_blocks,
        ),
        (1, 0, 0, 0, 0, 0, 0)
    );
    assert_eq!((report.mac_irqs, report.spurious_mac_irqs), (1, 0));
    assert_eq!((report.doorbell_unmasks, report.ignored_unmasks), (1, 0));
    assert_eq!((report.service_passes, report.service_errors), (3, 0));
    // The empty unit's discard and the staged units' rearm each settled
    // one reload doorbell, on its first observation.
    assert_eq!(report.max_reload_attempts, 1);
}

const STAGE: usize = 8;

type Returned = RefCell<Vec<HeldRxFrame<'static, 'static, STAGE, BUFFER>>>;

/// An upper layer that holds its leases while `holding`, through the
/// notifier, as a network queue would; otherwise it drops them at once.
struct ReturningSink {
    held: &'static Returned,
    holding: &'static Cell<bool>,
    returns: RxReturnNotifier<'static>,
    counters: &'static WifiRxCounters,
}

impl Publisher<'static, BUFFER, STAGE> for ReturningSink {
    const DEPTH: usize = STAGE;

    fn free_capacity(&self) -> usize {
        STAGE - self.held.borrow().len()
    }

    fn preview(&self, unit: CompletedUnit, _: [u8; 24]) -> Preview {
        self.unclassified_preview(unit)
    }

    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview {
        Preview {
            unit,
            frame_control: None,
            class: IngressClass::Unclassified,
            route: IngressRoute::Malformed,
        }
    }

    fn try_send(
        &self,
        frame: NetworkRxFrame<'static, STAGE, BUFFER>,
    ) -> Result<(), NetworkRxFrame<'static, STAGE, BUFFER>> {
        self.counters.received(frame.length());
        if self.holding.get() {
            self.held.borrow_mut().push(self.returns.hold(frame));
        }
        Ok(())
    }
}

fn returning_harness() -> (
    Harness,
    &'static Returned,
    &'static Cell<bool>,
    RxReturnNotifier<'static>,
) {
    let held: &'static Returned = leak(RefCell::new(Vec::new()));
    let holding: &'static Cell<bool> = leak(Cell::new(true));
    let mut notifier = None;
    let harness = harness_staged::<STAGE, _, _>(|counters, returns| {
        notifier = Some(returns);
        (
            ReturningSink {
                held,
                holding,
                returns,
                counters,
            },
            CountingAdmission::new(counters),
        )
    })
    .started();
    (
        harness,
        held,
        holding,
        notifier.expect("the harness lends the notifier"),
    )
}

fn mac_lines(log: &[Event]) -> usize {
    log.iter().filter(|event| **event == Event::MacLine).count()
}

#[test]
fn a_returned_lease_rearms_an_exhausted_walker_without_a_mac_interrupt() {
    let (mut harness, held, holding, _) = returning_harness();
    // The upper layer takes every staging slot and keeps it.
    for _ in 0..STAGE {
        harness.complete_unit(&[64]);
    }
    harness.rx_event();
    harness.settle();
    assert_eq!(held.borrow().len(), STAGE);
    assert_eq!(harness.rx.detached_buffer_count(), STAGE);

    // The rest of the ring fills behind them and the walker runs out: the
    // held head keeps every later descriptor from being rearmed.
    for _ in STAGE..COUNT {
        harness.complete_unit(&[64]);
    }
    harness
        .host
        .walker
        .borrow_mut()
        .set_cursor(harness.base, COUNT - 1, None);
    harness.rx_event();
    harness.settle();
    for _ in 0..3 {
        harness.settle();
    }
    let report = harness.report();
    assert_eq!(report.units, STAGE as u32);
    assert!(report.stage_capacity_blocks >= 1);
    // No descriptor is armed and the walker is exhausted: the model can
    // produce no further RX completion, so no further MAC interrupt.
    assert!(
        harness
            .rx
            .descriptors()
            .iter()
            .all(|descriptor| oer_esp32s31_ieee80211_dma::descriptor::rx_done(descriptor.word0()))
    );
    assert_eq!(harness.host.walker.borrow().next_descriptor_low, 0);
    assert!(!harness.host.mac().line_asserted());
    assert!(!harness.host.mac().rx_delivery_enabled());
    harness.host.take_log();

    // The upper layer returns its leases; nothing else happens.
    holding.set(false);
    held.borrow_mut().clear();
    harness.settle();
    let log = harness.host.take_log();
    assert_eq!(mac_lines(&log), 0, "the wake came from the return");
    // The runner rearmed the returned prefix, staged what waited behind it
    // and republished the head of the exhausted list through BASE.
    let report = harness.report();
    assert_eq!(report.units, COUNT as u32);
    let armed = oer_esp32s31_ieee80211_dma::descriptor::rx_armed_word(BUFFER as u32).unwrap();
    for descriptor in &harness.rx.descriptors()[..STAGE] {
        assert_eq!(descriptor.word0(), armed);
    }
    assert!(
        harness
            .host
            .walker
            .borrow()
            .operations()
            .contains(&RxWalkerOperation::WriteDescriptorBase(harness.base))
    );
    // Until the walker is seen to take the republished list, RX stays
    // masked and the runner probes.
    assert!(!harness.host.mac().rx_delivery_enabled());
    assert!(!log.iter().any(|event| matches!(event, Event::Ring { .. })));

    // The walker fetches the new head; the probe ends and RX is unmasked,
    // still without a MAC interrupt.
    harness.host.walker.borrow_mut().next_descriptor_low = harness.base & 0x000f_ffff;
    harness.settle();
    let log = harness.host.take_log();
    assert_eq!(mac_lines(&log), 0);
    assert!(log.contains(&Event::Acknowledge));
    assert!(harness.host.mac().rx_delivery_enabled());
    assert_eq!(harness.report().service_errors, 0);

    // A new frame lands in a rearmed descriptor and raises its own edge.
    harness.frame(150);
    harness.settle();
    assert_eq!(mac_lines(&harness.host.take_log()), 1);
    assert_eq!(harness.report().units, COUNT as u32 + 1);
}

#[test]
fn a_return_while_rx_is_live_is_harmless() {
    let (mut harness, held, holding, notifier) = returning_harness();
    harness.frame(100);
    harness.settle();
    assert_eq!(held.borrow().len(), 1);
    assert!(harness.host.mac().rx_delivery_enabled());
    let before = harness.report();
    harness.host.take_log();

    // RX is live: the return starts one pass, which re-enables what is
    // enabled already.
    holding.set(false);
    held.borrow_mut().clear();
    harness.settle();
    let log = harness.host.take_log();
    assert_eq!(mac_lines(&log), 0);
    let after = harness.report();
    assert_eq!(after.doorbell_unmasks, before.doorbell_unmasks + 1);
    assert_eq!((after.service_errors, after.ignored_unmasks), (0, 0));
    assert!(harness.host.mac().rx_delivery_enabled());
    assert!(harness.host.line(|line| !line.masked()));

    harness.frame(120);
    harness.settle();
    assert_eq!(harness.report().units, 2);

    // A notice after the epoch parked reaches no one.
    harness.stop.request_stop();
    harness.settle();
    let exit = harness.stop.try_exit();
    assert_eq!(exit.map(|exit| exit.end), Some(WifiRxEnd::Halted));
    harness.host.take_log();
    notifier.notify();
    harness.settle();
    assert!(harness.host.take_log().is_empty());
    assert_eq!(harness.walker_stops(), 1);
}

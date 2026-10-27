//! The receive runner over host models of every owner it drives.
//!
//! The interrupt line runs through a leaked [`HandlerCell`] and the same
//! line function the image's handler calls, over the host's part of both
//! bank models. [`Host::p3`] plays the radio interrupt level: while a bank
//! asserts the line, it runs the handler. The doorbell runs the handler
//! synchronously, as the software interrupt preempts the radio executor,
//! and then lets the level serve whatever the handler unmasked. Frames are completed by writing the
//! descriptor word and the walker's LAST/NEXT, as the DMA engine would.

use core::{
    cell::{Cell, RefCell},
    convert::Infallible,
    pin::Pin,
    task::{Context, Waker},
};
use std::{boxed::Box, vec::Vec};

use oer_esp32s31_hal::ieee80211::dma::{
    descriptor::{DESCRIPTOR_BYTES, rx_landed_word},
    rx_dma::model::{RxWalkerJournal, RxWalkerModel, RxWalkerOperation, SharedRxWalker},
    rx_ring::RxDmaArenaState,
    rx_storage::RxDmaStorage,
    space::HostDma,
};
use oer_esp32s31_hal::types::{MacInterruptEvents, MacInterruptObservation};
use oer_esp32s31_ieee80211_mac::{
    irq::{
        epoch::{EpochState, InterruptEpoch},
        model::{
            MacInterruptModel, MacInterruptOperation, PowerInterruptModel, PowerInterruptOperation,
        },
    },
    rx::{RxBufferFullCounter, RxRingError, pool::NetworkRxFrame, pool::RxStageTransactionError},
};
use oer_time_virtual::VirtualClock;

use super::*;
use crate::{
    irq::{DoorbellAcknowledge, HostActivation, WifiIrqPart, service_wifi_line},
    rx::{
        arena::{WifiRxArena, WifiRxStatic},
        control::ReceiveStopHandle,
        counters::{WifiRxContinuationReport, WifiRxReport, WifiRxTeardownReport},
        frontier::RxFrontierError,
        returns::{HeldRxFrame, RxReturnNotifier},
        sink::CountingAdmission,
        test_ring::CountingSink,
        transaction::{CompletedUnit, CreditPolicy, IngressClass, IngressRoute, Preview},
        upper::{FrameReceiver, QueueSink, RxFrameEnds},
    },
};

const COUNT: usize = 32;
const BUFFER: usize = 1_664;
const STORAGE: usize = BUFFER;
const SLOTS: usize = 32;

type HostLine<X = oer_espressif_interrupt_table::signal::host::Host> = WifiIrqLine<X>;
type HostPart = WifiIrqPart<HostLower, (), MacInterruptModel, PowerInterruptModel>;
type HostStatic = WifiRxStatic<
    COUNT,
    BUFFER,
    STORAGE,
    SLOTS,
    oer_espressif_interrupt_table::signal::host::Host,
    HostDma,
>;

/// The doorbell's lowering half of the host's part: [`HostDoorbell`]
/// records the lowering before it runs the handler.
struct HostLower;

impl DoorbellAcknowledge for HostLower {
    fn acknowledge(&mut self) {}
}
type HostStorage = RxDmaStorage<COUNT, BUFFER, STORAGE, HostDma>;

fn leak<T>(value: T) -> &'static mut T {
    Box::leak(Box::new(value))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Event {
    PromiscuousOn,
    PromiscuousOff,
    /// An entry of the handler activated the interrupt epoch.
    Activate,
    RuntimeResume,
    RuntimeStop,
    WalkerStop,
    Ring {
        released: usize,
    },
    Acknowledge,
    MacLine,
    PowerLine,
}

/// The hardware every host owner shares.
struct Host<X: SignalExecutor = oer_espressif_interrupt_table::signal::host::Host> {
    walker: RefCell<RxWalkerModel>,
    log: RefCell<Vec<Event>>,
    /// The radio handler's part: both bank models, closed until a request
    /// activates them.
    part: RefCell<HostPart>,
    /// The doorbell rings but its handler never runs.
    deaf: Cell<bool>,
    runtime_active: Cell<u8>,
    mac_started: Cell<bool>,
    core_prepared: Cell<bool>,
    cell: HandlerCell<HostLine<X>>,
}

impl<X: SignalExecutor> Host<X> {
    /// A leaked host whose walker reports to its event log.
    fn leaked() -> &'static Self {
        let host: &'static Self = leak(Self::new());
        host.walker.borrow_mut().set_journal(host);
        host
    }

    fn new() -> Self {
        Self {
            walker: RefCell::new(RxWalkerModel::new()),
            log: RefCell::new(Vec::new()),
            part: RefCell::new(WifiIrqPart::for_host_test(
                InterruptEpoch::new(MacInterruptModel::masked(), PowerInterruptModel::masked()),
                (),
                HostLower,
            )),
            deaf: Cell::new(false),
            runtime_active: Cell::new(0),
            mac_started: Cell::new(false),
            core_prepared: Cell::new(false),
            cell: HandlerCell::new(),
        }
    }

    fn record(&self, event: Event) {
        self.log.borrow_mut().push(event);
    }

    fn take_log(&self) -> Vec<Event> {
        core::mem::take(&mut *self.log.borrow_mut())
    }

    fn part<R>(&self, f: impl FnOnce(&mut HostPart) -> R) -> R {
        f(&mut self.part.borrow_mut())
    }

    fn mac(&self) -> MacInterruptModel {
        self.part(|part| part.epoch().banks().0.clone())
    }

    fn power(&self) -> PowerInterruptModel {
        self.part(|part| part.epoch().banks().1.clone())
    }

    /// Whether the handler masked both banks of the epoch.
    fn masked(&self) -> bool {
        self.part(|part| part.epoch().state() == EpochState::Masked)
    }

    /// One entry of the radio handler, as the line's vector runs it.
    fn enter(&self) {
        let activated = self.part(|part| {
            let before = part.epoch().state();
            service_wifi_line(&self.cell, part).expect("healthy interrupt");
            before != EpochState::Active && part.epoch().state() == EpochState::Active
        });
        if activated {
            self.record(Event::Activate);
        }
    }

    /// The radio interrupt level: while a bank asserts the line, run the
    /// handler; each entry serves the MAC bank first.
    fn p3(&self) {
        for _ in 0..8 {
            let (mac, power) = self.part(|part| {
                let (mac, power) = part.epoch().banks();
                (mac.line_asserted(), power.line_asserted())
            });
            if mac {
                self.record(Event::MacLine);
            } else if power {
                self.record(Event::PowerLine);
            } else {
                return;
            }
            self.enter();
        }
    }
}

struct HostRadio<X: SignalExecutor = oer_espressif_interrupt_table::signal::host::Host> {
    host: &'static Host<X>,
}

impl<X: SignalExecutor> RxBufferFullCounter for HostRadio<X> {
    fn buffer_full_count(&mut self) -> Option<u16> {
        None
    }
}

/// The walker's stops go into the host's event log, ordered among the MAC
/// events.
impl<X: SignalExecutor> RxWalkerJournal for Host<X> {
    fn record(&self, operation: RxWalkerOperation) {
        if matches!(operation, RxWalkerOperation::WriteDescriptorBase(_)) {
            assert!(self.core_prepared.get(), "BASE requires the MAC core edge");
        }
        if operation == RxWalkerOperation::Enable {
            assert!(
                self.core_prepared.get(),
                "the walker needs the prepared core clocked"
            );
        }
        if operation == RxWalkerOperation::Stop {
            Host::record(self, Event::WalkerStop);
        }
    }
}

impl<X: SignalExecutor> MacSnifferHardware for HostRadio<X> {
    fn configure_open_promiscuous_receive(&mut self) {
        self.host.record(Event::PromiscuousOn);
    }

    fn disable_open_promiscuous_receive(&mut self) {
        self.host.record(Event::PromiscuousOff);
    }
}

impl<X: SignalExecutor> MacRuntimeStopHardware for HostRadio<X> {
    fn request_mac_runtime_stop(&mut self) {
        self.host.record(Event::RuntimeStop);
    }

    fn mac_runtime_active_state(&mut self) -> u8 {
        self.host.runtime_active.get()
    }

    fn resume_mac_runtime(&mut self) {
        self.host.mac_started.set(self.host.core_prepared.get());
        self.host.record(Event::RuntimeResume);
    }
}

impl<X: SignalExecutor> MacRuntimeStartHardware for HostRadio<X> {
    fn prepare_mac_runtime<'a, const N: usize>(
        &mut self,
        ring: oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingPrepared<'a, N>,
    ) -> oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingCoreReady<'a, N> {
        assert!(!self.host.walker.borrow().walker_enabled);
        assert!(ring.topology_snapshot().valid);
        self.host.core_prepared.set(true);
        ring.core_ready_for_test()
    }

    fn start_mac_runtime<const N: usize>(
        &mut self,
        _ring: &oer_esp32s31_hal::ieee80211::dma::rx_ring::RxRingLive<'_, N>,
    ) {
        assert!(self.host.walker.borrow().walker_enabled);
        assert!(self.host.core_prepared.get());
        self.resume_mac_runtime();
    }
}

struct HostDoorbell<X: SignalExecutor = oer_espressif_interrupt_table::signal::host::Host> {
    host: &'static Host<X>,
    rx: &'static HostStorage,
}

impl<X: SignalExecutor> RadioDoorbell for HostDoorbell<X> {
    const HANDLER: oer_espressif_interrupt_table::Context =
        oer_espressif_interrupt_table::Context { level: 3, hart: 0 };
    const EXECUTOR: oer_espressif_interrupt_table::Context =
        oer_espressif_interrupt_table::Context { level: 2, hart: 0 };
    fn ring(&self) {
        self.host.record(Event::Ring {
            released: self.rx.released_buffer_count(),
        });
        if self.host.deaf.get() {
            return;
        }
        self.host.record(Event::Acknowledge);
        self.host.enter();
        // The level then serves whatever the handler unmasked.
        self.host.p3();
    }
}

/// A doorbell nobody listens to, for requests made before an epoch.
struct Silent;

impl RadioDoorbell for Silent {
    const HANDLER: oer_espressif_interrupt_table::Context =
        oer_espressif_interrupt_table::Context { level: 3, hart: 0 };
    const EXECUTOR: oer_espressif_interrupt_table::Context =
        oer_espressif_interrupt_table::Context { level: 2, hart: 0 };
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

struct Harness<X: SignalExecutor = oer_espressif_interrupt_table::signal::host::Host> {
    host: &'static Host<X>,
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
    harness_staged::<SLOTS, E, M>(|counters, _, _, _| owners(counters))
}

/// The host executor's signals, each serviced at its raise.
fn host_signals()
-> crate::rx::arena::WifiIrqSignals<oer_espressif_interrupt_table::signal::host::Host> {
    crate::rx::arena::WifiIrqSignals {
        mac: oer_espressif_interrupt_table::signal::host::signal(),
        power: oer_espressif_interrupt_table::signal::host::signal(),
    }
}

/// A harness whose arena has `STAGE` staging slots; `owners` also gets the
/// upper layer's return notifier, the staged-frame queue's ends and the
/// clock.
fn harness_staged<const STAGE: usize, E, M>(
    owners: impl FnOnce(
        &'static WifiRxCounters,
        RxReturnNotifier<'static, oer_espressif_interrupt_table::signal::host::Host>,
        RxFrameEnds<'static, STAGE, BUFFER, oer_espressif_interrupt_table::signal::host::Host>,
        &'static VirtualClock,
    ) -> (E, M),
) -> Harness
where
    E: Publisher<'static, BUFFER, STAGE> + 'static,
    M: Admission + 'static,
{
    harness_staged_with(host_signals(), owners)
}

fn harness_staged_with<const STAGE: usize, E, M, X: SignalExecutor>(
    signals: crate::rx::arena::WifiIrqSignals<X>,
    owners: impl FnOnce(
        &'static WifiRxCounters,
        RxReturnNotifier<'static, X>,
        RxFrameEnds<'static, STAGE, BUFFER, X>,
        &'static VirtualClock,
    ) -> (E, M),
) -> Harness<X>
where
    E: Publisher<'static, BUFFER, STAGE> + 'static,
    M: Admission + 'static,
{
    let host = Host::leaked();
    let arena: &'static mut WifiRxStatic<COUNT, BUFFER, STORAGE, STAGE, X, HostDma> =
        leak(bytemuck::Zeroable::zeroed());
    let (parts, stop, frames) =
        WifiRxArena::bind(oer_esp32s31_hal::dma_static::Placed::host(arena), signals)
            .expect("a zeroed arena binds");
    let (rx, base) = (parts.rx(), parts.descriptor_base());
    let (requests, counters) = (parts.requests(), parts.counters());
    let clock: &'static VirtualClock = leak(VirtualClock::new());
    let (publisher, admission) = owners(counters, parts.returns(), frames, clock);
    let (publisher, admission) = (&*leak(publisher), &*leak(admission));
    let hardware = leak(HostRadio { host });
    let doorbell = &*leak(HostDoorbell { host, rx });
    let walker = leak(SharedRxWalker(&host.walker));
    let runner = Box::pin(receive(
        hardware,
        walker,
        &HostActivation,
        parts,
        &host.cell,
        doorbell,
        clock,
        publisher,
        admission,
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

impl<X: SignalExecutor> Harness<X> {
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
        // A cold walker must finish its reserved unit before a distinct
        // frame can start wholly at the ring head.
        if self.host.walker.borrow().next_descriptor_low == self.reserved_low() {
            self.land_in_reserved(4);
        }
        for (step, &length) in lengths.iter().enumerate() {
            let index = (self.next + step) % COUNT;
            let terminal = step + 1 == lengths.len();
            self.rx.descriptors()[index].write_word0(rx_landed_word(
                BUFFER as u32,
                length,
                terminal,
            ));
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
            .part(|part| part.epoch_mut().banks_mut().0.latch(rx_success()));
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

    /// Bring the epoch up and forget its bring-up log, the banks' too.
    fn started(mut self) -> Self {
        self.settle();
        assert!(self.host.walker.borrow().walker_enabled);
        self.host.take_log();
        self.host.walker.borrow_mut().clear_operations();
        self.host.part(|part| {
            let (mac, power) = part.epoch_mut().banks_mut();
            mac.clear_operations();
            power.clear_operations();
        });
        self
    }
}

#[test]
fn cold_start_clocks_the_prepared_core_but_holds_rx_until_walker_is_live() {
    let mut harness = harness();
    harness.poll_once();
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::Prepared);
    assert!(!harness.host.walker.borrow().walker_enabled);
    assert!(!harness.host.mac_started.get());
    assert!(harness.host.core_prepared.get());
    assert!(!harness.host.take_log().contains(&Event::RuntimeResume));

    harness.settle();
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::Live);
    assert!(harness.host.mac_started.get());
    assert_eq!(
        harness
            .host
            .walker
            .borrow()
            .operation_count(RxWalkerOperation::Enable),
        1
    );
    assert_eq!(harness.host.take_log(), [Event::RuntimeResume]);
    assert!(harness.stop.try_exit().is_none());
}

#[test]
fn cold_start_never_opens_mac_when_walker_enable_is_unconfirmed() {
    let mut harness = harness();
    harness.poll_once();
    // The walker no longer agrees with the stopped capability: the enable
    // model refuses a second enable, so no live-ring proof is returned.
    harness.host.walker.borrow_mut().walker_enabled = true;
    harness.settle();
    assert!(!harness.host.mac_started.get());
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::ResetRequired);
    assert!(!harness.host.take_log().contains(&Event::RuntimeResume));
}

#[test]
fn the_handler_activates_the_epoch_after_the_install() {
    let mut harness = harness();
    // A request left by an earlier epoch is cleared by the install.
    harness.requests.ring(WifiIrqRequest::MaskAll, &Silent);
    harness.settle();
    assert_eq!(
        harness.host.take_log(),
        [
            Event::PromiscuousOn,
            Event::Ring { released: 0 },
            Event::Acknowledge,
            Event::Activate,
            Event::RuntimeResume,
        ]
    );
    // The walker started between the activation and the runtime resume.
    let walker = harness.host.walker.borrow();
    assert!(walker.walker_enabled);
    assert_eq!(walker.operation_count(RxWalkerOperation::Enable), 1);
    drop(walker);
    // The activation alone touched the banks; the stale mask was not
    // applied.
    assert!(harness.host.mac().operations().matches(&[
        MacInterruptOperation::Raw,
        MacInterruptOperation::MaskAll,
        MacInterruptOperation::Raw,
        MacInterruptOperation::AcknowledgeRaw,
        MacInterruptOperation::Publish(oer_esp32s31_hal::types::MacInterruptMask::STATION),
    ]));
    assert!(harness.host.mac().rx_delivery_enabled());
    assert!(!harness.requests.is_pending());
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
        MacInterruptOperation::Raw,
        MacInterruptOperation::MaskRxDelivery,
        MacInterruptOperation::Acknowledge(rx_success()),
        MacInterruptOperation::Raw,
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
        oer_esp32s31_hal::ieee80211::dma::descriptor::rx_armed_word(BUFFER as u32).unwrap()
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
    let armed = oer_esp32s31_hal::ieee80211::dma::descriptor::rx_armed_word(BUFFER as u32).unwrap();
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
        assert_eq!(harness.report().continuations.immediate, poll);
        assert_eq!(harness.clock.next_deadline(), None);
    }
    harness.poll_once();
    let report = harness.report();
    assert_eq!(
        (report.continuations.immediate, report.continuations.timed),
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
    assert!(!harness.host.masked());
}

#[test]
fn teardown_is_mask_close_drain_stop_once() {
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
                reason: WifiRxQuarantine::StopRefused(RxFrontierError::Ring(
                    RxRingError::WalkerStopUnconfirmed
                )),
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
            Event::PromiscuousOff,
            Event::RuntimeStop,
            Event::WalkerStop,
        ]
    );
    assert!(harness.host.masked());
    // Poisoned before the stop: the arena waits for a radio reset.
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::ResetRequired);
    let report = harness.report();
    assert_eq!((report.service_errors, report.doorbell_unmasks), (1, 0));
}

impl<X: SignalExecutor> Harness<X> {
    /// The reserved descriptor's low address, as the walker reports it.
    fn reserved_low(&self) -> u32 {
        (self.base + COUNT as u32 * oer_esp32s31_hal::ieee80211::dma::descriptor::DESCRIPTOR_BYTES)
            & 0x000f_ffff
    }

    /// The walker writes a frame of `length` bytes into the reserved
    /// descriptor and moves to its link.
    fn land_in_reserved(&self, length: u32) {
        let reserved = self.rx.reserved_descriptor();
        reserved.write_word0(
            oer_esp32s31_hal::ieee80211::dma::descriptor::rx_landed_word(
                BUFFER as u32,
                length,
                true,
            ),
        );
        let mut walker = self.host.walker.borrow_mut();
        walker.last_descriptor_low = self.reserved_low();
        walker.next_descriptor_low = reserved.next_address() & 0x000f_ffff;
    }
}

#[test]
fn the_epochs_first_frame_in_the_reserved_descriptor_is_counted_and_dropped() {
    let mut harness = harness().started();
    assert_eq!(
        harness.host.walker.borrow().next_descriptor_low,
        harness.reserved_low(),
        "the started walker fetches the reserved descriptor"
    );
    harness.land_in_reserved(80);
    harness.rx_event();
    harness.settle();
    let report = harness.report();
    assert_eq!((report.units, report.reload.reserved_landings), (0, 1));
    assert_eq!(report.service_errors, 0);
    assert!(harness.host.mac().rx_delivery_enabled());
    // The next frame lands in the ring head and is received.
    harness.frame(200);
    harness.settle();
    let report = harness.report();
    assert_eq!((report.units, report.bytes), (1, 200));
    assert_eq!(
        (
            report.reload.reserved_landings,
            report.reload.reserved_late_landings,
            report.reload.base_writes
        ),
        (1, 0, 1)
    );
    assert_eq!(harness.rx.reserved_descriptor().next_address(), 0);
}

#[test]
fn a_late_frame_in_the_reserved_descriptor_quarantines_the_epoch() {
    let mut harness = harness().started();
    harness.frame(200);
    harness.settle();
    assert_eq!(harness.report().units, 1);
    // After the walker reached the ring nothing names R; a write into it
    // fails closed.
    harness.land_in_reserved(80);
    harness.rx_event();
    harness.settle();
    let report = harness.report();
    assert_eq!(report.reload.reserved_late_landings, 1);
    assert_eq!(report.service_errors, 1);
    assert_eq!(
        harness.stop.try_exit().map(|exit| exit.end),
        Some(WifiRxEnd::Quarantined {
            reason: WifiRxQuarantine::Fault,
            walker_stopped: true,
        })
    );
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::ResetRequired);
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
            Event::PromiscuousOff,
            Event::RuntimeStop,
        ]
    );
    assert_eq!(harness.walker_stops(), 0);
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn cold_start_refuses_an_occupied_cell_without_opening_mac() {
    let mut first = harness().started();
    // A second epoch on the same cell: its line is refused and no epoch is
    // activated for it.
    let cell = &first.host.cell;
    let host: &Host = Host::leaked();
    let arena: &'static mut HostStatic = leak(bytemuck::Zeroable::zeroed());
    let (parts, mut stop, _frames) = WifiRxArena::bind(
        oer_esp32s31_hal::dma_static::Placed::host(arena),
        crate::rx::arena::WifiIrqSignals {
            mac: oer_espressif_interrupt_table::signal::host::signal(),
            power: oer_espressif_interrupt_table::signal::host::signal(),
        },
    )
    .unwrap();
    let counters = parts.counters();
    let mut runner = Box::pin(receive(
        leak(HostRadio { host }),
        leak(SharedRxWalker(&host.walker)),
        &HostActivation,
        parts,
        cell,
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
    assert!(!host.mac_started.get());
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
    assert_eq!(report.reload.max_reload_attempts, 1);
}

const STAGE: usize = 8;

type Returned<const N: usize = STAGE> = RefCell<
    Vec<
        HeldRxFrame<'static, 'static, N, BUFFER, oer_espressif_interrupt_table::signal::host::Host>,
    >,
>;

/// An upper layer that holds its leases while `holding`, through the
/// notifier, as a network queue would; otherwise it drops them at once.
struct ReturningSink<const N: usize = STAGE> {
    held: &'static Returned<N>,
    holding: &'static Cell<bool>,
    returns: RxReturnNotifier<'static, oer_espressif_interrupt_table::signal::host::Host>,
    counters: &'static WifiRxCounters,
}

impl<const N: usize> Publisher<'static, BUFFER, N> for ReturningSink<N> {
    const DEPTH: usize = N;

    fn free_capacity(&self) -> usize {
        N - self.held.borrow().len()
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
        frame: NetworkRxFrame<'static, N, BUFFER>,
    ) -> Result<(), NetworkRxFrame<'static, N, BUFFER>> {
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
    RxReturnNotifier<'static, oer_espressif_interrupt_table::signal::host::Host>,
) {
    returning_harness_with::<STAGE>()
}

/// [`returning_harness`] with `N` staging slots.
fn returning_harness_with<const N: usize>() -> (
    Harness,
    &'static Returned<N>,
    &'static Cell<bool>,
    RxReturnNotifier<'static, oer_espressif_interrupt_table::signal::host::Host>,
) {
    let held: &'static Returned<N> = leak(RefCell::new(Vec::new()));
    let holding: &'static Cell<bool> = leak(Cell::new(true));
    let mut notifier = None;
    let harness = harness_staged::<N, _, _>(|counters, returns, _, _| {
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
            .all(|descriptor| descriptor.word0().rx_done())
    );
    assert_eq!(harness.host.walker.borrow().next_descriptor_low, 0);
    assert!(!harness.host.mac().line_asserted());
    assert!(!harness.host.mac().rx_delivery_enabled());
    harness.host.take_log();

    // The upper layer returns its leases; nothing else happens.
    harness.host.walker.borrow_mut().clear_operations();
    holding.set(false);
    held.borrow_mut().clear();
    harness.settle();
    let log = harness.host.take_log();
    assert_eq!(mac_lines(&log), 0, "the wake came from the return");
    // The runner rearmed the returned prefix behind the exhausted tail and
    // rang the doorbell, which resumed the walker at the prefix's head: no
    // BASE write, no manual cursor.
    let report = harness.report();
    assert_eq!(report.units, COUNT as u32);
    let armed = oer_esp32s31_hal::ieee80211::dma::descriptor::rx_armed_word(BUFFER as u32).unwrap();
    for descriptor in &harness.rx.descriptors()[..STAGE] {
        assert_eq!(descriptor.word0(), armed);
    }
    let walker = harness.host.walker.borrow();
    assert!(
        !walker
            .operations()
            .iter()
            .any(|operation| matches!(operation, RxWalkerOperation::WriteDescriptorBase(_)))
    );
    assert_eq!(walker.next_descriptor_low, harness.base & 0x000f_ffff);
    drop(walker);
    // RX is unmasked in the same settle.
    assert!(log.contains(&Event::Acknowledge));
    assert!(harness.host.mac().rx_delivery_enabled());
    assert_eq!(report.service_errors, 0);
    assert_eq!(
        (report.reload.reload_rerings, report.reload.base_writes),
        (0, 1)
    );

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
    assert!(!harness.host.masked());

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

/// A harness whose publisher is the product queue; the application's end
/// of the queue is returned beside it.
fn queue_harness() -> (
    Harness,
    FrameReceiver<'static, STAGE, BUFFER, oer_espressif_interrupt_table::signal::host::Host>,
) {
    let mut receiver = None;
    let harness = harness_staged::<STAGE, _, _>(|counters, _, frames, clock| {
        let RxFrameEnds {
            sender,
            receiver: frames,
        } = frames;
        receiver = Some(frames);
        (
            QueueSink::new(sender, clock),
            CountingAdmission::new(counters),
        )
    })
    .started();
    (harness, receiver.expect("the harness lends the receiver"))
}

/// Poll the application's receive once.
fn poll_next(
    receiver: &mut FrameReceiver<
        'static,
        STAGE,
        BUFFER,
        oer_espressif_interrupt_table::signal::host::Host,
    >,
) -> Option<
    HeldRxFrame<'static, 'static, STAGE, BUFFER, oer_espressif_interrupt_table::signal::host::Host>,
> {
    let mut next = core::pin::pin!(receiver.next());
    match next.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(frame) => Some(frame),
        Poll::Pending => None,
    }
}

#[test]
fn a_full_queue_blocks_and_a_p1_drop_wakes_the_runner() {
    let (mut harness, mut frames) = queue_harness();
    // One unit more than the queue holds while the application takes none.
    for _ in 0..=STAGE {
        harness.complete_unit(&[64]);
    }
    harness.rx_event();
    harness.settle();
    let report = harness.report();
    assert_eq!(report.units, STAGE as u32);
    assert!(report.stage_capacity_blocks >= 1);
    assert!(!harness.host.mac().rx_delivery_enabled());
    harness.host.take_log();

    // The application takes one frame and drops it: the return alone wakes
    // the runner, which stages the unit that waited and unmasks.
    let held = poll_next(&mut frames).expect("a queued frame");
    harness.settle();
    assert_eq!(
        harness.report().units,
        STAGE as u32,
        "a held frame keeps its slot"
    );
    drop(held);
    harness.settle();
    assert_eq!(harness.report().units, STAGE as u32 + 1);
    let log = harness.host.take_log();
    assert_eq!(mac_lines(&log), 0, "the wake came from the return");
    assert!(log.contains(&Event::Acknowledge));
    assert!(harness.host.mac().rx_delivery_enabled());
    assert_eq!(harness.report().service_errors, 0);
}

#[test]
fn teardown_halts_after_p1_drains_queued_frames() {
    let (mut harness, mut frames) = queue_harness();
    for _ in 0..3 {
        harness.complete_unit(&[64]);
    }
    harness.rx_event();
    harness.settle();
    assert_eq!(harness.report().units, 3);

    // The queued frames are staged: no stop while they are out.
    harness.stop.request_stop();
    harness.settle();
    assert!(harness.stop.try_exit().is_none());
    assert_eq!(harness.walker_stops(), 0);

    // The application keeps draining after its stop request.
    let mut drained = 0;
    while let Some(held) = poll_next(&mut frames) {
        drop(held);
        drained += 1;
    }
    assert_eq!(drained, 3);
    harness.settle();
    assert_eq!(harness.walker_stops(), 1);
    assert_eq!(
        harness.stop.try_exit(),
        Some(WifiRxExit {
            cause: WifiRxCause::StopRequested,
            end: WifiRxEnd::Halted,
        })
    );
}

#[test]
fn teardown_quarantines_when_p1_never_drains() {
    let (mut harness, mut frames) = queue_harness();
    for _ in 0..3 {
        harness.complete_unit(&[64]);
    }
    harness.rx_event();
    harness.settle();

    harness.stop.request_stop();
    let exit = (0..64).find_map(|_| {
        harness.settle();
        harness.stop.try_exit()
    });
    assert_eq!(
        exit,
        Some(WifiRxExit {
            cause: WifiRxCause::StopRequested,
            end: WifiRxEnd::Quarantined {
                reason: WifiRxQuarantine::DrainTimeout,
                walker_stopped: false,
            },
        })
    );
    assert_eq!(harness.walker_stops(), 0);
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::ResetRequired);
    // The drain waited its whole bound, one poll at a time.
    let polls = DRAIN_TIMEOUT.as_micros() / DRAIN_POLL.as_micros();
    assert_eq!(
        harness.report().teardown,
        WifiRxTeardownReport {
            mask_confirm_us: 0,
            drain_us: DRAIN_TIMEOUT.as_micros() as u32,
            drain_polls: polls as u32,
        }
    );
    // The frames stay queued with their leases.
    assert!(poll_next(&mut frames).is_some());
}

#[test]
fn a_return_to_a_full_pool_snapshots_the_walker_before_its_drain() {
    let (mut harness, held, _, _) = returning_harness();
    // The upper layer takes and keeps every staging slot; nothing completes
    // after that, so nothing blocks either.
    for _ in 0..STAGE {
        harness.complete_unit(&[64]);
    }
    harness.rx_event();
    harness.settle();
    assert_eq!(held.borrow().len(), STAGE);
    assert_eq!(harness.report().stage_capacity_blocks, 0);
    assert_eq!(harness.report().exhaustion.snapshots, 0);
    let (next_low, last_low) = {
        let walker = harness.host.walker.borrow();
        (walker.next_descriptor_low, walker.last_descriptor_low)
    };

    // A second of hold, then one lease comes back.
    let release_at =
        oer_time::Clock::now(harness.clock).saturating_add(oer_time::Duration::from_millis(1_000));
    harness.clock.advance_to(release_at);
    harness.host.walker.borrow_mut().clear_operations();
    held.borrow_mut().pop();
    harness.poll_once();
    let exhaustion = harness.report().exhaustion;
    assert_eq!(exhaustion.snapshots, 1);
    let snapshot = exhaustion.longest.expect("the return took a snapshot");
    assert!(snapshot.blocked_us >= 1_000_000, "{snapshot:?}");
    assert_eq!(
        (snapshot.next_low, snapshot.next_upper, snapshot.last_low),
        (next_low, false, last_low)
    );
    // BASE names the reserved descriptor after the ring.
    assert_eq!(
        snapshot.base,
        Some(
            harness.base
                + COUNT as u32 * oer_esp32s31_hal::ieee80211::dma::descriptor::DESCRIPTOR_BYTES
        )
    );
    assert!(snapshot.walker_enabled);
    assert!(!snapshot.reload_doorbell);
    assert_eq!(snapshot.ring_base, harness.base);
    // The state of the full pool, one lease already back.
    assert_eq!(snapshot.staged, STAGE as u32 - 1);
    assert_eq!(snapshot.detached, STAGE as u32 - 1);
    // The snapshot read before the drain touched anything.
    let operations = harness.host.walker.borrow().operations().to_vec();
    assert_eq!(
        operations.get(..2),
        Some(
            &[
                RxWalkerOperation::ObserveNext,
                RxWalkerOperation::ObserveLast
            ][..]
        ),
        "{operations:?}"
    );

    // A return to a pool that is no longer full takes no snapshot.
    harness.settle();
    held.borrow_mut().pop();
    harness.settle();
    assert_eq!(harness.report().capacity_drains, 2);
    assert_eq!(harness.report().exhaustion.snapshots, 1);
}

/// Hold every staging slot of a returning harness for `hold_ms`.
fn hold_full_pool(harness: &mut Harness, held: &Returned, hold_ms: u32) {
    for _ in 0..STAGE {
        harness.complete_unit(&[64]);
    }
    harness.rx_event();
    harness.settle();
    assert_eq!(held.borrow().len(), STAGE);
    let release_at = oer_time::Clock::now(harness.clock)
        .saturating_add(oer_time::Duration::from_millis(hold_ms));
    harness.clock.advance_to(release_at);
}

#[test]
fn a_snapshot_keeps_the_next_its_first_passs_reload_read() {
    let (mut harness, held, _, _) = returning_harness();
    hold_full_pool(&mut harness, held, 1_000);
    let next_low = harness.host.walker.borrow().next_descriptor_low;
    assert_ne!(next_low, 0);
    // The oldest lease comes back: it heads the released prefix, so the
    // drain's first pass appends it and completes a reload, which reads
    // NEXT (nonzero: LAST is not read).
    let before = harness.report().reload.observations;
    held.borrow_mut().remove(0);
    harness.settle();
    let report = harness.report();
    assert_eq!(report.reload.observations, before + 1);
    let snapshot = report
        .exhaustion
        .latest
        .expect("the return took a snapshot");
    assert_eq!(snapshot.next_after_doorbell, Some(next_low));
    assert_eq!(snapshot.last_after_doorbell, None);
    // Nothing wrote a descriptor while the pool was full.
    assert!(!snapshot.descriptors_changed);
    assert_eq!(report.exhaustion.longest, Some(snapshot));
}

#[test]
fn a_snapshot_without_a_reload_in_its_first_pass_has_no_doorbell_read() {
    let (mut harness, held, _, _) = returning_harness();
    hold_full_pool(&mut harness, held, 1_000);
    // The newest lease comes back: it is not at the recycle frontier, so
    // the first pass appends nothing and completes no reload.
    let before = harness.report().reload.observations;
    held.borrow_mut().pop();
    harness.settle();
    let report = harness.report();
    assert_eq!(report.reload.observations, before);
    let snapshot = report
        .exhaustion
        .latest
        .expect("the return took a snapshot");
    assert_eq!(
        (snapshot.next_after_doorbell, snapshot.last_after_doorbell),
        (None, None)
    );
}

#[test]
fn a_snapshot_of_an_exhausted_walker_keeps_what_its_doorbell_resumed() {
    let (mut harness, held, _, _) = returning_harness();
    // Every descriptor completes behind the held leases and the walker
    // reaches NEXT = 0 at the last one.
    for _ in 0..COUNT {
        harness.complete_unit(&[64]);
    }
    harness
        .host
        .walker
        .borrow_mut()
        .set_cursor(harness.base, COUNT - 1, None);
    harness.rx_event();
    for _ in 0..4 {
        harness.settle();
    }
    assert_eq!(held.borrow().len(), STAGE);
    let last_low = harness.host.walker.borrow().last_descriptor_low;
    held.borrow_mut().remove(0);
    harness.settle();
    let snapshot = harness
        .report()
        .exhaustion
        .latest
        .expect("the return took a snapshot");
    // The registers before the drain: the exhausted walker.
    assert_eq!((snapshot.next_low, snapshot.last_low), (0, last_low));
    // The first pass's doorbell, rung with NEXT = 0, resumed the walker at
    // the returned head: its settle read that NEXT and no LAST.
    assert_eq!(
        (snapshot.next_after_doorbell, snapshot.last_after_doorbell),
        (Some(harness.base & 0x000f_ffff), None)
    );
    assert_eq!(harness.report().reload.reload_rerings, 0);
}

#[test]
fn a_lost_doorbell_is_rerung_and_the_snapshot_keeps_the_resumed_next() {
    let (mut harness, held, _, _) = returning_harness();
    for _ in 0..COUNT {
        harness.complete_unit(&[64]);
    }
    harness
        .host
        .walker
        .borrow_mut()
        .set_cursor(harness.base, COUNT - 1, None);
    harness.rx_event();
    for _ in 0..4 {
        harness.settle();
    }
    harness.host.walker.borrow_mut().lose_reloads = 1;
    held.borrow_mut().remove(0);
    harness.settle();
    let report = harness.report();
    // The settle saw NEXT = 0 with LAST short of the new tail and rang
    // once more; the second doorbell resumed the walker.
    assert_eq!(
        (report.reload.reload_rerings, report.reload.rering_failures),
        (1, 0)
    );
    let snapshot = report
        .exhaustion
        .latest
        .expect("the return took a snapshot");
    assert_eq!(
        snapshot.next_after_doorbell,
        Some(harness.base & 0x000f_ffff)
    );
    assert_eq!(report.service_errors, 0);
    assert_eq!(report.reload.base_writes, 1);
}

#[test]
fn a_descriptor_write_while_the_pool_is_full_marks_the_snapshot() {
    let (mut harness, held, _, _) = returning_harness();
    hold_full_pool(&mut harness, held, 1_000);
    // A word of a held lease's descriptor changes during the hold.
    harness.rx.descriptors()[3].write_word0(
        oer_esp32s31_hal::ieee80211::dma::descriptor::rx_landed_word(BUFFER as u32, 100, true),
    );
    held.borrow_mut().pop();
    harness.settle();
    let snapshot = harness
        .report()
        .exhaustion
        .latest
        .expect("the return took a snapshot");
    assert!(snapshot.descriptors_changed);
}

#[test]
fn the_latest_snapshot_follows_every_return_and_the_longest_keeps_the_longest() {
    let (mut harness, held, _, _) = returning_harness();
    hold_full_pool(&mut harness, held, 1_000);
    held.borrow_mut().remove(0);
    harness.settle();
    let first = harness.report().exhaustion.latest.expect("a snapshot");
    // One more frame fills the pool again; a shorter hold, then the newest
    // lease comes back.
    harness.frame(80);
    harness.settle();
    assert_eq!(held.borrow().len(), STAGE);
    let release_at =
        oer_time::Clock::now(harness.clock).saturating_add(oer_time::Duration::from_millis(500));
    harness.clock.advance_to(release_at);
    held.borrow_mut().pop();
    harness.settle();
    let exhaustion = harness.report().exhaustion;
    assert_eq!(exhaustion.snapshots, 2);
    let latest = exhaustion.latest.expect("a snapshot");
    assert!(latest.blocked_us < first.blocked_us, "{latest:?} {first:?}");
    assert_eq!(latest.next_after_doorbell, None);
    assert_eq!(exhaustion.longest, Some(first));
}

#[test]
fn a_drain_end_records_the_buffers_and_the_walker_after_the_drain() {
    let (mut harness, held, _, _) = returning_harness();
    assert_eq!(harness.report().drain_end, None);
    harness.frame(100);
    harness.settle();
    let live = harness.report().drain_end.expect("a drain ended");
    assert_eq!((live.detached, live.staged, live.released), (1, 1, 0));
    let (next_low, last_low) = {
        let walker = harness.host.walker.borrow();
        (walker.next_descriptor_low, walker.last_descriptor_low)
    };
    assert_ne!(next_low, 0);
    assert_eq!(
        (live.next_low, live.next_upper, live.last_low),
        (next_low, false, last_low)
    );

    // Every descriptor completes and the walker runs out at the last one.
    for _ in 1..COUNT {
        harness.complete_unit(&[64]);
    }
    harness
        .host
        .walker
        .borrow_mut()
        .set_cursor(harness.base, COUNT - 1, None);
    let last_low = harness.host.walker.borrow().last_descriptor_low;
    harness.rx_event();
    for _ in 0..4 {
        harness.settle();
    }
    assert_eq!(held.borrow().len(), STAGE);
    let exhausted = harness.report().drain_end.expect("a drain ended");
    assert_eq!(
        (exhausted.detached, exhausted.staged),
        (STAGE as u32, STAGE as u32)
    );
    assert_eq!((exhausted.next_low, exhausted.last_low), (0, last_low));
    assert_eq!(exhausted.recycle_start, 0);
}

#[test]
fn a_walker_restart_counts_no_republication_one_capacity_drain_and_frames_after() {
    let (mut harness, held, holding, _) = returning_harness();
    // Every lease is held while the ring fills behind them and the walker
    // reaches NEXT = 0.
    for _ in 0..COUNT {
        harness.complete_unit(&[64]);
    }
    harness
        .host
        .walker
        .borrow_mut()
        .set_cursor(harness.base, COUNT - 1, None);
    harness.rx_event();
    for _ in 0..4 {
        harness.settle();
    }
    let before = harness.report();
    assert_eq!(before.units, STAGE as u32);
    assert_eq!(before.capacity_drains, 0);
    assert_eq!(
        (before.reload.base_writes, before.reload.reload_rerings),
        (1, 0)
    );
    assert_eq!(harness.host.walker.borrow().next_descriptor_low, 0);

    // The release is one capacity drain; its doorbell restarts the walker
    // at the returned head. BASE is not written again.
    holding.set(false);
    held.borrow_mut().clear();
    harness.settle();
    let released = harness.report();
    assert_eq!(released.capacity_drains, 1);
    assert_eq!(
        (released.reload.base_writes, released.reload.reload_rerings),
        (1, 0)
    );
    assert_eq!(
        harness.host.walker.borrow().next_descriptor_low,
        harness.base & 0x000f_ffff
    );

    // A frame lands after the release.
    harness.frame(150);
    harness.settle();
    let after = harness.report();
    assert_eq!(after.capacity_drains, 1);
    assert_eq!(
        (after.reload.base_writes, after.reload.reload_rerings),
        (1, 0)
    );
    assert_eq!(after.units, COUNT as u32 + 1);
    assert_eq!(after.service_errors, 0);
    assert!(harness.host.mac().rx_delivery_enabled());
}

#[test]
fn a_reverse_full_ring_release_ends_unmasked_with_frames_after() {
    let (mut harness, held, holding, _) = returning_harness_with::<COUNT>();
    // The upper layer holds a lease of every descriptor; the walker runs
    // out at the last one.
    for _ in 0..COUNT {
        harness.complete_unit(&[64]);
    }
    harness
        .host
        .walker
        .borrow_mut()
        .set_cursor(harness.base, COUNT - 1, None);
    harness.rx_event();
    for _ in 0..4 {
        harness.settle();
    }
    assert_eq!(held.borrow().len(), COUNT);
    assert_eq!(harness.rx.detached_buffer_count(), COUNT);
    harness.host.walker.borrow_mut().clear_operations();

    // The leases return newest first: nothing can rearm until the head
    // returns, and then the whole ring is released at once.
    holding.set(false);
    while held.borrow_mut().pop().is_some() {
        harness.settle();
    }
    assert_eq!(harness.rx.detached_buffer_count(), 0);
    // All but the walker's LAST went back through the doorbell, which
    // resumed the walker at the head.
    assert_eq!(harness.rx.released_buffer_count(), 1);
    assert_eq!(
        harness.host.walker.borrow().next_descriptor_low,
        harness.base & 0x000f_ffff
    );
    assert!(
        !harness
            .host
            .walker
            .borrow()
            .operations()
            .iter()
            .any(|operation| matches!(operation, RxWalkerOperation::WriteDescriptorBase(_)))
    );
    assert!(harness.host.mac().rx_delivery_enabled());

    // Frames land after: the first moves the walker on and the old LAST
    // rejoins.
    for length in [100, 120, 140] {
        harness.frame(length);
        harness.settle();
    }
    let report = harness.report();
    assert_eq!(report.units, COUNT as u32 + 3);
    assert_eq!(report.service_errors, 0);
    assert_eq!(
        (
            report.reload.base_writes,
            report.reload.reload_rerings,
            report.reload.reserved_late_landings
        ),
        (1, 0, 0)
    );
    assert!(harness.host.mac().rx_delivery_enabled());
    assert_eq!(
        harness.rx.released_buffer_count(),
        1,
        "the newest LAST waits"
    );
}

#[test]
fn a_rearm_continuation_counts_as_drained() {
    let mut harness = harness().started();
    harness.frame(200);
    harness.settle();
    // The first pass staged the unit and released its buffer; the one
    // continuation reran the pass to rearm it, and the drain unmasked.
    let continuations = harness.report().continuations;
    assert_eq!(
        continuations,
        WifiRxContinuationReport {
            immediate: 1,
            longest_streak: 1,
            drained: 1,
            ..WifiRxContinuationReport::default()
        }
    );
    assert_eq!(continuations.continued(DatapathRxProgress::Drained), 1);
}

#[test]
fn probe_continuations_count_their_reason_and_the_longest_streak() {
    let mut harness = harness().started();
    harness
        .host
        .walker
        .borrow_mut()
        .set_cursor(harness.base, COUNT - 1, None);
    harness.rx_event();
    harness.host.p3();
    for _ in 0..=IMMEDIATE_CONTINUATIONS {
        harness.poll_once();
    }
    let streak = IMMEDIATE_CONTINUATIONS + 1;
    assert_eq!(
        harness.report().continuations,
        WifiRxContinuationReport {
            immediate: IMMEDIATE_CONTINUATIONS,
            timed: 1,
            longest_streak: streak,
            probe_pending: streak,
            ..WifiRxContinuationReport::default()
        }
    );

    // The probe keeps one drain running until the walker moves on: every
    // continuation is that drain's, so its streak is their count.
    harness.settle();
    harness.host.walker.borrow_mut().next_descriptor_low = harness.base & 0x000f_ffff;
    harness.settle();
    assert!(harness.host.mac().rx_delivery_enabled());
    let continuations = harness.report().continuations;
    let probes = continuations.continued(DatapathRxProgress::ProbePending);
    assert!(probes > streak);
    assert_eq!(continuations.longest_streak, probes);
    assert_eq!(continuations.immediate + continuations.timed, probes);
    assert_eq!(
        continuations.continued(DatapathRxProgress::StageCapacityBlocked),
        0
    );
}

#[test]
fn teardown_measures_no_wait_when_the_mask_confirms_and_nothing_is_staged() {
    let mut harness = harness().started();
    harness.frame(200);
    harness.settle();
    harness.stop.request_stop();
    harness.settle();
    assert_eq!(
        harness.stop.try_exit().map(|exit| exit.end),
        Some(WifiRxEnd::Halted)
    );
    assert_eq!(harness.report().teardown, WifiRxTeardownReport::default());
}

#[test]
fn teardown_measures_the_mask_timeout_of_a_deaf_doorbell() {
    let mut harness = harness().started();
    harness.host.deaf.set(true);
    harness.stop.request_stop();
    harness.settle();
    assert_eq!(
        harness.stop.try_exit().map(|exit| exit.end),
        Some(WifiRxEnd::Quarantined {
            reason: WifiRxQuarantine::MaskTimeout,
            walker_stopped: false,
        })
    );
    // The drain never ran.
    assert_eq!(
        harness.report().teardown,
        WifiRxTeardownReport {
            mask_confirm_us: MASK_CONFIRM_TIMEOUT.as_micros() as u32,
            drain_us: 0,
            drain_polls: 0,
        }
    );
}

#[test]
fn teardown_measures_the_drain_of_a_late_p1_drop() {
    let (mut harness, mut frames) = queue_harness();
    for _ in 0..3 {
        harness.complete_unit(&[64]);
    }
    harness.rx_event();
    harness.settle();
    harness.stop.request_stop();
    // Masked at once; the first drain check finds the queued frames and
    // waits one poll.
    harness.poll_once();
    let start = oer_time::Clock::now(harness.clock);
    for _ in 0..5 {
        harness.clock.advance_to_next_deadline();
        harness.poll_once();
    }
    assert!(harness.stop.try_exit().is_none());
    // The application drops its frames late; the next check passes.
    while let Some(held) = poll_next(&mut frames) {
        drop(held);
    }
    harness.clock.advance_to_next_deadline();
    harness.poll_once();
    assert_eq!(
        harness.stop.try_exit().map(|exit| exit.end),
        Some(WifiRxEnd::Halted)
    );
    let waited = oer_time::Clock::now(harness.clock).saturating_duration_since(start);
    assert_eq!(waited, Duration::from_micros(6 * DRAIN_POLL.as_micros()));
    assert_eq!(
        harness.report().teardown,
        WifiRxTeardownReport {
            mask_confirm_us: 0,
            drain_us: waited.as_micros() as u32,
            drain_polls: 6,
        }
    );
}

#[test]
fn a_walker_reported_on_a_held_lease_descriptor_quarantines_the_epoch() {
    let held: &'static Held = leak(RefCell::new(Vec::new()));
    let mut harness =
        harness_with(|counters| (HoldingSink { held }, CountingAdmission::new(counters))).started();
    harness.frame(200);
    harness.settle();
    assert_eq!(held.borrow().len(), 1);
    // The held lease's descriptor carries the owner bit clear.
    let index = harness.next + COUNT - 1;
    let word0 = harness.rx.descriptors()[index % COUNT].word0();
    assert!(!word0.owner());
    assert!(word0.rx_done());
    // A frame completes and the walker reports NEXT on that barrier:
    // impossible by construction, so the drain fails closed.
    harness.complete_unit(&[100]);
    harness.host.walker.borrow_mut().next_descriptor_low =
        (harness.base + (index % COUNT) as u32 * DESCRIPTOR_BYTES) & 0x000f_ffff;
    harness.rx_event();
    harness.settle();
    let report = harness.report();
    assert_eq!(report.reload.walker_on_detached, 1);
    assert_eq!(report.service_errors, 1);
    assert_eq!(harness.rx.lifecycle_state(), RxDmaArenaState::ResetRequired);
    held.borrow_mut().clear();
}

#[test]
fn a_runner_outside_its_signals_executor_ends_instead_of_waiting_forever() {
    let mut harness = harness_staged_with::<SLOTS, _, _, _>(
        crate::rx::arena::WifiIrqSignals {
            mac: oer_espressif_interrupt_table::signal::host::foreign_signal(),
            power: oer_espressif_interrupt_table::signal::host::foreign_signal(),
        },
        |counters, _, _, _| {
            (
                CountingSink::new(counters),
                CountingAdmission::new(counters),
            )
        },
    );
    harness.settle();
    assert!(matches!(
        harness.stop.try_exit(),
        Some(WifiRxExit {
            cause: WifiRxCause::ForeignExecutor,
            ..
        })
    ));
}

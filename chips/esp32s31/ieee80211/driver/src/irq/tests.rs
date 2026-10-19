use core::{
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, sync::Arc, task::Wake};

use oer_esp32s31_hal::types::{
    MacInterruptEvents, MacInterruptObservation, MacPowerInterruptObservation,
};
use oer_esp32s31_ieee80211_mac::irq::{
    IrqWork,
    consumer::{IrqConsumer, IrqStorage, PowerIrqConsumer, PowerIrqStorage},
    model::{
        MacInterruptModel, MacInterruptOperation, PowerInterruptModel, PowerInterruptOperation,
    },
};

use super::*;

type HostEpoch = ActiveEpoch<Moderated, MacInterruptModel, PowerInterruptModel>;
type HostLine = WifiIrqLine<HostEpoch>;

fn leak<T>(value: T) -> &'static mut T {
    Box::leak(Box::new(value))
}

struct Fixture {
    line: HostLine,
    irq: IrqConsumer<'static>,
    power: PowerIrqConsumer<'static>,
    requests: &'static WifiIrqRequests,
    counters: &'static WifiRxCounters,
}

fn host_epoch() -> HostEpoch {
    ActiveEpoch::for_host_test(MacInterruptModel::new(), PowerInterruptModel::new())
}

/// The arena's line parts and the consumers of its facts.
fn parts() -> (
    WifiIrqLineParts<'static, HostEpoch>,
    IrqConsumer<'static>,
    PowerIrqConsumer<'static>,
) {
    let (mac, irq) = leak(IrqStorage::new()).split();
    let (power, power_consumer) = leak(PowerIrqStorage::new()).split();
    let parts = WifiIrqLineParts {
        mac: leak(mac),
        power,
        requests: leak(WifiIrqRequests::new()),
        counters: leak(WifiRxCounters::new()),
        slot: leak(MaybeUninit::uninit()),
    };
    (parts, irq, power_consumer)
}

/// A line run directly, as its handlers would run it.
fn fixture() -> Fixture {
    let (parts, irq, power) = parts();
    let (requests, counters) = (parts.requests, parts.counters);
    Fixture {
        line: WifiIrqLine::new(host_epoch(), parts.mac, parts.power, requests, counters),
        irq,
        power,
        requests,
        counters,
    }
}

fn rx_success() -> MacInterruptObservation {
    MacInterruptObservation::from_semantic_events(MacInterruptEvents::RX_SUCCESS, false, false)
}

fn mac(line: &HostLine) -> &MacInterruptModel {
    line.epoch().banks().0
}

fn mac_mut(line: &mut HostLine) -> &mut MacInterruptModel {
    line.epoch_mut().banks_mut().0
}

/// Counts its wakes.
#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// A doorbell that only records its rings; tests run the handler by hand.
#[derive(Default)]
struct Rings(core::cell::Cell<usize>);

impl RadioDoorbell for Rings {
    fn ring(&self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn isr_model_masks_rx_acknowledges_and_wakes_once() {
    let mut fixture = fixture();
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut context = Context::from_waker(&waker);
    assert!(fixture.irq.poll_batch(&mut context).is_pending());

    mac_mut(&mut fixture.line).latch(rx_success());
    assert!(mac(&fixture.line).line_asserted());
    service_mac_line(&mut fixture.line);

    // Masked before the acknowledgement; the line drops.
    assert!(mac(&fixture.line).operations().matches(&[
        MacInterruptOperation::Status,
        MacInterruptOperation::MaskRxDelivery,
        MacInterruptOperation::Acknowledge(rx_success()),
    ]));
    assert!(!mac(&fixture.line).line_asserted());
    assert!(!mac(&fixture.line).rx_delivery_enabled());
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);

    // A frame arriving while masked latches without asserting the line.
    mac_mut(&mut fixture.line).latch(rx_success());
    assert!(!mac(&fixture.line).line_asserted());

    let Poll::Ready(mut batch) = fixture.irq.poll_batch(&mut context) else {
        panic!("the RX fact was published");
    };
    assert_eq!(batch.next(), Some(IrqWork::RxSuccess));
    assert_eq!(batch.next(), None);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    let report = fixture.counters.report();
    assert_eq!((report.mac_irqs, report.spurious_mac_irqs), (1, 0));
}

#[test]
fn a_spurious_mac_entry_is_counted_and_wakes_no_one() {
    let mut fixture = fixture();
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    assert!(
        fixture
            .irq
            .poll_batch(&mut Context::from_waker(&waker))
            .is_pending()
    );
    service_mac_line(&mut fixture.line);
    assert!(
        mac(&fixture.line)
            .operations()
            .matches(&[MacInterruptOperation::Status])
    );
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    let report = fixture.counters.report();
    assert_eq!((report.mac_irqs, report.spurious_mac_irqs), (1, 1));
}

#[test]
fn the_power_line_acknowledges_publishes_and_counts() {
    let mut fixture = fixture();
    let tbtt =
        MacPowerInterruptObservation::from_semantic_events(false, false, false, false, true, false);
    fixture.line.epoch_mut().banks_mut().1.latch(tbtt);
    service_power_line(&mut fixture.line);
    assert!(fixture.line.epoch().banks().1.operations().matches(&[
        PowerInterruptOperation::Status,
        PowerInterruptOperation::Acknowledge(tbtt),
    ]));
    assert!(fixture.power.take().sta_tbtt());

    service_power_line(&mut fixture.line);
    let report = fixture.counters.report();
    assert_eq!((report.power_irqs, report.spurious_power_irqs), (1, 1));
}

#[test]
fn unmask_after_mask_all_is_ignored() {
    let mut fixture = fixture();
    let rings = Rings::default();
    // Both requests before one doorbell run: the mask wins.
    fixture.requests.ring(WifiIrqRequest::UnmaskRx, &rings);
    fixture.requests.ring(WifiIrqRequest::MaskAll, &rings);
    service_doorbell_line(&mut fixture.line);
    assert!(fixture.line.masked());
    assert!(
        mac(&fixture.line)
            .operations()
            .matches(&[MacInterruptOperation::MaskAll])
    );
    assert!(
        fixture
            .line
            .epoch()
            .banks()
            .1
            .operations()
            .matches(&[PowerInterruptOperation::MaskAll])
    );
    assert!(fixture.irq.take_batch().masked());

    // A later unmask and a repeated mask change nothing.
    fixture.requests.ring(WifiIrqRequest::UnmaskRx, &rings);
    service_doorbell_line(&mut fixture.line);
    fixture.requests.ring(WifiIrqRequest::MaskAll, &rings);
    service_doorbell_line(&mut fixture.line);
    assert!(!mac(&fixture.line).rx_delivery_enabled());
    assert!(!mac(&fixture.line).other_sources_enabled());
    assert_eq!(mac(&fixture.line).operations().len(), 1);
    // The masked fact is published once.
    assert!(fixture.irq.take_batch().is_empty());
    let report = fixture.counters.report();
    assert_eq!((report.doorbell_unmasks, report.ignored_unmasks), (0, 2));
    assert_eq!(rings.0.get(), 4);
}

#[test]
fn doorbell_requests_coalesce_and_none_is_lost() {
    let (line_parts, mut irq, _power) = parts();
    let (requests, counters) = (line_parts.requests, line_parts.counters);
    let cell: &'static HandlerCell<HostLine> = leak(HandlerCell::new());
    let Ok(installed) = line_parts.install(host_epoch(), cell) else {
        panic!("an empty cell takes the line");
    };
    let rings = Rings::default();
    cell.with(|line| {
        mac_mut(line).latch(rx_success());
        service_mac_line(line);
        mac_mut(line).clear_operations();
    });
    assert_eq!(irq.take_batch().next(), Some(IrqWork::RxSuccess));

    // Two unmask requests before the handler runs are one unmask.
    requests.ring(WifiIrqRequest::UnmaskRx, &rings);
    requests.ring(WifiIrqRequest::UnmaskRx, &rings);
    assert!(run_doorbell(installed.cell(), || ()));
    assert_eq!(
        cell.with(|line| mac(line)
            .operations()
            .matches(&[MacInterruptOperation::UnmaskRxDelivery])),
        Some(true)
    );
    assert!(!requests.is_pending());
    assert_eq!(counters.report().doorbell_unmasks, 1);

    // A request made after the reset and before the take belongs to this
    // run: the handler resets the interrupt first.
    let mut order = std::vec::Vec::new();
    assert!(run_doorbell(installed.cell(), || {
        order.push("acknowledge");
        requests.ring(WifiIrqRequest::MaskAll, &rings);
    }));
    assert_eq!(order, ["acknowledge"]);
    assert_eq!(cell.with(|line| line.masked()), Some(true));
    assert!(!requests.is_pending());
    assert!(irq.take_batch().masked());
    assert_eq!(rings.0.get(), 3);
}

#[test]
fn install_clears_stale_requests() {
    let (first, mut irq, _power) = parts();
    let requests = first.requests;
    requests.ring(WifiIrqRequest::MaskAll, &Rings::default());
    let cell: &'static HandlerCell<HostLine> = leak(HandlerCell::new());
    let Ok(installed) = first.install(host_epoch(), cell) else {
        panic!("an empty cell takes the line");
    };
    assert!(!requests.is_pending());
    // The stale mask is gone: the doorbell finds nothing to apply.
    assert!(run_doorbell(installed.cell(), || ()));
    assert_eq!(cell.with(|line| line.masked()), Some(false));
    assert_eq!(
        cell.with(|line| mac(line).operations().is_empty()),
        Some(true)
    );
    assert!(irq.take_batch().is_empty());

    // A second line is refused and handed back.
    let (second, _, _) = parts();
    let Err(refused) = second.install(host_epoch(), cell) else {
        panic!("the cell holds the first line");
    };
    assert!(!refused.masked());
}

#[test]
fn the_doorbell_is_acknowledged_without_a_line() {
    let cell: &'static HandlerCell<HostLine> = leak(HandlerCell::new());
    let mut acknowledged = false;
    assert!(!run_doorbell(cell, || acknowledged = true));
    assert!(acknowledged);
}

#[test]
fn the_chip_line_can_live_in_a_static_cell() {
    fn sync<T: Sync>() {}
    sync::<HandlerCell<WifiIrqLine<ActiveEpoch<Moderated>>>>();
}

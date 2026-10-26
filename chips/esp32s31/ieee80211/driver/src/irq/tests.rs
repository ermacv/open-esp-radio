use core::{
    cell::Cell,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, Waker},
};
use std::{boxed::Box, sync::Arc, task::Wake};

use oer_esp32s31_hal::types::{
    MacInterruptEnableState, MacInterruptEvents, MacInterruptMask, MacInterruptObservation,
    MacPowerInterruptObservation,
};
use oer_esp32s31_ieee80211_mac::irq::{
    IrqWork, MacInterrupt,
    consumer::{IrqConsumer, IrqStorage, PowerIrqConsumer, PowerIrqStorage},
    epoch::{EpochState, InterruptEpoch},
    model::{
        MacInterruptModel, MacInterruptOperation, PowerInterruptModel, PowerInterruptOperation,
    },
};
use oer_interrupt_table::signal::host::Host;

use super::*;

/// Counts the doorbell's lowerings.
#[derive(Default)]
struct Lower(usize);

impl DoorbellAcknowledge for Lower {
    fn acknowledge(&mut self) {
        self.0 += 1;
    }
}

type HostPart = WifiIrqPart<Lower, (), MacInterruptModel, PowerInterruptModel>;

fn leak<T>(value: T) -> &'static mut T {
    Box::leak(Box::new(value))
}

/// The handler's part over closed host banks.
fn closed_part() -> HostPart {
    WifiIrqPart::for_host_test(
        InterruptEpoch::new(MacInterruptModel::masked(), PowerInterruptModel::masked()),
        (),
        Lower::default(),
    )
}

/// The handler's part over host banks as an activation leaves them.
fn active_part() -> HostPart {
    WifiIrqPart::for_host_test(
        InterruptEpoch::for_host_test(MacInterruptModel::new(), PowerInterruptModel::new()),
        (),
        Lower::default(),
    )
}

struct Fixture {
    cell: &'static HandlerCell<WifiIrqLine<Host>>,
    irq: IrqConsumer<'static, Host>,
    power: PowerIrqConsumer<'static, Host>,
    requests: &'static WifiIrqRequests,
    counters: &'static WifiRxCounters,
}

/// The arena's line parts and the consumers of its facts.
fn parts() -> (
    WifiIrqLineParts<'static, Host>,
    IrqConsumer<'static, Host>,
    PowerIrqConsumer<'static, Host>,
) {
    let (mac, irq) = leak(IrqStorage::new(oer_interrupt_table::signal::host::signal())).split();
    let (power, power_consumer) = leak(PowerIrqStorage::new(
        oer_interrupt_table::signal::host::signal(),
    ))
    .split();
    let parts = WifiIrqLineParts {
        mac: leak(mac),
        power,
        requests: leak(WifiIrqRequests::new()),
        counters: leak(WifiRxCounters::new()),
        slot: leak(MaybeUninit::uninit()),
    };
    (parts, irq, power_consumer)
}

/// A line installed in its handler cell.
fn fixture() -> Fixture {
    let (parts, irq, power) = parts();
    let (requests, counters) = (parts.requests, parts.counters);
    let cell: &'static HandlerCell<WifiIrqLine<Host>> = leak(HandlerCell::new());
    assert!(parts.install(cell).is_ok(), "an empty cell takes the line");
    Fixture {
        cell,
        irq,
        power,
        requests,
        counters,
    }
}

fn rx_success() -> MacInterruptObservation {
    MacInterruptObservation::from_semantic_events(MacInterruptEvents::RX_SUCCESS, false, false)
}

fn mac(part: &HostPart) -> &MacInterruptModel {
    part.epoch().banks().0
}

fn mac_mut(part: &mut HostPart) -> &mut MacInterruptModel {
    part.epoch_mut().banks_mut().0
}

fn power(part: &HostPart) -> &PowerInterruptModel {
    part.epoch().banks().1
}

fn clear_logs(part: &mut HostPart) {
    let (mac, power) = part.epoch_mut().banks_mut();
    mac.clear_operations();
    power.clear_operations();
}

/// Counts its wakes.
#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// A doorbell that only records its rings; tests run the handler by hand,
/// as the interrupt would preempt the ringing task.
#[derive(Default)]
struct Rings(Cell<usize>);

impl RadioDoorbell for Rings {
    const HANDLER: oer_interrupt_table::Context =
        oer_interrupt_table::Context { level: 3, hart: 0 };
    const EXECUTOR: oer_interrupt_table::Context =
        oer_interrupt_table::Context { level: 2, hart: 0 };
    fn ring(&self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn the_part_names_both_banks_and_the_doorbell_as_its_sources() {
    use oer_interrupt_table::InterruptSource;
    let sources = <WifiIrqPart<Lower> as InterruptSources>::SOURCES;
    assert_eq!(
        sources,
        [
            MacInterruptRegisters::SOURCE,
            MacPowerInterruptRegisters::SOURCE,
            FromCpu3Irq::SOURCE,
        ]
    );
}

#[test]
fn the_handler_lowers_the_doorbell_before_it_takes_the_requests() {
    let fixture = fixture();
    let mut part = active_part();
    let rings = Rings::default();
    fixture.requests.ring(WifiIrqRequest::MaskAll, &rings);
    assert!(service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt"));
    assert_eq!(part.acknowledger.0, 1);
    assert!(!fixture.requests.is_pending());
    assert_eq!(part.epoch().state(), EpochState::Masked);
}

#[test]
fn an_activation_clears_stale_events_then_publishes_the_station_mask() {
    let mut fixture = fixture();
    let mut part = closed_part();
    // A stale event from before the epoch, under a masked enable.
    mac_mut(&mut part).latch(rx_success());
    let rings = Rings::default();

    fixture.requests.activate(&HostActivation, &rings);
    assert!(fixture.requests.is_pending());
    assert!(service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt"));

    assert_eq!(part.epoch().state(), EpochState::Active);
    // The entry served the request alone: no bank was sampled.
    assert!(mac(&part).operations().matches(&[
        MacInterruptOperation::FatalAssertionRaw,
        MacInterruptOperation::MaskAll,
        MacInterruptOperation::FatalAssertionRaw,
        MacInterruptOperation::ClearAll,
        MacInterruptOperation::Publish(MacInterruptMask::STATION),
    ]));
    assert!(power(&part).operations().matches(&[
        PowerInterruptOperation::MaskAll,
        PowerInterruptOperation::ClearAll,
        PowerInterruptOperation::EnableStationTbtt,
    ]));
    assert!(mac(&part).latched().is_empty());
    assert!(!mac(&part).line_asserted());
    // The line published the enable image it read after the request.
    assert_eq!(
        fixture.requests.mac_interrupt_enable(),
        Some(MacInterruptEnableState::Station)
    );
    // Nothing fired: no fact.
    assert!(fixture.irq.take_batch().unwrap().is_empty());
    assert_eq!(fixture.counters.report().closed_irqs, 0);
}

#[test]
fn the_mac_bank_masks_rx_acknowledges_and_wakes_once() {
    let mut fixture = fixture();
    let mut part = active_part();
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    let mut context = Context::from_waker(&waker);
    assert!(fixture.irq.poll_batch(&mut context).is_pending());

    mac_mut(&mut part).latch(rx_success());
    assert!(mac(&part).line_asserted());
    assert!(service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt"));

    // Masked before the acknowledgement; the line drops.
    assert!(mac(&part).operations().matches(&[
        MacInterruptOperation::Status,
        MacInterruptOperation::FatalAssertionRaw,
        MacInterruptOperation::MaskRxDelivery,
        MacInterruptOperation::Acknowledge(rx_success()),
    ]));
    assert!(!mac(&part).line_asserted());
    assert!(!mac(&part).rx_delivery_enabled());
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);

    // A frame arriving while masked latches without asserting the line.
    mac_mut(&mut part).latch(rx_success());
    assert!(!mac(&part).line_asserted());

    let Poll::Ready(Ok(mut batch)) = fixture.irq.poll_batch(&mut context) else {
        panic!("the RX fact was published");
    };
    assert_eq!(batch.next(), Some(IrqWork::RxSuccess));
    assert_eq!(batch.next(), None);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    let report = fixture.counters.report();
    assert_eq!((report.mac_irqs, report.spurious_mac_irqs), (1, 0));
    // One cause per entry: the WDEVPWR bank was not sampled.
    assert!(power(&part).operations().is_empty());
}

#[test]
fn the_power_bank_acknowledges_publishes_and_counts() {
    let mut fixture = fixture();
    let mut part = active_part();
    let tbtt =
        MacPowerInterruptObservation::from_semantic_events(false, false, false, false, true, false);
    part.epoch_mut().banks_mut().1.latch(tbtt);
    assert!(service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt"));
    assert!(power(&part).operations().matches(&[
        PowerInterruptOperation::Status,
        PowerInterruptOperation::Acknowledge(tbtt),
    ]));
    assert!(fixture.power.take().unwrap().sta_tbtt());
    // The MAC bank was sampled first and held nothing; no MAC fact.
    assert!(mac(&part).operations().matches(&[
        MacInterruptOperation::Status,
        MacInterruptOperation::FatalAssertionRaw
    ]));
    let report = fixture.counters.report();
    assert_eq!(report.power_irqs, 1);
    assert_eq!((report.mac_irqs, report.spurious_mac_irqs), (0, 0));
    assert!(fixture.irq.take_batch().unwrap().is_empty());
}

#[test]
fn an_entry_without_a_cause_is_counted_and_wakes_no_one() {
    let mut fixture = fixture();
    let mut part = active_part();
    let wakes = Arc::new(WakeCount::default());
    let waker = Waker::from(wakes.clone());
    assert!(
        fixture
            .irq
            .poll_batch(&mut Context::from_waker(&waker))
            .is_pending()
    );
    service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt");
    assert!(mac(&part).operations().matches(&[
        MacInterruptOperation::Status,
        MacInterruptOperation::FatalAssertionRaw
    ]));
    assert!(
        power(&part)
            .operations()
            .matches(&[PowerInterruptOperation::Status])
    );
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    let report = fixture.counters.report();
    assert_eq!((report.mac_irqs, report.spurious_mac_irqs), (1, 1));
}

#[test]
fn an_unmask_after_mask_all_is_ignored_and_the_mask_posted_once() {
    let mut fixture = fixture();
    let mut part = active_part();
    let rings = Rings::default();
    // Both requests before one handler run: the mask wins.
    fixture.requests.ring(WifiIrqRequest::UnmaskRx, &rings);
    fixture.requests.ring(WifiIrqRequest::MaskAll, &rings);
    service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt");
    assert_eq!(part.epoch().state(), EpochState::Masked);
    assert!(mac(&part).operations().matches(&[
        MacInterruptOperation::FatalAssertionRaw,
        MacInterruptOperation::MaskAll
    ]));
    assert!(fixture.irq.take_batch().unwrap().masked());
    assert_eq!(
        fixture.requests.mac_interrupt_enable(),
        Some(MacInterruptEnableState::Disabled)
    );

    // A later unmask and a repeated mask change nothing.
    clear_logs(&mut part);
    fixture.requests.ring(WifiIrqRequest::UnmaskRx, &rings);
    service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt");
    fixture.requests.ring(WifiIrqRequest::MaskAll, &rings);
    service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt");
    assert!(!mac(&part).rx_delivery_enabled());
    assert!(!mac(&part).other_sources_enabled());
    assert!(mac(&part).operations().matches(&[
        MacInterruptOperation::FatalAssertionRaw,
        MacInterruptOperation::FatalAssertionRaw
    ]));
    // The masked fact is published once.
    assert!(fixture.irq.take_batch().unwrap().is_empty());
    let report = fixture.counters.report();
    assert_eq!((report.doorbell_unmasks, report.ignored_unmasks), (0, 2));
    assert_eq!(rings.0.get(), 4);
}

#[test]
fn an_unmask_restores_the_rx_group_of_an_active_epoch() {
    let mut fixture = fixture();
    let mut part = active_part();
    mac_mut(&mut part).latch(rx_success());
    service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt");
    assert_eq!(
        fixture.irq.take_batch().unwrap().next(),
        Some(IrqWork::RxSuccess)
    );
    clear_logs(&mut part);

    // Two unmask requests before the handler runs are one unmask.
    let rings = Rings::default();
    fixture.requests.ring(WifiIrqRequest::UnmaskRx, &rings);
    fixture.requests.ring(WifiIrqRequest::UnmaskRx, &rings);
    service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt");
    assert!(mac(&part).operations().matches(&[
        MacInterruptOperation::FatalAssertionRaw,
        MacInterruptOperation::UnmaskRxDelivery
    ]));
    assert!(mac(&part).rx_delivery_enabled());
    assert_eq!(fixture.counters.report().doorbell_unmasks, 1);
}

#[test]
fn closing_masks_and_clears_and_wins_over_every_other_request() {
    let fixture = fixture();
    let mut part = active_part();
    mac_mut(&mut part).latch(rx_success());
    clear_logs(&mut part);
    let rings = Rings::default();
    fixture.requests.ring(WifiIrqRequest::UnmaskRx, &rings);
    fixture.requests.activate(&HostActivation, &rings);
    fixture.requests.ring(WifiIrqRequest::MaskAll, &rings);
    fixture.requests.ring(WifiIrqRequest::Close, &rings);
    service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt");

    assert_eq!(part.epoch().state(), EpochState::Closed);
    // The entry served the requests alone: the latched RX was cleared, never
    // serviced.
    assert!(mac(&part).operations().matches(&[
        MacInterruptOperation::FatalAssertionRaw,
        MacInterruptOperation::MaskAll,
        MacInterruptOperation::FatalAssertionRaw,
        MacInterruptOperation::ClearAll,
    ]));
    assert!(mac(&part).latched().is_empty());
    assert_eq!(
        fixture.requests.mac_interrupt_enable(),
        Some(MacInterruptEnableState::Disabled)
    );
    assert_eq!(fixture.counters.report().ignored_unmasks, 1);
}

#[test]
fn a_stray_event_in_a_closed_epoch_closes_the_banks_again() {
    let fixture = fixture();
    let mut part = closed_part();
    // An enable a reset restored, with an event latched under it.
    mac_mut(&mut part).publish(MacInterruptMask::STATION);
    mac_mut(&mut part).latch(rx_success());
    assert!(mac(&part).line_asserted());

    service_wifi_line(fixture.cell, &mut part).expect("healthy interrupt");

    assert_eq!(part.epoch().state(), EpochState::Closed);
    assert!(!mac(&part).line_asserted() && mac(&part).latched().is_empty());
    let report = fixture.counters.report();
    assert_eq!(report.closed_irqs, 1);
    // Nothing was serviced as work.
    assert_eq!(report.mac_irqs, 0);
}

#[test]
fn without_a_line_the_handler_lowers_the_doorbell_and_closes_a_stray_event() {
    let cell: &'static HandlerCell<WifiIrqLine<Host>> = leak(HandlerCell::new());
    let mut part = closed_part();
    mac_mut(&mut part).publish(MacInterruptMask::STATION);
    mac_mut(&mut part).latch(rx_success());
    assert!(!service_wifi_line(cell, &mut part).expect("healthy interrupt"));
    assert_eq!(part.acknowledger.0, 1);
    assert!(!mac(&part).line_asserted() && mac(&part).latched().is_empty());
}

#[test]
fn install_clears_stale_requests_and_refuses_a_second_line() {
    let (first, mut irq, _power) = parts();
    let requests = first.requests;
    requests.ring(WifiIrqRequest::MaskAll, &Rings::default());
    let cell: &'static HandlerCell<WifiIrqLine<Host>> = leak(HandlerCell::new());
    assert!(first.install(cell).is_ok());
    assert!(!requests.is_pending());
    // The stale mask is gone: the handler finds nothing to apply.
    let mut part = active_part();
    service_wifi_line(cell, &mut part).expect("healthy interrupt");
    assert_eq!(part.epoch().state(), EpochState::Active);
    assert!(irq.take_batch().unwrap().is_empty());
    assert_eq!(requests.mac_interrupt_enable(), None);

    // A second line is refused and handed back.
    let (second, _, _) = parts();
    assert!(second.install(cell).is_err());
}

#[test]
fn the_chip_part_and_line_can_live_in_static_cells() {
    fn sync<T: Sync>() {}
    fn send<T: Send>() {}
    sync::<HandlerCell<WifiIrqLine<Host>>>();
    send::<WifiIrqPart<Lower>>();
}

#[test]
fn fatal_mac_sample_preempts_work_and_requests_without_releasing_owners() {
    for request in 0..4 {
        let mut fixture = fixture();
        let mut part = active_part();
        let raw = 0x9361_ace7;
        mac_mut(&mut part).latch(rx_success());
        mac_mut(&mut part).latch_fatal_assertion(raw);
        let rings = Rings::default();
        match request {
            1 => fixture.requests.ring(WifiIrqRequest::Close, &rings),
            2 => fixture.requests.ring(WifiIrqRequest::MaskAll, &rings),
            3 => fixture.requests.activate(&HostActivation, &rings),
            _ => (),
        }
        let fault = Fault::FatalAssertion { raw };
        assert_eq!(service_wifi_line(fixture.cell, &mut part), Err(fault));
        assert_eq!(part.epoch().state(), EpochState::Quarantined(fault));
        assert_eq!(mac(&part).latched_fatal_assertion_raw(), Some(raw));
        assert!(!mac(&part).line_asserted() && !power(&part).line_asserted());
        let expected = if request == 0 {
            &[
                MacInterruptOperation::Status,
                MacInterruptOperation::FatalAssertionRaw,
                MacInterruptOperation::MaskAll,
            ][..]
        } else {
            &[
                MacInterruptOperation::FatalAssertionRaw,
                MacInterruptOperation::MaskAll,
            ][..]
        };
        assert!(mac(&part).operations().matches(expected));
        assert!(
            power(&part)
                .operations()
                .matches(&[PowerInterruptOperation::MaskAll])
        );
        assert!(fixture.cell.with(|_| ()).is_some());
        assert!(fixture.irq.take_batch().expect("host context").is_empty());
    }
}

#[test]
fn an_uninstalled_line_captures_masked_inherited_fatal_raw_without_clearing() {
    let cell = HandlerCell::<WifiIrqLine<Host>>::new();
    let mut part = closed_part();
    let raw = 0x1987_6abc;
    mac_mut(&mut part).latch_fatal_assertion(raw);
    assert!(mac(&part).visible().is_empty());
    assert!(!mac(&part).line_asserted());
    let fault = Fault::FatalAssertion { raw };
    assert_eq!(service_wifi_line(&cell, &mut part), Err(fault));
    assert_eq!(part.epoch().state(), EpochState::Quarantined(fault));
    assert_eq!(mac(&part).latched_fatal_assertion_raw(), Some(raw));
    assert!(mac(&part).operations().matches(&[
        MacInterruptOperation::FatalAssertionRaw,
        MacInterruptOperation::MaskAll
    ]));
    assert!(
        power(&part)
            .operations()
            .matches(&[PowerInterruptOperation::MaskAll])
    );
}

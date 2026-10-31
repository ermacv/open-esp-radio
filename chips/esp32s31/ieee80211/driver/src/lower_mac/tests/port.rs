use super::{ACCESS_POINT, BSSID, Hardware, Power, RadioNine, STATION, channel, entropy, slot};
use crate::port::*;
use core::{
    convert::Infallible,
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_ieee80211_contracts_port::*;
use oer_ieee80211_wire::{
    address::IndividualAddress,
    frame::FrameControl,
    phy::{LegacyRate, PhyRate},
};
use oer_radio_port::{self as radio, PortBackend, PortBody, PortPhase};
use oer_time::{Clock, Instant, Timer};
use std::{
    boxed::Box,
    cell::{Cell, RefCell},
    rc::Rc,
    vec::Vec,
};

#[derive(Clone)]
struct TestTimer(Rc<oer_time_virtual::VirtualClock>, Rc<Cell<bool>>);
impl Clock for TestTimer {
    fn now(&self) -> Instant {
        self.0.now()
    }
}
impl Timer for TestTimer {
    async fn wait_until(&self, deadline: Instant) {
        let mut wait = pin!(self.0.wait_until(deadline));
        core::future::poll_fn(|cx| {
            if self.1.get() {
                // The strict-priority model dispatches due timers itself:
                // advancing a device step must not wake every future timer.
                wait.as_mut().poll(&mut Context::from_waker(Waker::noop()))
            } else {
                wait.as_mut().poll(cx)
            }
        })
        .await
    }
}

#[derive(Default)]
struct Observed {
    /// What the P2 driver reported at each publication's contention draw.
    draws: Rc<RefCell<Vec<crate::lower_mac::ContentionDraw>>>,
    check_context: bool,
    context: u8,
    device_polls: usize,
    abandon_contexts: Vec<u8>,
    erased_keys: usize,
    abandoned_publications: usize,
    publications: usize,
    statuses: Vec<(u8, u8)>,
    starts: usize,
    start_channels: Vec<oer_ieee80211_wire::channel::WifiChannel>,
    stops: usize,
    abandoned: bool,
    detached: bool,
    hold_start: bool,
    /// The next start fails with this fault, as a refused retune does.
    fail_start: Option<Fault>,
    hold_completion: bool,
    /// Hardware queues whose publications do not complete yet.
    held_queues: [bool; 4],
    /// The next device poll fails with this fault, poisoning the port.
    fail_poll: Option<Fault>,
    /// Key writes the hardware rejects from the next register access on.
    reject_key_writes: usize,
    /// The key table as abandonment found it, before erasing it.
    material_at_abandon: Vec<(u8, [u8; 16])>,
    /// Frames on air with the material byte they are protected under (0
    /// for none). Capture decrypts each against the key table at once;
    /// a frame whose material is not installed is dropped.
    air: std::collections::VecDeque<(TestReceive, u8)>,
    /// Air frames capture dropped for their material.
    undecryptable: usize,
    /// Frames captured in hardware and DMA, delivered through the RX cut.
    captured: std::collections::VecDeque<TestReceive>,
    /// Material writes the key table has seen.
    key_writes: usize,
    /// The next publication ends in the hardware's TX-timeout edge instead
    /// of a completion.
    timeout_edge: bool,
    hold_cut: bool,
    hold_stop: bool,
    cut_calls: usize,
    resumed_cuts: usize,
    backoffs: Vec<u16>,
    /// Each legacy publication program's AIFSN, in publication order.
    aifsns: Vec<u8>,
    received: std::collections::VecDeque<TestReceive>,
    continuous_rx: bool,
    rx_continuation: Option<Instant>,
    rx_poll_steps: usize,
    rx_saturated_polls: usize,
    rx_delivered: usize,
}
struct Payload(Rc<Cell<usize>>);
/// An opaque context: its value, and the destructor count of a fixture
/// that watches it. The backend returns the same object it was lent.
#[derive(Debug)]
struct Tag(usize, Option<Rc<Cell<usize>>>);
impl Drop for Tag {
    fn drop(&mut self) {
        if let Some(drops) = &self.1 {
            drops.set(drops.get() + 1);
        }
    }
}
impl PartialEq<usize> for Tag {
    fn eq(&self, other: &usize) -> bool {
        self.0 == *other
    }
}
impl<Id: radio::SlotIdentity> radio::ContextProjection<Id> for Tag {
    type View<'a, 'p>
        = usize
    where
        Id: 'p,
        'p: 'a;
    type Parts<'p>
        = Self
    where
        Id: 'p;
    fn view<'a, 'p>(&'a self, _: radio::ContextLoan<'p, Id>) -> usize
    where
        Id: 'p,
        'p: 'a,
    {
        self.0
    }
    fn into_parts<'p>(self, _: radio::ContextLoan<'p, Id>) -> Self
    where
        Id: 'p,
    {
        self
    }
}
impl<'p, Id: radio::SlotIdentity + 'p> radio::LoanedContext<'p, Id> for Tag {
    type Owned = Self;
    fn enclose(self) -> radio::PortContext<'p, Id, Self> {
        radio::PortContext::new(self)
    }
    fn restore(context: radio::PortContext<'p, Id, Self>) -> Self {
        context.into_parts()
    }
}
impl TxBody for Payload {
    fn bytes(&self) -> &[u8] {
        b"payload"
    }
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
struct TestDevice {
    hardware: Hardware,
    timer: TestTimer,
    observed: Rc<RefCell<Observed>>,
    completed: usize,
    /// Publications already completed, by publication order.
    done: Vec<bool>,
    stopped: bool,
}
impl TestDevice {
    /// The first publication not yet completed whose queue is not held.
    fn next_completion(&self, publications: usize) -> Option<(usize, u8)> {
        let observed = self.observed.borrow();
        (0..publications).find_map(|index| {
            let queue = self.hardware.legacy.get(index).map(|p| p.0).unwrap_or(2);
            (!self.done.get(index).copied().unwrap_or(false)
                && !observed.held_queues[usize::from(queue)])
            .then_some((index, queue))
        })
    }
}
impl TestDevice {
    fn assert_driver_context(&self) {
        let observed = self.observed.borrow();
        if observed.check_context {
            assert_eq!(observed.context, 2, "device access belongs to P2");
        }
    }
}
struct TestReceive {
    bytes: [u8; 26],
    released: Rc<Cell<usize>>,
}
impl ReceiveUnit for TestReceive {
    fn normalized(&self) -> Result<crate::mac::rx::NormalizedRxFrame<'_>, crate::mac::rx::RxError> {
        Ok(super::normalized(&self.bytes))
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}
impl Drop for TestReceive {
    fn drop(&mut self) {
        self.released.set(self.released.get() + 1);
    }
}
impl Device<512> for TestDevice {
    type Power = Power;
    type Coex = RadioNine;
    type Entropy = fn() -> u32;
    type Timer = TestTimer;
    type Hardware = Hardware;
    type Receive = TestReceive;
    type Body = Payload;
    type OwnedContext<Id: radio::SlotIdentity> = Tag;
    type Context<'p, Id: radio::SlotIdentity + 'p> = Tag;
    type OwnedConfigureContext<Id: radio::SlotIdentity> = Tag;
    type ConfigureContext<'p, Id: radio::SlotIdentity + 'p> = Tag;
    fn hardware(&mut self) -> &mut Hardware {
        self.assert_driver_context();
        self.hardware.reject_key_writes +=
            core::mem::take(&mut self.observed.borrow_mut().reject_key_writes);
        &mut self.hardware
    }
    fn timer(&self) -> &TestTimer {
        &self.timer
    }
    fn next_deadline(&self) -> Option<Instant> {
        self.observed.borrow().rx_continuation
    }
    fn take_power_profile(&mut self) -> Option<Self::Power> {
        None
    }
    fn radio_now(&mut self) -> Result<Ieee80211Instant, radio::ClockError> {
        self.assert_driver_context();
        // A held start models a retune: the radio actor holds the registers.
        if self.observed.borrow().hold_start {
            return Err(radio::ClockError::Unavailable);
        }
        Ok(Ieee80211Instant::from_micros(
            self.timer.now().as_micros() + 1234,
        ))
    }
    fn poll_start(
        &mut self,
        _: &mut Context<'_>,
        channel: oer_ieee80211_wire::channel::WifiChannel,
    ) -> Poll<Result<(), Fault>> {
        self.assert_driver_context();
        if self.observed.borrow().hold_start {
            return Poll::Pending;
        }
        if let Some(fault) = self.observed.borrow_mut().fail_start.take() {
            return Poll::Ready(Err(fault));
        }
        self.observed.borrow_mut().starts += 1;
        self.observed.borrow_mut().start_channels.push(channel);
        self.stopped = false;
        Poll::Ready(Ok(()))
    }
    fn poll_stop(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Fault>> {
        self.assert_driver_context();
        if self.observed.borrow().hold_stop {
            return Poll::Pending;
        }
        if !self.stopped {
            self.observed.borrow_mut().stops += 1;
            self.stopped = true;
        }
        Poll::Ready(Ok(()))
    }
    fn poll_receive(
        &mut self,
        receive_room: usize,
        _: &mut Context<'_>,
    ) -> Poll<Result<DeviceEvent<TestReceive>, Fault>> {
        self.assert_driver_context();
        if let Some(fault) = self.observed.borrow_mut().fail_poll.take() {
            return Poll::Ready(Err(fault));
        }
        let mut observed = self.observed.borrow_mut();
        observed.key_writes = self
            .hardware
            .crypto_order
            .iter()
            .filter(|operation| matches!(operation, super::CryptoOperation::Install(_)))
            .count();
        while let Some((frame, material)) = observed.air.pop_front() {
            if material == 0
                || self
                    .hardware
                    .key_material
                    .iter()
                    .any(|(_, installed)| *installed == [material; 16])
            {
                observed.received.push_back(frame);
            } else {
                observed.undecryptable += 1;
            }
        }
        observed.device_polls += 1;
        suspend_rx_continuation(receive_room, &mut observed.rx_continuation);
        if observed.continuous_rx {
            observed.rx_poll_steps += 1;
            assert!(
                observed.rx_poll_steps <= 32,
                "P2 must return to its executor"
            );
            observed.rx_saturated_polls += usize::from(receive_room == 0);
            self.timer
                .0
                .advance(oer_time::Duration::from_micros(10))
                .expect("finite model device step");
        }
        if receive_room != 0 {
            if let Some(frame) = observed.received.pop_front() {
                if observed.continuous_rx {
                    observed.rx_delivered += 1;
                    if observed.rx_delivered.is_multiple_of(4) {
                        observed.rx_continuation = Some(
                            self.timer
                                .now()
                                .saturating_add(crate::rx::receive::CONTINUATION_WAIT),
                        );
                    }
                }
                return Poll::Ready(Ok(DeviceEvent::Receive(frame)));
            }
            if observed
                .rx_continuation
                .is_some_and(|at| self.timer.now() >= at)
            {
                observed.rx_continuation = None;
            }
        }
        drop(observed);
        let publications = self.hardware.publications();
        self.observed.borrow_mut().backoffs = self
            .hardware
            .legacy
            .iter()
            .map(|(_, program)| program.contention_window())
            .collect();
        self.observed.borrow_mut().aifsns = self
            .hardware
            .legacy
            .iter()
            .map(|(_, program)| program.aifsn())
            .collect();
        self.observed.borrow_mut().publications = publications;
        self.done.resize(publications, false);
        let next = self.next_completion(publications);
        if let Some((index, queue)) = next
            && self.observed.borrow().timeout_edge
        {
            self.hardware.timeout_pending[usize::from(queue)] = true;
            self.done[index] = true;
            self.completed += 1;
            return Poll::Ready(Ok(DeviceEvent::Tx(crate::tx::WifiTxWake::Interrupt {
                events: crate::mac::irq::EVENT_TX_TIMEOUT,
            })));
        }
        if let Some((index, queue)) = next
            && !self.observed.borrow().hold_completion
        {
            let (status, detail) = self
                .observed
                .borrow()
                .statuses
                .get(self.completed)
                .copied()
                .unwrap_or((0, 0));
            self.hardware.completion[usize::from(queue)] = Some(
                oer_esp32s31_hal::types::MacTxCompletionObservation::new_model(status, detail),
            );
            self.done[index] = true;
            self.completed += 1;
            Poll::Ready(Ok(DeviceEvent::Tx(crate::tx::WifiTxWake::Interrupt {
                events: crate::mac::irq::EVENT_TX_COMPLETE,
            })))
        } else {
            Poll::Pending
        }
    }
    fn poll_rx_cut(
        &mut self,
        _: usize,
        _: &mut Context<'_>,
    ) -> Poll<Result<Option<TestReceive>, Fault>> {
        self.assert_driver_context();
        self.observed.borrow_mut().cut_calls += 1;
        if self.observed.borrow().hold_cut {
            return Poll::Pending;
        }
        // The cut drains what hardware, DMA and the pending interrupt
        // already captured, under the material active at capture.
        let mut observed = self.observed.borrow_mut();
        let frame = match observed.captured.pop_front() {
            Some(frame) => Some(frame),
            None => observed.received.pop_front(),
        };
        Poll::Ready(Ok(frame))
    }
    fn finish_rx_cut(&mut self) -> Result<(), Fault> {
        self.assert_driver_context();
        self.observed.borrow_mut().resumed_cuts += 1;
        Ok(())
    }
    fn recycle(&mut self, frame: TestReceive) {
        self.assert_driver_context();
        drop(frame);
    }
    fn abandon(&mut self) -> bool {
        self.assert_driver_context();
        let keys = self.hardware.installed_keys.len();
        self.observed.borrow_mut().material_at_abandon = self.hardware.key_material.clone();
        crate::mac::crypto::erase_lower_mac_keys(&mut self.hardware);
        assert!(self.hardware.installed_keys.is_empty());
        let mut observed = self.observed.borrow_mut();
        let context = observed.context;
        observed.abandon_contexts.push(context);
        observed.erased_keys += keys;
        observed.abandoned_publications = self.hardware.publications();
        observed.abandoned = true;
        false
    }
    fn detach(&mut self) {
        self.assert_driver_context();
        self.observed.borrow_mut().detached = true;
    }
}
/// The driver's observation receiver in these tests: it keeps every
/// contention draw in the fixture's `Observed::draws`.
struct Draws(Rc<RefCell<Vec<crate::lower_mac::ContentionDraw>>>);
impl oer_events::EventSink<crate::lower_mac::ContentionDraw> for Draws {
    fn event(&mut self, draw: crate::lower_mac::ContentionDraw) {
        self.0.borrow_mut().push(draw);
    }
}
type Backend<Id> = Esp32s31Backend<Id, TestDevice, 512, Draws>;
type Caps = Esp32s31Caps<512>;
type Family<Id> = Types<Id, TestDevice, 512, Draws>;
type Res<'id, Id> = <Backend<Id> as PortBackend<Id>>::Resources<'id>;

mod abandon;
mod barriers;
mod conformance;
mod retune;
mod rx_capacity;

fn installed(
    statuses: Vec<(u8, u8)>,
) -> (
    PortParts<TestDevice, 512, Draws>,
    Rc<RefCell<Observed>>,
    TestTimer,
) {
    installed_with_hardware(statuses, Hardware::default())
}

fn installed_with_hardware(
    statuses: Vec<(u8, u8)>,
    hardware: Hardware,
) -> (
    PortParts<TestDevice, 512, Draws>,
    Rc<RefCell<Observed>>,
    TestTimer,
) {
    installed_with_entropy(statuses, hardware, entropy)
}

/// A fixture whose transmitter draws its backoff from `entropy`.
fn installed_with_entropy(
    statuses: Vec<(u8, u8)>,
    hardware: Hardware,
    entropy: fn() -> u32,
) -> (
    PortParts<TestDevice, 512, Draws>,
    Rc<RefCell<Observed>>,
    TestTimer,
) {
    let timer = TestTimer(
        Rc::new(oer_time_virtual::VirtualClock::new()),
        Rc::new(Cell::new(false)),
    );
    let observed = Rc::new(RefCell::new(Observed {
        statuses,
        ..Observed::default()
    }));
    let buffers: [_; TX_TICKETS] = core::array::from_fn(|_| slot());
    let device = TestDevice {
        hardware,
        timer: timer.clone(),
        observed: observed.clone(),
        completed: 0,
        done: Vec::new(),
        stopped: false,
    };
    let device = Box::leak(Box::new(device));
    let storage = Box::leak(Box::new(PortStorage::new(
        device,
        Box::leak(Box::new(crate::port::PortSharedStorage::new())),
    )));
    let transmitter = crate::ordinary_tx::OrdinaryTxOwner::new(crate::tx::WifiTxResources {
        slot: slot(),
        policy: crate::mac::tx::runtime::WifiTxRuntimePolicy::vendor_defaults(),
        power: Power,
        entropy,
        timer: timer.clone(),
    })
    .into_idle()
    .ok()
    .expect("idle transmitter");
    let parts = PortParts {
        storage,
        transmitter,
        buffers,
        config: crate::lower_mac::LowerMacConfig {
            station_address: STATION,
            channel: channel(6),
            publication_timeout: PUBLICATION_BOUND,
            tsf_epoch: 17,
        },
        access_point_address: IndividualAddress::new(ACCESS_POINT).expect("AP address"),
        coex: &RadioNine,
        observations: Draws(observed.borrow().draws.clone()),
    };
    (parts, observed, timer)
}

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    for _ in 0..1000 {
        if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
            return result;
        }
    }
    panic!("port made no bounded progress")
}

enum Report<Out> {
    Uninstalled {
        output: Out,
    },
    FailStopped {
        output: Out,
        stopped: bool,
    },
    /// The port fail-stopped before the body returned its output.
    Abandoned,
}

fn run_body<B, Out>(
    parts: PortParts<TestDevice, 512, Draws>,
    body: B,
) -> impl Future<Output = Report<Out>>
where
    for<'scope> B: PortBody<radio::LocalId<'scope>, Backend<radio::LocalId<'scope>>, Output = Out>,
{
    radio::with_local_slot(async move |claim| {
        let mut slot = claim.place();
        let backend = Backend::new(parts).ok().expect("valid construction");
        let registered = slot.register(backend).ok().expect("fresh local slot");
        match radio::LocalExecution
            .scope(crate::port::Registration::guarded(registered), |frontend| {
                radio::run_port(frontend, body)
            })
            .await
        {
            radio::PortEnd::Uninstalled { joined, output } => {
                drop(joined);
                Report::Uninstalled { output }
            }
            radio::PortEnd::FailStopped { outcome, output } => Report::FailStopped {
                output,
                stopped: matches!(outcome.outcome(), radio::Abandon::Stopped(_)),
            },
            radio::PortEnd::Abandoned { .. } => Report::Abandoned,
        }
    })
}

struct Exchange {
    transmissions: u8,
    expected: usize,
    close: bool,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for Exchange {
    type Output = Vec<SingleOutcome<Ack, Infallible>>;
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Self::Output>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        let mut resources = resources;
        let mut outcomes = Vec::new();
        {
            let mut phase = owner.phase();
            let permission = resources
                .station_commands
                .configure
                .with_context(Tag(42, None));
            phase.station().configure(
                permission,
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(
                    Configured::Station(permission),
                )))) => assert_eq!(permission.into_parts().1, 42),
                _ => panic!("configuration terminal"),
            }
            let rate = phase
                .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
                .expect("supported rate");
            let [buffer, ..] = resources.buffers;
            let [ticket, ..] = resources.tickets;
            let header = HeaderSpec::new(
                FrameControl::new(oer_ieee80211_wire::frame::DATA_TO_DS).expect("frame control"),
                0,
                IndividualAddress::new(BSSID).expect("receiver"),
                STATION,
                BSSID,
                0x1230,
                None,
                None,
                None,
            )
            .expect("header");
            let frame = ClassFrame::new(TxFrame::Individual {
                frame: SizedFrame::new(
                    AckHeader::new(header).expect("ACK header"),
                    MpduLength::new(64).expect("length"),
                )
                .expect("sized frame"),
                plan: RetryPlan::new(RetryStage {
                    rate,
                    transmissions: core::num::NonZeroU8::new(self.transmissions)
                        .expect("retry count"),
                    protection: Protection::None,
                }),
            })
            .ok()
            .expect("class length");
            let source = TxSource::Fresh {
                buffer,
                frame,
                body: None,
                protection: TxProtection::Open,
            }
            .check()
            .ok()
            .expect("checked source");
            let prepared = <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<
                Station,
                Caps,
            >(source)
            .ok()
            .expect("prepared source");
            if self.close {
                phase.close_gate(resources.gate);
                match phase.next_event().await {
                    Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(gate)))) => {
                        resources.gate = phase.open_gate(gate);
                    }
                    _ => panic!("gate terminal"),
                }
            }
            phase
                .station()
                .submit(
                    Attempt {
                        context: Tag(71, None),
                        ticket,
                        source: prepared,
                        access_category: oer_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
                        protection: Protection::None,
                        power: SupportedPower::new(TxPower::Calibrated).expect("power"),
                        coex_operation: TxOperation::Data,
                    }
                    .admit(),
                )
                .expect("submit");
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(completed),
                )))) => {
                    let returned = completed.into_parts(&mut resources.terminal_access);
                    assert_eq!(returned.context, 71);
                    assert_eq!(returned.ticket.index(), 0);
                    let (buffer, body) = returned.payload.into_parts();
                    assert!(body.is_none());
                    let AnyBuffer::Sent(sent) = buffer else {
                        panic!("published MPDU returns Sent")
                    };
                    let (mut buffer, body) =
                        <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::renew(sent);
                    assert!(body.is_none());
                    assert_eq!(
                        &buffer.bytes_mut().expect("returned memory")[22..24],
                        &0x1230_u16.to_le_bytes()
                    );
                    outcomes.push(returned.outcome);
                }
                _ => panic!("TX terminal"),
            }
        }
        assert_eq!(outcomes.len(), self.expected);
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), outcomes)
    }
}

#[test]
fn port_returns_actual_operation_after_ack_and_gate_cycle() {
    let (backend, observed, _) = installed(Vec::new());
    let result = block_on(run_body(
        backend,
        Exchange {
            transmissions: 1,
            expected: 1,
            close: true,
        },
    ));
    let Report::Uninstalled { output, .. } = result else {
        panic!("clean uninstall")
    };
    assert!(output[0].is_delivered());
    let observed = observed.borrow();
    assert_eq!(observed.publications, 1);
    assert_eq!(observed.starts, 1);
    assert_eq!(observed.stops, 1);
    assert!(observed.detached);
    assert!(!observed.abandoned);
}

#[test]
fn port_exhausts_supplied_retry_plan_with_one_terminal() {
    let (backend, observed, _) = installed(std::vec![(5, 0); 3]);
    let result = block_on(run_body(
        backend,
        Exchange {
            transmissions: 3,
            expected: 1,
            close: false,
        },
    ));
    let Report::Uninstalled { output, .. } = result else {
        panic!("clean uninstall")
    };
    assert!(
        matches!(output[0], TxOutcome::Ended(SingleEnd::AckTimeout(air)) if air.transmissions.get() == 3)
    );
    assert_eq!(observed.borrow().publications, 3);
    // Standard BE CWmin=15 doubles after each unanswered publication;
    // the fixed entropy selects 8, 24 and 56 slots from those windows.
    assert_eq!(observed.borrow().backoffs, [8, 24, 56]);
}

/// Install the station PTK for the BSS and take the key its installation
/// terminal returns.
async fn installed_pairwise<'p, 'id: 'p, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, TestDevice, 512, Draws>,
    association: &<PortKeys<Id, Station> as KeyTypes<'id>>::Association,
    slot: <PortKeys<Id, Station> as KeyTypes<'id>>::PairwiseSlot,
    material: u8,
) -> PairwiseKey<'id, Id, Station> {
    let installation = phase.station().install_pairwise_key(
        association,
        slot,
        IndividualAddress::new(BSSID).expect("peer"),
        oer_ieee80211_contracts_security::Ccmp128Key::new(&[material; 16]),
    );
    match phase.next_event().await {
        Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::PairwiseKeyInstalled(
            InstalledPairwise::Station(key),
        )))) => {
            // The terminal's key is this installation's.
            assert!(
                <PortKeys<Id, Station> as KeyTypes<'id>>::pairwise_identity(&key) == installation
            );
            key
        }
        _ => panic!("the installation terminal returns the key"),
    }
}

fn submit_mpdu<'k, 'p, 'id: 'p + 'k, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, TestDevice, 512, Draws>,
    buffer: Buffer<'id, Id, TestDevice, 512, Draws>,
    ticket: radio::Slot<'id, Tickets, radio::Child<radio::Root<Id>, 2>>,
    body: Option<Payload>,
    key: Option<&'k PairwiseKey<'id, Id, Station>>,
) {
    submit_mpdu_budget(phase, buffer, ticket, body, key, 1);
}
fn submit_mpdu_budget<'k, 'p, 'id: 'p + 'k, Id: radio::SlotIdentity + 'id>(
    phase: &mut Esp32s31Phase<'p, 'id, Id, TestDevice, 512, Draws>,
    buffer: Buffer<'id, Id, TestDevice, 512, Draws>,
    ticket: radio::Slot<'id, Tickets, radio::Child<radio::Root<Id>, 2>>,
    body: Option<Payload>,
    key: Option<&'k PairwiseKey<'id, Id, Station>>,
    budget: u8,
) {
    let rate = phase
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .expect("rate");
    let header = HeaderSpec::new(
        FrameControl::new(oer_ieee80211_wire::frame::DATA_TO_DS).expect("control"),
        0,
        IndividualAddress::new(BSSID).expect("receiver"),
        STATION,
        BSSID,
        0x4560,
        None,
        None,
        None,
    )
    .expect("header");
    let frame = ClassFrame::new(TxFrame::Individual {
        frame: SizedFrame::new(
            AckHeader::new(header).expect("ACK"),
            MpduLength::new(64).expect("length"),
        )
        .expect("frame"),
        plan: RetryPlan::new(RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::new(budget).expect("nonzero test budget"),
            protection: Protection::None,
        }),
    })
    .ok()
    .expect("class");
    let source = TxSource::Fresh {
        buffer,
        frame,
        body,
        protection: match key {
            Some(key) => TxProtection::Ccmp(NumberedKey {
                key,
                packet_number: oer_ieee80211_wire::ccmp::CcmpPacketNumber::new(7).expect("PN"),
            }),
            None => TxProtection::Open,
        },
    }
    .check()
    .ok()
    .expect("layout");
    let source =
        <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<Station, Caps>(source)
            .ok()
            .expect("prepare");
    phase
        .station()
        .submit(
            Attempt {
                context: Tag(91, None),
                ticket,
                source,
                access_category: oer_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
                protection: Protection::None,
                power: SupportedPower::new(TxPower::Calibrated).expect("power"),
                coex_operation: TxOperation::Data,
            }
            .admit(),
        )
        .expect("submit");
}

#[derive(Clone, Copy, Debug)]
enum Suspension {
    Enable,
    Publication,
    Group,
    Disable,
}
struct SuspendBody {
    at: Suspension,
    dropped: Rc<Cell<usize>>,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for SuspendBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        match self.at {
            Suspension::Publication => {
                let mut phase = owner.phase();
                let [buffer, ..] = resources.buffers;
                let [ticket, ..] = resources.tickets;
                submit_mpdu(
                    &mut phase,
                    buffer,
                    ticket,
                    Some(Payload(self.dropped)),
                    None,
                );
                let _ = phase.next_event().await;
            }
            Suspension::Group => {
                let mut phase = owner.phase();
                let (start, bank) = resources.station_association.into_parts();
                let (_, [slot]) = bank.into_parts();
                let association = start.begin(oer_ieee80211_wire::security::Spp::NotNegotiated);
                phase.station().install_group_key(
                    &association,
                    resources.station_group_permission,
                    slot,
                    oer_ieee80211_wire::ccmp::GroupKeyId::new(1).expect("KeyID"),
                    oer_ieee80211_contracts_security::Ccmp128Key::new(&[3; 16]),
                );
                let _ = phase.next_event().await;
            }
            _ => {}
        }
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn cancelling_each_device_wait_quarantines_without_dropping_an_admitted_body() {
    for at in [
        Suspension::Enable,
        Suspension::Publication,
        Suspension::Group,
        Suspension::Disable,
    ] {
        let (backend, observed, _) = installed(Vec::new());
        {
            let mut observed = observed.borrow_mut();
            match at {
                Suspension::Enable => observed.hold_start = true,
                Suspension::Publication => observed.hold_completion = true,
                Suspension::Group => observed.hold_cut = true,
                Suspension::Disable => observed.hold_stop = true,
            }
        }
        let dropped = Rc::new(Cell::new(0));
        let mut future = Box::pin(run_body(
            backend,
            SuspendBody {
                at,
                dropped: dropped.clone(),
            },
        ));
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..12 {
            assert!(future.as_mut().poll(&mut cx).is_pending(), "wait {at:?}");
        }
        if matches!(at, Suspension::Publication) {
            assert_eq!(observed.borrow().publications, 1);
        }
        if matches!(at, Suspension::Group) {
            assert!(observed.borrow().cut_calls > 0);
        }
        drop(future);
        assert!(observed.borrow().abandoned, "wait {at:?}");
        assert_eq!(dropped.get(), 0);
        assert!(!observed.borrow().detached);
    }
}

struct GroupCycle;
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for GroupCycle {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        {
            let mut phase = owner.phase();
            let (start, bank) = resources.station_association.into_parts();
            let (_, [slot]) = bank.into_parts();
            let association = start.begin(oer_ieee80211_wire::security::Spp::NotNegotiated);
            let id = |id| oer_ieee80211_wire::ccmp::GroupKeyId::new(id).expect("KeyID");
            phase.station().install_group_key(
                &association,
                resources.station_group_permission,
                slot,
                id(1),
                oer_ieee80211_contracts_security::Ccmp128Key::new(&[1; 16]),
            );
            let (key, permission) = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
                    InstalledGroup::Station { key, permission },
                )))) => (key, permission),
                _ => panic!("installed"),
            };
            phase.station().replace_group_key(
                permission,
                key,
                id(2),
                oer_ieee80211_contracts_security::Ccmp128Key::new(&[2; 16]),
            );
            let (key, permission) = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyReplaced {
                    key,
                    permission,
                }))) => (key, permission),
                _ => panic!("replaced"),
            };
            phase.station().select_group_key(permission, key);
            let (key, permission) = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeySelected {
                    key,
                    permission,
                }))) => (key, permission),
                _ => panic!("selected"),
            };
            phase.station().remove_group_key(permission, key);
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(
                    Removed::Station(RemovedKey::Group { .. })
                ))))
            ));
        }
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn every_group_mutation_has_a_finite_rx_cut_and_returns_its_permission_once() {
    let (backend, observed, _) = installed(Vec::new());
    assert!(matches!(
        block_on(run_body(backend, GroupCycle)),
        Report::Uninstalled { .. }
    ));
    assert_eq!(observed.borrow().resumed_cuts, 4);
}

struct Expire {
    clock: TestTimer,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for Expire {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        mut resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        {
            let mut phase = owner.phase();
            phase.close_gate(resources.gate);
            let gate = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(gate)))) => gate,
                _ => panic!("gate"),
            };
            let [buffer, ..] = resources.buffers;
            let [ticket, ..] = resources.tickets;
            submit_mpdu(&mut phase, buffer, ticket, None, None);
            self.clock
                .0
                .advance(EdcaParameterSet::DEFAULT_MSDU_LIFETIME)
                .expect("advance clock");
            let completed = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(completed),
                )))) => completed,
                _ => panic!("expiry"),
            };
            let returned = completed.into_parts(&mut resources.terminal_access);
            assert!(matches!(returned.outcome, TxOutcome::Expired { msdus: 1 }));
            let (memory, _) = returned.payload.into_parts();
            assert!(matches!(memory, AnyBuffer::Fresh(_)));
            resources.gate = phase.open_gate(gate);
        }
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), ())
    }
}
#[test]
fn queued_msdu_expires_from_submit_without_publication_behind_a_closed_gate() {
    let (backend, observed, clock) = installed(Vec::new());
    assert!(matches!(
        block_on(run_body(backend, Expire { clock })),
        Report::Uninstalled { .. }
    ));
    assert_eq!(observed.borrow().publications, 0);
}

struct ReceiveBody {
    observed: Rc<RefCell<Observed>>,
    released: Rc<Cell<usize>>,
    lose: bool,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for ReceiveBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        {
            let mut phase = owner.phase();
            phase.station().configure(
                resources
                    .station_commands
                    .configure
                    .with_context(Tag(7, None)),
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_))))
            ));
            self.observed.borrow_mut().received.push_back(TestReceive {
                bytes: super::data_frame(STATION),
                released: self.released.clone(),
            });
            let frame = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Received { frame, meta })) => {
                    assert_eq!(meta.common().rate, RxEvidence::Unavailable);
                    assert_eq!(meta.common().rssi_dbm, RxEvidence::HardwareObserved(-42));
                    frame
                }
                _ => panic!("received MPDU"),
            };
            assert_eq!(frame.bytes(), &super::data_frame(STATION));
            assert_eq!(self.released.get(), 0);
            if self.lose {
                drop(frame);
                assert!(matches!(
                    phase.next_event().await,
                    Err(radio::EventEnd::Quarantined { .. })
                ));
            } else {
                phase.recycle(frame);
            }
        }
        if self.lose {
            return radio::Ended::fail_stop(owner.fail_stop(), ());
        }
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        assert_eq!(self.released.get(), 1);
        radio::Ended::uninstall(owner, (), ())
    }
}
#[test]
fn received_bss_mpdu_keeps_its_original_owner_until_explicit_recycling() {
    let (backend, observed, _) = installed(Vec::new());
    let released = Rc::new(Cell::new(0));
    assert!(matches!(
        block_on(run_body(
            backend,
            ReceiveBody {
                observed,
                released: released.clone(),
                lose: false
            }
        )),
        Report::Uninstalled { .. }
    ));
    assert_eq!(released.get(), 1);
}
#[test]
fn dropping_a_received_owner_quarantines_its_allocation_and_poison_is_permanent() {
    let (backend, observed, _) = installed(Vec::new());
    let released = Rc::new(Cell::new(0));
    let result = block_on(run_body(
        backend,
        ReceiveBody {
            observed: observed.clone(),
            released: released.clone(),
            lose: true,
        },
    ));
    assert!(matches!(
        result,
        Report::FailStopped {
            output: (),
            stopped: false
        }
    ));
    drop(result);
    assert_eq!(released.get(), 0);
    assert!(observed.borrow().abandoned);
}

struct Protected {
    dropped: Rc<Cell<usize>>,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for Protected {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        mut resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        {
            let mut phase = owner.phase();
            let (start, bank) = resources.station_association.into_parts();
            let ([slot], _) = bank.into_parts();
            let association = start.begin(oer_ieee80211_wire::security::Spp::NotNegotiated);
            phase.station().configure(
                resources
                    .station_commands
                    .configure
                    .with_context(Tag(19, None)),
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_))))
            ));
            let key = installed_pairwise(&mut phase, &association, slot, 9).await;
            let [buffer, ..] = resources.buffers;
            let [ticket, ..] = resources.tickets;
            submit_mpdu(
                &mut phase,
                buffer,
                ticket,
                Some(Payload(self.dropped.clone())),
                Some(&key),
            );
            let completed = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                    TxCompleted::Single(completed),
                )))) => completed,
                _ => panic!("protected TX"),
            };
            let returned = completed.into_parts(&mut resources.terminal_access);
            assert!(returned.outcome.is_delivered());
            assert_eq!(self.dropped.get(), 0);
            let (buffer, body) = returned.payload.into_parts();
            assert!(body.is_none());
            let AnyBuffer::Sent(mpdu) = buffer else {
                panic!("Sent")
            };
            assert_eq!(mpdu.packet_number().expect("caller PN").value(), 7);
            assert_eq!(mpdu.body().expect("retained body").bytes(), b"payload");
            phase.station().remove_pairwise_key(association, key);
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(
                    Removed::Station(RemovedKey::Pairwise { .. })
                ))))
            ));
            let rate = phase
                .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
                .expect("rate");
            let source = TxSource::Retransmission {
                mpdu,
                plan: RetransmissionPlan::Individual(RetryPlan::new(RetryStage {
                    rate,
                    transmissions: core::num::NonZeroU8::MIN,
                    protection: Protection::None,
                })),
            }
            .check()
            .ok()
            .expect("layout");
            let refused = <Family<Id> as TxBufferPool<'id, BufferClass<512>>>::prepare::<
                Station,
                Caps,
            >(source)
            .err()
            .expect("retired key refused before admission");
            assert_eq!(refused.reason, PreparationError::RetransmissionKey);
            drop(refused);
            assert_eq!(self.dropped.get(), 1);
        }
        let owner = match owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), ())
    }
}
#[test]
fn caller_pn_and_external_body_survive_tx_and_a_removed_key_refuses_retransmission() {
    let (backend, observed, _) = installed(Vec::new());
    let dropped = Rc::new(Cell::new(0));
    assert!(matches!(
        block_on(run_body(
            backend,
            Protected {
                dropped: dropped.clone()
            }
        )),
        Report::Uninstalled { .. }
    ));
    assert_eq!(dropped.get(), 1);
    assert_eq!(observed.borrow().publications, 1);
}

#[test]
fn emergency_erasure_reaches_the_last_ap_peer_as_well_as_station_and_group_keys() {
    use crate::mac::crypto::*;
    let mut hardware = Hardware::default();
    let station = install_sta_pairwise_ccmp(&mut hardware, BSSID, &[1; 16]).expect("STA PTK");
    let group = install_sta_group_ccmp(
        &mut hardware,
        oer_ieee80211_wire::ccmp::GroupKeyId::new(0).expect("KeyID"),
        &[2; 16],
    )
    .expect("GTK");
    let ap = install_ap_pairwise_ccmp(
        &mut hardware,
        super::PEER,
        u16::from(AP_PAIRWISE_SLOT_COUNT),
        &[3; 16],
    )
    .expect("last AP AID");
    let ap_group = install_ap_group_ccmp(
        &mut hardware,
        ApGroupKeyId::new(1).expect("AP KeyID"),
        &[4; 16],
    )
    .expect("AP GTK");
    assert_eq!(hardware.installed_keys.len(), 4);
    erase_lower_mac_keys(&mut hardware);
    assert!(hardware.installed_keys.is_empty());
    for index in [
        station.hardware_index(),
        group.hardware_index(),
        ap.hardware_index(),
        ap_group.hardware_index(),
    ] {
        assert!(hardware.cleared_keys.contains(&index));
    }
}

#[test]
fn an_outstanding_dma_allocation_refuses_installation_with_a_typed_error() {
    let (parts, observed, _) = installed(Vec::new());
    let refused = block_on(radio::with_local_slot(async |claim| {
        let mut slot = claim.place();
        let backend = Backend::new(parts).ok().expect("valid construction");
        assert!(backend.withhold_dma(3));
        match crate::port::Registration::guard_result(slot.register(backend)) {
            Err(crate::port::RegistrationError::Initialization {
                cause,
                registration,
            }) => {
                drop(registration);
                Some(cause)
            }
            _ => None,
        }
    }));
    assert_eq!(
        refused,
        Some(radio::SlotInitError::MissingResource(
            "Wi-Fi installation resources"
        )),
        "no endpoint is published without every buffer's DMA"
    );
    assert_eq!(observed.borrow().starts, 0, "no radio start");
}

#[test]
fn clean_uninstall_returns_all_dma_and_core_owners_for_a_fresh_installation() {
    let (parts, observed, _) = installed(Vec::new());
    let _ = block_on(reinstalled(
        parts,
        Exchange {
            transmissions: 1,
            expected: 1,
            close: true,
        },
        Exchange {
            transmissions: 1,
            expected: 1,
            close: false,
        },
    ));
    assert_eq!(observed.borrow().publications, 2);
    assert_eq!(observed.borrow().starts, 2);
    assert_eq!(observed.borrow().stops, 2);
}

struct Tune(Channel);
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for Tune {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        mut owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        _: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        owner.setup().set_channel(
            SupportedChannel::new(
                oer_radio_regulatory::RegulatoryDomain::of(
                    oer_radio_regulatory::Country::new(*b"DE").expect("test country"),
                )
                .expect("domain fixture")
                .permit(oer_ieee80211_contracts_port::RegulatedChannel::from(self.0))
                .expect("permitted test channel"),
            )
            .expect("supported test channel"),
        );
        let owner = owner.enable().await.ok().expect("enable");
        let owner = owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
            .ok()
            .expect("disable");
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn frontend_channel_setting_survives_normal_stop_and_reinstall() {
    let (parts, observed, _) = installed(Vec::new());
    let tuned = Channel::from_wifi_channel(channel(11));
    let _ = block_on(reinstalled(
        parts,
        Tune(tuned),
        Exchange {
            transmissions: 1,
            expected: 1,
            close: false,
        },
    ));
    assert_eq!(observed.borrow().start_channels, [channel(11), channel(11)]);
}

async fn reinstalled<First, Second, A, B>(
    parts: PortParts<TestDevice, 512, Draws>,
    first: First,
    second: Second,
) -> (A, B)
where
    for<'scope> First:
        PortBody<radio::LocalId<'scope>, Backend<radio::LocalId<'scope>>, Output = A>,
    for<'scope> Second:
        PortBody<radio::LocalId<'scope>, Backend<radio::LocalId<'scope>>, Output = B>,
{
    radio::with_local_slot(async move |claim| {
        let mut slot = claim.place();
        let backend = Backend::new(parts).ok().expect("valid construction");
        let registered = slot.register(backend).ok().expect("fresh local slot");
        let first = radio::LocalExecution
            .scope(crate::port::Registration::guarded(registered), |frontend| {
                radio::run_port(frontend, first)
            })
            .await;
        let radio::PortEnd::Uninstalled {
            joined,
            output: first,
        } = first
        else {
            panic!("first installation joins");
        };
        let registered = slot
            .reinstall(joined, |stopped| {
                stopped
                    .reinstall(18)
                    .ok()
                    .expect("all buffers and core retained in the joined slot")
            })
            .ok()
            .expect("one matching Joined authorizes reinstall");
        let second = radio::LocalExecution
            .scope(crate::port::Registration::guarded(registered), |frontend| {
                radio::run_port(frontend, second)
            })
            .await;
        let radio::PortEnd::Uninstalled {
            joined,
            output: second,
        } = second
        else {
            panic!("second installation joins");
        };
        drop(joined);
        (first, second)
    })
    .await
}

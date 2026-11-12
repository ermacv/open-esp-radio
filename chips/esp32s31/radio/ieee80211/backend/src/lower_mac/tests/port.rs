use super::{ACCESS_POINT, BSSID, Hardware, Power, RadioNine, STATION, channel, entropy, slot};
use crate::port::*;
use core::{
    convert::Infallible,
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_portable_time::{Clock, Instant, Timer};
use oer_protocols_ieee80211_contracts_port::*;
use oer_protocols_ieee80211_wire::{
    address::IndividualAddress,
    frame::FrameControl,
    phy::{LegacyRate, PhyRate},
};
use oer_protocols_radio_port as radio;
use std::{
    boxed::Box,
    cell::{Cell, RefCell},
    rc::Rc,
    vec::Vec,
};

#[derive(Clone)]
struct TestTimer(Rc<oer_portable_time_virtual::VirtualClock>, Rc<Cell<bool>>);
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

/// The raw completions the model device reports, by `Observed::statuses`.
const SUCCESS: (u8, u8) = (0, 0);
const ACK_TIMEOUT: (u8, u8) = (5, 0);
const CTS_TIMEOUT: (u8, u8) = (2, 0);
const COLLISION: (u8, u8) = (4, 1);
const KEY_REFUSED: (u8, u8) = (4, 0xc0);
/// No completion: the hardware's internal collision detaches the queue.
const COLLISION_DETACH: (u8, u8) = (u8::MAX, u8::MAX);
/// No completion: the hardware's TX-timeout edge, which starts the abort.
const TIMEOUT_EDGE: (u8, u8) = (u8::MAX, 0);

#[derive(Default)]
struct Observed {
    /// The device's registers are held elsewhere: `hardware` refuses.
    hardware_unavailable: bool,
    /// What the P2 driver reported at each publication's contention draw.
    draws: Rc<RefCell<Vec<crate::lower_mac::ContentionDraw>>>,
    check_context: bool,
    context: u8,
    device_polls: usize,
    publications: usize,
    statuses: Vec<(u8, u8)>,
    starts: usize,
    start_channels: Vec<oer_protocols_ieee80211_wire::channel::Ghz2_4Channel>,
    stops: usize,
    detached: bool,
    /// Times the driver rearmed units the network returned.
    returned: usize,
    hold_start: bool,
    /// The device's start begins (its receive path runs) before it holds
    /// or fails.
    begin_start: bool,
    /// The next start fails with this fault, as a refused retune does.
    fail_start: Option<Fault>,
    hold_completion: bool,
    /// Hardware queues whose publications do not complete yet.
    held_queues: [bool; 4],
    /// The next device poll fails with this fault, poisoning the port.
    fail_poll: Option<Fault>,
    /// Key writes the hardware rejects from the next register access on.
    reject_key_writes: usize,
    /// The key table reads every entry valid however it was cleared.
    key_entries_stuck: bool,
    /// The receive Block Ack banks refuse every agreement.
    reject_rx_block_acks: bool,
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
    /// Key places the registers cleared.
    key_clears: usize,
    /// The next publication ends in the hardware's TX-timeout edge instead
    /// of a completion.
    timeout_edge: bool,
    /// An abort finds every queue's VALID clear: the hardware took the PPDU.
    taken_by_air: bool,
    /// Publications are aggregates: each completes with this BlockAck's
    /// starting sequence, bitmap and whether it arrived, beside its status.
    block_ack: Option<(u16, u64, bool)>,
    hold_cut: bool,
    /// A held cut waits for units the network holds.
    cut_by_units: bool,
    hold_stop: bool,
    /// The device's next stop poll answers this fault, once.
    fail_stop: Option<Fault>,
    cut_calls: usize,
    resumed_cuts: usize,
    /// Cuts ended for a mutation that did not run.
    abandoned_cuts: usize,
    backoffs: Vec<u16>,
    /// Each HT publication's backoff and AIFSN, in order.
    ht_contention: Vec<(u16, u8)>,
    /// Each legacy publication program's AIFSN, in publication order.
    aifsns: Vec<u8>,
    received: std::collections::VecDeque<TestReceive>,
    continuous_rx: bool,
    rx_continuation: Option<Instant>,
    rx_delivered: usize,
    /// The body's pulls that reached the device.
    pulls: usize,
    /// The driver's event polls since the harness last polled its task.
    driver_steps: usize,
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
    /// The driver (context 2) and the body (context 1) both reach the
    /// device, each in its own task on the station's executor.
    fn assert_task_context(&self) {
        let observed = self.observed.borrow();
        if observed.check_context {
            assert!(
                matches!(observed.context, 1 | 2),
                "device access belongs to the station's executor tasks"
            );
        }
    }
    /// What the air delivered reaches the ring under the key material
    /// active at capture.
    fn capture(&self) {
        let mut observed = self.observed.borrow_mut();
        observed.key_writes = self
            .hardware
            .crypto_order
            .iter()
            .filter(|operation| matches!(operation, super::CryptoOperation::Install(_)))
            .count();
        observed.key_clears = self.hardware.cleared_keys.len();
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
    }
}
struct TestReceive {
    bytes: [u8; 26],
    /// The view in `bytes`, narrowed by the driver to the MPDU.
    view: core::ops::Range<usize>,
    released: std::sync::Arc<core::sync::atomic::AtomicUsize>,
}
impl ReceiveUnit for TestReceive {
    fn normalized(&self) -> Result<crate::mac::rx::NormalizedRxFrame<'_>, crate::mac::rx::RxError> {
        Ok(super::normalized(&self.bytes))
    }
    fn bytes(&self) -> &[u8] {
        &self.bytes[self.view.clone()]
    }
    fn narrow(
        &mut self,
        offset: usize,
        length: usize,
    ) -> Result<(), oer_portable_memory::FrameRangeError> {
        let start = self.view.start + offset;
        if start + length > self.view.end {
            return Err(oer_portable_memory::FrameRangeError {
                offset,
                length,
                initialized_offset: self.view.start,
                initialized_length: self.view.len(),
            });
        }
        self.view = start..start + length;
        Ok(())
    }
    fn storage(&self) -> &[u8] {
        &self.bytes[self.view.start..]
    }
    fn storage_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[self.view.start..]
    }
}
impl Drop for TestReceive {
    fn drop(&mut self) {
        self.released
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}
impl Device<512> for TestDevice {
    type Power = Power;
    type Coex = RadioNine;
    type Entropy = fn() -> u32;
    type Timer = TestTimer;
    type Hardware = Hardware;
    type Receive = TestReceive;
    type Lent = crate::port::SameHart;
    type Body = Payload;
    const LENT_RX: usize = 2;
    type OwnedContext<Id: radio::SlotIdentity> = Tag;
    type Context<'p, Id: radio::SlotIdentity + 'p> = Tag;
    type OwnedConfigureContext<Id: radio::SlotIdentity> = Tag;
    type ConfigureContext<'p, Id: radio::SlotIdentity + 'p> = Tag;
    fn hardware(&mut self) -> Result<&mut Hardware, Fault> {
        self.assert_task_context();
        // The clears every earlier call wrote.
        self.observed.borrow_mut().key_clears = self.hardware.cleared_keys.len();
        if self.observed.borrow().hardware_unavailable {
            return Err(Fault::HardwareUnavailable);
        }
        self.hardware.reject_key_writes +=
            core::mem::take(&mut self.observed.borrow_mut().reject_key_writes);
        self.hardware.reject_rx_block_acks = self.observed.borrow().reject_rx_block_acks;
        self.hardware.key_entries_stuck = self.observed.borrow().key_entries_stuck;
        Ok(&mut self.hardware)
    }
    fn next_deadline(&self) -> Option<Instant> {
        // As the device's: a due continuation is the body's to take.
        self.observed
            .borrow()
            .rx_continuation
            .filter(|at| self.timer.now() < *at)
    }
    fn event_waiting(&self) -> bool {
        false
    }
    fn take_power_profile(&mut self) -> Option<Self::Power> {
        None
    }
    fn radio_now(&mut self) -> Result<Ieee80211Instant, radio::ClockError> {
        self.assert_task_context();
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
        channel: oer_protocols_ieee80211_wire::channel::Ghz2_4Channel,
    ) -> Poll<Result<(), Fault>> {
        self.assert_task_context();
        if self.observed.borrow().begin_start {
            self.stopped = false;
        }
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
        self.assert_task_context();
        if self.observed.borrow().hold_stop {
            return Poll::Pending;
        }
        if let Some(fault) = self.observed.borrow_mut().fail_stop.take() {
            return Poll::Ready(Err(fault));
        }
        if !self.stopped {
            self.observed.borrow_mut().stops += 1;
            self.stopped = true;
        }
        Poll::Ready(Ok(()))
    }
    fn poll_event(&mut self, _: &mut Context<'_>) -> Poll<Result<DeviceEvent, Fault>> {
        self.assert_task_context();
        if let Some(fault) = self.observed.borrow_mut().fail_poll.take() {
            return Poll::Ready(Err(fault));
        }
        self.capture();
        let mut observed = self.observed.borrow_mut();
        observed.device_polls += 1;
        if observed.continuous_rx {
            observed.driver_steps += 1;
            assert!(
                observed.driver_steps <= 64,
                "the driver must return to its executor"
            );
        }
        self.hardware.taken_by_air = [observed.taken_by_air; 4];
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
        self.observed.borrow_mut().ht_contention = self
            .hardware
            .ht
            .iter()
            .map(|(_, program)| (program.contention_window(), program.aifsn()))
            .collect();
        self.observed.borrow_mut().publications = publications;
        self.done.resize(publications, false);
        let next = self.next_completion(publications);
        if let Some((index, queue)) = next
            && self.observed.borrow().statuses.get(self.completed) == Some(&COLLISION_DETACH)
        {
            self.hardware.collision_pending[usize::from(queue)] = true;
            self.done[index] = true;
            self.completed += 1;
            return Poll::Ready(Ok(DeviceEvent::Tx(crate::tx::WifiTxWake::Interrupt {
                events: crate::mac::irq::event_mask(
                    oer_chips_esp32s31_hal::types::MacInterruptEvents::TX_QUEUE_COLLISION,
                ),
            })));
        }
        if let Some((index, queue)) = next
            && (self.observed.borrow().timeout_edge
                || self.observed.borrow().statuses.get(self.completed) == Some(&TIMEOUT_EDGE))
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
            let completion = oer_chips_esp32s31_hal::types::MacTxCompletionObservation::new_model(
                status, detail,
            );
            match self.observed.borrow().block_ack {
                Some((start, bitmap, received)) => {
                    self.hardware.block_ack_completion[usize::from(queue)] = Some(
                        oer_chips_esp32s31_hal::types::MacHtAmpduCompletionObservation::new_model(
                            completion, 0, start, bitmap, received,
                        ),
                    );
                }
                None => self.hardware.completion[usize::from(queue)] = Some(completion),
            }
            self.done[index] = true;
            self.completed += 1;
            Poll::Ready(Ok(DeviceEvent::Tx(crate::tx::WifiTxWake::Interrupt {
                events: crate::mac::irq::EVENT_TX_COMPLETE,
            })))
        } else {
            Poll::Pending
        }
    }
    fn cut_held_by_units(&self) -> bool {
        self.observed.borrow().cut_by_units
    }
    fn poll_rx_cut(
        &mut self,
        _: usize,
        _: &mut Context<'_>,
    ) -> Poll<Result<Option<TestReceive>, Fault>> {
        self.assert_task_context();
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
        self.assert_task_context();
        self.observed.borrow_mut().resumed_cuts += 1;
        Ok(())
    }
    fn poll_abandon_rx_cut(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Fault>> {
        self.assert_task_context();
        self.observed.borrow_mut().abandoned_cuts += 1;
        Poll::Ready(Ok(()))
    }
    fn returns_quiet(&mut self, _held: usize, _returned: bool) -> bool {
        self.observed.borrow().rx_continuation.is_some()
    }
    fn poll_rx_work(&mut self, _: &mut Context<'_>) -> Result<bool, Fault> {
        self.capture();
        Ok(!self.observed.borrow().received.is_empty())
    }
    fn pull_batch(&mut self, receive_room: usize, now: Instant) -> Result<DevicePull, Fault> {
        self.capture();
        let mut observed = self.observed.borrow_mut();
        observed.pulls += 1;
        if receive_room == 0 {
            // The caller takes no unit: the ring's continuation waits for
            // its return, not for the alarm.
            observed.rx_continuation = None;
            return Ok(DevicePull::Empty);
        }
        // The ring waits for its continuation, as the device's does.
        match observed.rx_continuation {
            Some(at) if now < at => return Ok(DevicePull::Later(at)),
            Some(_) => observed.rx_continuation = None,
            None => {}
        }
        let mut ready = observed.received.len().min(receive_room);
        if observed.continuous_rx {
            // A continuous ring pauses after every fourth unit.
            ready = ready.min(4 - observed.rx_delivered % 4);
        }
        if ready == 0 {
            return Ok(DevicePull::Empty);
        }
        Ok(DevicePull::Ready(ready))
    }
    fn take_staged(&mut self, now: Instant) -> Option<TestReceive> {
        let mut observed = self.observed.borrow_mut();
        let frame = observed.received.pop_front()?;
        if observed.continuous_rx {
            observed.rx_delivered += 1;
            if observed.rx_delivered.is_multiple_of(4) {
                observed.rx_continuation =
                    Some(now.saturating_add(crate::rx::receive::CONTINUATION_WAIT));
            }
        }
        Some(frame)
    }
    fn recycle(&mut self, frame: TestReceive) {
        self.assert_task_context();
        drop(frame);
    }
    fn returned(&mut self) {
        self.assert_task_context();
        self.observed.borrow_mut().returned += 1;
    }
    /// As the radio device does: the retired installation's keys leave the
    /// key table.
    fn detach(&mut self) {
        self.assert_task_context();
        crate::mac::crypto::erase_lower_mac_keys(&mut self.hardware);
        self.observed.borrow_mut().detached = true;
    }
}
/// The driver's observation receiver in these tests: it keeps every
/// contention draw in the fixture's `Observed::draws`.
struct Draws(Rc<RefCell<Vec<crate::lower_mac::ContentionDraw>>>);
impl oer_portable_events::EventSink<crate::lower_mac::ContentionDraw> for Draws {
    fn event(&mut self, draw: crate::lower_mac::ContentionDraw) {
        self.0.borrow_mut().push(draw);
    }
}
type Caps = Esp32s31Caps<512>;
type Family<Id> = Types<Id, TestDevice, 512, Draws>;

mod data_port;

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
        Rc::new(oer_portable_time_virtual::VirtualClock::new()),
        Rc::new(Cell::new(false)),
    );
    let observed = Rc::new(RefCell::new(Observed {
        statuses,
        ..Observed::default()
    }));
    let device = TestDevice {
        hardware,
        timer: timer.clone(),
        observed: observed.clone(),
        completed: 0,
        done: Vec::new(),
        stopped: false,
    };
    let device = Box::leak(Box::new(device));
    let shared = Box::leak(Box::new(crate::port::PortSharedStorage::new()));
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
        device,
        shared,
        transmitter,
        pool: crate::mac::tx::test_support::model_pool().expect("model pool"),
        bank: crate::port::TX_BUFFERS as u8,
        descriptors: descriptors(),
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

/// Model descriptors the port publishes its pool's buffers through: four
/// bare ordinary descriptors and the aggregate chains.
fn descriptors() -> crate::port::TxDescriptors<512> {
    let descriptor = || {
        core::pin::Pin::static_mut(Box::leak(Box::new(
            crate::mac::tx::TxSlot::new_model_descriptor().expect("model descriptor"),
        )))
    };
    let chain = || {
        crate::mac::tx::ampdu::RetainedDmaAmpduTx::new_model(
            core::pin::Pin::static_mut(Box::leak(Box::new(
                crate::mac::tx::ampdu::HtAmpduTxStorage::new(),
            ))),
            Box::leak(Box::new(
                crate::mac::tx::ampdu::RetainedAmpduDmaStorage::new(),
            )),
        )
        .ok()
        .expect("model chain")
    };
    crate::port::TxDescriptors {
        single: core::array::from_fn(|_| descriptor()),
        chains: core::array::from_fn(|_| chain()),
    }
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

pub(super) type Released = std::sync::Arc<core::sync::atomic::AtomicUsize>;
pub(super) fn count(released: &Released) -> usize {
    released.load(core::sync::atomic::Ordering::Relaxed)
}

#[test]
fn emergency_erasure_reaches_the_last_ap_peer_as_well_as_station_and_group_keys() {
    use crate::mac::crypto::*;
    let mut hardware = Hardware::default();
    let station = install_sta_pairwise_ccmp(&mut hardware, BSSID, &[1; 16]).expect("STA PTK");
    let group = install_sta_group_ccmp(
        &mut hardware,
        oer_protocols_ieee80211_wire::ccmp::GroupKeyId::new(0).expect("KeyID"),
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

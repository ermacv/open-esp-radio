//! The S31 executor snapshots readiness at a round's start, then polls
//! network before application. A wake during a round waits for the next.
//! These cases use the image's RX credit 12 and intake budget 8.

use super::*;
use oer_protocols_network_core::NotLent;
use oer_protocols_network_runtime::{Diagnosis, Diagnostics, Received};

#[derive(Default)]
struct Record {
    round: usize,
    consumer_polls: usize,
    delivered: Vec<usize>,
    published: Vec<usize>,
    accepted: usize,
    accepted_with_rx_pending: usize,
    device: Option<Device>,
    diagnosis: Diagnosis,
    trace: Vec<String>,
    prefixes: Vec<usize>,
}

#[derive(Clone, Default)]
struct Counted(Rc<RefCell<Record>>);
impl Diagnostics for Counted {
    fn on(&mut self) -> bool {
        true
    }
    fn clock(&mut self) -> u64 {
        0
    }
    fn add(&mut self, count: &Diagnosis) {
        let mut record = self.0.borrow_mut();
        record.diagnosis.accumulate(count);
        let round = record.round;
        record.trace.push(format!(
            "round {round}: collected {}, no_credit {}, loans at refusal {}, returned at refusal {}",
            count.credits_restored,
            count.no_credit,
            count.no_credit_loans,
            count.no_credit_unreturned
        ));
    }
}
impl EventSink<NetEvent> for Counted {
    fn event(&mut self, event: NetEvent) {
        assert_eq!(event, NetEvent::NotLent(NotLent::NoCredit));
    }
}
impl EventSink<ServiceEvent> for Counted {
    fn event(&mut self, event: ServiceEvent) {
        if event == ServiceEvent::Accepted {
            let mut record = self.0.borrow_mut();
            record.accepted += 1;
            if !record.device.as_ref().unwrap().0.borrow().frames.is_empty() {
                record.accepted_with_rx_pending += 1;
            }
        }
        if matches!(event, ServiceEvent::TurnBegan | ServiceEvent::ReceiveTurn) {
            let mut record = self.0.borrow_mut();
            let round = record.round;
            let polls = record.consumer_polls;
            let published = record.published.len();
            record.trace.push(format!(
                "round {round}: {event:?}, consumer polls {polls}, returns published {published}"
            ));
        }
    }
}

struct TaskWake {
    ready: Arc<AtomicUsize>,
    bit: usize,
}
impl Wake for TaskWake {
    fn wake(self: Arc<Self>) {
        self.ready.fetch_or(self.bit, Ordering::SeqCst);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.ready.fetch_or(self.bit, Ordering::SeqCst);
    }
}
struct Round {
    ready: Arc<AtomicUsize>,
    wakers: [Waker; 2],
    count: Counted,
    reverse: bool,
}
impl Round {
    fn new(count: Counted) -> Self {
        let ready = Arc::new(AtomicUsize::new(3));
        let wakers = [1, 2].map(|bit| {
            Waker::from(Arc::new(TaskWake {
                ready: ready.clone(),
                bit,
            }))
        });
        Self {
            ready,
            wakers,
            count,
            reverse: false,
        }
    }
    fn poll(&mut self, net: &mut Pin<Box<impl Future>>, app: &mut Pin<Box<impl Future>>) {
        let snapshot = self.ready.load(Ordering::Acquire);
        self.count.0.borrow_mut().round += 1;
        for index in if self.reverse { [1, 0] } else { [0, 1] } {
            let bit = 1 << index;
            if snapshot & bit == 0 || self.ready.fetch_and(!bit, Ordering::AcqRel) & bit == 0 {
                continue;
            }
            let cx = &mut Context::from_waker(&self.wakers[index]);
            if index == 0 {
                assert!(net.as_mut().poll(cx).is_pending());
            } else {
                self.count.0.borrow_mut().consumer_polls += 1;
                assert!(app.as_mut().poll(cx).is_pending());
            }
        }
        let mut record = self.count.0.borrow_mut();
        let device = record.device.as_ref().unwrap().0.borrow();
        let queued: Vec<_> = device
            .lent
            .iter()
            .filter(|id| !record.delivered.contains(id))
            .copied()
            .collect();
        let held: Vec<_> = record
            .delivered
            .iter()
            .filter(|id| !record.published.contains(id))
            .copied()
            .collect();
        let physical = device.returned.lock().unwrap().clone();
        let free = 12 - (device.lent.len() - record.diagnosis.credits_restored as usize);
        let uncollected = record.published.len() - record.diagnosis.credits_restored as usize;
        assert_eq!(12 - free, queued.len() + held.len() + uncollected);
        assert!(record.published.iter().all(|id| physical.contains(id)));
        assert!(held.iter().all(|id| !physical.contains(id)));
        drop(device);
        let round = record.round;
        let published = record.published.clone();
        record.trace.push(format!("round {round}: queued {queued:?}, held {held:?}, physical back {physical:?}, published {published:?}, credit free {free}"));
    }
    fn settle(&mut self, net: &mut Pin<Box<impl Future>>, app: &mut Pin<Box<impl Future>>) {
        for _ in 0..128 {
            if self.ready.load(Ordering::Acquire) == 0 {
                return;
            }
            self.poll(net, app);
        }
        panic!("tasks never waited: {:?}", self.count.0.borrow().trace);
    }
}

type Packet<B> = Received<'static, 'static, B, SIZE, LentFrame<Bytes>>;

async fn consume<B>(
    socket: &mut Socket<B>,
    count: Counted,
    keep: usize,
    held: Rc<RefCell<Vec<Packet<B>>>>,
) {
    loop {
        let mut packet = socket.receive().await.unwrap();
        let id = match &mut packet {
            Received::Lent(lent) => usize::from(lent.data().unwrap()[0]),
            Received::Copied(_) => panic!("the model lent this frame"),
        };
        count.0.borrow_mut().delivered.push(id);
        if held.borrow().len() < keep {
            held.borrow_mut().push(packet);
        } else {
            drop(packet); // physical memory first, then the real Returns::give.
            let mut record = count.0.borrow_mut();
            record.published.push(id);
            let round = record.round;
            record
                .trace
                .push(format!("round {round}: application returned {id}"));
        }
    }
}

async fn consume_prefix<B>(
    socket: &mut Socket<B>,
    count: Counted,
    keep: usize,
    held: Rc<RefCell<Vec<Packet<B>>>>,
) {
    loop {
        let mut published = Vec::new();
        let prefix = socket
            .receive_prefix(8, |cell| {
                let id = usize::from(cell.as_mut().unwrap().data().unwrap()[0]);
                count.0.borrow_mut().delivered.push(id);
                if held.borrow().len() < keep {
                    held.borrow_mut().push(Received::Lent(cell.take().unwrap()));
                } else {
                    // Publication is after the whole synchronous prefix, when
                    // its physical batch and real credit counter both committed.
                    published.push(id);
                }
                true
            })
            .await
            .unwrap();
        let oer_protocols_network_runtime::ReceivedPrefix::Lent(taken @ 1..=8) = prefix else {
            panic!("the model supplied a nonempty lending prefix");
        };
        let mut record = count.0.borrow_mut();
        record.prefixes.push(taken);
        record.published.extend(published);
    }
}

#[derive(Clone, Copy, Debug)]
enum Arrivals {
    Backlog,
    PerFrame,
    Batches,
}

fn bursts<B: 'static>(keep: usize, mixed: bool, arrivals: Arrivals) {
    prefixes::<B>(keep, mixed, arrivals, false, false);
}

fn prefixes<B: 'static>(
    keep: usize,
    mixed: bool,
    arrivals: Arrivals,
    batched: bool,
    reverse: bool,
) {
    let counted = Counted::default();
    let Bench {
        mut service,
        mut socket,
        device,
        mut outgoing,
        mut returned,
    } = bench_credited::<B, 12, _>(counted.clone());
    counted.0.borrow_mut().device = Some(device.clone());
    let held = Rc::new(RefCell::new(Vec::<Packet<B>>::new()));
    let mut app = Box::pin(async {
        if batched {
            consume_prefix(&mut socket, counted.clone(), keep, held.clone()).await;
        } else {
            consume(&mut socket, counted.clone(), keep, held.clone()).await;
        }
    });
    let clock: VirtualClock = VirtualClock::new();
    let mut sink = counted.clone();
    let mut net = Box::pin(service.run(&clock, &mut sink));
    let mut executor = Round::new(counted.clone());
    executor.reverse = reverse;
    executor.settle(&mut net, &mut app);
    let mut flights = Vec::new();
    while let Ok(Some(frame)) = outgoing.try_receive_with(Option::take) {
        flights.push(frame.memory);
    }
    assert_eq!(flights.len(), SLOTS);
    assert_eq!(executor.ready.load(Ordering::Acquire), 0);
    let accepted_before = counted.0.borrow().accepted;
    let total = if matches!(arrivals, Arrivals::Backlog) {
        24
    } else {
        16
    };
    match arrivals {
        Arrivals::Backlog => {
            for _ in 0..total {
                device.receive_one();
            }
            executor.poll(&mut net, &mut app);
            if mixed {
                return_one(&mut returned, &mut flights);
            }
        }
        Arrivals::PerFrame | Arrivals::Batches => {
            // Identical physical arrivals and hart-1 opportunities: after
            // frame 1, after frame 8, after frame 16. Both policies notify
            // at each eighth frame; only visibility before the flush changes.
            // One TX buffer returns at frame 1, waking the same network task.
            for frame in 1..=total {
                device.stage_one();
                if matches!(arrivals, Arrivals::PerFrame) {
                    device.expose();
                }
                if frame % 8 == 0 {
                    device.flush();
                }
                if frame == 1 {
                    return_one(&mut returned, &mut flights);
                }
                if matches!(frame, 1 | 8 | 16) {
                    executor.poll(&mut net, &mut app);
                }
            }
        }
    }
    executor.settle(&mut net, &mut app);
    let record = counted.0.borrow();
    if batched && matches!(arrivals, Arrivals::Backlog) && keep != 12 {
        assert_eq!(
            record.prefixes,
            [8, 8, 8],
            "the application processes complete available prefixes"
        );
    }
    println!(
        "keep={keep}, mixed={mixed}, arrivals={arrivals:?}:\n{}",
        record.trace.join("\n")
    );
    if keep == 12 {
        assert_eq!(record.delivered.len(), 12);
        assert_eq!(record.diagnosis.no_credit, 12);
        assert_eq!(record.published.len(), 0);
        assert_eq!(record.diagnosis.credits_restored, 0);
    } else {
        assert_eq!(record.diagnosis.no_credit, 0, "{:?}", record.trace);
        assert_eq!(record.delivered.len(), total);
        assert_eq!(record.published.len(), total - keep);
        assert_eq!(record.diagnosis.credits_restored as usize, total - keep);
    }
    let tx_returned = mixed || !matches!(arrivals, Arrivals::Backlog);
    assert_eq!(record.accepted - accepted_before, usize::from(tx_returned));
    if matches!(arrivals, Arrivals::Backlog) {
        assert_eq!(record.accepted_with_rx_pending, usize::from(mixed));
    }
    let delivered = record.delivered.len();
    drop(record);
    drop(app);
    if keep != 0 {
        for mut packet in held.borrow_mut().drain(..) {
            let Received::Lent(lent) = &mut packet else {
                panic!("lent")
            };
            let id = usize::from(lent.data().unwrap()[0]);
            drop(packet);
            counted.0.borrow_mut().published.push(id);
        }
        executor.settle(&mut net, &mut Box::pin(core::future::pending::<()>()));
    }
    drop(net);
    let physical = device.0.borrow().returned.lock().unwrap().len();
    assert_eq!(
        physical, total,
        "every physical frame is back, delivered {delivered}"
    );
    while let Ok(Some(frame)) = outgoing.try_receive_with(Option::take) {
        flights.push(frame.memory);
    }
    assert_eq!(flights.len(), SLOTS);
    while !flights.is_empty() {
        return_one(&mut returned, &mut flights);
    }
    let mut ended = service.retire();
    ended.retired.port.give_back().unwrap();
    ended.collect();
    assert!(ended.retired.pool.loans().released());
}

#[test]
fn a_draining_consumer_gets_its_credit_back_between_rx_batches() {
    struct Brand;
    bursts::<Brand>(0, false, Arrivals::Backlog);
}
#[test]
fn a_draining_consumer_keeps_its_credit_while_tx_memory_returns() {
    struct Brand;
    bursts::<Brand>(0, true, Arrivals::Backlog);
}
#[test]
fn a_consumer_that_keeps_twelve_frames_really_exhausts_its_credit() {
    struct Brand;
    bursts::<Brand>(12, false, Arrivals::Backlog);
}

#[test]
fn four_frames_kept_by_the_consumer_leave_a_full_batch_of_credit() {
    struct Brand;
    bursts::<Brand>(4, false, Arrivals::Backlog);
}

#[test]
fn per_frame_publication_fits_twelve_credits_with_the_same_consumer_schedule() {
    struct Brand;
    bursts::<Brand>(0, false, Arrivals::PerFrame);
}

#[test]
fn batch_publication_fits_twelve_credits_with_the_same_consumer_schedule() {
    struct Brand;
    bursts::<Brand>(0, false, Arrivals::Batches);
}

#[test]
fn a_prefix_consumer_restores_credit_between_backlogged_batches() {
    struct Brand;
    prefixes::<Brand>(0, false, Arrivals::Backlog, true, false);
}
#[test]
fn prefix_returns_keep_tx_progress_before_rx_drains() {
    struct Brand;
    prefixes::<Brand>(0, true, Arrivals::Backlog, true, false);
}
#[test]
fn a_prefix_can_retain_four_individual_owners() {
    struct Brand;
    prefixes::<Brand>(4, false, Arrivals::Backlog, true, false);
}
#[test]
fn a_prefix_which_retains_twelve_really_exhausts_credit() {
    struct Brand;
    prefixes::<Brand>(12, false, Arrivals::Backlog, true, false);
}
#[test]
fn per_frame_arrivals_with_prefix_consumption_preserve_delivery() {
    struct Brand;
    prefixes::<Brand>(0, false, Arrivals::PerFrame, true, false);
}
#[test]
fn published_arrival_batches_with_prefix_consumption_preserve_delivery() {
    struct Brand;
    prefixes::<Brand>(0, false, Arrivals::Batches, true, false);
}
#[test]
fn consumer_first_rounds_need_no_network_order_assumption() {
    struct Brand;
    prefixes::<Brand>(0, true, Arrivals::Backlog, true, true);
}
#[test]
fn retained_owners_with_consumer_first_rounds_keep_the_same_accounting() {
    struct Brand;
    prefixes::<Brand>(4, false, Arrivals::Batches, true, true);
}

fn unavailable_consumer<B: 'static>(cancel_wait: bool) {
    let counted = Counted::default();
    let Bench {
        mut service,
        mut socket,
        device,
        mut outgoing,
        mut returned,
    } = bench_credited::<B, 12, _>(counted.clone());
    counted.0.borrow_mut().device = Some(device.clone());
    let held = Rc::new(RefCell::new(Vec::<Packet<B>>::new()));
    let mut app = Box::pin(async {
        let mut published = Vec::new();
        let _prefix = socket
            .receive_prefix(8, |cell| {
                let id = usize::from(cell.as_mut().unwrap().data().unwrap()[0]);
                counted.0.borrow_mut().delivered.push(id);
                if held.borrow().is_empty() {
                    held.borrow_mut().push(Received::Lent(cell.take().unwrap()));
                } else {
                    published.push(id);
                }
                true
            })
            .await
            .unwrap();
        counted.0.borrow_mut().published.extend(published);
        // Busy elsewhere, with one retained frame. This makes no receive
        // promise and must not make the service wait for that frame.
        core::future::pending::<()>().await;
    });
    let clock: VirtualClock = VirtualClock::new();
    let mut sink = counted.clone();
    let mut net = Box::pin(service.run(&clock, &mut sink));
    let mut executor = Round::new(counted.clone());
    executor.settle(&mut net, &mut app);
    let mut flights = Vec::new();
    while let Ok(Some(frame)) = outgoing.try_receive_with(Option::take) {
        flights.push(frame.memory);
    }
    let before = counted.0.borrow().accepted;
    for _ in 0..24 {
        device.receive_one();
    }
    executor.poll(&mut net, &mut app);
    return_one(&mut returned, &mut flights);
    if cancel_wait {
        // Cancel after publication, before the ready consumer's first poll.
        // Releasing the promise consumes and returns no queued owner.
        drop(app);
        executor.settle(&mut net, &mut Box::pin(core::future::pending::<()>()));
    } else {
        executor.settle(&mut net, &mut app);
        drop(app);
    }
    let record = counted.0.borrow();
    assert!(
        device.0.borrow().frames.is_empty(),
        "RX did not wait for the unavailable consumer"
    );
    assert_eq!(
        record.accepted - before,
        1,
        "TX continued while the consumer was unavailable"
    );
    assert_eq!(record.delivered.len(), if cancel_wait { 0 } else { 8 });
    assert_eq!(record.published.len(), if cancel_wait { 0 } else { 7 });
    assert_eq!(record.diagnosis.no_credit, if cancel_wait { 12 } else { 5 });
    drop(record);
    drop(net);
    while let Ok(Some(frame)) = outgoing.try_receive_with(Option::take) {
        flights.push(frame.memory);
    }
    while !flights.is_empty() {
        return_one(&mut returned, &mut flights);
    }
    let mut ended = service.retire();
    ended.retired.port.give_back().unwrap();
    drop(socket);
    held.borrow_mut().clear();
    ended.collect();
    assert!(ended.retired.pool.loans().released());
    assert_eq!(device.0.borrow().returned.lock().unwrap().len(), 24);
}

#[test]
fn cancelling_a_ready_consumer_leaves_rx_and_tx_live() {
    struct Brand;
    unavailable_consumer::<Brand>(true);
}

#[test]
fn a_consumer_busy_with_a_retained_frame_does_not_block_rx_or_tx() {
    struct Brand;
    unavailable_consumer::<Brand>(false);
}

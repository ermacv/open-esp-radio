//! The real sockets, service, core and policy on one shared medium.

#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    reason = "a host test"
)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Wake, Waker};

use embassy_futures::select::select;
use oer_portable_events::EventSink;
use oer_portable_memory::WakingSpscSender;
use oer_portable_time::{Clock, Duration, Instant, Timer};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_core::{
    Block, Interface, NetEvent, NetworkOwner, Outcome, Remote, claim,
};
use oer_protocols_network_port::{Egress, FrameSource};
use oer_protocols_network_runtime::{
    Direct, EndCause, Ended, MemoryPolicy, Observer, PublishEnd, Sent, Service, ServiceEvent,
    Socket, SocketLanes, StopLane,
};
use oer_protocols_network_testing_model::{Medium, Notice, PeerSpec, Spent, Ticket};
use oer_protocols_network_wire::ip::Dscp;

#[path = "scenarios/work.rs"]
mod work;

const SIZE: usize = 1200;
const SOCKETS: usize = 4;
const CREDITS: usize = 4;
const RECEIVED: usize = 2;
const NEIGHBORS: usize = 4;
const BLOCKS: usize = 24;
const DEPTH: usize = 8;
const LANE: usize = 2;
/// Frames the port holds at once.
const AT_PORT: usize = 2;
const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const OUR_IP: [u8; 4] = [192, 0, 2, 1];
const RETRY_LIMIT: u32 = 3;
const OVERHEAD: u32 = 100;
const FAST: u32 = 1;
const SLOW: u32 = 8;
/// What a client sends.
const PAYLOAD: usize = 1_000;
/// How long a scenario runs.
const SECOND: u64 = 1_000_000;
/// Passes before a scenario counts as spinning.
const PASSES: u64 = 50_000_000;

type Stopper = WakingSpscSender<'static, (), 1>;
type Sock<B> = Socket<'static, 'static, B, SIZE, DEPTH, LANE, CREDITS>;
type Owner<B, const N: usize> =
    NetworkOwner<'static, B, SIZE, SOCKETS, CREDITS, RECEIVED, NEIGHBORS, N, Medium>;
type Run<B, E, const N: usize> = Ended<
    'static,
    'static,
    B,
    SIZE,
    SOCKETS,
    CREDITS,
    N,
    DEPTH,
    LANE,
    Medium,
    E,
    <E as Egress<Ticket, Notice>>::Fault,
>;

fn peer(number: u8, micros_per_octet: u32) -> PeerSpec {
    PeerSpec {
        hardware: [2, 0, 0, 0, 1, number],
        address: [192, 0, 2, 10 + number],
        overhead_micros: OVERHEAD,
        micros_per_octet,
        estimated_per_octet: micros_per_octet,
        retries: 0,
        retry_limit: RETRY_LIMIT,
    }
}

fn medium(peers: &[PeerSpec]) -> Medium {
    Medium::new(LOCAL, OUR_IP, AT_PORT, peers)
}

fn owner<B: 'static, const N: usize>(medium: &Medium) -> Owner<B, N> {
    let memory = Box::leak(Box::new([[0; SIZE]; N]));
    let (pool, brand) = claim::<B, SIZE, N>(memory).unwrap();
    NetworkOwner::new(brand, interface(), pool, medium.clone())
}

fn interface() -> Interface {
    Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap()
}

/// One flow: a socket on `port` that always has a datagram for `address`.
#[derive(Clone, Copy)]
struct Flow {
    port: u16,
    to: [u8; 4],
    /// Flows of one number share a group of this limit.
    group: (u8, usize),
    /// When it starts.
    after: u64,
}

/// Something that happens at a time.
enum At {
    Sleep(usize, bool),
    Refuse(u32),
    /// Keep what the medium has counted.
    Mark,
    Stop,
}

struct Woken(AtomicBool);

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::SeqCst);
    }
}

type Task<'t> = Pin<Box<dyn Future<Output = ()> + 't>>;

/// Run `tasks`, each polled only once woken, the script's times counted from now. With nothing woken, time moves to
/// the next landing, wait or scripted event. Fails if time has nowhere to go.
fn run_tasks(
    clock: &VirtualClock,
    medium: &Medium,
    mut tasks: Vec<Task<'_>>,
    mut script: Vec<(u64, Box<dyn FnOnce() + '_>)>,
) {
    let flags: Vec<_> = tasks
        .iter()
        .map(|_| Arc::new(Woken(AtomicBool::new(true))))
        .collect();
    let wakers: Vec<_> = flags.iter().cloned().map(Waker::from).collect();
    let mut done = vec![false; tasks.len()];
    let start = clock.now().as_micros();
    medium.advance(clock.now());
    script.sort_by_key(|(at, _)| *at);
    let mut script = script
        .into_iter()
        .map(|(at, event)| (start + at, event))
        .peekable();
    for _ in 0..PASSES {
        for (index, task) in tasks.iter_mut().enumerate() {
            if !done[index] && flags[index].0.swap(false, Ordering::SeqCst) {
                let mut cx = Context::from_waker(&wakers[index]);
                done[index] = task.as_mut().poll(&mut cx).is_ready();
            }
        }
        if done.iter().all(|done| *done) {
            return;
        }
        let woken = (0..tasks.len()).any(|i| !done[i] && flags[i].0.load(Ordering::SeqCst));
        if woken {
            continue;
        }
        let next = [
            clock.next_deadline().map(Instant::as_micros),
            medium.next_landing().map(Instant::as_micros),
            script.peek().map(|(at, _)| *at),
        ];
        let next = next
            .into_iter()
            .flatten()
            .min()
            .expect("no task is woken and nothing is due: a wake was lost");
        clock.advance_to(Instant::from_micros(next));
        medium.advance(Instant::from_micros(next));
        while script.peek().is_some_and(|(at, _)| *at <= next) {
            if let Some((_, event)) = script.next() {
                event();
            }
        }
    }
    panic!("{PASSES} passes without an end: a task spins");
}

/// Send to `flow` for as long as the socket lasts, count what was evicted, and
/// return all blocks left.
async fn flood<B>(
    mut socket: Sock<B>,
    flow: Flow,
    left: &RefCell<Vec<Block<'static, B, SIZE>>>,
    evicted: &RefCell<BTreeMap<u16, u64>>,
    clock: &VirtualClock,
) {
    let start = clock.now().as_micros() + flow.after;
    clock.wait_until(Instant::from_micros(start)).await;
    let remote = Remote {
        address: flow.to,
        port: 7,
    };
    let tally = |sent: Sent| {
        if matches!(sent, Sent::Ended(Outcome::Evicted(_))) {
            *evicted.borrow_mut().entry(flow.port).or_default() += 1;
        }
    };
    let mut pending: VecDeque<Observer<'static>> = VecDeque::new();
    while let Ok(mut block) = socket.reserve().await {
        block.fill(&[0; PAYLOAD]).unwrap();
        if pending.len() == CREDITS
            && let Some(mut oldest) = pending.pop_front()
        {
            oldest.finished().await;
            tally(oldest.take().unwrap());
        }
        if socket.credit().await.is_err() {
            left.borrow_mut().push(block);
            break;
        }
        match socket.send(remote, block) {
            Ok(observer) => pending.push_back(observer),
            Err(refused) => {
                left.borrow_mut().push(refused.block);
                break;
            }
        }
    }
    while let Some(mut observer) = pending.pop_front() {
        observer.finished().await;
        tally(observer.take().unwrap());
    }
    while !socket.closed() {
        embassy_futures::yield_now().await;
    }
    while let Some(block) = socket.reclaim() {
        left.borrow_mut().push(block);
    }
}

/// What one turn of the service did, as its events and the medium tell it.
#[derive(Clone, Copy, Debug, Default)]
struct TurnCount {
    /// Notices the turn handed the policy.
    notices: u64,
    /// Frames the port took in the turn.
    accepted: u32,
    /// The turn left notices untold and published nothing.
    deferred: bool,
}

/// Counts each turn: the medium tells how many notices it handed on.
struct Turns {
    medium: Medium,
    taken: u64,
    turn: TurnCount,
    turns: Rc<RefCell<Vec<TurnCount>>>,
}

impl EventSink<NetEvent> for Turns {
    fn event(&mut self, _: NetEvent) {}
}

impl EventSink<ServiceEvent> for Turns {
    fn event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::Accepted => self.turn.accepted += 1,
            ServiceEvent::Published(PublishEnd::Deferred) => self.turn.deferred = true,
            ServiceEvent::TurnEnded(_) => {
                let taken = self.medium.notices_taken();
                self.turn.notices = taken - self.taken;
                self.taken = taken;
                self.turns
                    .borrow_mut()
                    .push(core::mem::take(&mut self.turn));
            }
            _ => {}
        }
    }
}

struct Report<B: 'static, E: Egress<Ticket, Notice>, const N: usize> {
    ended: Run<B, E, N>,
    /// Each turn of the service.
    turns: Vec<TurnCount>,

    marks: Vec<Vec<Spent>>,
    /// Datagrams evicted, by the flow's port.
    evicted: BTreeMap<u16, u64>,
    /// Every datagram a client still held is back in the pool.
    outstanding: usize,
}

struct Scenario<'s> {
    medium: &'s Medium,
    clock: &'static VirtualClock,
    flows: &'s [Flow],
    script: Vec<(u64, At)>,
    /// Cancel `run` at this time, and retire.
    cancel_at: Option<u64>,
    memory: MemoryPolicy,
}

fn clock() -> &'static VirtualClock {
    Box::leak(Box::new(VirtualClock::new()))
}

fn scenario<'s>(medium: &'s Medium, flows: &'s [Flow], script: Vec<(u64, At)>) -> Scenario<'s> {
    Scenario {
        medium,
        clock: clock(),
        flows,
        script,
        cancel_at: None,
        memory: MemoryPolicy::KEEP,
    }
}

impl Scenario<'_> {
    fn run<B: 'static, E: Egress<Ticket, Notice> + 'static, const N: usize>(
        self,
        owner: Owner<B, N>,
        egress: E,
    ) -> Report<B, E, N> {
        let Self {
            medium,
            clock,
            flows,
            script,
            cancel_at,
            memory,
        } = self;
        let (stop, stopping): (Stopper, _) = Box::leak(Box::new(StopLane::new())).split();
        let mut service: Service<
            'static,
            'static,
            B,
            SIZE,
            SOCKETS,
            CREDITS,
            RECEIVED,
            NEIGHBORS,
            N,
            DEPTH,
            LANE,
            Medium,
            E,
        > = Service::new(owner, egress, memory, stopping);
        let left = RefCell::new(Vec::new());
        let evicted = RefCell::new(BTreeMap::new());
        let ended = Rc::new(RefCell::new(None));
        let marks = RefCell::new(Vec::new());
        let mut tasks: Vec<Task<'_>> = Vec::new();
        let mut groups = BTreeMap::new();
        for flow in flows {
            let storage = Box::leak(Box::new(SocketLanes::new()));
            let (number, limit) = flow.group;
            let group = match groups.get(&number) {
                Some(group) => *group,
                None => {
                    let group = service.group(limit).unwrap();
                    groups.insert(number, group);
                    group
                }
            };
            let socket = service
                .bind(flow.port, Dscp::default(), group, storage)
                .ok()
                .unwrap();
            tasks.push(Box::pin(flood(socket, *flow, &left, &evicted, clock)));
        }
        let keep = ended.clone();
        let counted = Rc::new(RefCell::new(Vec::new()));
        let mut events = Turns {
            medium: medium.clone(),
            taken: medium.notices_taken(),
            turn: TurnCount::default(),
            turns: counted.clone(),
        };
        tasks.push(Box::pin(async move {
            match cancel_at {
                Some(at) => {
                    let at = clock.now().as_micros() + at;
                    let wait = clock.wait_until(Instant::from_micros(at));
                    let _first = select(service.run(clock, &mut events), wait).await;
                }
                None => {
                    let _cause = service.run(clock, &mut events).await;
                }
            }
            *keep.borrow_mut() = Some(service.retire());
        }));
        let stop = Cell::new(Some(stop));
        let marks_ref = &marks;
        let stop_ref = &stop;
        let events: Vec<(u64, Box<dyn FnOnce() + '_>)> = script
            .into_iter()
            .map(|(at, event)| {
                let action: Box<dyn FnOnce() + '_> = match event {
                    At::Sleep(peer, asleep) => Box::new(move || medium.set_asleep(peer, asleep)),
                    At::Refuse(count) => Box::new(move || medium.refuse(count)),
                    At::Mark => Box::new(move || marks_ref.borrow_mut().push(medium.spent())),
                    At::Stop => Box::new(move || {
                        if let Some(stop) = stop_ref.take() {
                            stop.try_send(()).ok().unwrap();
                        }
                    }),
                };
                (at, action)
            })
            .collect();
        run_tasks(clock, medium, tasks, events);
        let mut ended = ended.borrow_mut().take().expect("the service ended");
        for block in left.take() {
            ended.retired.pool.give(block);
        }
        let outstanding = ended.retired.pool.outstanding();
        Report {
            ended,
            turns: counted.take(),
            marks: marks.take(),
            evicted: evicted.take(),
            outstanding,
        }
    }
}

fn flows(to: &[(u16, PeerSpec)]) -> Vec<Flow> {
    (0_u8..)
        .zip(to)
        .map(|(group, (port, peer))| Flow {
            port: *port,
            to: peer.address,
            group: (group, usize::MAX),
            after: 0,
        })
        .collect()
}

#[test]
fn a_sleeping_peer_holds_only_its_own_credits_of_a_shared_pool() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let script = vec![
        (1_000, At::Sleep(1, true)),
        (SECOND, At::Mark),
        (SECOND, At::Sleep(1, false)),
        (2 * SECOND, At::Mark),
        (2 * SECOND, At::Stop),
    ];
    // Room for one socket's credits and a little: the pool runs out at once
    // but for what the asleep peer cannot hold.
    let report =
        scenario(&medium, &flows, script).run(owner::<Net, { CREDITS + 2 }>(&medium), Direct);
    let [slept, woke] = &report.marks[..] else {
        panic!();
    };
    assert!(slept[0].frames > 100, "the awake peer went on: {slept:?}");
    assert!(woke[1].frames > 100, "the sleeper's blocks moved: {woke:?}");
    assert_eq!(report.outstanding, 0);
}

#[test]
fn an_exhausted_pool_starves_no_flow() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let script = vec![(SECOND, At::Stop)];
    let report = scenario(&medium, &flows, script).run(owner::<Net, 2>(&medium), Direct);
    assert_eq!(report.outstanding, 0);
}

/// Two flows to the first peer in one group, and to the other peer a flow that
/// starts only after those filled the pool, while that peer sleeps.
fn to_a_sleeper(peers: &[PeerSpec; 2], limit: usize) -> [Flow; 3] {
    let flow = |port, peer: PeerSpec, group, limit, after| Flow {
        port,
        to: peer.address,
        group: (group, limit),
        after,
    };
    [
        flow(5000, peers[0], 0, limit, 0),
        flow(5001, peers[0], 0, limit, 0),
        flow(5002, peers[1], 1, usize::MAX, 50_000),
    ]
}

/// The peer asleep for a second, and two marks around it, and a second after.
fn a_second_asleep() -> Vec<(u64, At)> {
    vec![
        (1_000, At::Sleep(0, true)),
        (100_000, At::Mark),
        (SECOND, At::Mark),
        (SECOND, At::Sleep(0, false)),
        (2 * SECOND, At::Mark),
        (2 * SECOND, At::Stop),
    ]
}

#[test]
fn a_limit_keeps_a_sleepers_flows_to_their_share_of_the_pool() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = to_a_sleeper(&peers, CREDITS);
    let report = scenario(&medium, &flows, a_second_asleep())
        .run(owner::<Net, { 2 * CREDITS }>(&medium), Direct);
    let [start, end, after] = &report.marks[..] else {
        panic!();
    };
    assert!(
        end[1].since(start[1]).frames > 100,
        "the awake peer went on"
    );
    assert!(
        after[0].since(end[0]).frames > 100,
        "and so did the sleeper"
    );
    assert!(report.evicted.is_empty(), "{:?}", report.evicted);
    assert_eq!(report.outstanding, 0);
}

#[test]
fn an_exhausted_pool_evicts_nothing_while_all_can_move() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let mut run = scenario(&medium, &flows, vec![(SECOND, At::Stop)]);
    run.memory = MemoryPolicy {
        evict_after: Some(Duration::from_millis(50)),
    };
    // Three blocks in two groups leave one group holding more, and a waiter.
    let report = run.run(owner::<Net, 3>(&medium), Direct);
    assert!(report.evicted.is_empty(), "{:?}", report.evicted);
}

#[test]
fn a_group_holding_little_goes_first_when_blocks_are_scarce() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let list = flows(&[(5000, peers[0]), (5001, peers[0]), (5002, peers[1])]);
    let script = a_second_asleep();
    let report =
        scenario(&medium, &list, script).run(owner::<Net, { 2 * CREDITS }>(&medium), Direct);
    let [start, end, _] = &report.marks[..] else {
        panic!();
    };
    assert!(end[1].since(start[1]).frames > 100, "{end:?}");
    assert_eq!(report.outstanding, 0);
}

#[test]
fn control_work_keeps_its_turn_under_full_data() {
    struct Net;
    let peers = [peer(1, SLOW), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    // A neighbor lasts a minute, so both are asked for again in the middle.
    let script = vec![
        (59 * SECOND, At::Mark),
        (61 * SECOND, At::Mark),
        (63 * SECOND, At::Mark),
        (63 * SECOND, At::Stop),
    ];
    let report = scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), Direct);
    assert_eq!(report.marks.len(), 3);
    assert!(medium.spent_on_everyone().frames >= 4, "two peers, twice");
}

#[test]
fn refusals_return_their_reservations() {
    struct Net;
    let peers = [peer(1, SLOW), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let script = (1..20)
        .map(|step| (step * SECOND / 20, At::Refuse(3)))
        .chain([(SECOND, At::Stop)])
        .collect();
    let mut report = scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), Direct);
    assert!(medium.refused() > 0);
    land_all(&medium, &mut report);
    assert_eq!(medium.unsettled(), 0);
    assert_eq!(report.outstanding, 0);
}

/// Everything on the air ends, and the policy hears.
fn land_all<B, const N: usize>(medium: &Medium, report: &mut Report<B, Direct, N>) {
    while let Some(at) = medium.next_landing() {
        medium.advance(at);
    }
    while report.ended.retired.port.take_notice().is_some() {}
}

#[test]
fn a_cancelled_run_hands_every_owner_to_the_next_service() {
    struct Net;
    let peers = [peer(1, SLOW), peer(2, FAST)];
    let medium = medium(&peers);
    let flow_list = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let clock = clock();
    let first = Scenario {
        medium: &medium,
        clock,
        flows: &flow_list,
        script: Vec::new(),
        cancel_at: Some(SECOND),
        memory: MemoryPolicy::KEEP,
    };
    let report = first.run(owner::<Net, BLOCKS>(&medium), Direct);
    assert_eq!(report.ended.cause, EndCause::Stopped);
    assert_eq!(report.outstanding, 0, "blocks of unseen commands came back");
    let on_air = medium.unsettled();
    assert!(on_air > 0, "frames outlive the service that sent them");
    let before = medium.spent();

    let Ended {
        retired, egress, ..
    } = report.ended;
    let owner = NetworkOwner::new(retired.brand, interface(), retired.pool, retired.port);
    let flow_list = flows(&[(5100, peers[0]), (5101, peers[1])]);
    let second = Scenario {
        medium: &medium,
        clock,
        flows: &flow_list,
        script: vec![(SECOND, At::Stop)],
        cancel_at: None,
        memory: MemoryPolicy::KEEP,
    };
    let mut report = second.run(owner, egress);
    assert_eq!(
        report.ended.cause,
        EndCause::Stopped,
        "{:?}",
        report.ended.fault
    );
    assert_eq!(report.outstanding, 0);
    let after = medium.spent();
    assert!(after[0].since(before[0]).frames > 0);
    land_all(&medium, &mut report);
    assert_eq!(medium.unsettled(), 0);
}

/// One peer's frames go out in aggregates of 32, whose ends the medium
/// tells at once, as a Block Ack does: each batch of ends settles in the
/// turn that finds it, and that turn publishes again. No turn defers its
/// publication for the rest of an aggregate's ends.
#[test]
fn each_block_ack_of_32_settles_in_one_turn_which_publishes_again() {
    struct Net;
    const AGGREGATE: usize = 32;
    let peers = [peer(1, FAST)];
    let medium = Medium::new(LOCAL, OUR_IP, AGGREGATE, &peers);
    medium.batch(AGGREGATE);
    let flows: Vec<Flow> = (0..4)
        .map(|flow| Flow {
            port: 5000 + flow,
            to: peers[0].address,
            group: (0, usize::MAX),
            after: 0,
        })
        .collect();
    let script = vec![(SECOND / 4, At::Stop)];
    let report = scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), Direct);
    assert_eq!(report.ended.cause, EndCause::Stopped);
    let told: Vec<&TurnCount> = report
        .turns
        .iter()
        .filter(|turn| turn.notices != 0)
        .collect();
    let whole = told
        .iter()
        .filter(|turn| turn.notices == AGGREGATE as u64)
        .count();
    let deferred = report.turns.iter().filter(|turn| turn.deferred).count();
    let republished = told
        .iter()
        .filter(|turn| turn.notices == AGGREGATE as u64 && turn.accepted != 0)
        .count();
    let most = told.iter().map(|turn| turn.notices).max();
    println!(
        "turns {}, turns with notices {}, whole aggregates {whole}, deferred {deferred}, \
         published in the aggregate's turn {republished}, most notices in a turn {most:?}, \
         frames {}",
        report.turns.len(),
        told.len(),
        medium.spent()[0].frames,
    );
    assert!(whole >= 4, "aggregates went whole: {told:?}");
    assert_eq!(told.len(), whole, "every batch in one turn: {told:?}");
    assert_eq!(deferred, 0, "no turn waits for the rest of an aggregate");
    assert_eq!(republished, whole, "each such turn publishes again");
    assert_eq!(report.outstanding, 0);
}

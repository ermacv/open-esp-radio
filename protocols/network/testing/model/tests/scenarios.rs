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
use std::num::NonZeroU32;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Wake, Waker};

use embassy_futures::select::select;
use oer_portable_memory::WakingSpscSender;
use oer_portable_time::{Clock, Duration, Instant, Timer};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeStorage;
use oer_protocols_ieee80211_runtime::egress::{AirtimeEgress, RadioNotice};
use oer_protocols_network_core::{Block, Interface, NetworkOwner, Outcome, Remote, claim};
use oer_protocols_network_port::{Egress, FrameSource};
use oer_protocols_network_runtime::{
    Direct, EndCause, Ended, MemoryPolicy, Observer, Sent, Service, Socket, SocketLanes, StopLane,
};
use oer_protocols_network_testing_model::{Medium, PeerKey, PeerSpec, Spent};
use oer_protocols_network_wire::ip::Dscp;

const SIZE: usize = 1200;
const SOCKETS: usize = 4;
const CREDITS: usize = 4;
const RECEIVED: usize = 2;
const NEIGHBORS: usize = 4;
const BLOCKS: usize = 24;
const DEPTH: usize = 8;
const LANE: usize = 2;
const PEERS: usize = 8;
const HORIZON: usize = 2;
/// Frames the port holds at once.
const AT_PORT: usize = 2;
const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const OUR_IP: [u8; 4] = [192, 0, 2, 1];
const QUANTUM: u32 = 2_000;
const RETRY_LIMIT: u32 = 3;
const OVERHEAD: u32 = 100;
const FAST: u32 = 1;
const SLOW: u32 = 8;
/// What a client sends.
const PAYLOAD: usize = 1_000;
/// With headers, as the medium counts a frame.
const FRAME: u64 = PAYLOAD as u64 + 42;
/// The most one frame can cost here.
const LARGEST_FRAME: u64 = OVERHEAD as u64 + FRAME * SLOW as u64;
/// How far two backlogged peers' air may differ where a frame costs at most
/// `largest`: what the frames at the port and each peer's quantum leave open.
/// Worked out from the deficit rule, not read from the scheduler.
const fn fair_to(largest: u64) -> u64 {
    2 * (AT_PORT as u64 + 1) * largest + QUANTUM as u64
}
const FAIR_TO: u64 = fair_to(LARGEST_FRAME);
/// How long a scenario runs.
const SECOND: u64 = 1_000_000;
/// Passes before a scenario counts as spinning.
const PASSES: u64 = 50_000_000;

type Notice = RadioNotice<u32, PeerKey>;
type Fair = AirtimeEgress<'static, Medium, u32, PEERS, HORIZON>;
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
    <E as Egress<u32, Notice>>::Fault,
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

fn fair(medium: &Medium) -> Fair {
    let storage = Box::leak(Box::new(AirtimeStorage::new(
        NonZeroU32::new(QUANTUM).unwrap(),
    )));
    AirtimeEgress::new(storage, medium.clone())
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
    UnknownCost(usize),
    Leave(usize),
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

struct Report<B: 'static, E: Egress<u32, Notice>, const N: usize> {
    ended: Run<B, E, N>,
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
    fn run<B: 'static, E: Egress<u32, Notice> + 'static, const N: usize>(
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
        tasks.push(Box::pin(async move {
            let mut events = ();
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
                    At::UnknownCost(peer) => Box::new(move || medium.set_unknown_cost(peer)),
                    At::Leave(peer) => Box::new(move || medium.leave(peer)),
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
            marks: marks.take(),
            evicted: evicted.take(),
            outstanding,
        }
    }
}

fn airs(spent: &[Spent]) -> Vec<u64> {
    spent.iter().map(|peer| peer.air_micros).collect()
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
fn a_slow_and_a_fast_peer_share_the_air_and_not_the_frames() {
    struct Net;
    let peers = [peer(1, SLOW), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let script = vec![(SECOND, At::Stop)];
    let report =
        scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), fair(&medium));
    assert_eq!(report.ended.cause, EndCause::Stopped);
    let [slow, fast] = medium.spent()[..] else {
        panic!();
    };
    assert!(
        slow.air_micros.abs_diff(fast.air_micros) <= FAIR_TO,
        "{slow:?} {fast:?}"
    );
    assert!(fast.frames > 4 * slow.frames, "{slow:?} {fast:?}");
    assert_eq!(report.outstanding, 0);
}

#[test]
fn with_no_policy_the_same_peers_do_not_share_the_air() {
    struct Net;
    let peers = [peer(1, SLOW), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let script = vec![(SECOND, At::Stop)];
    scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), Direct);
    let [slow, fast] = medium.spent()[..] else {
        panic!();
    };
    assert!(slow.frames.abs_diff(fast.frames) <= 2, "{slow:?} {fast:?}");
    assert!(
        slow.air_micros > 4 * fast.air_micros,
        "the control must tell the policies apart: {slow:?} {fast:?}"
    );
}

#[test]
fn several_flows_of_one_peer_earn_it_no_more_air() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[
        (5000, peers[0]),
        (5001, peers[0]),
        (5002, peers[0]),
        (5003, peers[1]),
    ]);
    let script = vec![(SECOND, At::Stop)];
    let report =
        scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), fair(&medium));
    let air = airs(&medium.spent());
    assert!(air[0].abs_diff(air[1]) <= FAIR_TO, "{air:?}");
    let shares: Vec<_> = (5000..=5002)
        .map(|port| medium.spent_by_flow(port).frames)
        .collect();
    let (least, most) = (shares.iter().min(), shares.iter().max());
    assert!(
        most.unwrap() - least.unwrap() <= 2 * AT_PORT as u64,
        "{shares:?}"
    );
    assert_eq!(report.outstanding, 0);
}

#[test]
fn with_no_policy_flows_buy_a_peer_more_air() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[
        (5000, peers[0]),
        (5001, peers[0]),
        (5002, peers[0]),
        (5003, peers[1]),
    ]);
    let script = vec![(SECOND, At::Stop)];
    scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), Direct);
    let air = airs(&medium.spent());
    assert!(air[0] > 2 * air[1], "{air:?}");
}

#[test]
fn a_sleeping_peer_costs_the_others_nothing_and_wakes_to_its_share() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let script = vec![
        (SECOND, At::Sleep(1, true)),
        (SECOND + 10_000, At::Mark),
        (2 * SECOND, At::Mark),
        (2 * SECOND, At::Sleep(1, false)),
        (3 * SECOND, At::Mark),
        (3 * SECOND, At::Stop),
    ];
    let report =
        scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), fair(&medium));
    let [asleep, before_wake, after] = &report.marks[..] else {
        panic!();
    };
    let during = before_wake[0].since(asleep[0]).air_micros;
    let slept = before_wake[1].since(asleep[1]);
    assert_eq!(slept.frames, 0, "it was asleep: {slept:?}");
    assert!(
        during + LARGEST_FRAME * 2 + 10_000 >= SECOND,
        "the awake peer has the whole air: {during}"
    );
    let first = after[0].since(before_wake[0]).air_micros;
    let second = after[1].since(before_wake[1]).air_micros;
    assert!(
        first.abs_diff(second) <= FAIR_TO,
        "no burst: {first} {second}"
    );
    assert_eq!(report.outstanding, 0);
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
    let report = scenario(&medium, &flows, script)
        .run(owner::<Net, { CREDITS + 2 }>(&medium), fair(&medium));
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
    let report = scenario(&medium, &flows, script).run(owner::<Net, 2>(&medium), fair(&medium));
    let air = airs(&medium.spent());
    assert!(air[0].abs_diff(air[1]) <= FAIR_TO, "{air:?}");
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
        .run(owner::<Net, { 2 * CREDITS }>(&medium), fair(&medium));
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
fn pressure_evicts_the_oldest_of_whoever_holds_most() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = to_a_sleeper(&peers, usize::MAX);
    let mut run = scenario(&medium, &flows, a_second_asleep());
    run.memory = MemoryPolicy {
        evict_after: Some(Duration::from_millis(50)),
    };
    let report = run.run(owner::<Net, { 2 * CREDITS }>(&medium), fair(&medium));
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
    let of_sleeper: u64 = [5000, 5001]
        .iter()
        .filter_map(|port| report.evicted.get(port))
        .sum();
    assert!(of_sleeper > 0, "{:?}", report.evicted);
    assert_eq!(
        report.evicted.get(&5002),
        None,
        "the awake peer loses nothing"
    );
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
    let report = run.run(owner::<Net, 3>(&medium), fair(&medium));
    assert!(report.evicted.is_empty(), "{:?}", report.evicted);
    let air = airs(&medium.spent());
    assert!(air[0].abs_diff(air[1]) <= FAIR_TO, "{air:?}");
}

/// With neither limit nor eviction, flows to a peer that sleeps keep every block
/// they took until it wakes. Nothing is lost.
#[test]
fn with_neither_guard_flows_to_a_sleeper_hold_the_pool_until_it_wakes() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = to_a_sleeper(&peers, usize::MAX);
    let report = scenario(&medium, &flows, a_second_asleep())
        .run(owner::<Net, { 2 * CREDITS }>(&medium), fair(&medium));
    let [start, end, after] = &report.marks[..] else {
        panic!();
    };
    assert_eq!(end[1].since(start[1]).frames, 0, "the awake peer waits");
    assert!(
        after[1].since(end[1]).frames > 100,
        "and goes on: {after:?}"
    );
    assert_eq!(report.outstanding, 0);
}

#[test]
fn a_group_holding_little_goes_first_when_blocks_are_scarce() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let list = flows(&[(5000, peers[0]), (5001, peers[0]), (5002, peers[1])]);
    let script = a_second_asleep();
    let report =
        scenario(&medium, &list, script).run(owner::<Net, { 2 * CREDITS }>(&medium), fair(&medium));
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
    let report =
        scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), fair(&medium));
    let [_, renewed, end] = &report.marks[..] else {
        panic!();
    };
    assert!(medium.spent_on_everyone().frames >= 4, "two peers, twice");
    let air = [
        end[0].since(renewed[0]).air_micros,
        end[1].since(renewed[1]).air_micros,
    ];
    assert!(air[0].abs_diff(air[1]) <= FAIR_TO, "{air:?}");
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
    let mut report =
        scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), fair(&medium));
    assert!(medium.refused() > 0);
    let air = airs(&medium.spent());
    assert!(air[0].abs_diff(air[1]) <= FAIR_TO, "{air:?}");
    land_all(&medium, &mut report);
    assert_eq!(report.ended.egress.in_flight(), 0);
    assert_eq!(report.outstanding, 0);
}

/// Everything on the air ends, and the policy hears.
fn land_all<B, const N: usize>(medium: &Medium, report: &mut Report<B, Fair, N>) {
    while let Some(at) = medium.next_landing() {
        medium.advance(at);
    }
    while let Some(notice) = report.ended.retired.port.take_notice() {
        report.ended.egress.notice(notice).unwrap();
    }
}

/// One peer's estimate is a quarter of what its frames cost, by retries; the
/// other's frames cost that same air in one attempt, as it believes.
fn underestimated_and_accurate() -> ([PeerSpec; 2], u64) {
    let low = PeerSpec {
        retries: RETRY_LIMIT,
        ..peer(1, FAST)
    };
    let attempts = u64::from(RETRY_LIMIT) + 1;
    let real = attempts * (u64::from(OVERHEAD) + FRAME * u64::from(FAST));
    let accurate = PeerSpec {
        overhead_micros: 0,
        ..peer(2, u32::try_from(real / FRAME).unwrap() + 1)
    };
    ([low, accurate], real + FRAME)
}

#[test]
fn a_peer_estimated_too_low_still_pays_what_it_spent() {
    struct Net;
    let (peers, largest) = underestimated_and_accurate();
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let script = vec![(2 * SECOND, At::Stop)];
    scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), fair(&medium));
    let air = airs(&medium.spent());
    assert!(air[0].abs_diff(air[1]) <= fair_to(largest), "{air:?}");
}

#[test]
fn a_peer_of_unknown_cost_buys_no_air_with_a_low_estimate() {
    struct Net;
    let (peers, largest) = underestimated_and_accurate();
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let script = vec![(1_000, At::UnknownCost(0)), (2 * SECOND, At::Stop)];
    let mut report =
        scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), fair(&medium));
    let [unknown, known] = medium.spent()[..] else {
        panic!();
    };
    assert!(unknown.frames > 10, "still served: {unknown:?} {known:?}");
    // Charged its ceiling, this peer may get less than its share, never more.
    assert!(
        unknown.air_micros <= known.air_micros + fair_to(largest),
        "{unknown:?} {known:?}"
    );
    land_all(&medium, &mut report);
    assert_eq!(report.ended.egress.in_flight(), 0);
}

#[test]
fn associations_come_and_go_past_the_accounts_there_are() {
    struct Net;
    let peers = [peer(1, FAST), peer(2, FAST)];
    let medium = medium(&peers);
    let flows = flows(&[(5000, peers[0]), (5001, peers[1])]);
    let script = (1..=u64::try_from(4 * PEERS).unwrap())
        .map(|step| (step * 20_000, At::Leave(0)))
        .chain([(SECOND, At::Stop)])
        .collect();
    let mut report =
        scenario(&medium, &flows, script).run(owner::<Net, BLOCKS>(&medium), fair(&medium));
    assert_eq!(
        report.ended.cause,
        EndCause::Stopped,
        "{:?}",
        report.ended.fault
    );
    let air = airs(&medium.spent());
    assert!(air[0].abs_diff(air[1]) <= FAIR_TO, "{air:?}");
    land_all(&medium, &mut report);
    assert_eq!(report.ended.egress.in_flight(), 0);
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
    let report = first.run(owner::<Net, BLOCKS>(&medium), fair(&medium));
    assert_eq!(report.ended.cause, EndCause::Stopped);
    assert_eq!(report.outstanding, 0, "blocks of unseen commands came back");
    let on_air = report.ended.egress.in_flight();
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
    let air = [
        after[0].since(before[0]).air_micros,
        after[1].since(before[1]).air_micros,
    ];
    assert!(air[0].abs_diff(air[1]) <= FAIR_TO, "{air:?}");
    land_all(&medium, &mut report);
    assert_eq!(report.ended.egress.in_flight(), 0);
}

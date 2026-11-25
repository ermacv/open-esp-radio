//! A turn's publishing: the ready work the core lists, the sockets in turn,
//! each frame once, nothing lost at a refusal, and every owner home after a
//! stop or a cancelled run.

use crate::tests::Room;
use core::cell::Cell;
use core::future::poll_fn;
use oer_protocols_network_core::BlockPlace;

use super::*;
use crate::PublishEnd;

const PLAN_BLOCKS: usize = 32;
const PLAN_CREDITS: usize = 8;
const PLAN_DEPTH: usize = 16;

type PlanSocket<'n, B> = Socket<'n, 'n, B, SIZE, PLAN_DEPTH, RECEIVED, PLAN_CREDITS>;
type PlanService<'n, B, D = ()> = Service<
    'n,
    'n,
    B,
    SIZE,
    2,
    PLAN_CREDITS,
    RECEIVED,
    2,
    PLAN_BLOCKS,
    PLAN_DEPTH,
    RECEIVED,
    WirePort,
    (),
    D,
>;
type PlanEnded<'n, B> =
    Ended<'n, 'n, B, SIZE, 2, PLAN_CREDITS, PLAN_BLOCKS, PLAN_DEPTH, RECEIVED, WirePort>;

/// What the service's sink heard of its publishing, in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    /// A turn began.
    Turn,
    /// The port took a frame.
    Accepted,
    /// How a turn's publishing ended.
    Published(PublishEnd),
}

/// The steps, as [`TurnMarks`] writes them.
#[derive(Clone, Debug, Default)]
struct Planned(Rc<RefCell<Vec<Step>>>);

impl Planned {
    fn count(&self, step: Step) -> usize {
        self.0.borrow().iter().filter(|seen| **seen == step).count()
    }
}

struct PlanBench<'n, B: 'n, D = ()> {
    service: PlanService<'n, B, D>,
    sockets: [PlanSocket<'n, B>; 2],
    wire: WirePort,
    policy: Planned,
    stop: WakingSpscSender<'n, (), 1>,
    clock: &'static VirtualClock,
}

fn plan_bench<'n, B: Brand + 'n>(right: NetworkClaim<B>) -> PlanBench<'n, B> {
    plan_bench_diagnosed(right, ())
}

/// As [`plan_bench`], the service counting into `diagnostics`.
fn plan_bench_diagnosed<'n, B: Brand + 'n, D: crate::Diagnostics>(
    right: NetworkClaim<B>,
    diagnostics: D,
) -> PlanBench<'n, B, D> {
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    plan_bench_on(right, interface, diagnostics)
}

/// As [`plan_bench_diagnosed`], on `interface`.
fn plan_bench_on<'n, B: Brand + 'n, D: crate::Diagnostics>(
    right: NetworkClaim<B>,
    interface: Interface,
    diagnostics: D,
) -> PlanBench<'n, B, D> {
    let memory = Box::leak(Box::new([const { BlockPlace::<SIZE>::new() }; PLAN_BLOCKS]));
    let (pool, brand) = claim(right, memory);
    let wire = WirePort::default();
    wire.0.borrow_mut().switches.on(WireSwitch::Answers);
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service =
        Service::new(owner, MemoryPolicy::KEEP, stopping).with_diagnostics(diagnostics);
    let sockets = [5000, 5001].map(|port| {
        service
            .bind(
                port,
                Dscp::default(),
                Group::shared(),
                Box::leak(Box::new(SocketLanes::new())),
            )
            .ok()
            .unwrap()
    });
    PlanBench {
        service,
        sockets,
        wire,
        policy: Planned::default(),
        stop,
        clock: Box::leak(Box::new(VirtualClock::new())),
    }
}

async fn plan_send<'n, B: 'n>(socket: &mut PlanSocket<'n, B>, data: &[u8]) -> Observer<'n> {
    let mut block = socket.reserve().await.unwrap();
    block.fill(data).unwrap();
    socket.credit().await.unwrap();
    socket.send(peer(), block).ok().unwrap()
}

/// The first datagram resolves the peer, then the port fills up and each
/// socket queues four more, `a0`.. and `b0`.., all ready at the core.
async fn queue_four_each<'n, B: 'n>(
    sockets: &mut [PlanSocket<'n, B>; 2],
    wire: &WirePort,
) -> Vec<Observer<'n>> {
    let [a, b] = sockets;
    let first = plan_send(a, b"first").await;
    assert_eq!(result_of(first).await, Sent::Ended(Outcome::Accepted));
    open_port(wire, Room::Full);
    let mut observers = Vec::new();
    for round in 0..4 {
        observers.push(plan_send(a, &[b'a', b'0' + round]).await);
        observers.push(plan_send(b, &[b'b', b'0' + round]).await);
    }
    settle().await;
    assert_eq!(datagrams(&wire.0.borrow()).len(), 1, "the port is full");
    observers
}

/// Let the port take `count` frames, then fill up again.
fn take_then_fill(wire: &WirePort, count: usize) {
    let mut wire = wire.0.borrow_mut();
    wire.accepts = Some(count);
    wire.switches.off(WireSwitch::Full);
    wire.wake();
}

/// Each socket's data in its own order, and no socket twice running while
/// the other still had data ready: the sockets take turns.
fn in_turn(sent: &[Vec<u8>]) {
    let mut next = [b'0'; 2];
    let mut last = None;
    for datagram in sent {
        let socket = usize::from(datagram[0] == b'b');
        assert_eq!(datagram[1], next[socket], "{sent:?}");
        next[socket] += 1;
        assert_ne!(last, Some(socket), "sockets take turns: {sent:?}");
        last = Some(socket);
    }
}

/// The port refuses in the middle of a turn's publishing: the frames before
/// went, the publishing ended at the refusal, and the rest stays ready, loses
/// no block, and goes when the port has room, the sockets still in turn.
#[test]
fn a_refusal_in_the_middle_of_a_turn_loses_nothing() {
    with_local_network(|right| {
        let PlanBench {
            service,
            mut sockets,
            wire,
            policy,
            stop,
            clock,
        } = plan_bench(right);
        let mut events = TurnMarks(policy.clone());
        let drive = async {
            let observers = queue_four_each(&mut sockets, &wire).await;
            let mark = policy.0.borrow().len();
            take_then_fill(&wire, 5);
            settle().await;
            let sent = datagrams(&wire.0.borrow());
            assert_eq!(sent.len(), 6, "five frames went, then the port was full");
            in_turn(&sent[1..]);
            let steps = policy.0.borrow()[mark..].to_vec();
            let mut expected = std::vec![Step::Accepted; 5];
            expected.push(Step::Published(PublishEnd::Refused));
            let publishing = steps
                .split(|step| *step == Step::Turn)
                .find(|turn| turn.contains(&Step::Accepted))
                .unwrap();
            assert_eq!(publishing, expected, "five frames, then the refusal");
            open_port(&wire, Room::Free);
            let mut results = Vec::new();
            for observer in observers {
                results.push(result_of(observer).await);
            }
            stop.try_send(()).ok().unwrap();
            results
        };
        let serve = async {
            let mut service = service;
            let _cause = service.run(clock, &mut events).await;
            service.retire()
        };
        let (mut ended, results) = run_tasks(clock, serve, drive);
        assert!(
            results
                .iter()
                .all(|sent| *sent == Sent::Ended(Outcome::Accepted))
        );
        let sent = datagrams(&wire.0.borrow());
        assert_eq!(sent.len(), 9, "each datagram once");
        in_turn(&sent[1..]);
        home(&mut ended, &mut sockets);
    });
}

/// Every block comes home and the pool has them all.
fn home<'n, B: 'n>(ended: &mut PlanEnded<'n, B>, sockets: &mut [PlanSocket<'n, B>; 2]) {
    for socket in sockets {
        while let Some(block) = socket.reclaim() {
            ended.retired.pool.give(block);
        }
    }
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

/// How a test ends its service.
#[derive(Clone, Copy, Debug)]
enum End {
    /// A stop, which `run` sees.
    Stop,
    /// A cancelled `run`: its future is dropped at an await.
    Cancel,
}

/// Ready work not yet sent when the service ends, by a stop or a cancelled
/// run: the sent datagrams stay accepted, the rest end retired, and every
/// block comes home.
fn an_end_with_ready_work_unsent_returns_every_owner<'n, Net: Brand + 'n>(
    right: NetworkClaim<Net>,
    end: End,
) {
    let PlanBench {
        service,
        mut sockets,
        wire,
        stop,
        clock,
        ..
    } = plan_bench(right);
    let cancel = Cell::new(false);
    let mut events = ();
    let drive = async {
        let observers = queue_four_each(&mut sockets, &wire).await;
        take_then_fill(&wire, 3);
        settle().await;
        assert_eq!(datagrams(&wire.0.borrow()).len(), 4);
        match end {
            End::Stop => stop.try_send(()).ok().unwrap(),
            End::Cancel => {
                cancel.set(true);
                wire.0.borrow_mut().wake();
            }
        }
        observers
    };
    let serve = async {
        let mut service = service;
        {
            let mut run = pin!(service.run(clock, &mut events));
            poll_fn(|cx| {
                if cancel.get() {
                    return Poll::Ready(());
                }
                run.as_mut().poll(cx).map(|_| ())
            })
            .await;
        }
        service.retire()
    };
    let (mut ended, observers) = run_tasks(clock, serve, drive);
    let results: Vec<_> = observers
        .into_iter()
        .map(|observer| observer.take().unwrap())
        .collect();
    let accepted = results
        .iter()
        .filter(|sent| **sent == Sent::Ended(Outcome::Accepted))
        .count();
    let retired = results
        .iter()
        .filter(|sent| **sent == Sent::Ended(Outcome::Retired))
        .count();
    assert_eq!((accepted, retired), (3, 5), "{end:?}: {results:?}");
    assert_eq!(
        wire.0.borrow().sent.len(),
        5,
        "the ARP request, the first datagram and three after it"
    );
    home(&mut ended, &mut sockets);
}

#[test]
fn a_stop_with_ready_work_unsent_returns_every_owner() {
    with_local_network(|right| {
        an_end_with_ready_work_unsent_returns_every_owner(right, End::Stop);
    });
}

#[test]
fn a_cancelled_run_with_ready_work_unsent_returns_every_owner() {
    with_local_network(|right| {
        an_end_with_ready_work_unsent_returns_every_owner(right, End::Cancel);
    });
}

/// Eight datagrams ready, the port with room for all: one turn sends its
/// budget of them, the sockets in turn.
#[test]
fn one_turn_sends_its_budget_in_turn() {
    with_local_network(|right| {
        let PlanBench {
            service,
            mut sockets,
            wire,
            policy,
            stop,
            clock,
        } = plan_bench(right);
        let mut events = TurnMarks(policy.clone());
        let drive = async {
            let observers = queue_four_each(&mut sockets, &wire).await;
            let mark = policy.0.borrow().len();
            open_port(&wire, Room::Free);
            settle().await;
            let sent = datagrams(&wire.0.borrow());
            assert_eq!(sent.len(), 9);
            in_turn(&sent[1..]);
            let steps = policy.0.borrow()[mark..].to_vec();
            let mut expected = std::vec![Step::Accepted; crate::service::TURN_FRAMES];
            expected.push(Step::Published(PublishEnd::Budget));
            let publishing = steps
                .split(|step| *step == Step::Turn)
                .find(|turn| turn.contains(&Step::Accepted))
                .unwrap();
            assert_eq!(publishing, expected, "the eight in one turn");
            for observer in observers {
                assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Accepted));
            }
            stop.try_send(()).ok().unwrap();
        };
        let serve = async {
            let mut service = service;
            let _cause = service.run(clock, &mut events).await;
            service.retire()
        };
        let (mut ended, ()) = run_tasks(clock, serve, drive);
        assert_eq!(policy.count(Step::Accepted), 8 + 2);
        home(&mut ended, &mut sockets);
    });
}

/// Each turn's beginning, frames and publishing end, as the sink hears them.
struct TurnMarks(Planned);

impl oer_portable_events::EventSink<crate::ServiceEvent> for TurnMarks {
    fn event(&mut self, event: crate::ServiceEvent) {
        let step = match event {
            crate::ServiceEvent::TurnBegan => Step::Turn,
            crate::ServiceEvent::Accepted => Step::Accepted,
            crate::ServiceEvent::Published(end) => Step::Published(end),
            _ => return,
        };
        self.0.0.borrow_mut().push(step);
    }
}

impl oer_portable_events::EventSink<oer_protocols_network_core::NetEvent> for TurnMarks {
    fn event(&mut self, _: oer_protocols_network_core::NetEvent) {}
}

mod diagnostics;
mod idle;

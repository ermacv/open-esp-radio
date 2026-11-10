//! A turn's publishing over one plan: one collection of the ready work, a
//! reservation for each frame that goes and none past it, sockets in turn
//! across the plan's frames, and every owner home after a stop or a cancelled
//! run.

use core::cell::Cell;
use core::convert::Infallible;
use core::future::poll_fn;

use oer_protocols_network_port::{QUANTUM, Turns};

use super::*;

const PLAN_BLOCKS: usize = 32;
const PLAN_CREDITS: usize = 8;
const PLAN_DEPTH: usize = 16;

type PlanSocket<B> = Socket<'static, 'static, B, SIZE, PLAN_DEPTH, RECEIVED, PLAN_CREDITS>;
type PlanService<B, E = Planned> = Service<
    'static,
    'static,
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
    E,
>;
type PlanEnded<B, E = Planned> = Ended<
    'static,
    'static,
    B,
    SIZE,
    2,
    PLAN_CREDITS,
    PLAN_BLOCKS,
    PLAN_DEPTH,
    RECEIVED,
    WirePort,
    E,
    Infallible,
>;

/// What the policy was asked and answered, in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Step {
    /// A turn began, as the service's sink heard.
    Turn,
    Collected,
    Reserved,
    Accepted,
    Refused,
    Unknown,
}

/// [`Direct`]'s choice, with every collection and reservation and answer
/// written down.
#[derive(Clone, Debug, Default)]
struct Planned(Rc<RefCell<Vec<Step>>>);

impl Planned {
    fn count(&self, step: Step) -> usize {
        self.0.borrow().iter().filter(|seen| **seen == step).count()
    }

    /// Reservations no answer has met.
    fn open(&self) -> usize {
        self.count(Step::Reserved)
            - self.count(Step::Accepted)
            - self.count(Step::Refused)
            - self.count(Step::Unknown)
    }
}

impl Egress<u32, u32> for Planned {
    type Reservation = ();
    type Fault = Infallible;
    type Plan<C: Candidate> = Turns<C, QUANTUM>;

    fn plan<C: Candidate>(
        &mut self,
        ready: impl Iterator<Item = C> + Clone,
    ) -> Result<Turns<C, QUANTUM>, Infallible> {
        self.0.borrow_mut().push(Step::Collected);
        <Direct as Egress<u32, u32>>::plan(&mut Direct, ready)
    }

    fn next<C: Candidate>(
        &mut self,
        plan: &mut Turns<C, QUANTUM>,
    ) -> Result<Next<C, ()>, Infallible> {
        let next = <Direct as Egress<u32, u32>>::next(&mut Direct, plan)?;
        if matches!(next, Next::Send(..)) {
            self.0.borrow_mut().push(Step::Reserved);
        }
        Ok(next)
    }

    fn accepted(&mut self, (): (), _ticket: u32) -> Result<(), Infallible> {
        self.0.borrow_mut().push(Step::Accepted);
        Ok(())
    }

    fn refused(&mut self, (): ()) -> Result<(), Infallible> {
        self.0.borrow_mut().push(Step::Refused);
        Ok(())
    }

    fn unknown(&mut self, (): ()) -> Result<(), Infallible> {
        self.0.borrow_mut().push(Step::Unknown);
        Ok(())
    }

    fn notice(&mut self, _notice: u32) -> Result<(), Infallible> {
        Ok(())
    }
}

struct PlanBench<B: 'static, E: Egress<u32, u32, Fault = Infallible> = Planned> {
    service: PlanService<B, E>,
    sockets: [PlanSocket<B>; 2],
    wire: WirePort,
    policy: Planned,
    stop: WakingSpscSender<'static, (), 1>,
    clock: &'static VirtualClock,
}

fn plan_bench<B: 'static>() -> PlanBench<B> {
    let policy = Planned::default();
    plan_bench_with(policy.clone(), policy)
}

fn plan_bench_with<B: 'static, E: Egress<u32, u32, Fault = Infallible>>(
    egress: E,
    policy: Planned,
) -> PlanBench<B, E> {
    let memory = Box::leak(Box::new([[0; SIZE]; PLAN_BLOCKS]));
    let (pool, brand) = claim::<B, SIZE, PLAN_BLOCKS>(memory).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, egress, MemoryPolicy::KEEP, stopping);
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
        policy,
        stop,
        clock: Box::leak(Box::new(VirtualClock::new())),
    }
}

async fn plan_send<B>(socket: &mut PlanSocket<B>, data: &[u8]) -> Observer<'static> {
    let mut block = socket.reserve().await.unwrap();
    block.fill(data).unwrap();
    socket.credit().await.unwrap();
    socket.send(peer(), block).ok().unwrap()
}

/// The first datagram resolves the peer, then the port fills up and each
/// socket queues four more, `a0`.. and `b0`.., all ready at the core.
async fn queue_four_each<B>(
    sockets: &mut [PlanSocket<B>; 2],
    wire: &WirePort,
) -> Vec<Observer<'static>> {
    let [a, b] = sockets;
    let first = plan_send(a, b"first").await;
    assert_eq!(result_of(first).await, Sent::Ended(Outcome::Accepted));
    open_port(wire, true);
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
    wire.full = false;
    wire.wake();
}

/// Each socket's data in its own order, and no socket twice running while
/// the other still had data in the plan's frames: the turns of a choice per
/// frame.
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

/// The port refuses in the middle of a plan: the frames before went under one
/// collection, each with its own reservation answered, the refused one's
/// reservation came back, and nothing was reserved for the rest of the plan.
/// The rest stays ready, loses no block, and goes when the port has room,
/// the sockets still in turn.
#[test]
fn a_refusal_in_the_middle_of_a_plan_reserves_nothing_more_and_loses_nothing() {
    brand!(Net);
    let PlanBench {
        service,
        mut sockets,
        wire,
        policy,
        stop,
        clock,
    } = plan_bench::<Net>();
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
        let mut expected = std::vec![Step::Collected];
        for _ in 0..5 {
            expected.extend([Step::Reserved, Step::Accepted]);
        }
        expected.extend([Step::Reserved, Step::Refused]);
        let publishing = steps
            .split(|step| *step == Step::Turn)
            .find(|turn| turn.contains(&Step::Accepted))
            .unwrap();
        assert_eq!(
            publishing, expected,
            "the turn: one collection, one reservation a frame, none past the refusal"
        );
        open_port(&wire, false);
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
    assert_eq!(policy.open(), 0, "every reservation answered");
    assert_eq!(
        policy.count(Step::Refused),
        wire.0.borrow().refusals,
        "a reservation only for a frame the port saw"
    );
    home(&mut ended, &mut sockets);
}

/// Every block comes home and the pool has them all.
fn home<B, E: Egress<u32, u32, Fault = Infallible>>(
    ended: &mut PlanEnded<B, E>,
    sockets: &mut [PlanSocket<B>; 2],
) {
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

/// A plan's work not yet sent when the service ends, by a stop or a cancelled
/// run: no reservation is left open, the sent datagrams stay accepted, the
/// rest end retired, and every block comes home.
fn an_end_with_planned_work_unsent_returns_every_owner<Net: 'static>(end: End) {
    let PlanBench {
        service,
        mut sockets,
        wire,
        policy,
        stop,
        clock,
    } = plan_bench::<Net>();
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
    assert_eq!(policy.open(), 0, "{end:?}: no reservation left open");
    assert_eq!(
        policy.count(Step::Accepted),
        5,
        "the ARP request, the first datagram and three of the plan's"
    );
    home(&mut ended, &mut sockets);
}

#[test]
fn a_stop_with_planned_work_unsent_returns_every_owner() {
    brand!(Net);
    an_end_with_planned_work_unsent_returns_every_owner::<Net>(End::Stop);
}

#[test]
fn a_cancelled_run_with_planned_work_unsent_returns_every_owner() {
    brand!(Net);
    an_end_with_planned_work_unsent_returns_every_owner::<Net>(End::Cancel);
}

/// Eight datagrams ready, the port with room for all: one turn sends them
/// all over one collection, the sockets in turn.
#[test]
fn one_collection_serves_a_turn_s_frames_in_turn() {
    brand!(Net);
    let PlanBench {
        service,
        mut sockets,
        wire,
        policy,
        stop,
        clock,
    } = plan_bench::<Net>();
    let mut events = ();
    let drive = async {
        let observers = queue_four_each(&mut sockets, &wire).await;
        let mark = policy.0.borrow().len();
        open_port(&wire, false);
        settle().await;
        let sent = datagrams(&wire.0.borrow());
        assert_eq!(sent.len(), 9);
        in_turn(&sent[1..]);
        let steps = policy.0.borrow()[mark..].to_vec();
        let mut expected = std::vec![Step::Collected];
        for _ in 0..8 {
            expected.extend([Step::Reserved, Step::Accepted]);
        }
        assert_eq!(
            steps[..expected.len()],
            expected,
            "one collection for the eight, one reservation each"
        );
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
    home(&mut ended, &mut sockets);
}

/// A policy whose plans always stop at once: each collection is written
/// down, and past `GIVE_UP` of them it says nothing may go, so a service
/// that collects without bound ends the test rather than spinning in it.
struct AlwaysStops(Planned);

const GIVE_UP: usize = 100;

impl Egress<u32, u32> for AlwaysStops {
    type Reservation = ();
    type Fault = Infallible;
    type Plan<C: Candidate> = ();

    fn plan<C: Candidate>(
        &mut self,
        _ready: impl Iterator<Item = C> + Clone,
    ) -> Result<(), Infallible> {
        self.0.0.borrow_mut().push(Step::Collected);
        Ok(())
    }

    fn next<C: Candidate>(&mut self, (): &mut ()) -> Result<Next<C, ()>, Infallible> {
        Ok(if self.0.count(Step::Collected) > GIVE_UP {
            Next::Idle
        } else {
            Next::Stop(PlanStop::Changed)
        })
    }

    fn accepted(&mut self, (): (), _ticket: u32) -> Result<(), Infallible> {
        Ok(())
    }

    fn refused(&mut self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn unknown(&mut self, (): ()) -> Result<(), Infallible> {
        Ok(())
    }

    fn notice(&mut self, _notice: u32) -> Result<(), Infallible> {
        Ok(())
    }
}

/// Each turn's beginning, written beside the policy's steps.
struct TurnMarks(Planned);

impl oer_portable_events::EventSink<crate::ServiceEvent> for TurnMarks {
    fn event(&mut self, event: crate::ServiceEvent) {
        if event == crate::ServiceEvent::TurnBegan {
            self.0.0.borrow_mut().push(Step::Turn);
        }
    }
}

impl oer_portable_events::EventSink<oer_protocols_network_core::NetEvent> for TurnMarks {
    fn event(&mut self, _: oer_protocols_network_core::NetEvent) {}
}

/// A plan that stops at once lets a new collection choose, but no more of
/// them in a turn than its frames: a policy that cannot plan cannot hold
/// the turn.
#[test]
fn collections_in_a_turn_are_bounded_by_its_frames() {
    brand!(Net);
    let log = Planned::default();
    let PlanBench {
        service,
        mut sockets,
        stop,
        clock,
        ..
    } = plan_bench_with::<Net, _>(AlwaysStops(log.clone()), log.clone());
    let mut events = TurnMarks(log.clone());
    let drive = async {
        let [a, _] = &mut sockets;
        let _observer = plan_send(a, b"stuck").await;
        settle().await;
        stop.try_send(()).ok().unwrap();
    };
    let serve = async {
        let mut service = service;
        let _cause = service.run(clock, &mut events).await;
        service.retire()
    };
    let (_ended, ()) = run_tasks(clock, serve, drive);
    let steps = log.0.borrow();
    let mut most = 0;
    for turn in steps.split(|step| *step == Step::Turn) {
        most = most.max(turn.iter().filter(|step| **step == Step::Collected).count());
    }
    assert_eq!(
        most,
        crate::service::TURN_FRAMES,
        "a turn collects at most once a frame"
    );
}

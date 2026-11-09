//! What a service tells a counting sink of its turns, for one socket sending
//! as fast as it can.

use oer_portable_events::EventSink;
use oer_protocols_network_core::NetEvent;

use super::*;
use crate::{PublishEnd, RequestKind, ServiceEvent, TurnEnd};

/// The geometry of a sender that holds blocks ahead: every block it may ask
/// ahead, and credits and lane room for all of them.
const FLOW_BLOCKS: usize = 32;
const FLOW_CREDITS: usize = 16;
const FLOW_DEPTH: usize = 32;
/// Requests one turn takes from a lane.
const TURN_COMMANDS: usize = 4;

type FlowSocket<B> = Socket<'static, 'static, B, SIZE, FLOW_DEPTH, RECEIVED, FLOW_CREDITS>;
type FlowService<B> = Service<
    'static,
    'static,
    B,
    SIZE,
    1,
    FLOW_CREDITS,
    RECEIVED,
    2,
    FLOW_BLOCKS,
    FLOW_DEPTH,
    RECEIVED,
    WirePort,
    Direct,
>;

fn flow<B: 'static>() -> (
    FlowService<B>,
    FlowSocket<B>,
    WirePort,
    WakingSpscSender<'static, (), 1>,
) {
    let memory = Box::leak(Box::new([[0; SIZE]; FLOW_BLOCKS]));
    let (pool, brand) = claim::<B, SIZE, FLOW_BLOCKS>(memory).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, Direct, MemoryPolicy::KEEP, stopping);
    let socket = service
        .bind(
            5000,
            Dscp::default(),
            Group::shared(),
            Box::leak(Box::new(SocketLanes::new())),
        )
        .ok()
        .unwrap();
    (service, socket, wire, stop)
}

/// Every event, in order.
#[derive(Default)]
struct Seen(Vec<ServiceEvent>);

impl EventSink<ServiceEvent> for Seen {
    fn event(&mut self, event: ServiceEvent) {
        self.0.push(event);
    }
}

impl EventSink<NetEvent> for Seen {
    fn event(&mut self, _: NetEvent) {}
}

/// Every event, in order, where the test reads it while the service runs.
struct Tape(Rc<RefCell<Vec<ServiceEvent>>>);

impl EventSink<ServiceEvent> for Tape {
    fn event(&mut self, event: ServiceEvent) {
        self.0.borrow_mut().push(event);
    }
}

impl EventSink<NetEvent> for Tape {
    fn event(&mut self, _: NetEvent) {}
}

/// One turn, as its events tell it.
#[derive(Debug, Default)]
struct Turn {
    /// The turn said it began, before anything else.
    began: bool,
    took: Vec<RequestKind>,
    spent: usize,
    /// Received frames it took in, and notices it handed the policy.
    frames: usize,
    notices: usize,
    accepted: usize,
    published: Vec<PublishEnd>,
    end: Option<TurnEnd>,
}

impl Turn {
    fn took(&self, kind: RequestKind) -> usize {
        self.took.iter().filter(|took| **took == kind).count()
    }

    /// The reservations the turn took.
    fn reserves(&self) -> usize {
        self.took
            .iter()
            .filter(|took| matches!(took, RequestKind::Reserve { .. }))
            .count()
    }

    /// The blocks each asked for.
    fn sizes(&self) -> impl Iterator<Item = usize> + '_ {
        self.took.iter().filter_map(|took| match took {
            RequestKind::Reserve { blocks } => Some(usize::from(*blocks)),
            _ => None,
        })
    }

    /// The blocks they asked for.
    fn asked(&self) -> usize {
        self.took
            .iter()
            .map(|took| match took {
                RequestKind::Reserve { blocks } => usize::from(*blocks),
                _ => 0,
            })
            .sum()
    }
}

/// The turns `events` tell, each ended by its `TurnEnded`.
fn turns(events: &[ServiceEvent]) -> Vec<Turn> {
    let mut turns = Vec::new();
    let mut turn = Turn::default();
    for event in events {
        match *event {
            ServiceEvent::TurnBegan => {
                assert!(
                    !turn.began && turn.took.is_empty() && turn.frames == 0,
                    "a turn begins before anything of it"
                );
                turn.began = true;
            }
            ServiceEvent::Took(kind) => turn.took.push(kind),
            ServiceEvent::BudgetSpent => turn.spent += 1,
            ServiceEvent::FrameTaken => turn.frames += 1,
            ServiceEvent::Noticed => turn.notices += 1,
            ServiceEvent::Accepted => turn.accepted += 1,
            ServiceEvent::Published(end) => turn.published.push(end),
            ServiceEvent::TurnEnded(end) => {
                assert!(turn.began, "every turn that ends began");
                turn.end = Some(end);
                turns.push(core::mem::take(&mut turn));
            }
            ServiceEvent::IntakeFull
            | ServiceEvent::Yielded
            | ServiceEvent::Waited { .. }
            | ServiceEvent::Returned { .. }
            | ServiceEvent::Local(_) => {}
        }
    }
    assert!(
        turn.took.is_empty() && turn.accepted == 0,
        "every turn ends"
    );
    turns
}

/// Poll `run` once, which runs one turn when it has work.
fn pass<R: Future>(run: &mut core::pin::Pin<&mut R>) {
    let cx = &mut Context::from_waker(Waker::noop());
    assert!(run.as_mut().poll(cx).is_pending());
}

#[test]
fn an_unpaced_sender_s_turns_take_its_reserves_and_sends_within_the_budget() {
    brand!(Net);
    const DATAGRAMS: usize = 64;
    let (mut service, mut socket, wire, stop) = flow::<Net>();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut seen = Seen::default();
    let drive = async {
        socket.reserve_ahead(RESERVE_AHEAD);
        for count in 0..DATAGRAMS {
            let mut block = socket.reserve().await.unwrap();
            block.fill(&[u8::try_from(count).unwrap(); 32]).unwrap();
            socket.credit().await.unwrap();
            // The result is given up, as the load image's sender does.
            drop(socket.send(peer(), block).ok().unwrap());
        }
        while datagrams(&wire.0.borrow()).len() < DATAGRAMS {
            yield_now().await;
        }
        stop.try_send(()).ok().unwrap();
    };
    let (cause, ()) = run_tasks(clock, service.run(clock, &mut seen), drive);
    assert_eq!(cause, EndCause::Stopped);
    let turns = turns(&seen.0);

    let took = |kind| turns.iter().map(|turn| turn.took(kind)).sum::<usize>();
    assert_eq!(took(RequestKind::Send), DATAGRAMS);
    // One reservation asks for what the look-ahead lacks: every block sent
    // was asked for, and at most the look-ahead more.
    let reserves: usize = turns.iter().map(Turn::reserves).sum();
    let asked: usize = turns.iter().map(Turn::asked).sum();
    assert!(
        (DATAGRAMS..=DATAGRAMS + RESERVE_AHEAD).contains(&asked),
        "every block sent was asked for, at most the look-ahead not sent: {asked}"
    );
    // The first asks for the whole look-ahead; after it, each take that
    // leaves half of it out asks for that half: messages per datagram are
    // 1 + 1/8, not 2.
    let sizes: Vec<usize> = turns.iter().flat_map(Turn::sizes).collect();
    assert_eq!(sizes[0], RESERVE_AHEAD);
    assert!(
        sizes[1..].iter().all(|blocks| *blocks == RESERVE_AHEAD / 2),
        "{sizes:?}"
    );
    assert!(
        reserves <= DATAGRAMS.div_ceil(RESERVE_AHEAD / 2) + 1,
        "{reserves} reservations for {DATAGRAMS} datagrams"
    );
    assert_eq!(took(RequestKind::Release) + took(RequestKind::Close), 0);

    for turn in &turns {
        assert!(turn.took.len() <= TURN_COMMANDS, "{turn:?}");
        assert!(turn.spent <= 1, "one socket: {turn:?}");
        if turn.spent == 1 {
            assert_eq!(turn.took.len(), TURN_COMMANDS, "{turn:?}");
        }
        assert_eq!(turn.published.len(), 1, "{turn:?}");
    }
    let accepted: usize = turns.iter().map(|turn| turn.accepted).sum();
    assert_eq!(
        accepted,
        wire.0.borrow().sent.len(),
        "every frame the port took, ARP's among them"
    );

    // What the turns that took a request show: the sends alone still spend
    // the budget in most, and a reservation a look-ahead leaves more than
    // three datagrams a turn, where one a block left two.
    let busy: Vec<&Turn> = turns.iter().filter(|turn| !turn.took.is_empty()).collect();
    let spent = busy.iter().filter(|turn| turn.spent != 0).count();
    assert!(
        2 * spent > busy.len(),
        "{spent} of {} busy turns",
        busy.len()
    );
    let sent: usize = busy.iter().map(|turn| turn.took(RequestKind::Send)).sum();
    assert!(
        3 * busy.len() < sent,
        "{sent} datagrams over {} busy turns",
        busy.len()
    );
    assert!(
        turns
            .iter()
            .all(|turn| turn.end.is_some_and(|end| end.blocks_free)),
        "blocks never run out"
    );
    // Datagrams wait for the peer's address under their credits; from the
    // first one the port took, credits never run out either.
    let resolved = turns.iter().position(|turn| turn.accepted > 1).unwrap();
    assert!(
        turns[resolved..]
            .iter()
            .all(|turn| turn.end.is_some_and(|end| end.credits_free)),
        "credits never run out once datagrams go"
    );
    assert!(
        !turns
            .iter()
            .any(|turn| turn.published.contains(&PublishEnd::Refused)),
        "the port always has room"
    );
}

/// Take `count` blocks into the application's hands, one a turn of `run`,
/// leaving nothing asked.
fn hold<B, R: Future>(
    socket: &mut FlowSocket<B>,
    run: &mut core::pin::Pin<&mut R>,
    count: usize,
) -> Vec<Block<'static, B, SIZE>> {
    // One at a time: a look-ahead of one asks only when it finds none.
    socket.reserve_ahead(1);
    let mut held = Vec::new();
    while held.len() < count {
        match socket.try_reserve().unwrap() {
            Some(block) => held.push(block),
            None => pass(run),
        }
    }
    held
}

#[test]
fn a_spent_budget_counts_only_while_the_lane_holds_more() {
    brand!(Net);
    let (mut service, mut socket, _wire, _stop) = flow::<Net>();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut seen = Seen::default();
    {
        let mut run = pin!(service.run(clock, &mut seen));
        for block in hold(&mut socket, &mut run, TURN_COMMANDS) {
            socket.release(block).ok().unwrap();
        }
        pass(&mut run);
    }
    let release = ServiceEvent::Took(RequestKind::Release);
    assert_eq!(
        seen.0[seen.0.len() - 7..],
        [
            release,
            release,
            release,
            release,
            ServiceEvent::Published(PublishEnd::Chose),
            ServiceEvent::TurnEnded(TurnEnd {
                blocks_free: true,
                credits_free: true,
                loans: 0,
            }),
            ServiceEvent::Waited { ready: false },
        ],
        "a budget that empties the lane is not spent, and no turn follows unwoken"
    );

    brand!(More);
    let (mut service, mut socket, _wire, _stop) = flow::<More>();
    let mut seen = Seen::default();
    {
        let mut run = pin!(service.run(clock, &mut seen));
        for block in hold(&mut socket, &mut run, TURN_COMMANDS + 1) {
            socket.release(block).ok().unwrap();
        }
        pass(&mut run);
        pass(&mut run);
    }
    let turns = turns(&seen.0);
    // The two turns the releases took.
    let last = &turns[turns.len() - 2..];
    assert_eq!(
        (last[0].took(RequestKind::Release), last[0].spent),
        (TURN_COMMANDS, 1)
    );
    assert_eq!((last[1].took(RequestKind::Release), last[1].spent), (1, 0));
}

#[test]
fn a_turn_says_which_resource_was_out_when_it_ended() {
    brand!(Net);
    let (mut service, mut socket, wire, _stop) = flow::<Net>();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut seen = Seen::default();
    let mut observers = Vec::new();
    {
        let mut run = pin!(service.run(clock, &mut seen));
        // The application takes every block of the pool.
        socket.reserve_ahead(RESERVE_AHEAD);
        let mut blocks = Vec::new();
        while blocks.len() < FLOW_BLOCKS {
            match socket.try_reserve().unwrap() {
                Some(block) => blocks.push(block),
                None => pass(&mut run),
            }
        }
        // A full port, and a datagram under every credit: each stays queued.
        wire.0.borrow_mut().full = true;
        for mut block in blocks.drain(..FLOW_CREDITS) {
            block.fill(b"held").unwrap();
            observers.push(socket.send(peer(), block).ok().unwrap());
        }
        for _ in 0..FLOW_CREDITS.div_ceil(TURN_COMMANDS) {
            pass(&mut run);
        }
    }
    let turns = turns(&seen.0);
    let last = turns.last().unwrap();
    assert_eq!(last.took(RequestKind::Send), TURN_COMMANDS);
    assert_eq!(last.published, [PublishEnd::Refused]);
    assert_eq!(
        last.end,
        Some(TurnEnd {
            blocks_free: false,
            credits_free: false,
            loans: 0,
        })
    );
    assert_eq!(
        turns.iter().map(|turn| turn.accepted).sum::<usize>(),
        0,
        "the port took nothing"
    );
    assert!(wire.0.borrow().sent.is_empty());
}

#[test]
fn a_useful_turn_is_followed_by_a_wait_not_another_turn() {
    brand!(Net);
    let (mut service, mut socket, _wire, _stop) = flow::<Net>();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut seen = Seen::default();
    {
        let mut run = pin!(service.run(clock, &mut seen));
        pass(&mut run);
        socket.reserve_ahead(1);
        assert!(socket.try_reserve().unwrap().is_none());
        pass(&mut run);
    }
    let passes: Vec<_> = seen
        .0
        .iter()
        .filter(|event| matches!(event, ServiceEvent::Yielded | ServiceEvent::Waited { .. }))
        .collect();
    assert_eq!(
        passes,
        [
            // Nothing to do: the first poll ends waiting.
            &ServiceEvent::Waited { ready: false },
            // The reserve wakes it, and its turn gives a block.
            &ServiceEvent::Waited { ready: true },
            // That turn left nothing a wake does not name: the poll ends
            // waiting, with no turn between.
            &ServiceEvent::Waited { ready: false },
        ]
    );
    assert_eq!(
        seen.0
            .iter()
            .filter(|event| matches!(event, ServiceEvent::TurnEnded(_)))
            .count(),
        2,
        "the first turn and the reserve's"
    );
}

#[test]
fn a_full_intake_says_so() {
    brand!(Net);
    let (mut service, _socket, wire, _stop) = flow::<Net>();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut seen = Seen::default();
    {
        let mut run = pin!(service.run(clock, &mut seen));
        // The turn's whole budget of received frames, and one more.
        arrive(&wire, 9);
        pass(&mut run);
        pass(&mut run);
    }
    let full = |events: &[ServiceEvent]| {
        events
            .iter()
            .filter(|event| **event == ServiceEvent::IntakeFull)
            .count()
    };
    assert_eq!(
        full(&seen.0),
        1,
        "only the first turn met more than it takes"
    );
}

#[test]
fn each_turn_tells_the_frames_it_took_in_and_the_notices_it_handed_on() {
    brand!(Net);
    let (mut service, _socket, wire, _stop) = flow::<Net>();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut seen = Seen::default();
    {
        let mut run = pin!(service.run(clock, &mut seen));
        // One more frame than a turn takes in, and three notices.
        arrive(&wire, 9);
        wire.0.borrow_mut().ended.extend(0..3);
        pass(&mut run);
        pass(&mut run);
    }
    let turns = turns(&seen.0);
    let frames: Vec<_> = turns.iter().map(|turn| turn.frames).collect();
    let notices: Vec<_> = turns.iter().map(|turn| turn.notices).collect();
    assert_eq!(
        frames.first().copied(),
        Some(8),
        "the first turn takes in its budget"
    );
    assert_eq!(frames.iter().sum::<usize>(), 9, "every frame, once");
    assert_eq!(
        notices.first().copied(),
        Some(3),
        "the first turn hands on every notice"
    );
    assert_eq!(notices.iter().sum::<usize>(), 3, "every notice, once");
}

/// An egress whose reservations are all out: it chooses nothing until a
/// notice, which never comes here.
struct Exhausted;

impl Egress<u32, u32> for Exhausted {
    type Reservation = ();
    type Fault = core::convert::Infallible;

    fn choose<C: Candidate>(
        &mut self,
        _ready: impl Iterator<Item = C> + Clone,
    ) -> Result<Option<(C, ())>, core::convert::Infallible> {
        Ok(None)
    }

    fn accepted(&mut self, (): (), _ticket: u32) -> Result<(), core::convert::Infallible> {
        Ok(())
    }

    fn refused(&mut self, (): ()) -> Result<(), core::convert::Infallible> {
        Ok(())
    }

    fn unknown(&mut self, (): ()) -> Result<(), core::convert::Infallible> {
        Ok(())
    }

    fn notice(&mut self, _notice: u32) -> Result<(), core::convert::Infallible> {
        Ok(())
    }
}

/// Work is ready and the egress chooses none: the service turns once and
/// waits for an event, with no wake of its own and no turn more, however
/// often it is polled.
#[test]
fn ready_work_the_egress_does_not_choose_leaves_the_service_waiting() {
    brand!(Net);
    let memory = Box::leak(Box::new([[0; SIZE]; FLOW_BLOCKS]));
    let (pool, brand) = claim::<Net, SIZE, FLOW_BLOCKS>(memory).unwrap();
    let wire = WirePort::default();
    wire.0.borrow_mut().answers = true;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (_stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service: Service<
        'static,
        'static,
        Net,
        SIZE,
        1,
        FLOW_CREDITS,
        RECEIVED,
        2,
        FLOW_BLOCKS,
        FLOW_DEPTH,
        RECEIVED,
        WirePort,
        Exhausted,
    > = Service::new(owner, Exhausted, MemoryPolicy::KEEP, stopping);
    let mut socket = service
        .bind(
            5000,
            Dscp::default(),
            Group::shared(),
            Box::leak(Box::new(SocketLanes::new())),
        )
        .ok()
        .unwrap();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut seen = Seen::default();
    let woken = Arc::new(Woken(AtomicBool::new(false)));
    let waker = Waker::from(woken.clone());
    let cx = &mut Context::from_waker(&waker);
    {
        let mut run = core::pin::pin!(service.run(clock, &mut seen));
        // The block comes in the turns a reservation takes.
        let block = loop {
            if let Some(block) = socket.try_reserve().unwrap() {
                break block;
            }
            assert!(run.as_mut().poll(cx).is_pending());
        };
        let _observer = socket.send(peer(), block).ok().unwrap();
        // The send's wake, and the turns it brings.
        let mut quiet = false;
        for _ in 0..8 {
            woken.0.store(false, Ordering::SeqCst);
            assert!(run.as_mut().poll(cx).is_pending());
            if !woken.0.load(Ordering::SeqCst) {
                quiet = true;
                break;
            }
        }
        assert!(quiet, "the service waits");
        // However often it is polled from here, it neither wakes itself.
        for _ in 0..16 {
            woken.0.store(false, Ordering::SeqCst);
            assert!(run.as_mut().poll(cx).is_pending());
            assert!(!woken.0.load(Ordering::SeqCst), "no wake of its own");
        }
    }
    let turns = turns(&seen.0);
    assert!(
        turns.len() <= 8 + 16 + 8,
        "one turn a poll at most: {}",
        turns.len()
    );
    assert!(
        seen.0
            .iter()
            .rev()
            .take(3)
            .any(|event| matches!(event, ServiceEvent::Waited { ready: false })),
        "it waits, nothing ready"
    );
}

/// What a turn hands on of the port's notices: a Block Ack's 32 ends wait
/// at once, with room for the datagram that waited for one. The turn that
/// finds them settles all and publishes in the same turn: no publication
/// waits behind a part of one aggregate's ends.
#[test]
fn a_block_ack_s_ends_settle_in_one_turn_which_publishes_again() {
    brand!(Net);
    const AGGREGATE: u32 = 32;
    let (mut service, mut socket, wire, _stop) = flow::<Net>();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let tape = Rc::new(RefCell::new(Vec::new()));
    let mut seen = Tape(tape.clone());
    {
        let mut run = core::pin::pin!(service.run(clock, &mut seen));
        let mut send = |run: &mut core::pin::Pin<&mut _>| {
            let block = loop {
                if let Some(block) = socket.try_reserve().unwrap() {
                    break block;
                }
                pass(run);
            };
            drop(socket.send(peer(), block).ok().unwrap());
            for _ in 0..8 {
                pass(run);
            }
        };
        // The peer's address resolves, and the first datagram goes.
        send(&mut run);
        assert_eq!(datagrams(&wire.0.borrow()).len(), 1);
        // The port is full: an aggregate is out, and the next datagram waits.
        wire.0.borrow_mut().full = true;
        send(&mut run);
        assert_eq!(datagrams(&wire.0.borrow()).len(), 1);
        // Its Block Ack: every end at once, and the room they free.
        {
            let mut wire = wire.0.borrow_mut();
            wire.ended.extend(0..AGGREGATE);
            wire.full = false;
        }
        let before = turns(&tape.borrow()).len();
        while !wire.0.borrow().ended.is_empty() || datagrams(&wire.0.borrow()).len() < 2 {
            pass(&mut run);
            assert!(
                turns(&tape.borrow()).len() < before + 8,
                "the ends are handed on"
            );
        }
        let ack = &turns(&tape.borrow())[before..];
        assert_eq!(ack.len(), 1, "one turn settles them all: {ack:?}");
        assert_eq!(
            ack[0].published,
            [PublishEnd::Chose],
            "it publishes; nothing is deferred"
        );
        assert_eq!(ack[0].accepted, 1, "the waiting datagram goes in that turn");
    }
}

/// More datagrams ready than one turn publishes: the turn that spent its
/// budget turns again, though no lane and no port wakes it.
#[test]
fn a_spent_publishing_budget_turns_again_unwoken() {
    brand!(Net);
    const QUEUED: usize = 12;
    let (mut service, mut socket, wire, stop) = flow::<Net>();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut seen = Seen::default();
    let drive = async {
        socket.reserve_ahead(RESERVE_AHEAD);
        // The peer's address first, so nothing below waits for ARP.
        let mut block = socket.reserve().await.unwrap();
        block.fill(b"resolve").unwrap();
        socket.credit().await.unwrap();
        let first = socket.send(peer(), block).ok().unwrap();
        assert_eq!(result_of(first).await, Sent::Ended(Outcome::Accepted));
        wire.0.borrow_mut().full = true;
        for count in 0..QUEUED {
            let mut block = socket.reserve().await.unwrap();
            block.fill(&[u8::try_from(count).unwrap()]).unwrap();
            socket.credit().await.unwrap();
            drop(socket.send(peer(), block).ok().unwrap());
        }
        settle().await;
        {
            let mut wire = wire.0.borrow_mut();
            wire.full = false;
            wire.wake();
        }
        // Only the service's own next turn sends what the budget left.
        while datagrams(&wire.0.borrow()).len() < QUEUED + 1 {
            yield_now().await;
        }
        stop.try_send(()).ok().unwrap();
    };
    let (cause, ()) = run_tasks(clock, service.run(clock, &mut seen), drive);
    assert_eq!(cause, EndCause::Stopped);
    let turns = turns(&seen.0);
    assert!(
        turns
            .iter()
            .any(|turn| turn.published == [PublishEnd::Budget]),
        "one turn spent its budget"
    );
}

/// More notices than a turn hands the policy: that turn publishes nothing
/// and names its own next turn, which it takes after a yield, not after a
/// wait.
#[test]
fn notices_past_a_turn_s_budget_turn_again_after_a_yield() {
    brand!(Net);
    let (mut service, _socket, wire, _stop) = flow::<Net>();
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut seen = Seen::default();
    {
        let mut run = pin!(service.run(clock, &mut seen));
        pass(&mut run);
        wire.0.borrow_mut().ended.extend(0..65);
        pass(&mut run);
        pass(&mut run);
    }
    assert!(wire.0.borrow().ended.is_empty());
    let deferred = seen
        .0
        .iter()
        .position(|event| *event == ServiceEvent::Published(PublishEnd::Deferred))
        .unwrap();
    let after: Vec<_> = seen.0[deferred + 1..]
        .iter()
        .filter(|event| {
            matches!(
                event,
                ServiceEvent::Yielded | ServiceEvent::Waited { .. } | ServiceEvent::TurnBegan
            )
        })
        .take(2)
        .collect();
    assert_eq!(
        after,
        [&ServiceEvent::Yielded, &ServiceEvent::TurnBegan],
        "the deferred turn yields and turns again"
    );
}

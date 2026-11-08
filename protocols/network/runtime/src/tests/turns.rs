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

/// One turn, as its events tell it.
#[derive(Debug, Default)]
struct Turn {
    took: Vec<RequestKind>,
    spent: usize,
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
            ServiceEvent::Took(kind) => turn.took.push(kind),
            ServiceEvent::BudgetSpent => turn.spent += 1,
            ServiceEvent::Accepted => turn.accepted += 1,
            ServiceEvent::Published(end) => turn.published.push(end),
            ServiceEvent::TurnEnded(end) => {
                turn.end = Some(end);
                turns.push(core::mem::take(&mut turn));
            }
            ServiceEvent::IntakeFull | ServiceEvent::Yielded | ServiceEvent::Waited { .. } => {}
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
    // This sender takes every block it holds before it asks again, so each
    // reservation asks for the whole look-ahead: messages per datagram are
    // 1 + 1/16, not 2.
    assert_eq!(reserves * RESERVE_AHEAD, asked);
    assert!(
        reserves <= DATAGRAMS.div_ceil(RESERVE_AHEAD) + 1,
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

/// Take `count` blocks into the application's hands, a turn of `run` at a
/// time, leaving nothing asked.
fn hold<B, R: Future>(
    socket: &mut FlowSocket<B>,
    run: &mut core::pin::Pin<&mut R>,
    count: usize,
) -> Vec<Block<'static, B, SIZE>> {
    socket.reserve_ahead(count);
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
            }),
            ServiceEvent::Yielded,
        ],
        "a budget that empties the lane is not spent"
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
fn a_pass_says_whether_it_yielded_or_waited() {
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
            // The reserve wakes it: the turn gives a block and yields.
            &ServiceEvent::Yielded,
            // The next turn has nothing, so the poll ends waiting again.
            &ServiceEvent::Waited { ready: false },
        ]
    );
    assert_eq!(
        seen.0
            .iter()
            .filter(|event| matches!(event, ServiceEvent::TurnEnded(_)))
            .count(),
        3,
        "one turn before each"
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

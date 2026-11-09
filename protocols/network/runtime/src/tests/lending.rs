//! A lending socket through the service: credits, refusals, and loans that
//! outlive a stop.

extern crate std;

use std::boxed::Box;
use std::vec::Vec;

use oer_portable_events::EventSink;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_core::{
    Group, Interface, LoansRefused, NetEvent, NetworkOwner, NotLent, OpenRefused, claim,
};
use oer_protocols_network_port::{CopyReason, Holding};
use oer_protocols_network_wire::ip::Dscp;

use super::{
    BLOCKS, CREDITS, DEPTH, LOCAL, OUR_IP, RECEIVED, SIZE, TestPair, TestSocket, WireHeld,
    WirePort, run_tasks, settle, storage, udp_frame,
};
use crate::{
    Direct, LendingLanes, LendingSocket, MemoryPolicy, Received, Service, ServiceEvent, StopLane,
};

const LENDING: u16 = 6000;
const COPYING: u16 = 5000;

type Lending<B> = LendingSocket<'static, 'static, B, SIZE, DEPTH, RECEIVED, CREDITS, WireHeld>;

/// What the core dropped, and why.
#[derive(Default)]
struct Drops(Vec<NetEvent>);

impl EventSink<NetEvent> for Drops {
    fn event(&mut self, event: NetEvent) {
        self.0.push(event);
    }
}

impl EventSink<ServiceEvent> for Drops {
    fn event(&mut self, _: ServiceEvent) {}
}

fn lending_storage<B>()
-> &'static mut LendingLanes<'static, 'static, B, SIZE, DEPTH, RECEIVED, CREDITS, WireHeld> {
    Box::leak(Box::new(LendingLanes::new()))
}

/// A service whose account lends `buffers` of which `reserve` stay the
/// radio's, with a copying socket and a lending one of credit 1.
fn lending<B: 'static>(
    buffers: usize,
    reserve: usize,
) -> (
    TestPair<B, BLOCKS>,
    TestSocket<B>,
    Lending<B>,
    WirePort,
    oer_portable_memory::WakingSpscSender<'static, (), 1>,
) {
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (mut pool, brand) = claim::<B, SIZE, BLOCKS>(memory).unwrap();
    pool.loans().pool(buffers, reserve).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let wire = WirePort::default();
    {
        let mut state = wire.0.borrow_mut();
        state.answers = true;
        state.holding = Some(Holding::Lendable);
    }
    let owner = NetworkOwner::new(brand, interface, pool, wire.clone());
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service = Service::new(owner, Direct, MemoryPolicy::KEEP, stopping);
    let copying = service
        .bind(COPYING, Dscp::default(), Group::shared(), storage::<B>())
        .ok()
        .unwrap();
    let group = service.group(2).unwrap();
    let lending = service
        .bind_lending::<1>(LENDING, Dscp::default(), group, lending_storage::<B>())
        .ok()
        .unwrap();
    (service, copying, lending, wire, stop)
}

fn arrive(wire: &WirePort, port: u16, data: &[u8]) {
    let mut state = wire.0.borrow_mut();
    state.incoming.push_back(udp_frame(port, data));
    state.wake();
}

fn holding(wire: &WirePort, holding: Holding) {
    wire.0.borrow_mut().holding = Some(holding);
}

fn lent_out(wire: &WirePort) -> usize {
    wire.0.borrow().lent_out.get()
}

#[test]
fn a_slow_lending_socket_leaves_the_copying_socket_and_control_their_own() {
    #[derive(Debug)]
    struct Net;
    let (mut service, mut copying, mut lending, wire, stop) = lending::<Net>(4, 1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut drops = Drops::default();
    let drive = async {
        arrive(&wire, LENDING, b"held");
        let Received::Lent(mut held) = lending.receive().await.unwrap() else {
            panic!("a lendable datagram is lent");
        };
        assert_eq!(held.data().unwrap(), b"held");
        // The lending socket reads no more: what comes for it finds no
        // credit, and the copying socket's datagram still comes.
        arrive(&wire, LENDING, b"no credit");
        arrive(&wire, COPYING, b"copied");
        let copied = copying.receive().await.unwrap();
        assert_eq!(copied.block.octets(), b"copied");
        copying.release(copied.block).unwrap();
        // Control traffic needs no credit either: the service resolves the
        // peer and sends.
        let mut block = copying.reserve().await.unwrap();
        block.fill(b"out").unwrap();
        let mut observer = copying.send(super::peer(), block).ok().unwrap();
        observer.finished().await;
        assert_eq!(
            observer.take().ok(),
            Some(crate::Sent::Ended(
                oer_protocols_network_core::Outcome::Accepted
            ))
        );
        assert!(lending.try_receive().is_none(), "nothing more was lent");
        assert_eq!(lent_out(&wire), 1, "the dropped one went back at once");
        drop(held);
        stop.try_send(()).ok().unwrap();
    };
    run_tasks(clock, service.run(clock, &mut drops), drive);
    assert_eq!(drops.0, [NetEvent::NotLent(NotLent::NoCredit)]);
    let mut ended = service.retire();
    ended.collect();
    assert!(ended.retired.pool.loans().released());
    assert_eq!(lent_out(&wire), 0);
}

#[test]
fn a_returned_buffer_frees_its_credit_for_the_next_datagram() {
    #[derive(Debug)]
    struct Net;
    let (mut service, _copying, mut lending, wire, stop) = lending::<Net>(4, 1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut drops = Drops::default();
    let drive = async {
        arrive(&wire, LENDING, b"first");
        let first = lending.receive().await.unwrap();
        arrive(&wire, LENDING, b"dropped");
        settle().await;
        drop(first);
        settle().await;
        arrive(&wire, LENDING, b"second");
        let Received::Lent(mut second) = lending.receive().await.unwrap() else {
            panic!("lent once the first came back");
        };
        assert_eq!(second.data().unwrap(), b"second");
        drop(second);
        stop.try_send(()).ok().unwrap();
    };
    run_tasks(clock, service.run(clock, &mut drops), drive);
    assert_eq!(drops.0, [NetEvent::NotLent(NotLent::NoCredit)]);
}

#[test]
fn a_pressure_copy_is_dropped_and_a_format_copy_comes_in_a_block() {
    #[derive(Debug)]
    struct Net;
    let (mut service, _copying, mut lending, wire, stop) = lending::<Net>(4, 1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut drops = Drops::default();
    let drive = async {
        holding(&wire, Holding::Copied(CopyReason::Pressure));
        arrive(&wire, LENDING, b"pressed");
        settle().await;
        holding(&wire, Holding::Copied(CopyReason::Format));
        arrive(&wire, LENDING, b"formed");
        let Received::Copied(copied) = lending.receive().await.unwrap() else {
            panic!("a format copy comes in a block");
        };
        assert_eq!(copied.block.octets(), b"formed");
        lending.socket().release(copied.block).unwrap();
        settle().await;
        stop.try_send(()).ok().unwrap();
    };
    run_tasks(clock, service.run(clock, &mut drops), drive);
    assert_eq!(drops.0, [NetEvent::PressureCopy]);
    let mut ended = service.retire();
    assert!(ended.retired.pool.loans().released());
    assert_eq!(ended.retired.pool.outstanding(), 0);
}

#[test]
fn a_stop_ends_with_a_buffer_held_and_its_late_return_frees_the_loan() {
    #[derive(Debug)]
    struct Net;
    let (mut service, _copying, mut lending, wire, stop) = lending::<Net>(4, 1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut drops = Drops::default();
    let drive = async {
        arrive(&wire, LENDING, b"held");
        let held = lending.receive().await.unwrap();
        stop.try_send(()).ok().unwrap();
        held
    };
    // The service ends with the buffer in the application's hands.
    let (_, held) = run_tasks(clock, service.run(clock, &mut drops), drive);
    let mut ended = service.retire();
    assert_eq!(ended.retired.pool.loans().outstanding(), 1);
    assert_eq!(lent_out(&wire), 1);
    ended.collect();
    assert_eq!(
        ended.retired.pool.loans().outstanding(),
        1,
        "nothing expires"
    );
    // The pool's live holder takes the late return.
    drop(held);
    assert_eq!(lent_out(&wire), 0, "the buffer is the port's again");
    ended.collect();
    assert!(ended.retired.pool.loans().released());
    // The socket ends after its service, and its late part with its drop.
    assert!(matches!(
        pollster_receive(&mut lending),
        Err(crate::SocketClosed)
    ));
    assert!(ended.sockets[1].is_some());
    drop(lending);
    ended.collect();
    assert!(ended.sockets[1].is_none(), "nothing more is owed for it");
}

/// Receive once, with nothing to wait for.
fn pollster_receive<B>(
    lending: &mut Lending<B>,
) -> Result<Received<'static, 'static, B, SIZE, WireHeld>, crate::SocketClosed> {
    let mut receive = core::pin::pin!(lending.receive());
    let cx = &mut core::task::Context::from_waker(core::task::Waker::noop());
    match receive.as_mut().poll(cx) {
        core::task::Poll::Ready(received) => received,
        core::task::Poll::Pending => panic!("a stopped socket answers at once"),
    }
}

#[test]
fn a_late_return_reaches_the_next_service_that_adopted_the_socket() {
    #[derive(Debug)]
    struct Net;
    let (mut service, _copying, mut lending, wire, stop) = lending::<Net>(2, 0);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut drops = Drops::default();
    let drive = async {
        arrive(&wire, LENDING, b"held");
        let held = lending.receive().await.unwrap();
        stop.try_send(()).ok().unwrap();
        held
    };
    let (_, held) = run_tasks(clock, service.run(clock, &mut drops), drive);
    let mut ended = service.retire();
    let late = ended.sockets.each_mut().map(Option::take);
    let retired = ended.retired;
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(retired.brand, interface, retired.pool, retired.port);
    let (next_stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut next: TestPair<Net, BLOCKS> = Service::new(owner, Direct, MemoryPolicy::KEEP, stopping);
    for late in late.into_iter().flatten() {
        next.adopt(late).ok().unwrap();
    }
    // The old loan counts against the next installation's credit.
    let refused = next
        .bind_lending::<2>(
            LENDING,
            Dscp::default(),
            Group::shared(),
            lending_storage::<Net>(),
        )
        .err()
        .unwrap();
    assert_eq!(
        refused.reason,
        OpenRefused::Loans(LoansRefused::Pool { room: 1 })
    );
    let drive = async {
        drop(held);
        settle().await;
        next_stop.try_send(()).ok().unwrap();
    };
    run_tasks(clock, next.run(clock, &mut drops), drive);
    let mut ended = next.retire();
    assert!(
        ended.retired.pool.loans().released(),
        "the late return came home"
    );
}

#[test]
fn a_credit_the_account_cannot_hold_refuses_the_bind_and_returns_the_storage() {
    #[derive(Debug)]
    struct Net;
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (mut pool, brand) = claim::<Net, SIZE, BLOCKS>(memory).unwrap();
    pool.loans().pool(2, 1).unwrap();
    let interface = Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap();
    let owner = NetworkOwner::new(brand, interface, pool, WirePort::default());
    let (_stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service: TestPair<Net, BLOCKS> =
        Service::new(owner, Direct, MemoryPolicy::KEEP, stopping);
    let refused = service
        .bind_lending::<2>(
            LENDING,
            Dscp::default(),
            Group::shared(),
            lending_storage::<Net>(),
        )
        .err()
        .unwrap();
    assert_eq!(
        refused.reason,
        OpenRefused::Loans(LoansRefused::Pool { room: 1 })
    );
    // Nothing changed: the same storage binds a credit that fits.
    assert!(
        service
            .bind_lending::<1>(LENDING, Dscp::default(), Group::shared(), refused.storage)
            .is_ok()
    );
}

#[test]
fn serving_late_loans_after_a_stop_waits_for_the_held_buffer_and_closes_the_account() {
    #[derive(Debug)]
    struct Net;
    let (mut service, _copying, mut lending, wire, stop) = lending::<Net>(4, 1);
    let clock = Box::leak(Box::new(VirtualClock::new()));
    let mut drops = Drops::default();
    let drive = async {
        arrive(&wire, LENDING, b"held");
        let held = lending.receive().await.unwrap();
        stop.try_send(()).ok().unwrap();
        held
    };
    let (_, held) = run_tasks(clock, service.run(clock, &mut drops), drive);
    let mut ended = service.retire();
    assert_eq!(ended.retired.pool.loans().outstanding(), 1);
    let woken = std::sync::Arc::new(super::Woken(std::sync::atomic::AtomicBool::new(false)));
    let waker = core::task::Waker::from(woken.clone());
    let cx = &mut core::task::Context::from_waker(&waker);
    {
        let mut serving = core::pin::pin!(ended.serve_loans());
        assert!(serving.as_mut().poll(cx).is_pending(), "a loan is out");
        drop(held);
        assert!(
            woken.0.load(std::sync::atomic::Ordering::SeqCst),
            "the return wakes the server"
        );
        assert!(serving.as_mut().poll(cx).is_ready(), "the account closes");
    }
    assert!(ended.retired.pool.loans().released());
    assert_eq!(lent_out(&wire), 0);
}

/// Every event of both kinds, in order.
#[derive(Default)]
struct Heard {
    service: Vec<ServiceEvent>,
    drops: Vec<NetEvent>,
}

impl EventSink<ServiceEvent> for Heard {
    fn event(&mut self, event: ServiceEvent) {
        self.service.push(event);
    }
}

impl EventSink<NetEvent> for Heard {
    fn event(&mut self, event: NetEvent) {
        self.drops.push(event);
    }
}

/// Poll `run` once.
fn pass<R: core::future::Future>(run: &mut core::pin::Pin<&mut R>) {
    let cx = &mut core::task::Context::from_waker(core::task::Waker::noop());
    assert!(run.as_mut().poll(cx).is_pending());
}

/// A frame that finds no credit tells the loans then: the one out, and the
/// one already back with its port, dropped by another context after the
/// turn took its returns home and before the frame. The next turn gives
/// that loan back and says so; the buffer went back to the port at the drop,
/// before any of it.
#[test]
fn a_frame_without_credit_tells_the_loans_out_and_those_back_but_not_given_back() {
    use crate::{Credit, Home};
    #[derive(Debug)]
    struct Net;
    let (mut service, _copying, mut lending, wire, _stop) = lending::<Net>(4, 1);
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut heard = Heard::default();
    {
        let mut run = core::pin::pin!(service.run(clock, &mut heard));
        arrive(&wire, LENDING, b"first");
        pass(&mut run);
        let Some(Received::Lent(held)) = lending.try_receive() else {
            panic!("a lendable datagram is lent");
        };
        assert_eq!(lent_out(&wire), 1);
        // Another context drops it while the next turn runs, between its
        // returns and its frame.
        let port = wire.clone();
        wire.0.borrow_mut().on_take = Some(Box::new(move || {
            drop(held);
            assert_eq!(lent_out(&port), 0, "the buffer is back at the drop");
        }));
        arrive(&wire, LENDING, b"second");
        pass(&mut run);
        pass(&mut run);
    }
    assert_eq!(heard.drops, [NetEvent::NotLent(NotLent::NoCredit)]);
    let credit: Vec<Credit> = heard
        .service
        .iter()
        .filter_map(|event| match event {
            ServiceEvent::NoCredit(credit) => Some(*credit),
            _ => None,
        })
        .collect();
    assert_eq!(
        credit,
        [Credit {
            drops: 1,
            outstanding: 1,
            returned: 1,
        }],
        "the credit was out, and its loan was back but not given back"
    );
    let home: Vec<Home> = heard
        .service
        .iter()
        .filter_map(|event| match event {
            ServiceEvent::Home(home) => Some(*home),
            _ => None,
        })
        .collect();
    assert_eq!(
        home,
        [Home {
            blocks: 0,
            loans: 1
        }],
        "given back once"
    );
    let at_drop = heard
        .service
        .iter()
        .position(|event| matches!(event, ServiceEvent::NoCredit(_)))
        .unwrap();
    let given = heard
        .service
        .iter()
        .position(|event| matches!(event, ServiceEvent::Home(_)))
        .unwrap();
    assert!(given > at_drop, "given back in a later turn");
    assert_eq!(
        heard.service[..given]
            .iter()
            .rev()
            .find(|event| matches!(event, ServiceEvent::TurnBegan | ServiceEvent::TurnEnded(_))),
        Some(&ServiceEvent::TurnBegan),
        "within that turn"
    );
}

/// A turn whose only work is giving loans back says so, and a sink that
/// counts no turns sees nothing of it: loans given back are their own
/// count, apart from frames, notices and requests.
#[test]
fn loans_given_back_are_counted_apart_from_frames_and_notices() {
    use crate::Home;
    #[derive(Debug)]
    struct Net;
    let (mut service, _copying, mut lending, wire, _stop) = lending::<Net>(4, 1);
    let clock: &VirtualClock = Box::leak(Box::new(VirtualClock::new()));
    let mut heard = Heard::default();
    {
        let mut run = core::pin::pin!(service.run(clock, &mut heard));
        arrive(&wire, LENDING, b"one");
        pass(&mut run);
        let Some(Received::Lent(held)) = lending.try_receive() else {
            panic!("a lendable datagram is lent");
        };
        pass(&mut run);
        drop(held);
        pass(&mut run);
    }
    let turns: Vec<&[ServiceEvent]> = heard
        .service
        .split(|event| *event == ServiceEvent::TurnBegan)
        .collect();
    let homing: Vec<_> = turns
        .iter()
        .filter(|turn| {
            turn.iter()
                .any(|event| matches!(event, ServiceEvent::Home(_)))
        })
        .collect();
    let [turn] = homing[..] else {
        panic!("one turn gave the loan back: {homing:?}");
    };
    assert!(
        turn.contains(&ServiceEvent::Home(Home {
            blocks: 0,
            loans: 1
        })),
        "{turn:?}"
    );
    assert!(
        !turn.iter().any(|event| matches!(
            event,
            ServiceEvent::FrameTaken | ServiceEvent::Noticed | ServiceEvent::Took(_)
        )),
        "the turn did nothing else: {turn:?}"
    );
}

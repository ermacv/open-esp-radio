//! A turn with certainly no work collects nothing: its policy hears
//! `nothing_ready` in place of a plan, and the diagnostics count no
//! collection for it. Every source of work still reaches a collection, in
//! the turn it comes or the turn its time comes, and a turn with no work
//! still takes its frames, commands and notices.

use oer_portable_time::{Duration, Timer};
use oer_protocols_network_wire::dhcp::MessageType;

use super::super::probe::Probe;
use super::*;
use crate::ReturnLane;

/// The steps of each turn after `mark`, in order.
fn turns_after(policy: &Planned, mark: usize) -> Vec<Vec<Step>> {
    let steps = policy.0.borrow()[mark..].to_vec();
    steps
        .split(|step| *step == Step::Turn)
        .skip(1)
        .map(<[Step]>::to_vec)
        .collect()
}

/// A turn's publishing, in either form it may take.
const NOTHING: &[Step] = &[Step::NothingReady];
const ONE_FRAME: &[Step] = &[Step::Collected, Step::Reserved, Step::Accepted];

/// The peer asks for us: the service owes it a reply.
fn peer_asks() -> Vec<u8> {
    let mut frame = EthernetHeader {
        destination: [0xFF; 6],
        source: PEER,
        ether_type: EtherType::ARP,
    }
    .to_bytes()
    .to_vec();
    let mut packet = [0; arp::PACKET_LEN];
    Arp {
        operation: Operation::Request,
        sender_hardware: PEER,
        sender_protocol: PEER_IP,
        target_hardware: [0; 6],
        target_protocol: OUR_IP,
    }
    .write(&mut packet)
    .unwrap();
    frame.extend_from_slice(&packet);
    frame
}

fn is_arp_reply(frame: &[u8]) -> bool {
    let (header, payload) = EthernetHeader::parse(frame).unwrap();
    header.ether_type == EtherType::ARP
        && Arp::parse(payload).unwrap().unwrap().operation == Operation::Reply
}

/// A datagram comes in with a notice and the socket gives its block back:
/// each of those turns collects nothing and counts no collection, and still
/// takes the frame, the notice and the release. A send after them is
/// collected and goes.
#[test]
fn a_turn_with_nothing_ready_builds_no_plan_and_work_after_it_goes() {
    brand!(Net);
    let probe = Probe::on();
    let log = Planned::default();
    let PlanBench {
        service,
        mut sockets,
        wire,
        policy,
        stop,
        clock,
    } = plan_bench_diagnosed::<Net, _, _>(log.clone(), log, probe.clone());
    let mut events = TurnMarks(policy.clone());
    let drive = async {
        let [a, _] = &mut sockets;
        let first = plan_send(a, b"first").await;
        assert_eq!(result_of(first).await, Sent::Ended(Outcome::Accepted));
        settle().await;
        let mark = policy.0.borrow().len();
        let before = probe.total();
        {
            let mut wire = wire.0.borrow_mut();
            wire.incoming.push_back(udp_frame(5000, b"in"));
            wire.ended.push_back(1);
            wire.wake();
        }
        let datagram = a.receive().await.unwrap();
        assert_eq!(datagram.block.octets(), b"in");
        a.release(datagram.block).ok().unwrap();
        settle().await;
        assert!(wire.0.borrow().ended.is_empty(), "the notice was taken");
        let quiet = turns_after(&policy, mark);
        assert!(!quiet.is_empty());
        assert!(
            quiet.iter().all(|turn| turn == NOTHING),
            "no turn collected: {quiet:?}"
        );
        let during = probe.total();
        assert_eq!(
            during.turns - before.turns,
            u32::try_from(quiet.len()).unwrap()
        );
        assert_eq!(during.collections, before.collections, "none counted");
        let mark = policy.0.borrow().len();
        let later = plan_send(a, b"later").await;
        assert_eq!(result_of(later).await, Sent::Ended(Outcome::Accepted));
        let after = turns_after(&policy, mark);
        assert!(after.iter().any(|turn| turn == ONE_FRAME), "{after:?}");
        let collected = after
            .iter()
            .flatten()
            .filter(|step| **step == Step::Collected)
            .count();
        assert_eq!(
            probe.total().collections - during.collections,
            u32::try_from(collected).unwrap(),
            "each real collection counted, once"
        );
        stop.try_send(()).ok().unwrap();
    };
    let serve = async {
        let mut service = service;
        let _cause = service.run(clock, &mut events).await;
        service.retire()
    };
    let (mut ended, ()) = run_tasks(clock, serve, drive);
    home(&mut ended, &mut sockets);
    assert_eq!(
        datagrams(&wire.0.borrow()),
        [b"first".to_vec(), b"later".to_vec()]
    );
}

/// Control work a received frame makes is collected in that frame's turn.
#[test]
fn control_work_after_a_received_frame_goes_in_its_turn() {
    brand!(Net);
    let log = Planned::default();
    let PlanBench {
        service,
        mut sockets,
        wire,
        policy,
        stop,
        clock,
    } = plan_bench_with::<Net, _>(log.clone(), log);
    let mut events = TurnMarks(policy.clone());
    let drive = async {
        settle().await;
        let mark = policy.0.borrow().len();
        {
            let mut wire = wire.0.borrow_mut();
            wire.incoming.push_back(peer_asks());
            wire.wake();
        }
        settle().await;
        let turns = turns_after(&policy, mark);
        assert_eq!(turns.first().map(Vec::as_slice), Some(ONE_FRAME));
        assert!(turns[1..].iter().all(|turn| turn == NOTHING), "{turns:?}");
        let sent = wire.0.borrow().sent.clone();
        assert_eq!(sent.len(), 1);
        assert!(is_arp_reply(&sent[0]));
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

/// A DHCP client's retransmission is control work its time makes: the turn
/// its time comes collects it, and the turns between collect nothing.
#[test]
fn control_work_its_time_makes_goes_when_the_time_comes() {
    brand!(Net);
    let log = Planned::default();
    let interface = Interface::dhcp(LOCAL, 1500, 3).unwrap();
    let PlanBench {
        service,
        mut sockets,
        wire,
        policy,
        stop,
        clock,
    } = plan_bench_on::<Net, _, _>(interface, log.clone(), log, ());
    let mut events = TurnMarks(policy.clone());
    let drive = async {
        clock.wait_for(Duration::from_secs(20)).await.unwrap();
        stop.try_send(()).ok().unwrap();
    };
    let serve = async {
        let mut service = service;
        let _cause = service.run(clock, &mut events).await;
        service.retire()
    };
    let (mut ended, ()) = run_tasks(clock, serve, drive);
    home(&mut ended, &mut sockets);
    let sent = wire.0.borrow().sent.clone();
    let discovers = dhcp_messages(&sent)
        .iter()
        .filter(|(kind, _)| *kind == MessageType::Discover)
        .count();
    assert!(discovers >= 2, "the client asked again: {discovers}");
    let turns = turns_after(&policy, 0);
    let sending = turns.iter().filter(|turn| *turn == ONE_FRAME).count();
    assert_eq!(sending, discovers, "each discover in a turn of its own");
    assert!(
        turns
            .iter()
            .all(|turn| turn == ONE_FRAME || turn == NOTHING || turn.is_empty()),
        "every other turn collected nothing: {turns:?}"
    );
}

/// A block that comes home through the return lane while the socket waits
/// for one: the socket is granted it, and its datagram is collected and goes.
#[test]
fn a_block_returned_to_a_waiting_socket_goes_out_after_turns_with_nothing_ready() {
    brand!(Net);
    let log = Planned::default();
    let PlanBench {
        mut service,
        mut sockets,
        wire,
        policy,
        stop,
        clock,
    } = plan_bench_with::<Net, _>(log.clone(), log);
    let (returner, end) = Box::leak(Box::new(ReturnLane::new())).split();
    service.attach_returns(end).ok().unwrap();
    let mut events = TurnMarks(policy.clone());
    let drive = async {
        let [a, _] = &mut sockets;
        let mut held = Vec::new();
        for _ in 0..PLAN_BLOCKS {
            held.push(a.reserve().await.unwrap());
        }
        let cx = &mut Context::from_waker(Waker::noop());
        assert!(pin!(a.reserve()).poll(cx).is_pending(), "none is free");
        settle().await;
        let mark = policy.0.borrow().len();
        returner.give(held.pop().unwrap()).ok().unwrap();
        let mut block = a.reserve().await.unwrap();
        block.fill(b"returned").unwrap();
        a.credit().await.unwrap();
        let observer = a.send(peer(), block).ok().unwrap();
        assert_eq!(result_of(observer).await, Sent::Ended(Outcome::Accepted));
        let turns = turns_after(&policy, mark);
        assert!(turns.iter().any(|turn| turn == NOTHING), "{turns:?}");
        assert!(
            turns.iter().flatten().any(|step| *step == Step::Collected),
            "{turns:?}"
        );
        for block in held {
            returner.give(block).ok().unwrap();
        }
        settle().await;
        stop.try_send(()).ok().unwrap();
    };
    let serve = async {
        let mut service = service;
        let _cause = service.run(clock, &mut events).await;
        service.retire()
    };
    let (mut ended, ()) = run_tasks(clock, serve, drive);
    ended.close_returns();
    home(&mut ended, &mut sockets);
    assert_eq!(datagrams(&wire.0.borrow()), [b"returned".to_vec()]);
}

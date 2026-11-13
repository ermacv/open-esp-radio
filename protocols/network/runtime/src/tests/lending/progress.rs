//! A saved receive future does not own the service's ability to receive.

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};

use super::*;

fn saved_wait<B>(socket: &mut Lending<B>, prefix: bool) -> Pin<Box<dyn Future<Output = ()> + '_>> {
    if prefix {
        Box::pin(async {
            let _received = socket.receive_prefix(8, |_| true).await;
        })
    } else {
        Box::pin(async {
            let _received = socket.receive().await;
        })
    }
}

fn parked_receive<B: 'static>(prefix: bool) {
    let (mut service, mut copying, mut lending, wire, stop) = lending::<B>(4, 1);
    let clock: VirtualClock = VirtualClock::new();
    let mut events = Drops::default();
    let cx = &mut Context::from_waker(Waker::noop());
    let mut run = Box::pin(service.run(&clock, &mut events));
    assert!(run.as_mut().poll(cx).is_pending());
    let mut first = saved_wait(&mut lending, prefix);
    assert!(first.as_mut().poll(cx).is_pending());
    arrive(&wire, LENDING, b"parked");
    assert!(run.as_mut().poll(cx).is_pending());
    assert_eq!(lent_out(&wire), 1);

    // Keep first alive but never poll it again. The application instead
    // polls its second socket, with no clock advance or cancellation.
    for payload in [b"second".as_slice(), b"third".as_slice()] {
        arrive(&wire, COPYING, payload);
        let mut other = Box::pin(copying.receive());
        let mut received = None;
        for _ in 0..4 {
            assert!(run.as_mut().poll(cx).is_pending());
            if let Poll::Ready(answer) = other.as_mut().poll(cx) {
                received = Some(answer.unwrap());
                break;
            }
        }
        let received =
            received.expect("the other socket progresses before the first is polled or cancelled");
        assert_eq!(received.block.octets(), payload);
        drop(other);
        assert!(copying.release(received.block).is_ok());
        assert!(wire.0.borrow().incoming.is_empty());
        assert_eq!(lent_out(&wire), 1, "the parked receive keeps its frame");
    }

    // Stop also completes with the first future and its queued owner intact.
    stop.try_send(()).ok().unwrap();
    let mut stopped = false;
    for _ in 0..4 {
        if let Poll::Ready(cause) = run.as_mut().poll(cx) {
            assert_eq!(cause, crate::EndCause::Stopped);
            stopped = true;
            break;
        }
    }
    assert!(stopped);
    assert_eq!(lent_out(&wire), 1);
    drop(run);
    drop(first);
    let Some(Received::Lent(mut original)) = lending.try_receive() else {
        panic!("the original owner remains queued");
    };
    assert_eq!(original.data().unwrap(), b"parked");
    drop(original);
    assert_eq!(lent_out(&wire), 0);
    let mut ended = service.retire();
    drop(lending);
    drop(copying);
    ended.collect();
    assert!(ended.retired.pool.loans().released());
    assert_eq!(ended.retired.pool.outstanding(), 0);
    assert!(events.0.is_empty());
}

#[test]
fn a_saved_prefix_wait_leaves_the_other_socket_independent() {
    struct Net;
    parked_receive::<Net>(true);
}

#[test]
fn a_saved_ordinary_receive_leaves_the_other_socket_independent() {
    struct Net;
    parked_receive::<Net>(false);
}

fn two_lenders<B: 'static>() -> (
    TestPair<B, BLOCKS>,
    Lending<B>,
    Lending<B>,
    WirePort,
    oer_portable_memory::WakingSpscSender<'static, (), 1>,
) {
    let memory = Box::leak(Box::new([[0; SIZE]; BLOCKS]));
    let (mut pool, brand) = claim::<B, SIZE, BLOCKS>(memory).unwrap();
    pool.loans().pool(4, 1).unwrap();
    let wire = WirePort::default();
    holding(&wire, Holding::Lendable);
    let owner = NetworkOwner::new(
        brand,
        Interface::new(LOCAL, OUR_IP, 24, None, 1500).unwrap(),
        pool,
        wire.clone(),
    );
    let (stop, stopping) = Box::leak(Box::new(StopLane::new())).split();
    let mut service: TestPair<B, BLOCKS> = Service::new(owner, MemoryPolicy::KEEP, stopping);
    let first = service
        .bind_lending::<1>(LENDING, Dscp::default(), Group::shared(), lending_storage())
        .ok()
        .unwrap();
    let second = service
        .bind_lending::<1>(COPYING, Dscp::default(), Group::shared(), lending_storage())
        .ok()
        .unwrap();
    (service, first, second, wire, stop)
}

fn parked_with_lending_peer<B: 'static>(prefix: bool) {
    let (mut service, mut lending, mut other, wire, stop) = two_lenders::<B>();
    let clock: VirtualClock = VirtualClock::new();
    let mut events = Drops::default();
    let cx = &mut Context::from_waker(Waker::noop());
    let mut run = Box::pin(service.run(&clock, &mut events));
    assert!(run.as_mut().poll(cx).is_pending());
    let mut first = saved_wait(&mut lending, prefix);
    assert!(first.as_mut().poll(cx).is_pending());
    arrive(&wire, LENDING, b"parked");
    assert!(run.as_mut().poll(cx).is_pending());
    for payload in [b"second".as_slice(), b"third".as_slice()] {
        let mut bytes = Vec::new();
        let mut receiving = Box::pin(other.receive_prefix(8, |cell| {
            bytes.extend_from_slice(cell.as_mut().unwrap().data().unwrap());
            true
        }));
        assert!(receiving.as_mut().poll(cx).is_pending());
        arrive(&wire, COPYING, payload);
        let mut received = false;
        for _ in 0..4 {
            assert!(run.as_mut().poll(cx).is_pending());
            if let Poll::Ready(answer) = receiving.as_mut().poll(cx) {
                assert!(matches!(answer, Ok(crate::ReceivedPrefix::Lent(1))));
                received = true;
                break;
            }
        }
        assert!(received, "a different lending socket has its own progress");
        drop(receiving);
        assert_eq!(bytes, payload);
        assert_eq!(lent_out(&wire), 1);
    }
    stop.try_send(()).ok().unwrap();
    let mut stopped = false;
    for _ in 0..4 {
        if let Poll::Ready(cause) = run.as_mut().poll(cx) {
            assert_eq!(cause, crate::EndCause::Stopped);
            stopped = true;
            break;
        }
    }
    assert!(stopped);
    drop(run);
    drop(first);
    drop(lending);
    drop(other);
    let mut ended = service.retire();
    ended.collect();
    assert_eq!(lent_out(&wire), 0);
    assert!(ended.retired.pool.loans().released());
    assert!(events.0.is_empty());
}

#[test]
fn a_saved_prefix_wait_leaves_a_lending_peer_independent() {
    struct Net;
    parked_with_lending_peer::<Net>(true);
}

#[test]
fn a_saved_ordinary_receive_leaves_a_lending_peer_independent() {
    struct Net;
    parked_with_lending_peer::<Net>(false);
}

#[test]
fn two_saved_receives_defer_once_and_leave_protocol_input_live() {
    use super::super::{PEER, PEER_IP};
    use oer_protocols_network_wire::arp::{self, Arp, Operation};
    use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
    struct Net;
    let (mut service, mut first, mut second, wire, _stop) = two_lenders::<Net>();
    let clock: VirtualClock = VirtualClock::new();
    let mut events = Drops::default();
    let cx = &mut Context::from_waker(Waker::noop());
    let mut run = Box::pin(service.run(&clock, &mut events));
    assert!(run.as_mut().poll(cx).is_pending());
    let mut waiting_first = saved_wait(&mut first, true);
    let mut waiting_second = saved_wait(&mut second, false);
    assert!(waiting_first.as_mut().poll(cx).is_pending());
    assert!(waiting_second.as_mut().poll(cx).is_pending());
    arrive(&wire, LENDING, b"first");
    arrive(&wire, COPYING, b"second");
    assert!(run.as_mut().poll(cx).is_pending());
    assert_eq!(lent_out(&wire), 2);
    let mut request = EthernetHeader {
        destination: LOCAL,
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
    request.extend_from_slice(&packet);
    let before = wire.0.borrow().takes;
    wire.0.borrow_mut().incoming.push_back(request);
    wire.0.borrow_mut().wake();
    assert!(run.as_mut().poll(cx).is_pending());
    assert_eq!(
        wire.0.borrow().takes,
        before,
        "one courtesy pass takes no RX"
    );
    assert!(run.as_mut().poll(cx).is_pending());
    assert!(
        wire.0.borrow().takes > before,
        "both offers were consumed in one deferral"
    );
    assert!(run.as_mut().poll(cx).is_pending());
    assert!(wire.0.borrow().sent.iter().any(|frame| {
        let (header, payload) = EthernetHeader::parse(frame).unwrap();
        header.ether_type == EtherType::ARP
            && Arp::parse(payload)
                .unwrap()
                .is_some_and(|arp| arp.operation == Operation::Reply)
    }));
    assert_eq!(lent_out(&wire), 2, "neither parked owner was expired");
    drop(run);
    drop(waiting_first);
    drop(waiting_second);
    drop(first);
    drop(second);
    let mut ended = service.retire();
    ended.collect();
    assert!(ended.retired.pool.loans().released());
    assert_eq!(lent_out(&wire), 0);
    assert!(events.0.is_empty());
}

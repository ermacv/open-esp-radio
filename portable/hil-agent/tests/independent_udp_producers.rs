//! Exercise the target's real producer with independently controlled sockets.
use multi_tx::Producer;
use oer_hil_agent::traffic::multi_tx;
use oer_hil_protocol::network::*;
use oer_hil_protocol::wifi::WifiNetworkInterface;
use std::{
    cell::Cell,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn config() -> SessionConfig {
    SessionConfig {
        network_interface: WifiNetworkInterface::AccessPoint,
        transport: Transport::Udp,
        direction: Direction::Tx,
        completion: Completion::DurationMillis(12_000),
        link_requirements: SessionLinkRequirements::NONE,
        flows: [0, 1].map(|id| {
            Some(SessionFlowConfig {
                flow_id: id,
                peer: Some(Ipv4Endpoint {
                    address: [10, 43, 0, id + 2],
                    port: 9002 + u16::from(id),
                }),
                target_rx: None,
                target_tx: Some(FlowConfig {
                    payload_bytes: 1472,
                    offered_rate_bps: None,
                    pacing_group_datagrams: None,
                }),
                payload_identity: None,
            })
        }),
    }
}

#[test]
fn blocked_peer_does_not_stop_ready_peer_and_release_preserves_sequences() {
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    let mut producer = Producer::new(config(), 0, 1000, 16, 1);
    let mut sent = [vec![], vec![]];
    let mut pending_waker = None;
    assert!(
        producer
            .poll(
                &mut cx,
                || 0,
                |index, packet, cx| {
                    if index == 0 {
                        pending_waker = Some(cx.waker().clone());
                        return Poll::Pending;
                    }
                    if sent[1].len() == 5 {
                        return Poll::Pending;
                    }
                    assert_eq!(packet.peer.port, 9003);
                    assert_eq!(packet.payload_bytes, 1472);
                    sent[1].push(packet.sequence);
                    Poll::Ready(Ok::<_, ()>(()))
                }
            )
            .is_pending()
    );
    assert_eq!(sent[0], Vec::<u32>::new());
    assert_eq!(sent[1], [0, 1, 2, 3, 4]);
    assert_eq!(
        wakes.0.load(Ordering::Relaxed),
        0,
        "blocked sockets must not trigger a retry loop"
    );
    assert_eq!(producer.deadline(), 1000);
    pending_waker.unwrap().wake();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    let _ = producer.poll(
        &mut cx,
        || 20,
        |index, packet, _| {
            if index == 1 || sent[0].len() == 3 {
                return Poll::Pending;
            }
            sent[0].push(packet.sequence);
            Poll::Ready(Ok::<_, ()>(()))
        },
    );
    assert_eq!(sent[0], [0, 1, 2]);
    assert!(
        producer
            .poll(
                &mut cx,
                || 1000,
                |_, _, _| -> Poll<Result<(), ()>> { panic!("no publication after deadline") }
            )
            .is_ready()
    );
    let evidence = producer.evidence(1000);
    assert_eq!(evidence[0].unwrap().tx_units, 3);
    assert_eq!(evidence[1].unwrap().tx_units, 5);
}

#[test]
fn sparse_deadline_remains_armed_while_other_peer_is_blocked() {
    let mut config = config();
    let flow = config.flows[1]
        .as_mut()
        .unwrap()
        .target_tx
        .as_mut()
        .unwrap();
    flow.offered_rate_bps = Some(235_520);
    flow.pacing_group_datagrams = Some(1);
    let mut producer = Producer::new(config, 0, 120_000, 16, 1);
    let mut cx = Context::from_waker(Waker::noop());
    let mut sent = vec![];
    for time in [0, 50_000, 100_000] {
        assert!(
            producer
                .poll(
                    &mut cx,
                    || time,
                    |index, packet, _| {
                        if index == 0 {
                            Poll::Pending
                        } else {
                            sent.push(packet.sequence);
                            Poll::Ready(Ok::<_, ()>(()))
                        }
                    }
                )
                .is_pending()
        );
        assert_eq!(producer.deadline(), (time + 50_000).min(120_000));
    }
    assert_eq!(sent, [0, 1, 2]);
}

#[test]
fn cooperative_quantum_and_absolute_deadline_bound_an_always_ready_sender() {
    let mut producer = Producer::new(config(), 0, 10, 4, 1);
    let clock = Cell::new(0);
    let mut cx = Context::from_waker(Waker::noop());
    let mut sent = [0, 0];
    assert!(
        producer
            .poll(
                &mut cx,
                || clock.get(),
                |i, _, _| {
                    sent[i] += 1;
                    Poll::Ready(Ok::<_, ()>(()))
                }
            )
            .is_pending()
    );
    assert_eq!(sent, [2, 2]);
    assert!(
        producer
            .poll(
                &mut cx,
                || clock.get(),
                |_, _, _| {
                    clock.set(10);
                    Poll::Ready(Ok::<_, ()>(()))
                }
            )
            .is_ready()
    );
}

#[test]
fn terminal_send_error_stops_only_that_flow_without_error_spin() {
    let mut producer = Producer::new(config(), 0, 100, 16, 1);
    let mut cx = Context::from_waker(Waker::noop());
    let mut calls = [0, 0];
    assert!(
        producer
            .poll(
                &mut cx,
                || 0,
                |i, _, _| {
                    calls[i] += 1;
                    if i == 0 {
                        Poll::Ready(Err(()))
                    } else if calls[i] < 3 {
                        Poll::Ready(Ok(()))
                    } else {
                        Poll::Pending
                    }
                }
            )
            .is_pending()
    );
    assert_eq!(calls, [1, 3]);
    let evidence = producer.evidence(100);
    assert_eq!(evidence[0].unwrap().transport_errors, 1);
    assert_eq!(evidence[1].unwrap().tx_units, 2);
}

#[test]
fn completely_blocked_producer_resumes_on_socket_event_without_retry_timer() {
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    let mut producer = Producer::new(config(), 0, 12_000_000, 16, 1);
    let mut waiting = [None, None];
    let mut polls = [0, 0];
    assert!(
        producer
            .poll(
                &mut cx,
                || 0,
                |index, _, cx| {
                    polls[index] += 1;
                    waiting[index] = Some(cx.waker().clone());
                    Poll::<Result<(), ()>>::Pending
                }
            )
            .is_pending()
    );
    assert_eq!(polls, [1, 1]);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    assert_eq!(
        producer.deadline(),
        12_000_000,
        "only the session watchdog remains"
    );

    waiting[1].take().unwrap().wake();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    let mut sent = vec![];
    assert!(
        producer
            .poll(
                &mut cx,
                || 6_000_000,
                |index, packet, _| {
                    if index == 0 || !sent.is_empty() {
                        Poll::Pending
                    } else {
                        sent.push(packet.sequence);
                        Poll::Ready(Ok::<_, ()>(()))
                    }
                }
            )
            .is_pending()
    );
    assert_eq!(sent, [0], "blocked publication must retain its sequence");
    assert_eq!(
        wakes.0.load(Ordering::Relaxed),
        1,
        "no empty self-repoll after the burst"
    );
    let evidence = producer.evidence(6_000_000);
    assert_eq!(evidence[0].unwrap().tx_units, 0);
    assert_eq!(evidence[1].unwrap().tx_units, 1);
}

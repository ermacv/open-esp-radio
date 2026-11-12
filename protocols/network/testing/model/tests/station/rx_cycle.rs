//! The cycle of received buffers through the whole composition: the model
//! port lends a bounded pool of buffers, larger than a turn's intake, the
//! lending socket holds a finite credit, and the peer sends bursts larger
//! than a turn's budget while the application's socket or the network's
//! own producer sends too, the consumer releasing at once or after a hold. Each physical buffer goes back to the
//! port once its datagram is released, the credit comes back with it, and
//! nothing is refused for pressure while the application keeps up: a
//! datagram past the credit is dropped by the network (`NoCredit`), which
//! returns its buffer. Each case checks that the turns ran as packet turns
//! past full intakes, then the physical buffers, the credit and the stream
//! separately. The model's executor costs nothing and its air hands frames
//! over one by one, so these cases check progress and ownership, not the
//! hardware's timing or a driver ring that fills.

use std::collections::VecDeque;

use oer_portable_time::Duration;
use oer_protocols_network_core::{Outcome, Remote};
use oer_protocols_network_runtime::{Received, Sent};

use crate::simulation::wired::PEER_IP;
use crate::simulation::{PORT, Setup, data_of, simulate};

/// The peer's port.
const PEER_PORT: u16 = 7000;
/// Datagrams in a burst: twice a turn's budget of frames.
const BURST: u16 = 16;
/// Bursts.
const BURSTS: u16 = 6;

fn datagram(n: u16) -> Vec<u8> {
    let mut payload = vec![(n % 251) as u8; 400];
    payload[..2].copy_from_slice(&n.to_be_bytes());
    payload
}

/// Bursts past the turn's budget, each datagram taken and released at
/// once while the application's own datagrams go out: every received
/// datagram is lent to the socket or dropped for credit, none is refused
/// for pressure, every sent one ends accepted, and every loan and block
/// comes home at the end. The socket's sends keep each turn whole.
#[test]
fn bursts_past_a_turn_s_budget_keep_the_lend_cycle_whole_while_the_socket_sends() {
    struct Net;
    let setup = Setup {
        lending: true,
        rx_buffers: Some(RX_BUFFERS),
        ..Setup::default()
    };
    let report = simulate::<Net>(setup, |world| {
        Box::pin(async move {
            world.join().await;
            let peer = Remote {
                address: PEER_IP,
                port: PEER_PORT,
            };
            let mut open = VecDeque::new();
            let mut next = 0_u16;
            let mut taken = 0_usize;
            for burst in 0..BURSTS {
                // The peer's burst goes out at once; one datagram of ours
                // for each two taken.
                for n in burst * BURST..(burst + 1) * BURST {
                    world.ap().wired.send_udp(PEER_PORT, PORT, &datagram(n));
                }
                // Take what comes, releasing each at once, until the burst
                // is either taken or dropped for credit.
                let goal = usize::from((burst + 1) * BURST);
                while taken + world.counts().no_credit + world.counts().rx_pressure < goal {
                    let mut received = world.receive().await;
                    assert!(matches!(received, Received::Lent(_)), "lent");
                    let _data = data_of(&mut received);
                    world.release(received);
                    taken += 1;
                    if taken.is_multiple_of(2) {
                        open.push_back(world.send(peer, &datagram(1000 + next)).await);
                        next += 1;
                    }
                }
                while open.len() > 4 {
                    let sent = world.result(open.pop_front().expect("a result")).await;
                    assert_eq!(sent, Sent::Ended(Outcome::Accepted));
                }
            }
            let counts = world.counts();
            assert!(counts.intake_full > 0, "a burst filled a turn's intake");
            // A socket's sends are commands, control work: no turn takes
            // the frames alone. The network's own producer is what does.
            assert_eq!(counts.packet_turns, 0, "{counts:?}");
            assert_eq!(counts.rx_frames_dropped, 0);
            assert_eq!(counts.rx_pressure, 0, "nothing refused for pressure");
            assert_eq!(taken + counts.no_credit, usize::from(BURSTS * BURST));
            while let Some(observer) = open.pop_front() {
                assert_eq!(world.result(observer).await, Sent::Ended(Outcome::Accepted));
            }
            world
                .until("the peer has ours", Duration::from_secs(1), |world| {
                    world.ap().wired.udp.len() == usize::from(next)
                })
                .await;
        })
    });
    report.assert_stopped_with_every_owner();
}

/// Bursts past the turn's budget and nothing sent: received frames are the
/// only work, so the turns after a full intake take only the received
/// frames, each after a wait that read every source. The
/// cycle stays whole: each datagram is taken or dropped for credit, none is
/// refused for pressure, and every loan comes home.
#[test]
fn bursts_past_a_turn_s_budget_alone_keep_the_lend_cycle_whole() {
    struct Net;
    let setup = Setup {
        lending: true,
        rx_buffers: Some(RX_BUFFERS),
        ..Setup::default()
    };
    let report = simulate::<Net>(setup, |world| {
        Box::pin(async move {
            world.join().await;
            let mut taken = 0_usize;
            for burst in 0..BURSTS {
                for n in burst * BURST..(burst + 1) * BURST {
                    world.ap().wired.send_udp(PEER_PORT, PORT, &datagram(n));
                }
                let goal = usize::from((burst + 1) * BURST);
                while taken + world.counts().no_credit + world.counts().rx_pressure < goal {
                    let mut received = world.receive().await;
                    assert!(matches!(received, Received::Lent(_)), "lent");
                    let _data = data_of(&mut received);
                    world.release(received);
                    taken += 1;
                }
            }
            let counts = world.counts();
            assert!(counts.intake_full > 0, "a burst filled a turn's intake");
            assert!(counts.receive_turns > 0, "receive turns ran: {counts:?}");
            assert_eq!(counts.rx_frames_dropped, 0);
            assert_eq!(counts.rx_pressure, 0, "nothing refused for pressure");
            assert_eq!(taken + counts.no_credit, usize::from(BURSTS * BURST));
        })
    });
    report.assert_stopped_with_every_owner();
}

/// The stream's own port and the peer's port it sends to.
const STREAM_PORT: u16 = 6000;
const STREAM_PEER_PORT: u16 = 7001;
/// The stream's datagrams.
const STREAM: usize = 240;

/// One run of the stream beside the peer's bursts, the consumer holding each
/// datagram for `hold`; what the case reads back.
struct Cycle {
    taken: usize,
    counts: crate::simulation::composition::Counts,
    stream: crate::simulation::stream::StreamCounts,
    /// The RX buffers out at the end, and the most ever out at once.
    rx_out: (usize, usize),
    /// Datagrams of the last burst, sent once all before were released: all
    /// lent while the restored credit lasts.
    after: usize,
}

fn cycle<Net: 'static>(hold: Duration, rx_buffers: usize) -> Cycle {
    let stream = crate::simulation::stream::Stream::new(
        Remote {
            address: PEER_IP,
            port: STREAM_PEER_PORT,
        },
        400,
        STREAM,
    );
    let setup = Setup {
        lending: true,
        stream: Some((STREAM_PORT, stream.clone())),
        rx_buffers: Some(rx_buffers),
        ..Setup::default()
    };
    let out = std::rc::Rc::new(std::cell::RefCell::new(None));
    let kept = out.clone();
    let report = simulate::<Net>(setup, move |world| {
        let stream = stream.clone();
        let kept = kept.clone();
        Box::pin(async move {
            world.join().await;
            let mut taken = 0_usize;
            for burst in 0..BURSTS {
                for n in burst * BURST..(burst + 1) * BURST {
                    world.ap().wired.send_udp(PEER_PORT, PORT, &datagram(n));
                }
                let goal = usize::from((burst + 1) * BURST);
                while taken + world.counts().no_credit + world.counts().rx_pressure < goal {
                    let mut received = world.receive().await;
                    assert!(matches!(received, Received::Lent(_)), "lent");
                    let _data = data_of(&mut received);
                    // The application holds the datagram, its buffer and
                    // its credit with it, for `hold`.
                    world.sleep(hold).await;
                    world.release(received);
                    taken += 1;
                }
            }
            // Every buffer back home: the physical count is the proof, not
            // the credit.
            world
                .until("every RX buffer is home", Duration::from_secs(1), |world| {
                    world.air.rx_out().0 == 0
                })
                .await;
            // The credit is whole again: a burst of the credit's size is
            // lent whole, with no drop for credit.
            let dropped = world.counts().no_credit;
            let mut after = 0;
            for n in 0..crate::simulation::LEND_CREDIT as u16 {
                world
                    .ap()
                    .wired
                    .send_udp(PEER_PORT, PORT, &datagram(5000 + n));
            }
            while after < crate::simulation::LEND_CREDIT {
                let received = world.receive().await;
                assert!(matches!(received, Received::Lent(_)), "lent");
                world.release(received);
                after += 1;
            }
            assert_eq!(world.counts().no_credit, dropped, "the credit came back");
            world
                .until("the stream is sent", Duration::from_secs(2), |_| {
                    stream.counts().accepted + stream.counts().other == STREAM
                })
                .await;
            world
                .until(
                    "the peer heard the stream",
                    Duration::from_secs(1),
                    |world| {
                        world
                            .ap()
                            .wired
                            .udp
                            .iter()
                            .filter(|udp| udp.port == STREAM_PEER_PORT)
                            .count()
                            == STREAM
                    },
                )
                .await;
            *kept.borrow_mut() = Some(Cycle {
                taken,
                counts: world.counts(),
                stream: stream.counts(),
                rx_out: world.air.rx_out(),
                after,
            });
        })
    });
    report.assert_stopped_with_every_owner();
    out.borrow_mut().take().expect("the case ran")
}

/// The station's receive buffers in these cases: more than a turn takes in,
/// so a burst reaches the intake's budget. The model's own four never do.
const RX_BUFFERS: usize = 16;

/// The cycle's checks, apart from what the hold changes: the turns ran as
/// packet turns past full intakes, the stream went whole, no frame was lost
/// or refused, each physical buffer came home and the credit with it.
fn assert_cycle(cycle: &Cycle) {
    let counts = cycle.counts;
    assert!(
        counts.intake_full > 0,
        "a burst filled a turn's intake: {counts:?}"
    );
    assert!(counts.packet_turns > 0, "packet turns ran: {counts:?}");
    assert!(
        counts.turns > counts.packet_turns,
        "control took full turns"
    );
    let stream = cycle.stream;
    assert_eq!(
        (stream.offered, stream.filled, stream.accepted, stream.other),
        (STREAM, STREAM, STREAM, 0),
        "each stream datagram was filled once and accepted"
    );
    assert_eq!(counts.rx_frames_dropped, 0);
    assert_eq!(counts.rx_queue_full, 0);
    assert_eq!(counts.rx_pressure, 0, "nothing refused for pressure");
    assert_eq!(cycle.taken + counts.no_credit, usize::from(BURSTS * BURST));
    assert_eq!(cycle.rx_out.0, 0, "every RX buffer is home");
    assert!(cycle.rx_out.1 <= RX_BUFFERS, "the pool bounds what is out");
    assert_eq!(cycle.after, crate::simulation::LEND_CREDIT);
}

/// A consumer that releases at once: the stream and the bursts share the
/// turns, and the cycle stays whole.
#[test]
fn a_stream_beside_bursts_past_the_intake_budget_keeps_both_cycles_whole() {
    struct Net;
    let cycle = cycle::<Net>(Duration::ZERO, RX_BUFFERS);
    assert_cycle(&cycle);
}

/// A consumer that holds each datagram: what arrives while the credit is
/// held is dropped for credit and its buffer goes home, so each burst is
/// lent only the credit, and the stream goes on undisturbed.
#[test]
fn a_slow_consumer_is_lent_only_its_credit_and_the_stream_goes_on() {
    struct Net;
    let cycle = cycle::<Net>(Duration::from_millis(5), RX_BUFFERS);
    assert_cycle(&cycle);
    assert_eq!(
        cycle.taken,
        usize::from(BURSTS) * crate::simulation::LEND_CREDIT,
        "each burst lends the credit alone"
    );
}

//! The cycle of received buffers through the whole composition: the model
//! port lends a bounded number of buffers, the lending socket holds a
//! finite credit, and the peer sends bursts larger than a turn's budget
//! while the application sends too. Each physical buffer goes back to the
//! port once its datagram is released, the credit comes back with it, and
//! nothing is refused for pressure while the application keeps up: a
//! datagram past the credit is dropped by the network (`NoCredit`), which
//! returns its buffer. The model's executor costs nothing, so these cases
//! check progress and ownership, not the hardware's timing.

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
/// comes home at the end.
#[test]
fn bursts_past_a_turn_s_budget_keep_the_lend_cycle_whole_while_the_socket_sends() {
    struct Net;
    let setup = Setup {
        lending: true,
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
                    if taken % 2 == 0 {
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
/// only work, so the turns after a full intake take only the frames. The
/// cycle stays whole: each datagram is taken or dropped for credit, none is
/// refused for pressure, and every loan comes home.
#[test]
fn bursts_past_a_turn_s_budget_alone_keep_the_lend_cycle_whole() {
    struct Net;
    let setup = Setup {
        lending: true,
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
            assert_eq!(counts.rx_frames_dropped, 0);
            assert_eq!(counts.rx_pressure, 0, "nothing refused for pressure");
            assert_eq!(taken + counts.no_credit, usize::from(BURSTS * BURST));
        })
    });
    report.assert_stopped_with_every_owner();
}

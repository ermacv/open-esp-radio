//! The access point's stop of the network, through the station's network
//! end: every network memory the runner holds goes home first, then the
//! network is asked to stop and its report settles the stop.

use super::*;
use oer_protocols_ieee80211_core_roles_sta::data::{EthernetFrame, MsduParts, WmmUserPriority};
use oer_protocols_network_port::{NetDevice, OwnedTransmit};

fn frame() -> EthernetFrame {
    let mut frame = EthernetFrame::empty();
    frame
        .set_parts(MsduParts {
            destination: STATION,
            source: ACCESS_POINT,
            ether_type: 0x0800,
            payload: &[0x45; 28],
        })
        .unwrap();
    frame
}

fn received() -> MsduParts<'static> {
    MsduParts {
        destination: ACCESS_POINT,
        source: STATION,
        ether_type: 0x0800,
        payload: &[0x46; 40],
    }
}

/// Run `runner` until it serves, then stop it and let the stop end.
macro_rules! serve_and_stop {
    ($runner:ident, $counted:ident, $air:ident, $shared:ident) => {{
        let script = async {
            $counted.idle(64).await;
            assert!($shared.status().running, "the access point serves");
            stop_and_finish(&$counted, &$air, &$shared).await;
        };
        drive($runner.run(&$shared, &$counted), script).await
    }};
}

/// Received copies the network has not read when the stop comes: the stop
/// does not wait for them, the network is asked once and reports, and the
/// copies stay the network's to read, each whole.
#[test]
fn a_stop_while_the_network_holds_unread_copies_ends_confirmed_and_leaves_them_the_network_s() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, device, transmit, supply, side, shared);
        assert!(runner.homes.deliver(received()));
        assert!(runner.homes.deliver(received()));
        let end = serve_and_stop!(runner, counted, air, shared);
        stopped(end);
        assert_eq!(side.asked.get(), 1, "asked once");
        let mut device = device;
        for _ in 0..2 {
            let Some(crate::station::Received::Copied(copy)) = device.try_receive() else {
                panic!("a copy the network holds");
            };
            let read = device.read_copied(copy, |view| (view.source, view.payload.len()));
            assert_eq!(read, Ok((STATION, 40)), "the copy, whole");
        }
        assert!(device.try_receive().is_none());
        transmit.reclaim();
        assert_eq!(supply.free(), super::super::AP_TX_DEPTH);
    }));
}

/// A network whose participant is lost and never reports, with frames in
/// the runner's queues and its transport at the stop: the outcome is
/// `NetworkLost` once the runner's share is home, and no memory of the
/// runner's is left out.
#[test]
fn a_stop_the_network_never_answers_ends_network_lost_with_every_memory_home() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, transmit, supply, side, shared);
        side.answer.set(Some(Err(crate::station::NetworkStopLost)));
        runner.homes.set_link(true);
        // Frames the runner holds in its queues, and more in the transport.
        let queue =
            crate::datapath::queues::QueueId::new(GROUP_PEER, WmmUserPriority::UP0).unwrap();
        for _ in 0..3 {
            assert!(transmit.try_transmit(frame()).is_ok());
        }
        transmit.flush();
        while let Some(outgoing) = runner.homes.take_network() {
            let msdu = OutboundMsdu {
                memory: outgoing.memory,
                length: usize::from(outgoing.length),
                priority: WmmUserPriority::UP0,
            };
            assert!(runner.homes.queues.push(queue, msdu).is_ok());
        }
        for _ in 0..2 {
            assert!(transmit.try_transmit(frame()).is_ok());
        }
        transmit.flush();
        runner.homes.set_link(false);
        assert_eq!(supply.free(), super::super::AP_TX_DEPTH - 5);
        let end = serve_and_stop!(runner, counted, air, shared);
        match end {
            ApEnd::NetworkLost(observed) => assert_eq!(observed.memory_held, 0, "{observed:?}"),
            other => panic!("the network is lost: {other:?}"),
        }
        assert!(runner.homes.queues.is_empty());
        transmit.reclaim();
        assert_eq!(
            supply.free(),
            super::super::AP_TX_DEPTH,
            "every memory came home"
        );
    }));
}

/// A frame the network published before it saw the stop's request: it
/// waits in the transport queue after the network reported its stop. The
/// runner takes it at the report and hands it to the return queue before it
/// confirms the stop, where the network's transmit owner takes it back.
#[test]
fn a_memory_returned_after_the_networks_report_reaches_the_networks_owner() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, transmit, supply, side, shared);
        side.answer.set(None);
        let link = runner.homes.link;
        let late = core::cell::RefCell::new(&mut transmit);
        let script = async {
            counted.idle(64).await;
            assert!(shared.status().running, "the access point serves");
            shared.request_stop();
            for _ in 0..2048 {
                if side.asked.get() == 1 && side.answer.get().is_none() {
                    // Its last turn published one more frame before it saw
                    // the request, then it reports.
                    // The link it read then was still up.
                    let down = link.snapshot();
                    link.publish(oer_protocols_network_port::LinkSnapshot {
                        state: oer_protocols_network_port::LinkState::Up,
                        epoch: down.epoch,
                    });
                    let mut transmit = late.borrow_mut();
                    assert!(transmit.try_transmit(frame()).is_ok());
                    transmit.flush();
                    link.publish(down);
                    side.answer.set(Some(Ok(())));
                }
                counted.advance(&air, Duration::from_millis(1)).await;
            }
        };
        stopped(drive(runner.run(&shared, &counted), script).await);
        assert_eq!(side.asked.get(), 1);
        transmit.reclaim();
        assert_eq!(
            supply.free(),
            super::super::AP_TX_DEPTH,
            "the late frame's memory came home"
        );
    }));
}

//! The homes refuse a terminal whole: a terminal whose owners have no home
//! of the runner's leaves nothing in them and stays with the port.

use super::*;
use core::future::poll_fn;
use oer_protocols_ieee80211_core_roles_ap::engine::{ApFrame, ApFrameKind};

/// A deauthentication to the station, written into `bytes`: its length.
fn deauthentication(bytes: &mut [u8]) -> usize {
    let frame: [u8; 26] = [
        0xc0, 0x00, 0x00, 0x00, // frame control, duration
        0x02, 0x00, 0x00, 0x00, 0x00, 0x01, // receiver: the station
        0x02, 0x00, 0x00, 0x00, 0x00, 0x02, // transmitter: the access point
        0x02, 0x00, 0x00, 0x00, 0x00, 0x02, // BSSID
        0x00, 0x00, // sequence control
        0x03, 0x00, // reason: leaving
    ];
    bytes[..frame.len()].copy_from_slice(&frame);
    frame.len()
}

/// A single MPDU's terminal whose ticket carries no purpose of the
/// runner's is refused whole: no ticket, buffer or purpose goes home, the
/// break is recorded, and the port keeps it, through its stop, until the
/// homes take it whole.
#[test]
fn a_terminal_without_a_home_of_the_runners_is_refused_whole_and_stays_with_the_port() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, _shared);
        let now = || oer_portable_time::Clock::now(&*counted.clock);
        let started = poll_fn(|cx| runner.port.poll_start(cx, supported(), now())).await;
        assert!(started.is_ok(), "the radio starts");
        // A ticket and a buffer taken out by hand: the runner gave the
        // ticket no purpose.
        let ticket = runner.homes.tickets.pop().unwrap();
        let index = ticket.index();
        let mut buffer = runner.homes.buffers.pop().unwrap();
        let len = deauthentication(buffer.bytes_mut().unwrap());
        let frame = ApFrame {
            len,
            kind: ApFrameKind::Management { peer: STATION },
        };
        let Ok(admitted) = runner.admit(ticket, buffer, frame, TxOperation::Data, None) else {
            panic!("the frame is admitted");
        };
        assert!(
            runner
                .port
                .submit(admitted, now(), &mut runner.homes)
                .is_ok()
        );
        let (tickets, buffers) = (runner.homes.tickets.len(), runner.homes.buffers.len());
        for _ in 0..64 {
            counted.clock.advance(Duration::from_millis(1)).unwrap();
            air.clock_advanced();
            let served = runner
                .port
                .service_tx(TxFacts::DUE, false, now(), &mut runner.homes);
            assert!(served.is_ok());
        }
        // Refused whole: nothing of it went home.
        assert_eq!(
            runner.homes.broken,
            Some(ApContract::UnknownTicket(index))
        );
        assert_eq!(runner.homes.tickets.len(), tickets);
        assert_eq!(runner.homes.buffers.len(), buffers);
        assert!(runner.homes.kept.is_empty(), "nothing is kept in the runner");
        // The port keeps it through its stop; offered after it, it is
        // refused again and stays.
        runner.homes.broken = None;
        let stop = poll_fn(|cx| {
            let at = now();
            let _served = runner
                .port
                .service_tx(TxFacts::DUE, false, at, &mut runner.homes);
            match runner.port.poll_stop(cx, at) {
                core::task::Poll::Pending => {
                    cx.waker().wake_by_ref();
                    core::task::Poll::Pending
                }
                ready => ready,
            }
        })
        .await;
        assert!(stop.is_ok(), "the stop confirms");
        assert_eq!(
            runner.port.take_unconsumed(&mut runner.homes),
            Ok(Unconsumed::Kept)
        );
        assert_eq!(runner.homes.tickets.len(), tickets);
        // Once the ticket has its purpose, the homes take it whole, once.
        runner.homes.purposes[usize::from(index)] = Some(Purpose::Data);
        assert_eq!(
            runner.port.take_unconsumed(&mut runner.homes),
            Ok(Unconsumed::None)
        );
        assert_eq!(runner.homes.tickets.len(), tickets + 1);
        assert_eq!(runner.homes.buffers.len(), buffers + 1);
        assert!(runner.homes.purposes[usize::from(index)].is_none());
        assert_eq!(
            runner.port.take_unconsumed(&mut runner.homes),
            Ok(Unconsumed::None)
        );
        assert_eq!(runner.homes.tickets.len(), tickets + 1);
    }));
}

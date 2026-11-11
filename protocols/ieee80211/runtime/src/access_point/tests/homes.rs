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
        assert_eq!(runner.homes.broken, Some(ApContract::UnknownTicket(index)));
        assert_eq!(runner.homes.tickets.len(), tickets);
        assert_eq!(runner.homes.buffers.len(), buffers);
        assert!(runner.homes.kept.is_none(), "nothing is kept in the runner");
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

/// Owners an operation stopped on stay out whole and alone: while the
/// homes hold a terminal's taken-apart owners, every later terminal is
/// refused whole and stays with the port, and no run starts while any are
/// out; each set leaves once through `take_kept`, then the port's terminal
/// comes home.
#[test]
fn owners_an_operation_stopped_on_hold_every_later_terminal_in_the_port_and_leave_once() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let now = || oer_portable_time::Clock::now(&*counted.clock);
        // A run ended on owners its step stopped on: a spare pairwise slot
        // stands for them. No new run starts while they are out.
        let slot = runner.homes.pairwise.pop().unwrap();
        runner.homes.stopped_on = Some((ApContract::ForeignKey, Kept::PairwiseSlot(slot)));
        let rerun = drive(runner.run(&shared, &counted), async {
            counted.idle(8).await;
        })
        .await;
        assert!(matches!(rerun, ApEnd::Contract(ApContract::ForeignKey)));
        let started = poll_fn(|cx| runner.port.poll_start(cx, supported(), now())).await;
        assert!(started.is_ok(), "the radio starts");
        // A frame of the runner's, with its purpose: its terminal would come
        // home, but a terminal's owners are out without a home.
        let Some(Kept::PairwiseSlot(spare)) = runner.take_kept() else {
            panic!("the slot comes back as it went");
        };
        assert!(runner.take_kept().is_none(), "once");
        runner.homes.kept = Some((ApContract::ForeignKey, Kept::PairwiseSlot(spare)));
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
        runner.homes.purposes[usize::from(index)] = Some(Purpose::Data);
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
        // Refused whole: nothing of it went home, and no break is new.
        assert_eq!(runner.homes.tickets.len(), tickets);
        assert_eq!(runner.homes.buffers.len(), buffers);
        assert!(runner.homes.purposes[usize::from(index)].is_some());
        let rerun = drive(runner.run(&shared, &counted), async {
            counted.idle(8).await;
        })
        .await;
        assert!(
            matches!(rerun, ApEnd::Contract(ApContract::ForeignKey)),
            "no run"
        );
        // The set leaves once; the port's terminal then comes home, once.
        let Some(Kept::PairwiseSlot(slot)) = runner.take_kept() else {
            panic!("the terminal's owners come back whole");
        };
        assert!(runner.take_kept().is_none(), "once");
        assert!(runner.homes.pairwise.push(slot).is_ok());
        let served = runner
            .port
            .service_tx(TxFacts::DUE, false, now(), &mut runner.homes);
        assert!(served.is_ok());
        assert_eq!(runner.homes.tickets.len(), tickets + 1);
        assert_eq!(runner.homes.buffers.len(), buffers + 1);
        assert!(runner.homes.purposes[usize::from(index)].is_none());
    }));
}

/// A terminal the homes refuse that the port cannot keep comes back beside
/// the service's answer: the turn ends on it in the homes' one place, no
/// run starts while it is there, and it leaves once.
#[test]
fn an_item_the_port_cannot_keep_ends_the_turn_and_leaves_the_runner_once() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let now = || oer_portable_time::Clock::now(&*counted.clock);
        let started = poll_fn(|cx| runner.port.poll_start(cx, supported(), now())).await;
        assert!(started.is_ok(), "the radio starts");
        // A frame whose ticket has no purpose: its terminal finds no home.
        let ticket = runner.homes.tickets.pop().unwrap();
        let mut buffer = runner.homes.buffers.pop().unwrap();
        let len = deauthentication(buffer.bytes_mut().unwrap());
        let frame = ApFrame {
            len,
            kind: ApFrameKind::Management { peer: STATION },
        };
        let Ok(admitted) = runner.admit(ticket, buffer, frame, TxOperation::Data, None) else {
            panic!("the frame is admitted");
        };
        assert!(matches!(
            runner.port.submit(admitted, now(), &mut runner.homes),
            Ok(Ok(()))
        ));
        air.unkeep_next(oer_protocols_ieee80211_testing_model::ModelUnkeep::AtDrain);
        let mut halted = false;
        for _ in 0..64 {
            counted.clock.advance(Duration::from_millis(1)).unwrap();
            air.clock_advanced();
            let facts = Facts {
                tx: Some(TxFacts::DUE),
                ..Facts::default()
            };
            if let Err(halt) = runner.turn(now(), facts) {
                assert!(matches!(halt, Halt::Unkept), "the turn ends on the item");
                halted = true;
                break;
            }
        }
        assert!(halted, "the terminal came back unkept");
        // No run starts while it is here.
        assert!(matches!(
            runner.run(&shared, &*counted.clock).await,
            ApEnd::Unkept
        ));
        assert!(matches!(
            runner.take_unkept(),
            Some(oer_protocols_ieee80211_contracts_port::Unkept::Terminal(
                LowerMacTerminal::TxCompleted(TxCompleted::Single(_))
            ))
        ));
        assert!(runner.take_unkept().is_none(), "once");
    }));
}

/// A run that ended on an item the port could not keep: its stop's watch
/// returns at once while the item is held, making no port call, and goes
/// on to the port's confirmation once its owner took the item; the port's
/// fault is asked back once.
#[test]
fn a_stop_watch_returns_while_an_item_is_unkept_and_goes_on_once_it_is_taken() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let now = || oer_portable_time::Clock::now(&*counted.clock);
        let started = poll_fn(|cx| runner.port.poll_start(cx, supported(), now())).await;
        assert!(started.is_ok(), "the radio starts");
        let ticket = runner.homes.tickets.pop().unwrap();
        let mut buffer = runner.homes.buffers.pop().unwrap();
        let len = deauthentication(buffer.bytes_mut().unwrap());
        let frame = ApFrame {
            len,
            kind: ApFrameKind::Management { peer: STATION },
        };
        let Ok(admitted) = runner.admit(ticket, buffer, frame, TxOperation::Data, None) else {
            panic!("the frame is admitted");
        };
        assert!(matches!(
            runner.port.submit(admitted, now(), &mut runner.homes),
            Ok(Ok(()))
        ));
        air.unkeep_next(oer_protocols_ieee80211_testing_model::ModelUnkeep::AtDrain);
        let mut halted = false;
        for _ in 0..64 {
            counted.clock.advance(Duration::from_millis(1)).unwrap();
            air.clock_advanced();
            let facts = Facts {
                tx: Some(TxFacts::DUE),
                ..Facts::default()
            };
            if runner.turn(now(), facts).is_err() {
                halted = true;
                break;
            }
        }
        assert!(halted, "the terminal came back unkept");
        assert!(matches!(
            runner.run(&shared, &*counted.clock).await,
            ApEnd::Unkept
        ));
        // While the item is held the watch returns at once.
        let held = async {
            for _ in 0..64 {
                counted.advance(&air, Duration::from_millis(1)).await;
            }
            core::future::pending::<()>().await;
        };
        match select(Woken::new(runner.finish_stop(&counted, |_| {})), held).await {
            Either::First(watched) => assert_eq!(watched, Ok(StopWatch::Unkept)),
            Either::Second(()) => panic!("the watch waits on nothing while it holds the item"),
        }
        let Some(item) = runner.take_unkept() else {
            panic!("the item stays with the runner");
        };
        // Taken, the watch goes on to the port's confirmation.
        let script = async {
            for _ in 0..256 {
                counted.advance(&air, Duration::from_millis(1)).await;
            }
            core::future::pending::<()>().await;
        };
        match select(Woken::new(runner.finish_stop(&counted, |_| {})), script).await {
            Either::First(watched) => assert_eq!(watched, Ok(StopWatch::Confirmed)),
            Either::Second(()) => panic!("the port confirms the stop once the item is out"),
        }
        drop(item);
    }));
}

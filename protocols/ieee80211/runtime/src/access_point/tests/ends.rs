//! The runner's ends over the model's port: an unconfirmed stop that the
//! port confirms later, and owners that come back with the run's end.

use super::*;

/// The end's kind, for a failing assertion.
fn kind<F: core::fmt::Debug>(end: &ApEnd<F>) -> std::string::String {
    std::format!("{end:?}")
}

#[test]
fn an_unconfirmed_stop_that_the_port_confirms_later_returns_every_owner() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let script = async {
            counted.idle(64).await;
            assert!(shared.status().running);
            // The MAC does not go quiet for the stop.
            air.hold_stop(true);
            shared.request_stop();
            for _ in 0..256 {
                counted.advance(&air, Duration::from_millis(1)).await;
            }
        };
        let end = drive(runner.run(&shared, &counted), script).await;
        let ApEnd::StopUnconfirmed(first) = end else {
            panic!(
                "a MAC that does not go quiet leaves the stop unconfirmed: {}",
                kind(&end)
            );
        };
        assert!(!shared.status().running);
        let observed = Cell::new(0_u32);
        let script = async {
            // The stop stays unconfirmed while the MAC is not quiet: the
            // watch sleeps, and the stop is not asked for again.
            let turns = counted.idle(256).await;
            assert!(turns <= IDLE_TURNS, "{turns} turns while the stop is held");
            for _ in 0..32 {
                counted.advance(&air, Duration::from_millis(1)).await;
            }
            air.hold_stop(false);
            for _ in 0..256 {
                counted.advance(&air, Duration::from_millis(1)).await;
            }
        };
        let finish = runner.finish_stop(&counted, |unconfirmed| {
            assert_eq!(unconfirmed.cause, first.cause);
            observed.set(observed.get() + 1);
        });
        match select(Woken::new(finish), script).await {
            Either::First(()) => {}
            Either::Second(()) => panic!("the port confirms the stop once the MAC is quiet"),
        }
        // Every owner is home: tickets, buffers, the beacon permission, the
        // configure permission, the group key's slot and permission.
        assert_eq!(
            runner.homes.tickets.len(),
            oer_protocols_ieee80211_testing_model::MODEL_TICKETS
        );
        assert_eq!(
            runner.homes.buffers.len(),
            oer_protocols_ieee80211_testing_model::MODEL_LARGE_BUFFERS
        );
        assert!(runner.homes.beacon.is_some());
        assert!(runner.homes.configure.is_some());
        assert!(runner.homes.group_slot.is_some() && runner.homes.group_permission.is_some());
        assert!(runner.homes.association.is_some());
        assert!(runner.take_kept().is_none() && runner.take_returned().is_none());
        // The port answers its stop confirmed from now on.
        let stopped = core::future::poll_fn(|cx| {
            let now = oer_portable_time::Clock::now(&*counted.clock);
            runner.port_mut().poll_stop(cx, now)
        });
        assert!(embassy_futures::block_on(stopped).is_ok());
    }));
}

#[test]
fn the_terminals_of_one_call_each_bring_their_owners_home() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, shared);
        let script = async {
            counted.idle(64).await;
            // Probe responses wait in their held queue, each with a ticket
            // and a buffer of the runner's.
            air.hold(WmmAccessCategory::Voice, true);
            for _ in 0..4 {
                air.receive(&probe_request(), ModelRxMeta::new(channel(), -40, -90));
                counted.idle(16).await;
            }
            // The stop withdraws them in one of the port's calls: each
            // terminal brings its owners home during that call.
            stop_and_finish(&counted, &air, &shared).await;
        };
        stopped(drive(runner.run(&shared, &counted), script).await);
        assert_eq!(
            runner.homes.tickets.len(),
            oer_protocols_ieee80211_testing_model::MODEL_TICKETS
        );
        assert_eq!(
            runner.homes.buffers.len(),
            oer_protocols_ieee80211_testing_model::MODEL_LARGE_BUFFERS
        );
        assert!(runner.homes.purposes.iter().all(Option::is_none));
        assert!(runner.take_kept().is_none());
        assert!(runner.homes.held.is_none() && runner.homes.returned.is_none());
    }));
}

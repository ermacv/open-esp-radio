use super::*;
use oer_protocols_ieee80211_core_arbiter::{ArbiterEnd, ArbiterNotice, RetiredArbiter};

#[test]
fn a_finished_station_restarts_in_place_reclaims_old_keys_then_connects() {
    installed(|mint| {
        let (mut world, _) = wpa2_connected::<2>(mint);
        let nonce_calls = world.station.config.callbacks.nonce_calls;
        assert!(nonce_calls > 0);
        let previous_epoch = world.station.epoch;
        let previous_lease = world.station.lease.id();
        // The service takes the link back before the station finishes.
        world.return_link();
        let mut arbiter = world.arbiter.finish(ArbiterEnd::Stopped);
        let mut places = world.station.finish(StaDisconnectReason::Requested);
        while let Some(notice) = arbiter.poll_notice() {
            if let ArbiterNotice::Station(notice) = notice {
                places.returned(notice);
            }
        }
        while world.station.poll_output().is_some() {}
        let RetiredArbiter::Stopped(stopped) = arbiter else {
            panic!()
        };
        let (arbiter, places) = stopped
            .restart(channel(6), places)
            .ok()
            .unwrap()
            .into_parts();
        let (deferred, notice) = world.station.restart(places);
        assert_eq!(world.station.config.callbacks.nonce_calls, nonce_calls);
        assert!(matches!(deferred, DeferredEapol::Empty));
        assert!(notice.is_none());
        world.arbiter = arbiter;
        world.take();
        world.switches.on(Switch::HoldGroupRemovals);
        world.run(at(50));
        assert!(world.station.is_reclaiming());
        assert!(world.link.is_none());
        assert_eq!(world.pending_removals.len(), 1);
        assert!(matches!(
            world.command(at(51), 999, StaCommand::Connect),
            Err(ObservedCommandRefused {
                reason: CommandRefusal::Transitioning,
                ..
            })
        ));
        let input = world.pending_removals.pop().unwrap();
        world.input(at(52), input);
        world.switches.off(Switch::HoldGroupRemovals);
        world.run(at(52));
        assert!(!world.station.is_reclaiming());
        assert!(world.station.keys.clear());
        assert_eq!(
            world.take(),
            [Seen::RemoveKey, Seen::RemoveKey, Seen::Configure("off")]
        );
        world.command(at(53), 1000, StaCommand::Connect).unwrap();
        world.run(at(53));
        assert_ne!(world.station.epoch, previous_epoch);
        assert_ne!(world.station.lease.id(), previous_lease);
        world.advance();
        world.receive(at(66), &wpa2_beacon());
        world.advance();
        world.answer_authentication(at(83), 0);
        world.receive(at(84), &association_response(0));
        let m1 = RsnTxFrame::<512>::message1(
            oer_protocols_ieee80211_wire::security::rsn::Akm::Psk,
            Eui48::new(LOCAL),
            oer_protocols_ieee80211_wire::eapol::ReplayCounter::new(1),
            ANONCE,
        )
        .unwrap();
        world.receive(at(85), &eapol_frame(m1.as_bytes()));
        assert_eq!(world.station.config.callbacks.nonce_calls, nonce_calls + 1);
        let ptk = Pmk::from_bytes(PMK).derive_ptk(
            oer_protocols_ieee80211_wire::security::rsn::Akm::Psk,
            oer_protocols_ieee80211_core_security_rsn::PtkContext {
                authenticator_address: Eui48::new(BSSID),
                supplicant_address: Eui48::new(LOCAL),
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            },
        );
        world.receive(at(86), &retry_message3(&ptk, 2, 0x99));
        assert!(world.station.is_connected());
        assert!(
            world
                .completed
                .iter()
                .any(|terminal| terminal.request.correlation() == 1000
                    && matches!(terminal.outcome, StaCommandOutcome::Connected { .. }))
        );
    });
}

#[test]
fn a_frame_out_at_a_finish_holds_its_place_across_the_restart_until_its_end() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world.switches.on(Switch::HoldTransmissions);
        world.command(at(0), 1, StaCommand::Connect).unwrap();
        world.run(at(0));
        assert_eq!(world.losses.built, 1);
        assert_eq!(world.submitted.len(), 1);
        let old = world.submitted[0];
        assert!(old.is_probe());
        let mut world = restarted(world);
        // The old probe is still the port's: its place stays taken.
        assert!(world.station.out.holds(TxPurpose::Probe));
        world.run(at(50));
        assert!(!world.station.is_reclaiming());
        assert!(world.station.out.holds(TxPurpose::Probe));
        assert_eq!(world.losses.built, 1, "no probe over the one out");
        // The new run's scan closes the gate, which ends the old probe; only
        // then does the new run's probe go, under the new connection.
        world.command(at(51), 2, StaCommand::Connect).unwrap();
        world.run(at(51));
        assert_eq!(world.losses.built, 2);
        assert_eq!(world.submitted.len(), 1);
        assert_ne!(world.submitted[0].epoch(), old.epoch());
        assert!(world.station.out.holds(TxPurpose::Probe));
        world.switches.off(Switch::HoldTransmissions);
        world.run(at(52));
        assert!(world.submitted.is_empty());
        assert!(!world.station.out.holds(TxPurpose::Probe));
        assert!(matches!(world.station.state, State::Scan(_)));
    });
}

#[test]
fn a_restart_reclaims_through_removals_that_expired() {
    installed(|mint| {
        let (world, _) = wpa2_connected::<2>(mint);
        let mut world = restarted(world);
        world.take();
        world.removal_outcome = StaKeyOutcome::Expired;
        world.run(at(50));
        assert!(!world.station.is_reclaiming());
        assert!(world.station.keys.clear());
        assert_eq!(
            world.take(),
            [Seen::RemoveKey, Seen::RemoveKey, Seen::Configure("off")]
        );
        world.command(at(51), 1000, StaCommand::Connect).unwrap();
    });
}

#[test]
fn a_restart_sets_receive_off_only_after_the_last_runs_configuration_ended() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world.switches.on(Switch::HoldConfigured);
        world.command(at(0), 1, StaCommand::Connect).unwrap();
        world.run(at(0));
        assert!(
            world.switches.has(Switch::Configuring),
            "the scan's configuration is out"
        );
        let mut world = restarted(world);
        world.take();
        // The world refuses a second configuration while one is out.
        world.run(at(50));
        assert!(world.station.is_reclaiming());
        assert_eq!(world.take(), []);
        world.switches.off(Switch::HoldConfigured);
        world.run(at(51));
        assert_eq!(world.take(), [Seen::Configure("off")]);
        assert!(!world.station.is_reclaiming());
    });
}

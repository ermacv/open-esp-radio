use super::*;
use oer_ieee80211_arbiter::{ArbiterEnd, ArbiterNotice, RetiredArbiter};

#[test]
fn stopped_bank_reclaims_old_keys_then_connects_using_the_same_installation() {
    installed(|mint| {
        let (mut world, _) = wpa2_connected::<2>(mint);
        let previous_epoch = world.station.epoch;
        let previous_lease = world.station.lease.id();
        let mut arbiter = world.arbiter.finish(ArbiterEnd::Stopped);
        let (mut bank, mut places) = world.station.finish(StaDisconnectReason::Requested);
        while let Some(notice) = arbiter.poll_notice() {
            if let ArbiterNotice::Station(notice) = notice {
                places.returned(notice);
            }
        }
        while bank.poll_output().is_some() {}
        let RetiredArbiter::Stopped(stopped) = arbiter else {
            panic!()
        };
        let (arbiter, places) = stopped
            .restart(channel(6), places)
            .ok()
            .unwrap()
            .into_parts();
        let (station, deferred, notice) = bank.restart(places).into_parts();
        assert!(matches!(deferred, DeferredEapol::Empty));
        assert!(notice.is_none());
        world.station = station;
        world.arbiter = arbiter;
        world.take();
        world.hold_group_removals = true;
        world.run(at(50));
        assert!(world.station.is_reclaiming());
        assert!(world.station.link().is_none());
        assert_eq!(world.pending_removals.len(), 1);
        assert!(matches!(
            world
                .station
                .command(at(51), StaRequest::new(999, StaCommand::Connect)),
            Err(StaCommandRefused {
                reason: CommandRefusal::Transitioning,
                ..
            })
        ));
        world
            .station
            .input(at(52), world.pending_removals.pop().unwrap());
        world.hold_group_removals = false;
        world.run(at(52));
        assert!(!world.station.is_reclaiming());
        assert!(world.station.keys.clear());
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
        assert_eq!(
            world.take(),
            [Seen::RemoveKey, Seen::RemoveKey, Seen::Configure("off")]
        );
        world
            .station
            .command(at(53), StaRequest::new(1000, StaCommand::Connect))
            .unwrap();
        world.run(at(53));
        assert_ne!(world.station.epoch, previous_epoch);
        assert_ne!(world.station.lease.id(), previous_lease);
        world.advance();
        world.receive(at(66), &wpa2_beacon());
        world.advance();
        world.answer_authentication(at(83), 0);
        world.receive(at(84), &association_response(0));
        let m1 = RsnTxFrame::<512>::message1(
            oer_ieee80211_mac::security::rsn::Akm::Psk,
            LOCAL,
            1,
            ANONCE,
        )
        .unwrap();
        world.receive(at(85), &eapol_frame(m1.as_bytes()));
        let ptk = Pmk::from_bytes(PMK).derive_ptk(
            oer_ieee80211_mac::security::rsn::Akm::Psk,
            oer_ieee80211_rsn::PtkContext {
                authenticator_address: BSSID,
                supplicant_address: LOCAL,
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
        assert_eq!(world.station.lost_tokens(), 0);
    });
}

#[test]
fn a_missing_place_is_retained_by_the_arbiter_and_cannot_form_a_restart_bank() {
    installed(|mint| {
        let (world, _) = wpa2_connected::<2>(mint);
        let (bank, places) = world.station.finish(StaDisconnectReason::Requested);
        let RetiredArbiter::Stopped(stopped) = world.arbiter.finish(ArbiterEnd::Stopped) else {
            panic!()
        };
        let incomplete = stopped.restart(channel(6), places).err().unwrap();
        assert_eq!(
            incomplete.missing,
            oer_ieee80211_arbiter::MissingArbiterReturn::StationPlaces
        );
        assert_eq!(bank.frame_credits(), (2, 2));
        assert_eq!(bank.key_owners(), (1, 2, true));
        let mut arbiter = RetiredArbiter::Stopped(incomplete.bank);
        let mut places = incomplete.places;
        while let Some(notice) = arbiter.poll_notice() {
            if let ArbiterNotice::Station(notice) = notice {
                places.returned(notice);
            }
        }
        let RetiredArbiter::Stopped(stopped) = arbiter else {
            panic!()
        };
        let (_arbiter, places) = stopped
            .restart(channel(6), places)
            .ok()
            .unwrap()
            .into_parts();
        let (_station, deferred, _notice) = bank.restart(places).into_parts();
        assert!(matches!(deferred, DeferredEapol::Empty));
    });
}

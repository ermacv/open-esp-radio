//! The asynchronous key-data unwrap: one ticket, an explicit wait, stale
//! responses returning only their ticket.

use super::*;

/// Message 3 with group key 1 and replay counter `counter`.
fn message3(ptk: &oer_ieee80211_core_security_rsn::Ptk, counter: u64) -> Vec<u8> {
    use oer_ieee80211_core_security_rsn::frames::RsnTxFrame;
    use oer_ieee80211_wire::security::rsn::Akm;

    let mut plain = Vec::from(RSN);
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
    plain.extend_from_slice(&[0x99; 16]);
    plain.extend_from_slice(&[0xdd, 0]);
    let wrapped =
        oer_ieee80211_core_security_cipher::aes::software_aes128_key_wrap(ptk.kek(), &plain)
            .unwrap();
    let message3 =
        RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, counter, ANONCE, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(ptk);
    eapol_frame(message3.as_bytes())
}

/// Group Message 1 of key `key_id` with replay counter `replay`, protected
/// under packet number `pn`.
fn group_message1_at(
    ptk: &oer_ieee80211_core_security_rsn::Ptk,
    replay: u64,
    key_id: u8,
    pn: u64,
) -> Vec<u8> {
    use oer_ieee80211_core_security_rsn::frames::RsnTxFrame;
    use oer_ieee80211_wire::security::rsn::Akm;

    let mut plain = Vec::new();
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, key_id, 0x00]);
    plain.extend_from_slice(&[0x77; 16]);
    plain.extend_from_slice(&[0xdd, 0, 0, 0, 0, 0, 0, 0]);
    let wrapped =
        oer_ieee80211_core_security_cipher::aes::software_aes128_key_wrap(ptk.kek(), &plain)
            .unwrap();
    let message =
        RsnTxFrame::<512>::group_message1(Akm::Psk, LOCAL, replay, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(ptk);
    protected_received(eapol_frame(message.as_bytes()), pn)
}

fn handshake_step<'w>(world: &'w World<'_>) -> Option<&'w HandshakeStep> {
    match &world.station.state {
        State::Handshake(handshake) => Some(&handshake.step),
        _ => None,
    }
}

#[test]
fn the_handshake_waits_for_its_unwrap_and_ignores_a_repeated_message3() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_associated::<2>(mint);
        world.hold_unwraps = true;
        let message3 = message3(&ptk, 2);
        world.receive(at(33), &message3);
        assert_eq!(world.unwraps, 1);
        assert!(matches!(
            handshake_step(&world),
            Some(HandshakeStep::AwaitUnwrap { .. })
        ));
        assert!(world.station.key_unwrap.in_flight());
        let deadline = world.station.poll_deadline();
        assert!(deadline.is_some());

        // The access point repeats Message 3 while the unwrap is out: the
        // station asks for nothing more and its deadline stays.
        world.receive(at(34), &message3);
        assert_eq!(world.unwraps, 1);
        assert_eq!(world.station.poll_deadline(), deadline);
        assert!(world.take().is_empty());

        world.release_unwrap(at(35));
        world.run(at(35));
        assert!(world.station.is_connected());
        assert!(!world.station.key_unwrap.in_flight());
        assert_eq!(world.unwraps, 1);
    });
}

#[test]
fn a_response_after_the_handshake_left_returns_only_its_ticket_and_a_new_handshake_waits_for_it() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_associated::<2>(mint);
        world.hold_unwraps = true;
        world.receive(at(33), &message3(&ptk, 2));
        assert_eq!(world.unwraps, 1);
        world.command(at(34), 2, StaCommand::Disconnect).unwrap();
        world.run(at(34));
        assert!(handshake_step(&world).is_none());

        // A new connection reaches its Message 3 while the old unwrap is out:
        // its request waits for the ticket.
        world.command(at(40), 3, StaCommand::Connect).unwrap();
        world.run(at(40));
        let now = world.advance();
        world.receive(now, &wpa2_beacon());
        let now = world.advance();
        world.answer_authentication(now, 0);
        world.receive(now, &association_response(0));
        let message1 = oer_ieee80211_core_security_rsn::frames::RsnTxFrame::<512>::message1(
            oer_ieee80211_wire::security::rsn::Akm::Psk,
            LOCAL,
            1,
            ANONCE,
        )
        .unwrap();
        world.receive(now, &eapol_frame(message1.as_bytes()));
        world.receive(now, &message3(&ptk, 2));
        assert!(matches!(
            handshake_step(&world),
            Some(HandshakeStep::Unwrap(..))
        ));
        assert_eq!(world.unwraps, 1);

        // The old response finds no handshake waiting for it: only the
        // ticket comes back, and the new request goes out with it.
        world.release_unwrap(now);
        assert!(matches!(
            handshake_step(&world),
            Some(HandshakeStep::Unwrap(..))
        ));
        world.run(now);
        assert_eq!(world.unwraps, 2);
        assert!(matches!(
            handshake_step(&world),
            Some(HandshakeStep::AwaitUnwrap { .. })
        ));
        world.release_unwrap(now);
        world.run(now);
        assert!(world.station.is_connected());
        assert!(!world.station.key_unwrap.in_flight());
    });
}

#[test]
fn a_group_rekey_waits_for_its_unwrap_and_takes_the_next_frame_after_its_response() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        world.unwraps = 0;
        world.hold_unwraps = true;
        world.receive_verified(at(50), &group_message1_at(&ptk, 3, 2, 3));
        assert_eq!(world.unwraps, 1);

        // The same frame again is stale while its unwrap is out; a newer one
        // waits in the deferred place.
        assert_eq!(
            world.station.receive(
                at(51),
                &group_message1_at(&ptk, 3, 2, 4),
                verified_meta(),
                |_| {},
                &mut world.losses,
            ),
            Err(StaRxError::Security(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::StaleReplayCounter
            )))
        );
        world.receive_verified(at(51), &group_message1_at(&ptk, 4, 1, 5));
        assert_eq!(world.unwraps, 1);
        let before = world.frames.len();

        world.release_unwrap(at(52));
        world.run(at(52));
        assert_eq!(world.unwraps, 2);
        world.release_unwrap(at(53));
        world.run(at(53));
        assert_eq!(world.frames.len(), before + 2, "GM2 for each counter");
        assert!(world.station.is_connected());
        assert!(!world.station.key_unwrap.in_flight());
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
    });
}

#[test]
fn a_response_after_the_link_left_installs_nothing() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        world.unwraps = 0;
        world.hold_unwraps = true;
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        assert_eq!(world.unwraps, 1);
        world.command(at(51), 2, StaCommand::Disconnect).unwrap();
        world.run(at(51));
        assert!(!world.station.is_connected());
        world.take();

        world.release_unwrap(at(52));
        world.run(at(52));
        assert!(!world.station.key_unwrap.in_flight());
        assert!(world.take().is_empty());
        assert!(world.station.keys.clear());
    });
}

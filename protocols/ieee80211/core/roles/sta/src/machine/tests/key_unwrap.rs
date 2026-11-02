//! The key-data unwrap runs in the step, over the composition's block
//! primitive: no ticket, no action, no wait.

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
/// under packet number `pn`; `corrupt` flips a bit of the wrapped key data.
fn group_message1_at(
    ptk: &oer_ieee80211_core_security_rsn::Ptk,
    replay: u64,
    key_id: u8,
    pn: u64,
    corrupt: bool,
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
    let mut wrapped = wrapped.as_bytes().to_vec();
    if corrupt {
        wrapped[0] ^= 1;
    }
    let message = RsnTxFrame::<512>::group_message1(Akm::Psk, LOCAL, replay, [0; 8], &wrapped)
        .unwrap()
        .authenticate(ptk);
    protected_received(eapol_frame(message.as_bytes()), pn)
}

#[test]
fn message3_key_data_is_unwrapped_in_the_step_that_takes_it() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_associated::<2>(mint);
        let before = world.unwraps();
        // The receive and the actions it owes connect: nothing waits for an
        // engine or a later input.
        world.receive(at(33), &message3(&ptk, 2));
        assert_eq!(world.unwraps(), before + 1);
        assert!(world.station.is_connected());
    });
}

#[test]
fn a_group_rekey_unwraps_each_frame_in_its_step_and_answers_each() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        let before = world.unwraps();
        let frames = world.frames.len();
        world.receive_verified(at(50), &group_message1_at(&ptk, 3, 2, 3, false));
        world.run(at(50));
        world.receive_verified(at(51), &group_message1_at(&ptk, 4, 1, 4, false));
        world.run(at(51));
        assert_eq!(world.unwraps(), before + 2);
        assert_eq!(world.frames.len(), frames + 2, "GM2 for each counter");
        assert!(world.station.is_connected());
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
    });
}

#[test]
fn key_data_that_fails_its_integrity_check_is_a_counted_drop_and_the_link_stays() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        let before = world.losses.rsn;
        let frames = world.frames.len();
        world.receive_verified(at(50), &group_message1_at(&ptk, 3, 2, 3, true));
        world.run(at(50));
        assert_eq!(world.losses.rsn, before + 1);
        assert_eq!(world.frames.len(), frames, "no GM2 for refused key data");
        assert!(world.station.is_connected());
        // The next valid group message still installs and answers.
        world.receive_verified(at(51), &group_message1_at(&ptk, 4, 1, 4, false));
        world.run(at(51));
        assert_eq!(world.frames.len(), frames + 1);
    });
}

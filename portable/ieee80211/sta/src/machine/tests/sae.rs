use super::*;
use oer_ieee80211_mac::security::rsn::{Akm, RsnElement};
use oer_ieee80211_rsn::{
    Ptk, PtkContext,
    frames::RsnTxFrame,
    sae::{SaeCommitValues, SaeKeys},
};
use std::vec;

fn advertised(transition: bool, h2e: bool) -> Vec<u8> {
    let mut beacon = wpa2_beacon().to_vec();
    beacon[74] = 8;
    beacon[75] |= 0x80;
    if transition {
        beacon[56] += 4;
        beacon[69] = 2;
        beacon.splice(75..75, [0, 0x0f, 0xac, 2]);
    }
    if h2e {
        beacon.extend_from_slice(&[244, 1, 0x20]);
    }
    beacon
}

fn personal() -> StaSecurity {
    StaSecurity::Personal {
        password: SaePassword::new(b"password").unwrap(),
        // Deliberately unrelated to the SAE password: a SAE handshake must
        // prove the negotiated PMK, never this provisioned PSK.
        pmk: Pmk::from_bytes(PMK),
    }
}

fn begin(world: &mut World<'_>, beacon: &[u8], start: u32, request: u64) {
    world
        .station
        .command(at(start), StaRequest::new(request, StaCommand::Connect))
        .unwrap();
    world.run(at(start));
    world.advance();
    world.receive(at(start + 13), beacon);
    world.advance();
}

/// Independent wire peer: verify all three addresses before decoding auth.
fn outgoing_authentication(frame: &[u8], algorithm: u16, transaction: u16) -> (u16, &[u8]) {
    assert_eq!(
        u16::from_le_bytes(frame[0..2].try_into().unwrap()) & 0x00fc,
        0x00b0
    );
    assert_eq!(&frame[4..10], &BSSID);
    assert_eq!(&frame[10..16], &LOCAL);
    assert_eq!(&frame[16..22], &BSSID);
    assert_eq!(
        u16::from_le_bytes(frame[24..26].try_into().unwrap()),
        algorithm
    );
    assert_eq!(
        u16::from_le_bytes(frame[26..28].try_into().unwrap()),
        transaction
    );
    (
        u16::from_le_bytes(frame[28..30].try_into().unwrap()),
        &frame[30..],
    )
}

fn peer_authentication(transaction: u16, status: u16, body: &[u8]) -> Vec<u8> {
    let mut wire = vec![0; 30];
    wire[0] = 0xb0;
    wire[4..10].copy_from_slice(&LOCAL);
    wire[10..16].copy_from_slice(&BSSID);
    wire[16..22].copy_from_slice(&BSSID);
    wire[24..26].copy_from_slice(&3_u16.to_le_bytes());
    wire[26..28].copy_from_slice(&transaction.to_le_bytes());
    wire[28..30].copy_from_slice(&status.to_le_bytes());
    wire.extend_from_slice(body);
    wire
}

fn commit(world: &mut World<'_>, h2e: bool, now: u32) -> SaeKeys {
    let commit_wire = world.frames.last().unwrap();
    let (status, body) = outgoing_authentication(commit_wire, 3, 1);
    assert_eq!(status, if h2e { 126 } else { 0 });
    let station = SaeCommitValues::parse(body, h2e).unwrap();
    let element = if h2e {
        SaePasswordToken::derive(SSID, b"password", None).password_element(BSSID, LOCAL)
    } else {
        SaePasswordElement::hunting_and_pecking(b"password", BSSID, LOCAL).unwrap()
    };
    let peer = SaeCommit::new(element, [0x40; 32], [0x41; 32]).unwrap();
    let keys = peer.process(station).unwrap();
    let mut body = [0; oer_ieee80211_rsn::sae::SAE_COMMIT_LEN];
    let length = peer.values().encode(None, h2e, &mut body).unwrap();
    world.receive(at(now), &peer_authentication(1, status, &body[..length]));
    let (status, confirm) = outgoing_authentication(world.frames.last().unwrap(), 3, 2);
    assert_eq!(status, 0);
    assert_eq!(keys.verify_peer_confirm(confirm), Ok(1));
    keys
}

fn authenticate(world: &mut World<'_>, h2e: bool, now: u32) -> SaeKeys {
    let keys = commit(world, h2e, now);
    world.receive(
        at(now + 1),
        &peer_authentication(2, 0, &keys.own_confirm(1)),
    );
    keys
}

fn association_rsn(world: &World<'_>) -> Vec<u8> {
    let wire = world.frames.last().unwrap();
    assert_eq!(wire[0] & 0xfc, 0);
    assert_eq!(&wire[4..10], &BSSID);
    assert_eq!(&wire[10..16], &LOCAL);
    assert_eq!(&wire[16..22], &BSSID);
    let mut ies = &wire[28..];
    while let [kind, length, rest @ ..] = ies {
        let element_length = usize::from(*length) + 2;
        if *kind == 48 {
            return ies[..element_length].to_vec();
        }
        ies = &rest[usize::from(*length)..];
    }
    panic!("association must carry RSN");
}

fn finish_handshake(
    world: &mut World<'_>,
    advertisement: &[u8],
    akm: Akm,
    pmk: &Pmk,
    now: u32,
) -> Ptk {
    world.receive(at(now), &association_response(0));
    let message1 = RsnTxFrame::<512>::message1(akm, LOCAL, 1, ANONCE).unwrap();
    world.receive(at(now + 1), &eapol_frame(message1.as_bytes()));
    let ptk = pmk.derive_ptk(
        akm,
        PtkContext {
            authenticator_address: BSSID,
            supplicant_address: LOCAL,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    );
    let message2 = OwnedEapolFrame::<512>::try_copy(
        RsnInterface::AccessPoint,
        LOCAL,
        &world.frames.last().unwrap()[32..],
    )
    .unwrap();
    assert!(
        message2.key_frame().verify_mic(&ptk),
        "wire M2 must use the authenticated PMK"
    );
    let mut plain = advertisement[55..].to_vec();
    plain.extend_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, 1, 0]);
    plain.extend_from_slice(&[0x99; 16]);
    if akm == Akm::Sae {
        plain.extend_from_slice(&[0xdd, 28, 0, 0x0f, 0xac, 9, 4, 0, 1, 0, 0, 0, 0, 0]);
        plain.extend_from_slice(&[0x77; 16]);
    }
    if !plain.len().is_multiple_of(8) {
        plain.push(0xdd);
        while !plain.len().is_multiple_of(8) {
            plain.push(0);
        }
    }
    let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
    let message3 = RsnTxFrame::<512>::message3(akm, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
        .unwrap()
        .authenticate(&ptk);
    world.receive(at(now + 2), &eapol_frame(message3.as_bytes()));
    assert!(
        world.station.is_connected(),
        "wire SAE/PSK handshake completes"
    );
    assert_eq!(world.station.tickets.len(), 2);
    assert_eq!(world.station.buffers.len(), 2);
    ptk
}

#[test]
fn review_sae_selection_transmits_a_wire_commit() {
    for transition in [false, true] {
        installed(|mint| {
            let mut world = World::with_security(mint, 3, personal());
            begin(&mut world, &advertised(transition, false), 0, 91);
            let (_, body) = outgoing_authentication(world.frames.last().unwrap(), 3, 1);
            assert!(SaeCommitValues::parse(body, false).is_ok());
        });
    }
}

#[test]
fn wire_peer_connects_sae_only_transition_and_resumes_pmksa() {
    for transition in [false, true] {
        for h2e in [false, true] {
            installed(|mint| {
                let mut world = World::with_security(mint, 3, personal());
                let beacon = advertised(transition, h2e);
                begin(&mut world, &beacon, 0, 91);
                let keys = authenticate(&mut world, h2e, 30);
                let association = association_rsn(&world);
                let rsn = RsnElement::parse(&association).unwrap();
                assert!(rsn.akm_suites().contains([0, 0x0f, 0xac, 8]));
                assert!(!rsn.akm_suites().contains([0, 0x0f, 0xac, 2]));
                let pmk = Pmk::from_bytes(keys.pmk);
                finish_handshake(&mut world, &beacon, Akm::Sae, &pmk, 32);
                world
                    .station
                    .command(at(40), StaRequest::new(92, StaCommand::Disconnect))
                    .unwrap();
                world.run(at(40));
                assert!(matches!(world.station.state, State::Idle));
                begin(&mut world, &beacon, 100, 93);
                outgoing_authentication(world.frames.last().unwrap(), 0, 1);
                world.answer_authentication(at(130), 0);
                let association = association_rsn(&world);
                let rsn = RsnElement::parse(&association).unwrap();
                assert_eq!(rsn.pmkids().collect::<Vec<_>>(), [keys.pmkid]);
                assert!(rsn.akm_suites().contains([0, 0x0f, 0xac, 8]));
                finish_handshake(&mut world, &beacon, Akm::Sae, &pmk, 131);
            });
        }
    }
}

#[test]
fn wire_peer_connects_psk_only_using_the_provisioned_pmk() {
    installed(|mint| {
        let mut world = World::with_security(mint, 3, personal());
        let beacon = wpa2_beacon();
        begin(&mut world, &beacon, 0, 91);
        outgoing_authentication(world.frames.last().unwrap(), 0, 1);
        world.answer_authentication(at(30), 0);
        let association = association_rsn(&world);
        let rsn = RsnElement::parse(&association).unwrap();
        assert!(rsn.akm_suites().contains([0, 0x0f, 0xac, 2]));
        assert!(!rsn.akm_suites().contains([0, 0x0f, 0xac, 8]));
        finish_handshake(&mut world, &beacon, Akm::Psk, &Pmk::from_bytes(PMK), 31);
    });
}

#[test]
fn invalid_confirm_returns_credits_and_never_caches_the_pmk() {
    installed(|mint| {
        let mut world = World::with_security(mint, 3, personal());
        let beacon = advertised(true, false);
        begin(&mut world, &beacon, 0, 91);
        let keys = commit(&mut world, false, 30);
        let mut confirm = keys.own_confirm(1);
        confirm[2] ^= 1;
        world.receive(at(31), &peer_authentication(2, 0, &confirm));
        assert!(world.seen.contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::Sae(StaSaeFailure::Protocol(
                oer_ieee80211_rsn::sae::SaeError::ConfirmMismatch
            ))
        ))));
        assert!(matches!(world.station.state, State::Idle));
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
        assert!(
            world
                .station
                .pmksa
                .resume(
                    oer_ieee80211_mac::address::Bssid::new(IndividualAddress::new(BSSID).unwrap())
                        .unwrap(),
                    SSID
                )
                .is_none()
        );
        begin(&mut world, &beacon, 100, 92);
        outgoing_authentication(world.frames.last().unwrap(), 3, 1);
    });
}

#[test]
fn anti_clogging_response_waits_for_credit_without_extending_the_deadline() {
    installed(|mint| {
        let mut world = World::with_security(mint, 3, personal());
        begin(&mut world, &advertised(true, false), 0, 91);
        let first_commit = world.frames.last().unwrap().clone();
        let deadline = world.station.poll_deadline().unwrap();
        let first = world.station.reserve_transmit().unwrap();
        let second = world.station.reserve_transmit().unwrap();
        let before = world.frames.len();
        world.receive(at(30), &peer_authentication(1, 76, &[19, 0, 7, 8, 9]));
        assert_eq!(world.frames.len(), before);
        assert_eq!(world.station.poll_deadline(), Some(deadline));
        world.station.return_credit(first);
        world.run(at(31));
        let (_, token_commit) = outgoing_authentication(world.frames.last().unwrap(), 3, 1);
        assert_eq!(&token_commit[2..5], &[7, 8, 9]);
        assert_eq!(&token_commit[5..], &first_commit[32..]);
        world.station.return_credit(second);
        world.station.input(deadline, StaInput::Tick);
        world.run(deadline);
        assert!(world.seen.contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::Sae(StaSaeFailure::Timeout)
        ))));
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
    });
}

use super::*;
use oer_protocols_ieee80211_core_security_rsn::{
    Ptk, PtkContext, frames::RsnTxFrame, sae::SaeKeys,
};
use oer_protocols_ieee80211_wire::sae::SaeCommitValues;
use oer_protocols_ieee80211_wire::security::SaePwe;
use oer_protocols_ieee80211_wire::security::rsn::{Akm, RsnElement};
use std::vec;

/// Whether a WPA3 network also admits WPA2's PSK: its transition mode.
#[derive(Clone, Copy, PartialEq)]
enum Transition {
    SaeOnly,
    WithPsk,
}

fn advertised(transition: Transition, pwe: SaePwe) -> Vec<u8> {
    let mut beacon = wpa2_beacon().to_vec();
    beacon[74] = 8;
    beacon[75] |= 0x80;
    if transition == Transition::WithPsk {
        beacon[56] += 4;
        beacon[69] = 2;
        beacon.splice(75..75, [0, 0x0f, 0xac, 2]);
    }
    if pwe == SaePwe::HashToElement {
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
        .command(at(start), request, StaCommand::Connect)
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

fn commit(world: &mut World<'_>, pwe: SaePwe, now: u32) -> SaeKeys {
    let commit_wire = world.frames.last().unwrap();
    let (status, body) = outgoing_authentication(commit_wire, 3, 1);
    assert_eq!(
        status,
        match pwe {
            SaePwe::HashToElement => 126,
            SaePwe::HuntingAndPecking => 0,
        }
    );
    let station = SaeCommitValues::parse(body, pwe).unwrap();
    let element = match pwe {
        SaePwe::HashToElement => SaePasswordToken::derive(SSID, b"password", None)
            .unwrap()
            .password_element(Eui48::new(BSSID), Eui48::new(LOCAL)),
        SaePwe::HuntingAndPecking => SaePasswordElement::hunting_and_pecking(
            b"password",
            Eui48::new(BSSID),
            Eui48::new(LOCAL),
        )
        .unwrap(),
    };
    let peer = SaeCommit::new(&element, [0x40; 32], [0x41; 32]).unwrap();
    let keys = peer.process(station).unwrap();
    let mut body = [0; oer_protocols_ieee80211_wire::sae::SAE_COMMIT_LEN];
    let length = peer.values().encode(None, pwe, &mut body).unwrap();
    world.receive(at(now), &peer_authentication(1, status, &body[..length]));
    let (status, confirm) = outgoing_authentication(world.frames.last().unwrap(), 3, 2);
    assert_eq!(status, 0);
    assert_eq!(keys.verify_peer_confirm(confirm), Ok(1));
    keys
}

fn authenticate(world: &mut World<'_>, pwe: SaePwe, now: u32) -> SaeKeys {
    let keys = commit(world, pwe, now);
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
    let (ptk, message3) = handshake_message3(world, advertisement, akm, pmk, now);
    world.receive(at(now + 2), &eapol_frame(message3.as_bytes()));
    assert!(
        world.station.is_connected(),
        "wire SAE/PSK handshake completes"
    );
    ptk
}

/// Associate, answer Message 1 and build the access point's Message 3.
fn handshake_message3(
    world: &mut World<'_>,
    advertisement: &[u8],
    akm: Akm,
    pmk: &Pmk,
    now: u32,
) -> (Ptk, RsnTxFrame<512>) {
    world.receive(at(now), &association_response(0));
    let message1 = RsnTxFrame::<512>::message1(akm, Eui48::new(LOCAL), 1, ANONCE).unwrap();
    world.receive(at(now + 1), &eapol_frame(message1.as_bytes()));
    let ptk = pmk.derive_ptk(
        akm,
        PtkContext {
            authenticator_address: Eui48::new(BSSID),
            supplicant_address: Eui48::new(LOCAL),
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    );
    let message2 = OwnedEapolFrame::<512>::try_copy(
        RsnInterface::AccessPoint,
        Eui48::new(LOCAL),
        &world.frames.last().unwrap()[32..],
    )
    .unwrap();
    assert!(
        message2
            .key_frame()
            .unwrap()
            .verify_mic(&ptk, &mut SoftwareAes128)
            .unwrap(),
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
    let wrapped = software_wrap(ptk.kek(), &plain).unwrap();
    let message3 = RsnTxFrame::<512>::message3(
        akm,
        Eui48::new(LOCAL),
        2,
        ANONCE,
        [0; 8],
        wrapped.as_bytes(),
    )
    .unwrap()
    .authenticate(&ptk, &mut SoftwareAes128)
    .unwrap();
    (ptk, message3)
}

/// An engine that refuses Message 4's MIC ends the handshake where the
/// role ends every handshake: its installed keys are removed, no Message 4
/// goes and the station does not connect. SAE computes the MIC with
/// AES-CMAC on the engine.
#[test]
fn a_refused_message4_mic_ends_the_handshake_and_removes_its_keys() {
    installed(|mint| {
        let mut world = World::with_security(mint, 3, personal());
        let beacon = advertised(Transition::SaeOnly, SaePwe::HuntingAndPecking);
        begin(&mut world, &beacon, 0, 91);
        let keys = authenticate(&mut world, SaePwe::HuntingAndPecking, 30);
        let pmk = Pmk::from_bytes(*keys.pmk());
        let (_, message3) = handshake_message3(&mut world, &beacon, Akm::Sae, &pmk, 32);
        let eapol = world.eapol.len();
        world.switches.on(Switch::HoldKeyInstalls);
        world.receive(at(34), &eapol_frame(message3.as_bytes()));
        // The pairwise key, then the group key: the engine refuses once
        // both are in.
        let pairwise = world.pending_installs.pop().unwrap();
        world.input(at(35), pairwise);
        world.run(at(35));
        let group = world.pending_installs.pop().unwrap();
        world.switches.off(Switch::HoldKeyInstalls);
        world.station.config.callbacks.blocks.refuse =
            Some(oer_portable_crypto::block::BlockError::Timeout { observation: 1 });
        world.input(at(36), group);
        world.run(at(36));
        world.station.config.callbacks.blocks.refuse = None;
        world.run(at(37));
        assert!(!world.station.is_connected());
        assert_eq!(world.eapol.len(), eapol, "no Message 4 goes");
        assert_eq!(
            world
                .seen
                .iter()
                .filter(|event| **event == Seen::RemoveKey)
                .count(),
            2,
            "the pairwise and the group key are removed"
        );
        assert!(
            !world
                .seen
                .iter()
                .any(|event| matches!(event, Seen::Notify(StaNotice::Connected { .. })))
        );
    });
}

#[test]
fn review_sae_selection_transmits_a_wire_commit() {
    for transition in [Transition::SaeOnly, Transition::WithPsk] {
        installed(|mint| {
            let mut world = World::with_security(mint, 3, personal());
            begin(
                &mut world,
                &advertised(transition, SaePwe::HuntingAndPecking),
                0,
                91,
            );
            let (_, body) = outgoing_authentication(world.frames.last().unwrap(), 3, 1);
            assert!(
                SaeCommitValues::parse(
                    body,
                    oer_protocols_ieee80211_wire::security::SaePwe::HuntingAndPecking
                )
                .is_ok()
            );
        });
    }
}

#[test]
fn wire_peer_connects_sae_only_transition_and_resumes_pmksa() {
    for transition in [Transition::SaeOnly, Transition::WithPsk] {
        for pwe in [SaePwe::HuntingAndPecking, SaePwe::HashToElement] {
            installed(|mint| {
                let mut world = World::with_security(mint, 3, personal());
                let beacon = advertised(transition, pwe);
                begin(&mut world, &beacon, 0, 91);
                let keys = authenticate(&mut world, pwe, 30);
                let association = association_rsn(&world);
                let rsn = RsnElement::parse(&association).unwrap();
                assert!(rsn.akm_suites().contains([0, 0x0f, 0xac, 8]));
                assert!(!rsn.akm_suites().contains([0, 0x0f, 0xac, 2]));
                let pmk = Pmk::from_bytes(*keys.pmk());
                finish_handshake(&mut world, &beacon, Akm::Sae, &pmk, 32);
                world.command(at(40), 92, StaCommand::Disconnect).unwrap();
                world.run(at(40));
                assert!(matches!(world.station.state, State::Idle));
                begin(&mut world, &beacon, 100, 93);
                outgoing_authentication(world.frames.last().unwrap(), 0, 1);
                world.answer_authentication(at(130), 0);
                let association = association_rsn(&world);
                let rsn = RsnElement::parse(&association).unwrap();
                assert_eq!(rsn.pmkids().collect::<Vec<_>>(), [*keys.pmkid()]);
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
        let beacon = advertised(Transition::WithPsk, SaePwe::HuntingAndPecking);
        begin(&mut world, &beacon, 0, 91);
        let keys = commit(&mut world, SaePwe::HuntingAndPecking, 30);
        let mut confirm = keys.own_confirm(1);
        confirm[2] ^= 1;
        world.receive(at(31), &peer_authentication(2, 0, &confirm));
        assert!(world.seen.contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::Sae(StaSaeFailure::Protocol(
                oer_protocols_ieee80211_core_security_rsn::sae::SaeError::ConfirmMismatch
            ))
        ))));
        assert!(matches!(world.station.state, State::Idle));
        assert!(
            world
                .station
                .pmksa
                .resume(
                    oer_protocols_ieee80211_wire::address::Bssid::new(
                        IndividualAddress::new(Eui48::new(BSSID)).unwrap()
                    )
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
fn anti_clogging_response_waits_for_the_commit_out_without_extending_the_deadline() {
    installed(|mint| {
        let mut world = World::with_security(mint, 3, personal());
        begin(
            &mut world,
            &advertised(Transition::WithPsk, SaePwe::HuntingAndPecking),
            0,
            91,
        );
        let first_commit = world.frames.last().unwrap().clone();
        let deadline = world.station.poll_deadline().unwrap();
        // An SAE frame is still out: its purpose's place is taken.
        world.station.out.take(TxPurpose::SaeAuthentication);
        let before = world.frames.len();
        world.receive(at(30), &peer_authentication(1, 76, &[19, 0, 7, 8, 9]));
        assert_eq!(world.frames.len(), before);
        assert_eq!(world.station.poll_deadline(), Some(deadline));
        // Its end frees the place: the token commit goes.
        world.station.out.release(TxPurpose::SaeAuthentication);
        world.run(at(31));
        let (_, token_commit) = outgoing_authentication(world.frames.last().unwrap(), 3, 1);
        assert_eq!(&token_commit[2..5], &[7, 8, 9]);
        assert_eq!(&token_commit[5..], &first_commit[32..]);
        world.input(deadline, StaInput::Tick);
        world.run(deadline);
        assert!(world.seen.contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::Sae(StaSaeFailure::Timeout)
        ))));
    });
}

#[test]
fn password_element_invariant_is_a_local_fault_with_its_peer_and_derivation() {
    for pwe in [
        oer_protocols_ieee80211_wire::security::SaePwe::HuntingAndPecking,
        oer_protocols_ieee80211_wire::security::SaePwe::HashToElement,
    ] {
        assert_eq!(
            sae_derivation_failure(
                oer_protocols_ieee80211_core_security_rsn::sae::SaeError::Crypto,
                BSSID,
                pwe
            ),
            StaDisconnectReason::Sae(StaSaeFailure::Crypto {
                peer: BSSID,
                pwe: pwe.into()
            })
        );
        assert_eq!(
            sae_derivation_failure(
                oer_protocols_ieee80211_core_security_rsn::sae::SaeError::NoPasswordElement,
                BSSID,
                pwe
            ),
            StaDisconnectReason::Sae(StaSaeFailure::Protocol(
                oer_protocols_ieee80211_core_security_rsn::sae::SaeError::NoPasswordElement
            ))
        );
    }
}

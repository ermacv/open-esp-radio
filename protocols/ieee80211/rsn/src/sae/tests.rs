#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::unreachable
)]
use super::*;

// IEEE Std 802.11-2020 Annex J.10, as hostap's `sae_tests` carries it.
const ADDR1: [u8; 6] = [0x4d, 0x3f, 0x2f, 0xff, 0xe3, 0x87];
const ADDR2: [u8; 6] = [0xa5, 0xd8, 0xaa, 0x95, 0x8e, 0x3c];
const PASSWORD: &[u8] = b"mekmitasdigoat";

fn hex<const N: usize>(text: &str) -> [u8; N] {
    let text: std::string::String = text.split_whitespace().collect();
    core::array::from_fn(|index| u8::from_str_radix(&text[2 * index..2 * index + 2], 16).unwrap())
}

const LOCAL_RAND: &str = "992465fd3daa3c60aa6565b7f62a2a7f2e12dd12f198faf4fbed89d7ff1ace94";
const LOCAL_MASK: &str = "9507a90f777a044d6a0830b91ea3d5dd70bece44e1acffb86983b5e1bf9fb322";
const LOCAL_COMMIT: &str = "1300 2e2c0f0db52440ad146d967114ce005ce1eab0aa2c2e5c2871b774f6c2575c65
    d5ad9e00829707aa36ba8b859738fc961d08243505f47c035376d7ac4bc8d7b95083bf43827d0fc31ed778dd3671
    fd21a46d1091d64b6f9a1e1272621325dbe1";
const PEER_COMMIT: &str = "1300 591b96f3397fb945100848e7b550543b6720d88337ee93fc49fd6df7e08b5223
    e71b9bb048d3873f20556953a96c91536fd8ee6ca9b4a68a148b056a909be03e83ae208f60f8ef5537858074db06
    68703239986299 9b511e0a1552a5fea317c2";
const KCK: &str = "1e733f6d9bd5325628730433883 1b09a39406d121017073a5c30db36f36cb81a";
const PMK: &str = "4e4dfab1a2dd8ac1a91790f953faaa452ae5c6873ab75b63605ba663f8a7fe59";
const PMKID: &str = "8747a600eea3f9f22475df58ca1e5498";

#[test]
fn hunting_and_pecking_matches_annex_j10() {
    let pwe = SaePasswordElement::hunting_and_pecking(PASSWORD, ADDR1, ADDR2).unwrap();
    let commit = SaeCommit::new(&pwe, hex(LOCAL_RAND), hex(LOCAL_MASK)).unwrap();
    let mut body = [0; SAE_COMMIT_LEN];
    assert_eq!(
        commit.values().encode(None, false, &mut body),
        Ok(SAE_COMMIT_LEN)
    );
    assert_eq!(body, hex::<SAE_COMMIT_LEN>(LOCAL_COMMIT));
    let peer = SaeCommitValues::parse(&hex::<SAE_COMMIT_LEN>(PEER_COMMIT), false).unwrap();
    let keys = commit.process(peer).unwrap();
    assert_eq!(keys.kck, hex(KCK));
    assert_eq!(keys.pmk, hex(PMK));
    assert_eq!(keys.pmkid, hex(PMKID));
}

#[test]
fn hash_to_element_matches_annex_j10() {
    let token = SaePasswordToken::derive(b"byteme", PASSWORD, Some(b"psk4internet")).unwrap();
    let pwe = token.password_element(
        [0x00, 0x09, 0x5b, 0x66, 0xec, 0x1e],
        [0x00, 0x0b, 0x6b, 0xd9, 0x02, 0x46],
    );
    let expected: [u8; 64] = hex(
        "c93049b9e64000f848201649e999f2b5c22dea69b5632c9df4d633b8aa1f6c1e
         73634e94b53d82e7383a8d258199d9dc1a5ee8269d060382ccbf33e614ff59a0",
    );
    assert_eq!(element_bytes(&pwe.0.to_affine()), Ok(expected));
}

fn peers(derive: impl Fn([u8; 6], [u8; 6]) -> SaePasswordElement) -> (SaeKeys, SaeKeys) {
    let station = SaeCommit::new(&derive(ADDR1, ADDR2), [0x11; 32], [0x22; 32]).unwrap();
    let access_point = SaeCommit::new(&derive(ADDR2, ADDR1), [0x33; 32], [0x44; 32]).unwrap();
    (
        station.process(access_point.values()).unwrap(),
        access_point.process(station.values()).unwrap(),
    )
}

#[test]
fn both_peers_derive_one_pmk_and_verify_each_confirm() {
    for (station, access_point) in [
        peers(|local, peer| {
            SaePasswordElement::hunting_and_pecking(PASSWORD, local, peer).unwrap()
        }),
        peers(|local, peer| {
            SaePasswordToken::derive(b"byteme", PASSWORD, None)
                .unwrap()
                .password_element(local, peer)
        }),
    ] {
        assert_eq!(station.pmk, access_point.pmk);
        assert_eq!(station.pmkid, access_point.pmkid);
        assert_eq!(
            access_point.verify_peer_confirm(&station.own_confirm(1)),
            Ok(1)
        );
        assert_eq!(
            station.verify_peer_confirm(&access_point.own_confirm(7)),
            Ok(7)
        );
        let mut forged = station.own_confirm(1);
        forged[5] ^= 1;
        assert_eq!(
            access_point.verify_peer_confirm(&forged),
            Err(SaeError::ConfirmMismatch)
        );
    }
}

#[test]
fn a_wrong_password_fails_the_confirm() {
    let station = SaeCommit::new(
        &SaePasswordElement::hunting_and_pecking(PASSWORD, ADDR1, ADDR2).unwrap(),
        [0x11; 32],
        [0x22; 32],
    )
    .unwrap();
    let access_point = SaeCommit::new(
        &SaePasswordElement::hunting_and_pecking(b"another password", ADDR2, ADDR1).unwrap(),
        [0x33; 32],
        [0x44; 32],
    )
    .unwrap();
    let station_keys = station.process(access_point.values()).unwrap();
    let access_point_keys = access_point.process(station.values()).unwrap();
    assert_eq!(
        access_point_keys.verify_peer_confirm(&station_keys.own_confirm(1)),
        Err(SaeError::ConfirmMismatch)
    );
}

#[test]
fn invalid_commits_are_refused() {
    let pwe = SaePasswordElement::hunting_and_pecking(PASSWORD, ADDR1, ADDR2).unwrap();
    assert!(SaeCommit::new(&pwe, [0; 32], [0x22; 32]).is_err());
    assert!(SaeCommit::new(&pwe, [0xff; 32], [0x22; 32]).is_err());
    let commit = SaeCommit::new(&pwe, [0x11; 32], [0x22; 32]).unwrap();
    assert_eq!(
        commit.process(commit.values()).err(),
        Some(SaeError::Reflection)
    );
    let mut off_curve = commit.values();
    off_curve.element[63] ^= 1;
    off_curve.scalar[31] ^= 1;
    assert_eq!(
        commit.process(off_curve).err(),
        Some(SaeError::InvalidElement)
    );
    let mut body = [0; SAE_COMMIT_LEN];
    commit.values().encode(None, false, &mut body).unwrap();
    body[0] = 20;
    assert_eq!(
        SaeCommitValues::parse(&body, false),
        Err(SaeError::UnsupportedGroup(20))
    );
    body[0] = 19;
    assert_eq!(
        SaeCommitValues::parse(&body[..10], false),
        Err(SaeError::Malformed)
    );
}

#[test]
fn tokens_sit_where_the_vendor_writes_them() {
    let commit = SaeCommit::new(
        &SaePasswordElement::hunting_and_pecking(PASSWORD, ADDR1, ADDR2).unwrap(),
        [0x11; 32],
        [0x22; 32],
    )
    .unwrap();
    let token = [0xab; 5];
    let mut hunting = [0; 128];
    let length = commit
        .values()
        .encode(Some(&token), false, &mut hunting)
        .unwrap();
    assert_eq!(length, SAE_COMMIT_LEN + 5);
    assert_eq!(&hunting[2..7], &token);
    assert_eq!(&hunting[7..39], &commit.values().scalar);

    let mut h2e = [0; 128];
    let length = commit
        .values()
        .encode(Some(&token), true, &mut h2e)
        .unwrap();
    assert_eq!(length, SAE_COMMIT_LEN + 3 + 5);
    assert_eq!(&h2e[2..34], &commit.values().scalar);
    assert_eq!(&h2e[SAE_COMMIT_LEN..SAE_COMMIT_LEN + 3], &[255, 6, 93]);

    // A status-76 refusal carries the token after the group.
    assert_eq!(
        anti_clogging_token(&[19, 0, 1, 2, 3], false),
        Ok(&[1, 2, 3][..])
    );
    assert_eq!(
        anti_clogging_token(&[19, 0, 255, 4, 93, 1, 2, 3], true),
        Ok(&[1, 2, 3][..])
    );
    assert!(anti_clogging_token(&[19, 0, 1, 2, 3], true).is_err());
}

#[test]
fn an_h2e_commit_admits_only_a_rejected_groups_element_without_19() {
    let commit = SaeCommit::new(
        &SaePasswordElement::hunting_and_pecking(PASSWORD, ADDR1, ADDR2).unwrap(),
        [0x11; 32],
        [0x22; 32],
    )
    .unwrap();
    let mut body = [0; SAE_COMMIT_LEN + 5];
    commit.values().encode(None, true, &mut body).unwrap();
    body[SAE_COMMIT_LEN..].copy_from_slice(&[255, 3, 92, 20, 0]);
    assert_eq!(SaeCommitValues::parse(&body, true), Ok(commit.values()));
    assert_eq!(
        SaeCommitValues::parse(&body, false),
        Err(SaeError::Malformed)
    );
    body[SAE_COMMIT_LEN + 3] = 19;
    assert_eq!(
        SaeCommitValues::parse(&body, true),
        Err(SaeError::Malformed)
    );
}

#[test]
fn an_access_point_parses_the_commit_a_station_writes() {
    let token = [0x5a; SAE_ANTI_CLOGGING_TOKEN_LEN];
    let peer = SaeCommitValues::parse(&hex::<SAE_COMMIT_LEN>(PEER_COMMIT), false).unwrap();
    let mut body = [0; SAE_COMMIT_LEN + SAE_ANTI_CLOGGING_TOKEN_LEN + 3];

    let len = peer.encode(None, false, &mut body).unwrap();
    let received = SaeReceivedCommit::parse(&body[..len], false).unwrap();
    assert_eq!(received.values, peer);
    assert_eq!(received.token, None);

    let len = peer.encode(Some(&token), false, &mut body).unwrap();
    let received = SaeReceivedCommit::parse(&body[..len], false).unwrap();
    assert_eq!(received.values, peer);
    assert_eq!(received.token, Some(&token[..]));

    let len = peer.encode(Some(&token), true, &mut body).unwrap();
    let received = SaeReceivedCommit::parse(&body[..len], true).unwrap();
    assert_eq!(received.values, peer);
    assert_eq!(received.token, Some(&token[..]));
    assert!(!received.rejects_offered_group());
}

#[test]
fn an_access_point_reads_rejected_groups_and_refuses_bad_commits() {
    let peer = hex::<SAE_COMMIT_LEN>(PEER_COMMIT);
    let mut body = [0; SAE_COMMIT_LEN + 7];
    body[..SAE_COMMIT_LEN].copy_from_slice(&peer);
    body[SAE_COMMIT_LEN..].copy_from_slice(&[255, 5, 92, 20, 0, 19, 0]);
    let received = SaeReceivedCommit::parse(&body, true).unwrap();
    assert_eq!(received.rejected_groups, Some(&[20, 0, 19, 0][..]));
    assert!(received.rejects_offered_group());

    let mut other_group = peer;
    other_group[0] = 20;
    assert_eq!(
        SaeReceivedCommit::parse(&other_group, false),
        Err(SaeCommitRefusal::UnsupportedGroup)
    );
    let mut zero_scalar = peer;
    zero_scalar[2..34].fill(0);
    assert_eq!(
        SaeReceivedCommit::parse(&zero_scalar, false),
        Err(SaeCommitRefusal::Unspecified)
    );
    let mut off_curve = peer;
    off_curve[SAE_COMMIT_LEN - 1] ^= 1;
    assert_eq!(
        SaeReceivedCommit::parse(&off_curve, false),
        Err(SaeCommitRefusal::Unspecified)
    );
    assert_eq!(
        SaeReceivedCommit::parse(&peer[..SAE_COMMIT_LEN - 1], false),
        Err(SaeCommitRefusal::Unspecified)
    );
}

#[test]
fn a_comeback_token_is_bound_to_its_station_and_used_once() {
    let station = [2, 0, 0, 0, 0, 7];
    let mut tokens = SaeComebackTokens::new();
    let token = tokens.issue(station, oer_time::Instant::from_micros(1_000), || {
        [0x42; 32]
    });
    assert_eq!(token[..2], [0, 1]);
    assert_eq!(
        tokens.issue(
            station,
            oer_time::Instant::from_micros(2_000),
            || unreachable!()
        ),
        token
    );
    assert!(!tokens.check([2, 0, 0, 0, 0, 8], &token));
    let mut forged = token;
    forged[31] ^= 1;
    assert!(!tokens.check(station, &forged));
    assert!(tokens.check(station, &token));
    assert!(!tokens.check(station, &token));

    let reissued = tokens.issue(station, oer_time::Instant::from_micros(70_000_000), || {
        [0x43; 32]
    });
    assert_ne!(reissued, token);
    assert!(tokens.check(station, &reissued));
}

#[test]
fn secret_owners_wipe_and_redact_debug() {
    fn wipes_on_drop<T: ZeroizeOnDrop>() {}
    wipes_on_drop::<SaePassword>();
    wipes_on_drop::<SaePasswordElement>();
    wipes_on_drop::<SaePasswordToken>();
    wipes_on_drop::<SaeKeys>();
    wipes_on_drop::<SaeComebackTokens>();

    let mut password = SaePassword::new(PASSWORD).unwrap();
    assert_eq!(std::format!("{password:?}"), "SaePassword(<redacted>)");
    password.zeroize();
    assert!(password.as_bytes().is_empty());
    assert!(password.bytes.is_empty());

    let mut token = SaePasswordToken::derive(b"byteme", PASSWORD, None).unwrap();
    assert_eq!(std::format!("{token:?}"), "SaePasswordToken(<redacted>)");
    let mut element = token.password_element(ADDR1, ADDR2);
    assert_eq!(
        std::format!("{element:?}"),
        "SaePasswordElement(<redacted>)"
    );
    token.zeroize();
    element.zeroize();
    assert!(bool::from(token.0.is_identity()));
    assert!(bool::from(element.0.is_identity()));

    let (mut keys, _) =
        peers(|a, b| SaePasswordElement::hunting_and_pecking(PASSWORD, a, b).unwrap());
    assert_eq!(std::format!("{keys:?}"), "SaeKeys(<redacted>)");
    keys.zeroize();
    assert_eq!(keys.kck(), &[0; SAE_KEY_LEN]);
    assert_eq!(keys.pmk(), &[0; SAE_KEY_LEN]);
    assert_eq!(keys.pmkid(), &[0; SAE_PMKID_LEN]);

    let mut comeback = SaeComebackTokens::new();
    comeback.issue(ADDR1, Instant::EPOCH, || [0x42; 32]);
    assert_eq!(
        std::format!("{comeback:?}"),
        "SaeComebackTokens(<redacted>)"
    );
    comeback.zeroize();
    assert_eq!(comeback.key, [0; 32]);
}

#[test]
fn a_wiped_password_element_cannot_panic_when_reused_for_a_commit() {
    let mut pwe = SaePasswordToken::derive(b"byteme", PASSWORD, None)
        .unwrap()
        .password_element(ADDR1, ADDR2);
    pwe.zeroize();
    assert!(matches!(
        SaeCommit::new(&pwe, [0x11; 32], [0x22; 32]),
        Err(SaeError::InfiniteSecret)
    ));
}

#[test]
fn sae_password_lengths_are_admitted_before_copying_and_wiping_keeps_a_valid_empty_view() {
    assert!(SaePassword::new(&[]).is_none());
    assert!(SaePassword::new(&[0xff; SAE_PASSWORD_MAX_LEN + 1]).is_none());
    for length in [1, SAE_PASSWORD_MAX_LEN] {
        let bytes = [0xff; SAE_PASSWORD_MAX_LEN];
        let mut password = SaePassword::new(&bytes[..length]).unwrap();
        assert_eq!(password.as_bytes(), &bytes[..length]);
        password.zeroize();
        assert!(password.as_bytes().is_empty());
    }
}

#[test]
fn failed_h2e_maps_return_crypto_errors_after_evaluating_both_elements() {
    for failed_call in [1, 2] {
        let mut calls = 0;
        let result = SaePasswordToken::derive_with_map(b"ssid", PASSWORD, None, |_| {
            calls += 1;
            CtOption::new(
                ProjectivePoint::GENERATOR,
                Choice::from(u8::from(calls != failed_call)),
            )
        });
        assert!(matches!(result, Err(SaeError::Crypto)));
        assert_eq!(calls, 2);
    }
}

#[test]
fn invalid_curve_coordinates_keep_their_validity_bit() {
    let invalid = point(&FieldElement::ZERO, &FieldElement::ZERO);
    assert!(bool::from(invalid.is_none()));
    assert!(peer_element(&[0; 2 * SAE_PRIME_LEN]).is_none());
    let pwe = SaePasswordToken::derive(b"ssid", PASSWORD, None)
        .unwrap()
        .password_element(ADDR1, ADDR2);
    let encoded = element_bytes(&pwe.0.to_affine()).unwrap();
    assert!(peer_element(&encoded).is_some());
    let mut changed = encoded;
    changed[2 * SAE_PRIME_LEN - 1] ^= 2;
    assert!(peer_element(&changed).is_none());
    for value in [
        FieldElement::ZERO,
        FieldElement::ONE,
        FieldElement::from_u64(10),
    ] {
        assert!(bool::from(sswu(&value).is_some()));
    }
}

#[test]
fn every_comeback_slot_is_updated_without_affecting_another_slot() {
    let mut tokens = SaeComebackTokens::new();
    for slot in 0_u8..=u8::MAX {
        tokens.set_pending(slot, u16::from(slot) + 1);
    }
    for (pending, slot) in tokens.pending.iter().zip(0_u8..=u8::MAX) {
        assert_eq!(*pending, u16::from(slot) + 1);
    }
    tokens.set_pending(0, 0);
    tokens.set_pending(255, 0);
    assert_eq!(tokens.pending[0], 0);
    assert_eq!(tokens.pending[255], 0);
    assert_eq!(tokens.pending[1], 2);
    assert_eq!(tokens.pending[254], 255);
}

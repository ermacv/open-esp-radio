use super::*;
use oer_ieee80211_mac::security::SaePwe;

use oer_ieee80211_rsn::sae::SaePasswordElement;
use std::vec::Vec;

const LOCAL: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
const BSSID: [u8; 6] = [0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];
const PASSWORD: &[u8] = b"correct horse";

fn commit(local: [u8; 6], peer: [u8; 6], seed: u8) -> SaeCommit {
    SaeCommit::new(
        SaePasswordElement::hunting_and_pecking(PASSWORD, local, peer).unwrap(),
        [seed; 32],
        [seed + 1; 32],
    )
    .unwrap()
}

/// One Authentication frame from the access point.
fn from_access_point(transaction: u16, status: u16, body: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 30];
    frame[0] = 0xb0;
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&3_u16.to_le_bytes());
    frame[26..28].copy_from_slice(&transaction.to_le_bytes());
    frame[28..30].copy_from_slice(&status.to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

fn access_point_commit(access_point: &SaeCommit) -> Vec<u8> {
    let mut body = [0; SAE_COMMIT_LEN];
    access_point
        .values()
        .encode(None, false, &mut body)
        .unwrap();
    from_access_point(1, 0, &body)
}

fn start(now: Instant) -> (StaSaeAuthentication, StaSaeTransmission) {
    StaSaeAuthentication::new(
        OwnAddress::new(oer_ieee80211_mac::address::IndividualAddress::new(LOCAL).unwrap()),
        Bssid::new(oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap()).unwrap(),
        commit(LOCAL, BSSID, 0x20),
        SaePwe::HuntingAndPecking,
        now,
    )
    .unwrap()
}

#[test]
fn a_full_exchange_yields_the_access_point_pmk() {
    let access_point = commit(BSSID, LOCAL, 0x40);
    let (station, sent) = start(Instant::EPOCH);
    assert_eq!((sent.transaction, sent.status_code), (1, 0));
    let station_values = SaeCommitValues::parse(sent.body(), false).unwrap();
    let StaSaeEvent::Transmit {
        authentication: station,
        transmission: confirm,
    } = station.observe_management_frame(&access_point_commit(&access_point), Instant::EPOCH)
    else {
        panic!("station confirms after peer commit");
    };
    assert_eq!((confirm.transaction, confirm.status_code), (2, 0));
    let peer = access_point.process(station_values).unwrap();
    assert_eq!(peer.verify_peer_confirm(confirm.body()), Ok(1));
    let StaSaeEvent::Authenticated(keys) = station.observe_management_frame(
        &from_access_point(2, 0, &peer.own_confirm(1)),
        Instant::EPOCH,
    ) else {
        panic!("authenticated");
    };
    assert_eq!(keys.pmkid, peer.pmkid);
    assert!(
        keys.pmk
            .bind_association_security_ies(b"sae")
            .matches(&Pmk::from_bytes(peer.pmk), b"sae")
    );
}

#[test]
fn an_anti_clogging_refusal_repeats_the_commit_with_the_token() {
    let (station, first) = start(Instant::EPOCH);
    let deadline = station.next_deadline();
    let StaSaeEvent::Transmit {
        authentication,
        transmission: again,
    } = station
        .observe_management_frame(&from_access_point(1, 76, &[19, 0, 7, 8, 9]), Instant::EPOCH)
    else {
        panic!("station repeats its commit");
    };
    assert_eq!(&again.body()[2..5], &[7, 8, 9]);
    assert_eq!(&again.body()[5..], &first.body()[2..]);
    assert_eq!(
        authentication.next_deadline(),
        deadline,
        "repeated tokens cannot extend the bound"
    );
}

#[test]
fn the_timer_is_four_seconds_then_two_after_the_confirm() {
    let now = Instant::from_micros(1000);
    let (station, _) = start(now);
    let deadline = now.checked_add(STA_SAE_COMMIT_TIMEOUT).unwrap();
    assert_eq!(station.next_deadline(), deadline);
    let confirmed = deadline.checked_sub(Duration::from_micros(1)).unwrap();
    let StaSaeEvent::Irrelevant(station) = station.on_deadline(confirmed) else {
        panic!("not due")
    };
    let access_point = commit(BSSID, LOCAL, 0x40);
    let StaSaeEvent::Transmit {
        authentication: station,
        ..
    } = station.observe_management_frame(&access_point_commit(&access_point), confirmed)
    else {
        panic!("confirm")
    };
    let confirm_deadline = confirmed.checked_add(STA_SAE_CONFIRM_TIMEOUT).unwrap();
    assert_eq!(station.next_deadline(), confirm_deadline);
    let StaSaeEvent::Irrelevant(station) = station.on_deadline(deadline) else {
        panic!("not due")
    };
    assert!(matches!(
        station.on_deadline(confirm_deadline),
        StaSaeEvent::Failed(StaSaeFailure::Timeout)
    ));
    assert_eq!(STA_SAE_COMMIT_TIMEOUT, Duration::from_secs(4));
    assert_eq!(STA_SAE_CONFIRM_TIMEOUT, Duration::from_secs(2));
}

#[test]
fn a_wrong_password_or_a_refusal_fails() {
    let (station, sent) = start(Instant::EPOCH);
    let impostor = SaeCommit::new(
        SaePasswordElement::hunting_and_pecking(b"wrong", BSSID, LOCAL).unwrap(),
        [0x40; 32],
        [0x41; 32],
    )
    .unwrap();
    let values = SaeCommitValues::parse(sent.body(), false).unwrap();
    let StaSaeEvent::Transmit {
        authentication: station,
        ..
    } = station.observe_management_frame(&access_point_commit(&impostor), Instant::EPOCH)
    else {
        panic!("confirm")
    };
    let peer = impostor.process(values).unwrap();
    assert!(matches!(
        station.observe_management_frame(
            &from_access_point(2, 0, &peer.own_confirm(1)),
            Instant::EPOCH
        ),
        StaSaeEvent::Failed(StaSaeFailure::Protocol(SaeError::ConfirmMismatch))
    ));
    let (station, _) = start(Instant::EPOCH);
    assert!(matches!(
        station.observe_management_frame(&from_access_point(1, 77, &[20, 0]), Instant::EPOCH),
        StaSaeEvent::Failed(StaSaeFailure::Rejected { status_code: 77 })
    ));
    let (station, _) = start(Instant::EPOCH);
    assert!(matches!(
        station.observe_management_frame(&from_access_point(1, 126, &[]), Instant::EPOCH),
        StaSaeEvent::Failed(StaSaeFailure::Protocol(SaeError::Malformed))
    ));
}

#[test]
fn deadline_overflow_refuses_before_issuing_a_commit() {
    let result = StaSaeAuthentication::new(
        OwnAddress::new(oer_ieee80211_mac::address::IndividualAddress::new(LOCAL).unwrap()),
        Bssid::new(oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap()).unwrap(),
        commit(LOCAL, BSSID, 0x20),
        SaePwe::HuntingAndPecking,
        Instant::from_micros(u64::MAX),
    );
    assert!(matches!(result, Err(StaSaeFailure::DeadlineOverflow)));
}

#[test]
fn confirmation_deadline_overflow_keeps_no_live_exchange() {
    let (station, _) = start(Instant::from_micros(u64::MAX - 4_000_000));
    let peer = commit(BSSID, LOCAL, 0x40);
    assert!(matches!(
        station.observe_management_frame(
            &access_point_commit(&peer),
            Instant::from_micros(u64::MAX - 1_000_000)
        ),
        StaSaeEvent::Failed(StaSaeFailure::DeadlineOverflow)
    ));
}

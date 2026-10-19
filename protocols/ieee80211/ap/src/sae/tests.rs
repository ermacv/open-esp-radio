use super::*;

use oer_ieee80211_rsn::sae::anti_clogging_token;

const ACCESS_POINT: [u8; 6] = [2, 0, 0, 0, 0, 1];
const STATION: [u8; 6] = [2, 0, 0, 0, 0, 2];
const SSID: &[u8] = b"open-radio-ap";
const PASSWORD: &[u8] = b"correct horse battery";

struct Counter(u8);

impl ApSaeRandom for Counter {
    fn fill(&mut self, bytes: &mut [u8]) {
        for byte in bytes.iter_mut() {
            self.0 = self.0.wrapping_add(0x3b);
            *byte = self.0 | 0x10;
        }
        bytes[0] = 0x11;
    }
}

fn frame(peer: [u8; 6], transaction: u16, status: u16, body: &[u8]) -> ApSaeFrame<'_> {
    ApSaeFrame {
        peer,
        transaction,
        status,
        body,
    }
}

fn responder() -> ApSaeResponder {
    ApSaeResponder::new(
        ACCESS_POINT,
        ApSaeCredential::derive(SSID, SaePassword::new(PASSWORD).unwrap()),
    )
}

/// The station side of one exchange, built from the same primitives the
/// station role uses.
struct Station {
    commit: SaeCommit,
    h2e: bool,
}

impl Station {
    fn new(station: [u8; 6], password: &[u8], h2e: bool) -> Self {
        let pwe = if h2e {
            SaePasswordToken::derive(SSID, password, None).password_element(station, ACCESS_POINT)
        } else {
            SaePasswordElement::hunting_and_pecking(password, station, ACCESS_POINT).unwrap()
        };
        Self {
            commit: SaeCommit::new(pwe, [0x21; 32], [0x43; 32]).unwrap(),
            h2e,
        }
    }

    fn status(&self) -> u16 {
        if self.h2e {
            AP_SAE_STATUS_HASH_TO_ELEMENT
        } else {
            AP_SAE_STATUS_SUCCESS
        }
    }

    fn commit_body(&self, token: Option<&[u8]>) -> ([u8; 160], usize) {
        let mut body = [0; 160];
        let len = self
            .commit
            .values()
            .encode(token, self.h2e, &mut body)
            .unwrap();
        (body, len)
    }
}

fn only_reply(output: &ApSaeOutput) -> ApSaeReply {
    let mut replies = output.replies();
    let reply = *replies.next().expect("one reply");
    assert!(replies.next().is_none());
    reply
}

fn exchange(h2e: bool) {
    let mut access_point = responder();
    let mut random = Counter(0);
    let station = Station::new(STATION, PASSWORD, h2e);
    let (commit, len) = station.commit_body(None);

    let output = access_point.receive(
        frame(STATION, 1, station.status(), &commit[..len]),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    assert_eq!(output.result, ApSaeResult::Continue);
    let reply = only_reply(&output);
    assert_eq!((reply.transaction, reply.status), (1, station.status()));
    let access_point_commit = SaeCommitValues::parse(reply.body(), h2e).unwrap();
    let keys = station.commit.process(access_point_commit).unwrap();

    let confirm = keys.own_confirm(1);
    let output = access_point.receive(
        frame(STATION, 2, 0, &confirm),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    let ApSaeResult::Accepted { pmk, pmkid } = output.result else {
        panic!("the confirm is accepted: {output:?}");
    };
    assert_eq!(pmk, keys.pmk);
    assert_eq!(pmkid, keys.pmkid);
    let reply = only_reply(&output);
    assert_eq!((reply.transaction, reply.status), (2, 0));
    assert_eq!(keys.verify_peer_confirm(reply.body()), Ok(1));

    // A repeated Confirm is ignored but the stored Confirm is sent again.
    let output = access_point.receive(
        frame(STATION, 2, 0, &confirm),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    assert_eq!(output.result, ApSaeResult::Continue);
    assert_eq!(only_reply(&output).body(), reply.body());
}

#[test]
fn a_hunting_and_pecking_exchange_is_accepted_with_the_stations_pmk() {
    exchange(false);
}

#[test]
fn a_hash_to_element_exchange_is_accepted_with_the_stations_pmk() {
    exchange(true);
}

#[test]
fn a_wrong_password_fails_the_confirm() {
    let mut access_point = responder();
    let mut random = Counter(7);
    let station = Station::new(STATION, b"wrong password", false);
    let (commit, len) = station.commit_body(None);
    let output = access_point.receive(
        frame(STATION, 1, 0, &commit[..len]),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    let access_point_commit = SaeCommitValues::parse(only_reply(&output).body(), false).unwrap();
    let keys = station.commit.process(access_point_commit).unwrap();

    let output = access_point.receive(
        frame(STATION, 2, 0, &keys.own_confirm(1)),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    assert_eq!(
        output.result,
        ApSaeResult::Failed {
            status: AP_SAE_STATUS_CHALLENGE_FAILURE
        }
    );
    assert_eq!(only_reply(&output).status, AP_SAE_STATUS_CHALLENGE_FAILURE);
}

#[test]
fn two_open_sessions_require_an_anti_clogging_token() {
    let mut access_point = responder();
    let mut random = Counter(3);
    for peer in [[2, 0, 0, 0, 0, 3], [2, 0, 0, 0, 0, 4]] {
        let station = Station::new(peer, PASSWORD, false);
        let (commit, len) = station.commit_body(None);
        let output = access_point.receive(
            frame(peer, 1, 0, &commit[..len]),
            oer_time::Instant::from_micros(0),
            0,
            &mut random,
        );
        assert_eq!(only_reply(&output).status, AP_SAE_STATUS_SUCCESS);
    }

    let station = Station::new(STATION, PASSWORD, false);
    let (commit, len) = station.commit_body(None);
    let output = access_point.receive(
        frame(STATION, 1, 0, &commit[..len]),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    let request = only_reply(&output);
    assert_eq!(request.status, AP_SAE_STATUS_ANTI_CLOGGING_TOKEN_REQUIRED);
    assert_eq!(output.result, ApSaeResult::Continue);
    let token = anti_clogging_token(request.body(), false).unwrap();

    let (commit, len) = station.commit_body(Some(token));
    let output = access_point.receive(
        frame(STATION, 1, 0, &commit[..len]),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    assert_eq!(only_reply(&output).status, AP_SAE_STATUS_SUCCESS);

    // A token is valid once.
    let output = access_point.receive(
        frame([2, 0, 0, 0, 0, 5], 1, 0, &commit[..len]),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    assert_eq!(output.replies().count(), 0);
    assert_eq!(
        output.result,
        ApSaeResult::Failed {
            status: AP_SAE_STATUS_UNSPECIFIED_FAILURE
        }
    );
}

#[test]
fn queued_commits_count_toward_anti_clogging() {
    let mut access_point = responder();
    let station = Station::new(STATION, PASSWORD, true);
    let (commit, len) = station.commit_body(None);
    let output = access_point.receive(
        frame(STATION, 1, AP_SAE_STATUS_HASH_TO_ELEMENT, &commit[..len]),
        oer_time::Instant::from_micros(0),
        2,
        &mut Counter(1),
    );
    let request = only_reply(&output);
    assert_eq!(request.status, AP_SAE_STATUS_ANTI_CLOGGING_TOKEN_REQUIRED);
    assert_eq!(anti_clogging_token(request.body(), true).unwrap().len(), 32);
}

#[test]
fn a_commit_of_another_group_is_refused_with_its_group() {
    let mut access_point = responder();
    let station = Station::new(STATION, PASSWORD, false);
    let (mut commit, len) = station.commit_body(None);
    commit[0] = 20;
    let output = access_point.receive(
        frame(STATION, 1, 0, &commit[..len]),
        oer_time::Instant::from_micros(0),
        0,
        &mut Counter(2),
    );
    let reply = only_reply(&output);
    assert_eq!(reply.status, AP_SAE_STATUS_UNSUPPORTED_GROUP);
    assert_eq!(reply.body(), [20, 0]);
    assert_eq!(
        output.result,
        ApSaeResult::Failed {
            status: AP_SAE_STATUS_UNSUPPORTED_GROUP
        }
    );
}

#[test]
fn a_reflected_commit_is_dropped() {
    let mut access_point = responder();
    let mut random = Counter(9);
    let station = Station::new(STATION, PASSWORD, false);
    let (commit, len) = station.commit_body(None);
    let output = access_point.receive(
        frame(STATION, 1, 0, &commit[..len]),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    let reflected = only_reply(&output);

    let output = access_point.receive(
        frame(STATION, 1, 0, reflected.body()),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    assert_eq!(output, ApSaeOutput::new());
}

#[test]
fn a_full_responder_refuses_a_new_station_and_forgets_a_removed_one() {
    let mut access_point = responder();
    let mut random = Counter(4);
    for index in 0..AP_MAX_CLIENTS as u8 {
        let peer = [2, 0, 0, 0, 1, index];
        access_point.receive(
            frame(peer, 1, 7, &[]),
            oer_time::Instant::from_micros(0),
            0,
            &mut random,
        );
    }
    let station = Station::new(STATION, PASSWORD, false);
    let (commit, len) = station.commit_body(None);
    let output = access_point.receive(
        frame(STATION, 1, 0, &commit[..len]),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    assert_eq!(
        only_reply(&output).status,
        AP_SAE_STATUS_UNABLE_TO_HANDLE_NEW_STA
    );

    access_point.forget([2, 0, 0, 0, 1, 0]);
    let output = access_point.receive(
        frame(STATION, 1, 0, &commit[..len]),
        oer_time::Instant::from_micros(0),
        0,
        &mut random,
    );
    assert_eq!(only_reply(&output).status, AP_SAE_STATUS_SUCCESS);
}

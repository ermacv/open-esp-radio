use super::*;
use oer_hil_link::test_support::received;

/// `body` as the host receives it.
fn message<M: oer_hil_protocol::Message>(body: M) -> Received {
    received(oer_hil_protocol::Envelope::new(7, 0, 0, 1, body))
}
use oer_hil_protocol::{
    wifi::WifiAccessPointEvidence, wifi::WifiAirtimePeer, wifi::WifiAirtimeReport,
};

fn peer() -> WifiAirtimePeerEvidence {
    WifiAirtimePeerEvidence {
        peer: WifiAirtimePeer::Group,
        grants: 3,
        granted_micros: 3000,
        settlements: 2,
        settled_grants_micros: 2000,
        charged_micros: 2500,
        cancellations: 1,
        cancelled_grants_micros: 1000,
        balance_after_last_event_micros: -500,
        outstanding: 0,
        outstanding_micros: 0,
        maximum_outstanding: 2,
    }
}
fn report() -> Received {
    message(oer_hil_protocol::wifi::AirtimeReport(WifiAirtimeReport {
        peer_records: 1,
        ..Default::default()
    }))
}
fn stopped() -> Received {
    message(oer_hil_protocol::wifi::AccessPointStopped(
        WifiAccessPointEvidence::default(),
    ))
}

#[test]
fn complete_accounting_accepts_debt_but_requires_every_reservation_closed() {
    assert!(
        validate(
            [
                message(oer_hil_protocol::wifi::AirtimePeer(peer())),
                report(),
                stopped()
            ]
            .iter()
        )
        .is_ok()
    );
    let pending = WifiAirtimePeerEvidence {
        settlements: 1,
        settled_grants_micros: 1000,
        outstanding: 1,
        outstanding_micros: 1000,
        ..peer()
    };
    assert!(
        validate(
            [
                message(oer_hil_protocol::wifi::AirtimePeer(pending)),
                report(),
                stopped()
            ]
            .iter()
        )
        .is_err()
    );
}

#[test]
fn missing_duplicate_late_and_inconsistent_evidence_is_rejected() {
    assert!(validate([stopped()].iter()).is_err());
    assert!(
        validate(
            [
                message(oer_hil_protocol::wifi::AirtimePeer(peer())),
                stopped(),
                report()
            ]
            .iter()
        )
        .is_err()
    );
    assert!(
        validate(
            [
                message(oer_hil_protocol::wifi::AirtimePeer(peer())),
                message(oer_hil_protocol::wifi::AirtimePeer(peer())),
                report(),
                stopped()
            ]
            .iter()
        )
        .is_err()
    );
    let wrong = WifiAirtimePeerEvidence {
        granted_micros: 3001,
        ..peer()
    };
    assert!(
        validate(
            [
                message(oer_hil_protocol::wifi::AirtimePeer(wrong)),
                report(),
                stopped()
            ]
            .iter()
        )
        .is_err()
    );
    for incomplete in [
        WifiAirtimeReport {
            peer_records: 2,
            ..Default::default()
        },
        WifiAirtimeReport {
            peer_records: 1,
            saturated: true,
            ..Default::default()
        },
        WifiAirtimeReport {
            peer_records: 1,
            dropped_events: 1,
            ..Default::default()
        },
    ] {
        assert!(
            validate(
                [
                    message(oer_hil_protocol::wifi::AirtimePeer(peer())),
                    message(oer_hil_protocol::wifi::AirtimeReport(incomplete)),
                    stopped()
                ]
                .iter()
            )
            .is_err()
        );
    }
}

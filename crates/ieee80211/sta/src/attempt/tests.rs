use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_mac::station::StaTxSequenceCounters;

use super::*;

static PMKSA: StaSharedPmksa = StaSharedPmksa::new();

fn counters() -> StaTxSequenceCounters {
    StaTxSequenceCounters::new(SequenceNumber::new(0).unwrap())
}

fn personal() -> StaAttemptSecurity<'static> {
    StaAttemptSecurity::new(
        StaPersonalCredentials::wpa2(
            Pmk::from_bytes([1; 32]),
            SaePassword::new(b"password").unwrap(),
            || 7,
            &PMKSA,
        ),
        [3; 32],
        counters(),
        Wpa2Message4Protection::Unprotected,
    )
}

/// Whether two PMKs hold the same key.
fn same(pmk: &Pmk, bytes: [u8; 32]) -> bool {
    pmk.bind_association_security_ies(b"ie")
        .matches(&Pmk::from_bytes(bytes), b"ie")
}

#[test]
fn progress_records_each_completed_stage() {
    let mut progress = StaAttemptProgress::default();
    progress.mark_completed(StaAttemptStage::Candidate);
    progress.mark_completed(StaAttemptStage::Association);
    progress.mark_completed(StaAttemptStage::Association);
    assert_eq!(progress.completed_count(), 2);
    assert!(progress.completed(StaAttemptStage::Association));
    assert!(!progress.completed(StaAttemptStage::Authentication));
    assert_eq!(
        StaAttemptStage::RsnKeyInstall.lifecycle_stage(),
        StaLifecycleStage::Security
    );
}

#[test]
fn an_open_attempt_has_no_handshake_material() {
    let mut security = StaAttemptSecurity::open(counters());
    assert_eq!(security.policy(), StaSecurityPolicy::Open);
    assert!(security.wpa2_material().is_none());
    assert!(security.wpa2_handshake_parts().is_none());
    assert!(!security.has_connected_wpa2());
}

#[test]
fn the_sae_pmk_of_the_attempt_replaces_the_psk() {
    let mut security = personal();
    let (pmk, nonce, protection) = security.wpa2_material().unwrap();
    assert!(same(pmk, [1; 32]));
    assert_eq!(nonce, [3; 32]);
    assert_eq!(protection, Wpa2Message4Protection::Unprotected);
    security.set_sae_pmk(Some(Pmk::from_bytes([2; 32])));
    assert!(same(security.wpa2_material().unwrap().0, [2; 32]));
    security.set_sae_pmk(None);
    assert!(same(security.wpa2_material().unwrap().0, [1; 32]));
}

use super::*;

#[test]
fn retirement_returns_the_table_without_secrets_and_preserves_its_frontier() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = wpa3_service(&mut storage);
    assert_eq!(
        service
            .authenticate_sae(
                PEER,
                Pmk::from_bytes(SAE_PMK),
                SAE_PMKID,
                oer_time::Instant::from_micros(1)
            )
            .unwrap(),
        AP_STATUS_SUCCESS,
    );
    let old = service.peer_status(PEER).unwrap().association_identity();
    assert!(service.storage().pmksa.find(PEER, SAE_PMKID).is_some());
    let peers = service.into_peer_storage();
    assert!(peers.storage.peers.iter().all(Option::is_none));
    assert!(peers.storage.pmksa.find(PEER, SAE_PMKID).is_none());
    assert_eq!(peers.storage.generation, old.association_epoch());

    let mut restarted =
        AccessPointService::<oer_ieee80211_mac::security::spp::Association>::new_open(
            AP,
            AccessPointClientLimit::new(2).unwrap(),
            AccessPointInactiveTimeout::default(),
            peers,
        );
    assert!(restarted.peer_status(PEER).is_none());
    restarted.authenticate_open(PEER, oer_time::Instant::from_micros(2));
    let new = restarted.peer_status(PEER).unwrap().association_identity();
    assert_eq!(new.association_id(), old.association_id());
    assert!(new.association_epoch() > old.association_epoch());
    assert!(!restarted.association_is_current(old));
}

#[test]
fn dropping_an_active_service_clears_its_caller_placed_secrets() {
    let mut storage = AccessPointPeerStorage::new();
    let old_generation = {
        let mut service = wpa3_service(&mut storage);
        assert_eq!(
            service
                .authenticate_sae(
                    PEER,
                    Pmk::from_bytes(SAE_PMK),
                    SAE_PMKID,
                    oer_time::Instant::from_micros(1)
                )
                .unwrap(),
            AP_STATUS_SUCCESS,
        );
        assert!(service.storage().pmksa.find(PEER, SAE_PMKID).is_some());
        service.storage().generation
    };
    assert!(storage.peers.iter().all(Option::is_none));
    assert!(storage.pmksa.find(PEER, SAE_PMKID).is_none());
    assert_eq!(storage.generation, old_generation);
}

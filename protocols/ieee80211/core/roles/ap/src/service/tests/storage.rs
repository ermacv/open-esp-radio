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
        status_code::SUCCESS,
    );
    let old = service.peer_status(PEER).unwrap().association_identity();
    assert!(service.storage().pmksa.find(PEER, SAE_PMKID).is_some());
    let peers = service.into_peer_storage();
    assert!(peers.storage.peers.iter().all(Option::is_none));
    assert!(peers.storage.pmksa.find(PEER, SAE_PMKID).is_none());
    assert_eq!(peers.storage.generation, old.association_epoch());

    let mut restarted =
        AccessPointService::<oer_ieee80211_wire::security::spp::Association>::new_open(
            AP,
            AccessPointClientLimit::new(2).unwrap(),
            AccessPointInactiveTimeout::default(),
            peers,
        )
        .unwrap();
    assert!(restarted.peer_status(PEER).is_none());
    restarted
        .authenticate_open(PEER, oer_time::Instant::from_micros(2))
        .unwrap();
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
            status_code::SUCCESS,
        );
        assert!(service.storage().pmksa.find(PEER, SAE_PMKID).is_some());
        service.storage().generation
    };
    assert!(storage.peers.iter().all(Option::is_none));
    assert!(storage.pmksa.find(PEER, SAE_PMKID).is_none());
    assert_eq!(storage.generation, old_generation);
}

#[test]
fn exhausted_storage_refuses_every_security_constructor_without_reusing_its_frontier() {
    for policy in [
        ApSecurityPolicy::Open,
        ApSecurityPolicy::Wpa2Personal,
        ApSecurityPolicy::Wpa3Personal,
    ] {
        let mut storage = AccessPointPeerStorage::new();
        storage.generation = u32::MAX;
        let limit = AccessPointClientLimit::new(2).unwrap();
        let timeout = AccessPointInactiveTimeout::default();
        let gtk = RsnGtk::new(
            oer_ieee80211_wire::ccmp::GroupKeyId::new(1).unwrap(),
            true,
            &[0x55; 16],
        );
        let result = match policy {
            ApSecurityPolicy::Open => AccessPointService::<
                oer_ieee80211_wire::security::spp::Association,
            >::new_open(AP, limit, timeout, storage.lend()),
            ApSecurityPolicy::Wpa2Personal => AccessPointService::new(
                AP,
                Pmk::from_bytes([1; 32]),
                gtk,
                limit,
                timeout,
                storage.lend(),
            ),
            ApSecurityPolicy::Wpa3Personal => AccessPointService::new_wpa3(
                AP,
                gtk,
                RsnIgtk::new(4, [0; 6], &[2; 16]).unwrap(),
                limit,
                timeout,
                storage.lend(),
            ),
        };
        assert_eq!(result.err(), Some(ApServiceError::GenerationExhausted));
        assert_eq!(storage.generation, u32::MAX);
    }
}

#[test]
fn the_last_generation_is_usable_and_exhaustion_preserves_the_live_peer_and_pmksa() {
    let mut storage = AccessPointPeerStorage::new();
    storage.generation = u32::MAX - 2;
    let mut service = wpa3_service(&mut storage);
    let now = oer_time::Instant::from_micros(1);
    assert_eq!(
        service.authenticate_sae(PEER, Pmk::from_bytes(SAE_PMK), SAE_PMKID, now),
        Ok(status_code::SUCCESS)
    );
    let binding = service.bind_peer(PEER).unwrap();
    let status = service.peer_status(PEER).unwrap();
    assert_eq!(status.association_epoch, u32::MAX);
    let revision = service.status_revision();
    service.storage_mut().management_replay[0].activate();
    for peer in [PEER, OTHER] {
        assert_eq!(
            service.authenticate_open(peer, now),
            Err(ApServiceError::GenerationExhausted)
        );
        assert_eq!(
            service.authenticate_sae(peer, Pmk::from_bytes([0xaa; 32]), [0xbb; 16], now),
            Err(ApServiceError::GenerationExhausted)
        );
    }
    assert_eq!(
        service.remove_peer(PEER),
        Err(ApServiceError::GenerationExhausted)
    );
    assert_eq!(service.bound_peer_status(binding), Some(status));
    assert_eq!(service.status_revision(), revision);
    assert_eq!(service.storage().generation, u32::MAX);
    assert!(service.storage().management_replay[0].active());
    assert!(service.storage().pmksa.find(PEER, SAE_PMKID).is_some());
    assert!(service.storage().pmksa.find(PEER, [0xbb; 16]).is_none());
    assert!(service.peer_status(OTHER).is_none());
}

#[test]
fn removing_the_last_generation_keeps_the_frontier_exhausted_across_restart() {
    let mut storage = AccessPointPeerStorage::new();
    let mut service = wpa3_service(&mut storage);
    service
        .authenticate_open(PEER, oer_time::Instant::from_micros(0))
        .unwrap();
    service.storage_mut().generation = u32::MAX - 1;
    assert_eq!(
        service.remove_peer(PEER),
        Ok(ApMlmeAction::PeerRemoved { peer: PEER })
    );
    let peers = service.into_peer_storage();
    let result = AccessPointService::<oer_ieee80211_wire::security::spp::Association>::new_open(
        AP,
        AccessPointClientLimit::new(1).unwrap(),
        AccessPointInactiveTimeout::default(),
        peers,
    );
    assert_eq!(result.err(), Some(ApServiceError::GenerationExhausted));
    assert_eq!(storage.generation, u32::MAX);
}

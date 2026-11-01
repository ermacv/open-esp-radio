#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    clippy::unreachable
)]

use super::*;
use oer_ieee80211_wire::{address::Bssid, management::MAX_SSID_LEN};

type ApCache = PmksaCache<[u8; 6]>;
type StaEntry = Pmksa<Bssid, heapless::Vec<u8, MAX_SSID_LEN>>;
type StaCache = PmksaCache<Bssid, heapless::Vec<u8, MAX_SSID_LEN>>;

// The access point's cache: per station, selected by PMKID.

fn entry(last: u8) -> Pmksa<[u8; 6]> {
    Pmksa::new(
        [2, 0, 0, 0, 0, last],
        (),
        Pmk::from_bytes([last; 32]),
        [last; RSN_PMKID_LEN],
    )
}

#[test]
fn a_station_resumes_only_with_its_own_pmkid() {
    let mut cache = ApCache::new();
    cache.insert(entry(1));
    cache.insert(entry(2));
    assert_eq!(
        cache
            .find([2, 0, 0, 0, 0, 1], [1; RSN_PMKID_LEN])
            .map(Pmksa::pmkid),
        Some([1; RSN_PMKID_LEN])
    );
    assert!(cache.find([2, 0, 0, 0, 0, 1], [2; RSN_PMKID_LEN]).is_none());
    assert!(cache.find([2, 0, 0, 0, 0, 3], [1; RSN_PMKID_LEN]).is_none());
}

#[test]
fn a_new_entry_replaces_the_stations_old_one_and_a_full_cache_drops_the_oldest() {
    let mut cache = ApCache::new();
    for last in 0..PMKSA_CAPACITY as u8 {
        cache.insert(entry(last));
    }
    cache.insert(Pmksa::new(
        [2, 0, 0, 0, 0, 0],
        (),
        Pmk::from_bytes([0xaa; 32]),
        [0xaa; RSN_PMKID_LEN],
    ));
    assert!(cache.find([2, 0, 0, 0, 0, 0], [0; RSN_PMKID_LEN]).is_none());
    assert!(
        cache
            .find([2, 0, 0, 0, 0, 0], [0xaa; RSN_PMKID_LEN])
            .is_some()
    );

    // Station 1 is now the oldest.
    cache.insert(entry(0x20));
    assert!(cache.find([2, 0, 0, 0, 0, 1], [1; RSN_PMKID_LEN]).is_none());
    assert!(cache.find([2, 0, 0, 0, 0, 2], [2; RSN_PMKID_LEN]).is_some());

    cache.remove([2, 0, 0, 0, 0, 2]);
    assert!(cache.find([2, 0, 0, 0, 0, 2], [2; RSN_PMKID_LEN]).is_none());
}

#[test]
fn removing_any_entry_preserves_fifo_order_and_the_next_free_slot() {
    for removed in 0..PMKSA_CAPACITY as u8 {
        let mut cache = ApCache::new();
        for last in 0..PMKSA_CAPACITY as u8 {
            cache.insert(entry(last));
        }
        cache.remove([2, 0, 0, 0, 0, removed]);
        cache.insert(entry(0x20));
        let actual: std::vec::Vec<_> = cache
            .entries
            .iter()
            .flatten()
            .map(|entry| entry.peer[5])
            .collect();
        let expected: std::vec::Vec<_> = (0..PMKSA_CAPACITY as u8)
            .filter(|last| *last != removed)
            .chain([0x20])
            .collect();
        assert_eq!(actual, expected);
        cache.insert(entry(0x21));
        let actual: std::vec::Vec<_> = cache
            .entries
            .iter()
            .flatten()
            .map(|entry| entry.peer[5])
            .collect();
        let expected: std::vec::Vec<_> = expected.into_iter().skip(1).chain([0x21]).collect();
        assert_eq!(actual, expected);
    }
}

// The station's cache: per access point and SSID.

fn sta_entry(last: u8, ssid: &[u8]) -> StaEntry {
    Pmksa::new(
        bssid([2, 0, 0, 0, 0, last]),
        heapless::Vec::from_slice(ssid).unwrap(),
        Pmk::from_bytes([last; 32]),
        [last; RSN_PMKID_LEN],
    )
}

#[test]
fn an_association_resumes_only_for_its_access_point_and_ssid() {
    let mut cache = StaCache::new();
    cache.insert(sta_entry(1, b"lab"));
    assert_eq!(
        cache
            .resume(bssid([2, 0, 0, 0, 0, 1]), b"lab")
            .map(Pmksa::pmkid),
        Some([1; RSN_PMKID_LEN])
    );
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 2]), b"lab").is_none());
    // The same access point under another SSID flushes its entry.
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 1]), b"other").is_none());
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 1]), b"lab").is_none());
}

#[test]
fn a_full_cache_drops_its_oldest_entry_and_replacing_refreshes_age() {
    let mut cache = StaCache::new();
    for last in 0..PMKSA_CAPACITY as u8 {
        cache.insert(sta_entry(last, b"lab"));
    }
    // Re-adding the oldest makes it the newest.
    cache.insert(sta_entry(0, b"lab"));
    cache.insert(sta_entry(100, b"lab"));
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 0]), b"lab").is_some());
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 1]), b"lab").is_none());
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 100]), b"lab").is_some());
}

#[test]
fn removing_forgets_one_access_point() {
    let mut cache = StaCache::new();
    cache.insert(sta_entry(1, b"lab"));
    cache.insert(sta_entry(2, b"lab"));
    cache.remove(bssid([2, 0, 0, 0, 0, 1]));
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 1]), b"lab").is_none());
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 2]), b"lab").is_some());
}

#[test]
fn an_owned_cache_outlives_connections_without_shared_state() {
    let mut cache = StaCache::new();
    let bssid = bssid([2, 1, 2, 3, 4, 5]);
    cache.insert(Pmksa::new(
        bssid,
        heapless::Vec::from_slice(b"lab").unwrap(),
        Pmk::from_bytes([7; 32]),
        [9; 16],
    ));
    // A connection borrows the owner, then returns it for the next one.
    for _ in 0..2 {
        let cached = cache.resume(bssid, b"lab").unwrap();
        assert_eq!(cached.pmkid(), [9; 16]);
        let binding = cached.pmk().bind_association_security_ies(b"association");
        assert!(binding.matches(&Pmk::from_bytes([7; 32]), b"association"));
    }
    cache.remove(bssid);
    assert!(cache.resume(bssid, b"lab").is_none());
}

#[test]
fn a_changed_ssid_compacts_the_cache_without_changing_other_entries_ages() {
    let mut cache = StaCache::new();
    for last in 0..PMKSA_CAPACITY as u8 {
        cache.insert(sta_entry(last, b"lab"));
    }
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 5]), b"other").is_none());
    cache.insert(sta_entry(100, b"lab"));
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 0]), b"lab").is_some());
    cache.insert(sta_entry(101, b"lab"));
    assert!(cache.resume(bssid([2, 0, 0, 0, 0, 0]), b"lab").is_none());
    for last in [1, 2, 3, 4, 6, 7, 8, 9, 100, 101] {
        assert!(cache.resume(bssid([2, 0, 0, 0, 0, last]), b"lab").is_some());
    }
}

fn bssid(bytes: [u8; 6]) -> Bssid {
    Bssid::new(oer_ieee80211_wire::address::IndividualAddress::new(bytes).unwrap()).unwrap()
}

#[test]
fn forgetting_a_network_drops_only_its_associations_and_keeps_the_order() {
    let mut cache = StaCache::new();
    cache.insert(sta_entry(1, b"old"));
    cache.insert(sta_entry(2, b"kept"));
    cache.insert(sta_entry(3, b"old"));
    cache.insert(sta_entry(4, b"kept"));
    cache.forget_context(b"old".as_slice());
    let peer = |last| bssid([2, 0, 0, 0, 0, last]);
    assert!(cache.find(peer(1), [1; RSN_PMKID_LEN]).is_none());
    assert!(cache.find(peer(3), [3; RSN_PMKID_LEN]).is_none());
    assert!(cache.find(peer(2), [2; RSN_PMKID_LEN]).is_some());
    assert!(cache.find(peer(4), [4; RSN_PMKID_LEN]).is_some());
    // The kept entries moved to the front: the cache fills again behind them.
    for last in 10..10 + PMKSA_CAPACITY as u8 - 2 {
        cache.insert(sta_entry(last, b"new"));
    }
    assert!(cache.find(peer(2), [2; RSN_PMKID_LEN]).is_some());
    cache.insert(sta_entry(0x30, b"new"));
    assert!(cache.find(peer(2), [2; RSN_PMKID_LEN]).is_none());
    assert!(cache.find(peer(4), [4; RSN_PMKID_LEN]).is_some());
}

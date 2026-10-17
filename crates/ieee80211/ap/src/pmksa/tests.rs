use super::*;

fn entry(last: u8) -> ApPmksa {
    ApPmksa::new(
        [2, 0, 0, 0, 0, last],
        Pmk::from_bytes([last; 32]),
        [last; AP_PMKID_LEN],
    )
}

#[test]
fn a_station_resumes_only_with_its_own_pmkid() {
    let mut cache = ApPmksaCache::new();
    cache.insert(entry(1));
    cache.insert(entry(2));
    assert_eq!(
        cache
            .find([2, 0, 0, 0, 0, 1], [1; AP_PMKID_LEN])
            .map(ApPmksa::pmkid),
        Some([1; AP_PMKID_LEN])
    );
    assert!(cache.find([2, 0, 0, 0, 0, 1], [2; AP_PMKID_LEN]).is_none());
    assert!(cache.find([2, 0, 0, 0, 0, 3], [1; AP_PMKID_LEN]).is_none());
}

#[test]
fn a_new_entry_replaces_the_stations_old_one_and_a_full_cache_drops_the_oldest() {
    let mut cache = ApPmksaCache::new();
    for last in 0..AP_PMKSA_CAPACITY as u8 {
        cache.insert(entry(last));
    }
    cache.insert(ApPmksa::new(
        [2, 0, 0, 0, 0, 0],
        Pmk::from_bytes([0xaa; 32]),
        [0xaa; AP_PMKID_LEN],
    ));
    assert!(cache.find([2, 0, 0, 0, 0, 0], [0; AP_PMKID_LEN]).is_none());
    assert!(
        cache
            .find([2, 0, 0, 0, 0, 0], [0xaa; AP_PMKID_LEN])
            .is_some()
    );

    // Station 1 is now the oldest.
    cache.insert(entry(0x20));
    assert!(cache.find([2, 0, 0, 0, 0, 1], [1; AP_PMKID_LEN]).is_none());
    assert!(cache.find([2, 0, 0, 0, 0, 2], [2; AP_PMKID_LEN]).is_some());

    cache.remove([2, 0, 0, 0, 0, 2]);
    assert!(cache.find([2, 0, 0, 0, 0, 2], [2; AP_PMKID_LEN]).is_none());
}

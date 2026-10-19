use super::*;

fn key(peer: [u8; 6], value: u8) -> RsnKeyInstall {
    RsnKeyInstall {
        interface: RsnInterface::Station,
        peer,
        kind: RsnKeyKind::Pairwise,
        receive_sequence: [0; 8],
        key: Ccmp128Key::new(&[value; 16]),
    }
}

#[test]
fn static_key_table_replaces_matching_slot_and_fails_when_full() {
    let mut keys = StaticRsnKeys::<1>::new();
    assert_eq!(keys.insert(key([1; 6], 2)), Ok(0));
    assert_eq!(keys.insert(key([1; 6], 3)), Ok(0));
    assert_eq!(keys.get(0).unwrap().key().as_bytes(), &[3; 16]);
    assert_eq!(keys.insert(key([2; 6], 4)), Err(StaticKeyTableError::Full));
}

#[test]
fn group_transmit_flag_change_reuses_key_id_slot() {
    let mut keys = StaticRsnKeys::<1>::new();
    let first = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
        false,
        &[6; 16],
    );
    let second = RsnGtk::new(
        oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
        true,
        &[7; 16],
    );
    assert_eq!(
        keys.insert(RsnKeyInstall::group(
            RsnInterface::AccessPoint,
            &first,
            [0; 8]
        )),
        Ok(0)
    );
    assert_eq!(
        keys.insert(RsnKeyInstall::group(
            RsnInterface::AccessPoint,
            &second,
            [1; 8]
        )),
        Ok(0)
    );
    assert_eq!(
        keys.get(0).unwrap().kind(),
        RsnKeyKind::Group {
            key_id: oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
            transmit: true
        }
    );
}

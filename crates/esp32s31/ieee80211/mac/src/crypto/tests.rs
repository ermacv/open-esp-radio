use super::*;

#[derive(Default)]
struct Hardware {
    occupied: bool,
    reject_installs: u8,
    installs: u8,
    clears: u8,
    last_index: Option<u8>,
    last_install: Option<(MacCcmpKeyIdentity, [u8; CCMP_KEY_BYTES])>,
}

impl CcmpKeyHardware for Hardware {
    fn configure_ccmp_association(
        &mut self,
        _interface: oer_esp32s31_hal::types::MacInterface,
        _mode: oer_esp32s31_hal::types::MacSppMode,
    ) {
    }
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        identity: MacCcmpKeyIdentity,
        temporal_key: &[u8; CCMP_KEY_BYTES],
    ) -> MacKeyInstallOutcome {
        if self.reject_installs != 0 {
            self.reject_installs -= 1;
            return MacKeyInstallOutcome::Rejected;
        }
        if self.occupied {
            return MacKeyInstallOutcome::Occupied;
        }
        self.occupied = true;
        self.installs += 1;
        self.last_index = Some(index);
        self.last_install = Some((identity, *temporal_key));
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, _index: u8) {
        self.occupied = false;
        self.clears += 1;
    }
}

#[test]
fn tx_packet_number_emits_the_48_bit_maximum_once_then_fails_closed() {
    let mut packet_number =
        CcmpTxPacketNumber::after(u64::from(u16::MAX) << 32 | u64::from(u32::MAX - 3));
    let key_id = CcmpKeyId::new(1).unwrap();
    assert_eq!(
        packet_number.next_header(key_id),
        Ok([0xff, 0xff, 0, 0x60, 0xff, 0xff, 0xff, 0xff])
    );
    assert_eq!(
        packet_number.next_header(key_id),
        Err(CcmpTxPacketNumberError::Exhausted)
    );
    assert_eq!(
        packet_number.next_header(key_id),
        Err(CcmpTxPacketNumberError::Exhausted)
    );
}

#[test]
fn exhausted_pairwise_slot_retains_hardware_clear_authority() {
    let mut hardware = Hardware {
        occupied: true,
        ..Hardware::default()
    };
    let mut slot = StaPairwiseCcmpSlot {
        peer: [1, 2, 3, 4, 5, 6],
        tx_packet_number: CcmpTxPacketNumber::after((1 << 48) - 1),
    };
    assert_eq!(
        slot.next_tx_ccmp_header(),
        Err(CcmpTxPacketNumberError::Exhausted)
    );
    slot.clear(&mut hardware);
    assert_eq!(hardware.clears, 1);
    assert!(!hardware.occupied);
}

#[test]
fn ap_pairwise_and_group_slots_share_the_fail_closed_pn_boundary() {
    let exhausted = || CcmpTxPacketNumber::after((1 << 48) - 1);
    let mut pairwise = ApPairwiseCcmpSlot {
        peer: [1, 2, 3, 4, 5, 6],
        association_id: 1,
        hardware_index: AP_PAIRWISE_HARDWARE_INDEX_BASE,
        tx_packet_number: exhausted(),
    };
    let mut group = ApGroupCcmpSlot {
        key_id: ApGroupKeyId::new(1).unwrap(),
        hardware_index: AP_GROUP_HARDWARE_INDEX_BASE + 1,
        tx_packet_number: exhausted(),
    };
    assert_eq!(
        pairwise.next_tx_ccmp_header(),
        Err(CcmpTxPacketNumberError::Exhausted)
    );
    assert_eq!(
        group.next_tx_ccmp_header(),
        Err(CcmpTxPacketNumberError::Exhausted)
    );
}

#[test]
fn first_ap_peer_and_gtk_own_the_evidenced_disjoint_slots() {
    let mut hardware = Hardware::default();
    let pairwise =
        install_ap_pairwise_ccmp(&mut hardware, [1, 2, 3, 4, 5, 6], 1, &[7; 16]).unwrap();
    assert_eq!(pairwise.hardware_index(), 8);
    assert_eq!(hardware.last_index, Some(8));
    pairwise.clear(&mut hardware);

    let group =
        install_ap_group_ccmp(&mut hardware, ApGroupKeyId::new(1).unwrap(), &[9; 16]).unwrap();
    assert_eq!(group.hardware_index(), 2);
    assert_eq!(hardware.last_index, Some(2));
    group.clear(&mut hardware);
    assert_eq!(hardware.clears, 2);
}

#[test]
fn an_ap_group_key_id_zero_is_refused_before_it_touches_the_station_group_entry() {
    let hardware = Hardware::default();
    assert!(ApGroupKeyId::new(0).is_none());
    assert_eq!(ApGroupKeyId::new(1).map(ApGroupKeyId::get), Some(1));
    assert_eq!(ApGroupKeyId::new(3).map(ApGroupKeyId::get), Some(3));
    assert!(ApGroupKeyId::new(4).is_none());
    assert_eq!(hardware.installs, 0);
    assert_eq!(hardware.last_index, None);
}

#[test]
fn a_station_gtk_with_wire_id_zero_owns_the_group_hardware_slot() {
    let mut hardware = Hardware::default();
    let key_id = oer_ieee80211_mac::ccmp::GroupKeyId::new(0).unwrap();
    let slot = install_sta_group_ccmp(&mut hardware, key_id, &[9; 16]).unwrap();
    assert_eq!(hardware.last_index, Some(STA_GROUP_HARDWARE_INDEX));
    assert_ne!(hardware.last_index, Some(STA_PAIRWISE_HARDWARE_INDEX));
    assert_eq!(
        hardware.last_install,
        Some((MacCcmpKeyIdentity::Group { key_id: 0 }, [9; 16]))
    );
    slot.clear(&mut hardware);
    assert_eq!(hardware.clears, 1);
    assert!(!hardware.occupied);
}

#[test]
fn group_rekey_reuses_one_authority_and_clears_the_replacement_once() {
    let mut hardware = Hardware::default();
    let mut slot = install_sta_group_ccmp(
        &mut hardware,
        oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
        &[1; 16],
    )
    .unwrap();

    replace_sta_group_ccmp(
        &mut hardware,
        &mut slot,
        oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
        &[2; 16],
    )
    .unwrap();
    assert_eq!(slot.key_id().get(), 2);
    assert_eq!(hardware.installs, 2);
    assert_eq!(hardware.clears, 1);

    slot.clear(&mut hardware);
    assert_eq!(hardware.clears, 2);
    assert!(!hardware.occupied);
}

#[test]
fn rejected_rekey_invalidates_the_token_and_teardown_does_not_double_clear() {
    let mut hardware = Hardware::default();
    let mut slot = install_sta_group_ccmp(
        &mut hardware,
        oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
        &[1; 16],
    )
    .unwrap();
    hardware.reject_installs = 1;

    assert_eq!(
        replace_sta_group_ccmp(
            &mut hardware,
            &mut slot,
            oer_ieee80211_mac::ccmp::GroupKeyId::new(2).unwrap(),
            &[2; 16]
        ),
        Err(CryptoKeyError::HardwareRejected)
    );
    assert_eq!(hardware.clears, 1);
    slot.clear(&mut hardware);
    assert_eq!(hardware.clears, 1);
}

#[test]
fn group_rekey_failure_restores_exact_old_key_and_slot_authority() {
    let mut hardware = Hardware::default();
    let mut slot = install_sta_group_ccmp(
        &mut hardware,
        oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
        &[1; 16],
    )
    .unwrap();
    let current = StaGroupCcmpKeyMaterial::new(GroupKeyId::new(1).unwrap(), [1; 16]);
    let replacement = StaGroupCcmpKeyMaterial::new(GroupKeyId::new(2).unwrap(), [2; 16]);
    let old_install = hardware.last_install;
    hardware.reject_installs = 1;

    assert_eq!(
        replace_sta_group_ccmp_with_rollback(&mut hardware, &mut slot, &current, &replacement,),
        Err(StaGroupCcmpReplaceError::ReplacementRolledBack(
            CryptoKeyError::HardwareRejected,
        ))
    );
    assert_eq!(slot.key_id().get(), 1);
    assert!(hardware.occupied);
    assert_eq!(hardware.last_install, old_install);
    slot.clear(&mut hardware);
    assert_eq!(hardware.clears, 2);
}

#[test]
fn group_rekey_rollback_failure_invalidates_slot_and_requires_quarantine() {
    let mut hardware = Hardware::default();
    let mut slot = install_sta_group_ccmp(
        &mut hardware,
        oer_ieee80211_mac::ccmp::GroupKeyId::new(1).unwrap(),
        &[1; 16],
    )
    .unwrap();
    let current = StaGroupCcmpKeyMaterial::new(GroupKeyId::new(1).unwrap(), [1; 16]);
    let replacement = StaGroupCcmpKeyMaterial::new(GroupKeyId::new(2).unwrap(), [2; 16]);
    hardware.reject_installs = 2;

    assert_eq!(
        replace_sta_group_ccmp_with_rollback(&mut hardware, &mut slot, &current, &replacement,),
        Err(StaGroupCcmpReplaceError::RollbackFailed {
            replacement: CryptoKeyError::HardwareRejected,
            rollback: CryptoKeyError::HardwareRejected,
        })
    );
    assert!(!hardware.occupied);
    slot.clear(&mut hardware);
    assert_eq!(hardware.clears, 1);
}

#[test]
fn group_material_comparison_covers_same_and_different_key_id_cases() {
    let current = StaGroupCcmpKeyMaterial::new(GroupKeyId::new(1).unwrap(), [0x11; 16]);
    let same_key_other_id = StaGroupCcmpKeyMaterial::new(GroupKeyId::new(2).unwrap(), [0x11; 16]);
    let changed_same_id = StaGroupCcmpKeyMaterial::new(GroupKeyId::new(1).unwrap(), [0x22; 16]);
    assert!(current.same_temporal_key(&same_key_other_id));
    assert!(!current.same_temporal_key(&changed_same_id));
}

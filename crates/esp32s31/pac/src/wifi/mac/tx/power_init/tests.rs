use super::*;

#[test]
fn table_retains_both_bytes_and_vendor_rate_remap() {
    let entries = core::array::from_fn(|rate| MacTxPowerPair {
        primary: rate as i8,
        alternate: -(rate as i8),
    });
    let table = MacTxPowerTable::new(entries);
    assert_eq!(
        table.pair(42),
        Some(MacTxPowerPair {
            primary: 42,
            alternate: -42
        })
    );
    assert_eq!(table.pair(43), None);
    assert_eq!(table.primary_index(25), 25);
    assert_eq!(table.primary_index(26), 16);
}

#[test]
fn tb_ru_delta_matches_unsigned_blob_clamp() {
    assert_eq!(relative_index(3, 10), 0);
    assert_eq!(relative_index(10, 10), 0);
    assert_eq!(relative_index(21, 10), 11);
    assert_eq!(relative_index(0x80, 10), 54);
}

#[test]
fn partial_ru_power_selector_matches_complete_hal_jump_tables() {
    let expected = [0, 1, 2, 3, 4, 5, 6, 7, 8, 37, 38, 39, 40, 53, 54, 61];
    let mut admitted = [0_u8; 16];
    let mut count = 0;
    for raw in 0..=u8::MAX {
        if let Some(selector) = MacPartialRuPowerSelector::from_trigger_encoding(raw) {
            admitted[count] = selector.trigger_encoding();
            count += 1;
        }
    }
    assert_eq!(count, expected.len());
    assert_eq!(admitted, expected);
    assert_eq!(
        partial_ru_power_slot(MacPartialRuPowerSelector::from_trigger_encoding(0).unwrap()),
        PartialRuPowerSlot::Packed { word: 0, lane: 0 }
    );
    assert_eq!(
        partial_ru_power_slot(MacPartialRuPowerSelector::from_trigger_encoding(37).unwrap()),
        PartialRuPowerSlot::Packed { word: 1, lane: 4 }
    );
    assert_eq!(
        partial_ru_power_slot(MacPartialRuPowerSelector::from_trigger_encoding(53).unwrap()),
        PartialRuPowerSlot::Packed { word: 2, lane: 3 }
    );
    assert_eq!(
        partial_ru_power_slot(MacPartialRuPowerSelector::from_trigger_encoding(61).unwrap()),
        PartialRuPowerSlot::Tail
    );
    assert!(MacPartialRuPowerSelector::from_trigger_encoding(62).is_none());
}

#[test]
fn runtime_power_index_is_bounded_to_six_bits() {
    assert_eq!(MacTxPowerIndex::new(0).unwrap().value(), 0);
    assert_eq!(MacTxPowerIndex::new(63).unwrap().value(), 63);
    assert!(MacTxPowerIndex::new(64).is_none());
}

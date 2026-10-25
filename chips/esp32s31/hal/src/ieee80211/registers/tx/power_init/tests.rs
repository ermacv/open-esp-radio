use super::*;
use crate::ieee80211::registers::HostWords;

fn selector(raw: u8) -> MacPartialRuPowerSelector {
    MacPartialRuPowerSelector::from_trigger_encoding(raw).expect("an admitted selector")
}

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
        partial_ru_power_slot(selector(0)),
        PartialRuPowerSlot::Packed { word: 0, lane: 0 }
    );
    assert_eq!(
        partial_ru_power_slot(selector(37)),
        PartialRuPowerSlot::Packed { word: 1, lane: 4 }
    );
    assert_eq!(
        partial_ru_power_slot(selector(53)),
        PartialRuPowerSlot::Packed { word: 2, lane: 3 }
    );
    assert_eq!(
        partial_ru_power_slot(selector(61)),
        PartialRuPowerSlot::Tail
    );
    assert!(MacPartialRuPowerSelector::from_trigger_encoding(62).is_none());
}

#[test]
fn runtime_power_index_is_bounded_to_six_bits() {
    assert_eq!(MacTxPowerIndex::new(0).map(MacTxPowerIndex::value), Some(0));
    assert_eq!(
        MacTxPowerIndex::new(63).map(MacTxPowerIndex::value),
        Some(63)
    );
    assert!(MacTxPowerIndex::new(64).is_none());
}

#[test]
fn partial_ru_power_round_trips_and_keeps_adjacent_lanes() {
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    {
        let mut owners = memory.owners();
        let registers = &mut owners.registers;
        for (raw, power) in [(0, 1), (8, 2), (37, 3), (54, 4), (61, 5)] {
            registers.set_partial_ru_max_tx_power(
                selector(raw),
                MacTxPowerIndex::new(power).expect("a six-bit index"),
            );
        }
        for (raw, power) in [(0, 1), (1, 0), (8, 2), (37, 3), (54, 4), (61, 5)] {
            assert_eq!(
                registers.partial_ru_max_tx_power(selector(raw)).value(),
                power
            );
        }
    }
    // Three packed words and the tail word, nothing else.
    let changes = before.changes(&memory);
    assert_eq!(changes.len(), 4);
    assert!(
        changes
            .iter()
            .all(|(part, _, _)| *part == "wifi_mac_tx_power_init")
    );
}

#[test]
fn cold_power_publication_fills_every_table_from_the_rate_powers() {
    let entries = core::array::from_fn(|rate| MacTxPowerPair {
        primary: 20 + rate as i8,
        alternate: 0,
    });
    let table = MacTxPowerTable::new(entries);
    let before = HostWords::filled(0);
    let mut memory = before.clone();
    memory.owners().registers.initialize_mac_tx_power(&table);

    let owners = memory.owners();
    let init = &owners.registers.mac.wifi_mac_tx_power_init;
    let tb0 = init.read_tb_power(element::<3>(0));
    assert_eq!(
        [tb0.power_0(), tb0.power_1(), tb0.power_2(), tb0.power_3()].map(|v| v.get()),
        [36, 37, 38, 39]
    );
    let tb2 = init.read_tb_power(element::<3>(2));
    assert_eq!(
        [
            tb2.power_0().get(),
            tb2.power_1().get(),
            tb2.power_2().get()
        ],
        [44, 45, 0]
    );
    // The last two immediate-response words both look up rate 18.
    for word in [8, 9] {
        let response = init.read_immediate_response(element::<10>(word));
        assert_eq!(response.bits_22_23_opaque().get(), 2);
        assert_eq!(response.rate_index().get(), 18);
        assert_eq!(response.power_index().get(), 38);
    }
    let first = init.read_immediate_response(element::<10>(1));
    assert_eq!(
        (first.rate_index().get(), first.power_index().get()),
        (1, 20)
    );
    // Rate 16's primary is 36: deltas 26, 29 and 32, the tail keeps 36.
    let ru1 = init.read_tb_ru_power(element::<3>(1));
    assert_eq!((ru1.power_3().get(), ru1.power_4().get()), (26, 29));
    let ru2 = init.read_tb_ru_power(element::<3>(2));
    assert_eq!((ru2.power_2().get(), ru2.power_3().get()), (29, 32));
    assert_eq!(init.read_tb_ru_power_tail().power_index().get(), 36);
    // Three TB words, ten response words, three RU words and the tail.
    assert_eq!(before.changes(&memory).len(), 17);
}

//! A fixed timer discard must leave delivery configuration and sibling words
//! alone. Host storage observes bus writes, not silicon W1C behavior.
use oer_esp32s31_pac::{
    bt_mac::{BluetoothInterruptBank, irq_clear_1},
    instances,
};

#[test]
fn explicit_indication_discards_do_not_mask_or_replay_sibling_state() {
    for (cause, indication) in [(0, 0x10), (1, 0x20), (2, 2)] {
        let mut memory = vec![u32::MAX; instances::bt_mac::SIZE / size_of::<u32>()];
        {
            let mut owner = BluetoothInterruptBank::host(&mut memory).expect("host aperture");
            match cause {
                0 => owner.discard_bluetooth_compare_0_indication(),
                1 => owner.discard_bluetooth_compare_1_indication(),
                _ => owner.discard_bluetooth_controller_time_overflow_indication(),
            }
        }
        let slot = irq_clear_1::OFFSET / size_of::<u32>();
        for (n, value) in memory.into_iter().enumerate() {
            assert_eq!(value, if n == slot { indication } else { u32::MAX });
        }
    }
}

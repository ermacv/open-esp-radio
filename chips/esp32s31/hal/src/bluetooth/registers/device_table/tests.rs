use std::vec::Vec;

use super::*;

#[derive(Debug, Eq, PartialEq)]
enum Write {
    Count(u32),
    FirstEntry(u32),
}

#[derive(Default)]
struct Recorder {
    writes: Vec<Write>,
}

impl BluetoothDeviceTableTransaction for Recorder {
    fn publish_entry_count(&mut self, count: u8) {
        self.writes.push(Write::Count(u32::from(count)));
    }

    fn publish_first_entry(&mut self, first_entry: DmaRef<BleDeviceTable>) {
        self.writes.push(Write::FirstEntry(first_entry.bits()));
    }
}

#[test]
fn the_count_is_published_before_the_first_entry() {
    let first = crate::memory::dma_address::model_reference(0x2f07_e508);
    let mut recorder = Recorder::default();
    execute_device_table_publication(&mut recorder, first, 2);
    assert_eq!(
        recorder.writes,
        [Write::Count(2), Write::FirstEntry(0x2f07_e508)]
    );
}

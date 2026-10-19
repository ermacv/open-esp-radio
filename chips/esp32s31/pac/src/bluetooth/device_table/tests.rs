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
    fn publish_entry_count(&mut self, count: BluetoothDeviceTableEntryCount) {
        self.writes.push(Write::Count(count.get()));
    }

    fn publish_first_entry(&mut self, first_entry: BluetoothControllerSramAddress) {
        self.writes.push(Write::FirstEntry(first_entry.address()));
    }
}

#[test]
fn the_count_is_published_before_the_first_entry() {
    let first = BluetoothControllerSramAddress::new(0x2f07_e508).unwrap();
    let mut recorder = Recorder::default();
    execute_device_table_publication(
        &mut recorder,
        first,
        BluetoothDeviceTableEntryCount::new(2).unwrap(),
    );
    assert_eq!(
        recorder.writes,
        [Write::Count(2), Write::FirstEntry(0x2f07_e508)]
    );
}

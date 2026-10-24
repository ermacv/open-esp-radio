use crate::{
    MacInterface,
    wifi_mac_interface_address::{SIZE, WifiMacInterfaceAddress},
};

const ADDRESS: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];

/// Every word set, so a word the transaction does not write stays visible.
fn memory() -> [u32; SIZE / 4] {
    [u32::MAX; SIZE / 4]
}

#[test]
fn an_interface_address_is_stored_whole_and_then_enabled() {
    let mut words = memory();
    let mut block = WifiMacInterfaceAddress::host(&mut words).expect("memory holds the block");
    block.program_receive_interface_address(MacInterface::AccessPoint, ADDRESS);
    let high = block.read_address_high(MacInterface::AccessPoint.index());
    assert_eq!(high.bytes_4_5(), 0x5544);
    assert!(high.rx_policy_enable());
    assert_eq!(
        high.bits_17_31_opaque().get(),
        0,
        "the high store starts from zero"
    );
    drop(block);
    // The access point's pair is the second; every other word keeps its value.
    assert_eq!(
        words,
        [
            u32::MAX,
            u32::MAX,
            0x3322_1102,
            0x0001_5544,
            u32::MAX,
            u32::MAX,
            u32::MAX,
            u32::MAX,
        ]
    );
}

#[test]
fn each_interface_selects_its_own_address_pair() {
    for (interface, pair) in [
        (MacInterface::Station, 0),
        (MacInterface::AccessPoint, 1),
        (MacInterface::Context2, 2),
        (MacInterface::Context3, 3),
    ] {
        let mut words = memory();
        let mut block = WifiMacInterfaceAddress::host(&mut words).expect("memory holds the block");
        block.program_receive_interface_address(interface, ADDRESS);
        drop(block);
        for (word, value) in words.iter().enumerate() {
            let written = word / 2 == pair;
            assert_eq!(*value != u32::MAX, written, "{interface:?} word {word}");
        }
    }
}

/// One hardware operation of an interface-address publication.
#[derive(Debug, Eq, PartialEq)]
enum Operation {
    StoreLow(usize, u32),
    StoreHigh(usize, u16),
    EnableReceivePolicy(usize),
}

/// The operations in the order they were performed.
#[derive(Default)]
struct Recorder {
    operations: [Option<Operation>; 4],
    count: usize,
}

impl Recorder {
    fn record(&mut self, operation: Operation) {
        self.operations[self.count] = Some(operation);
        self.count += 1;
    }
}

impl super::InterfaceAddressPort for Recorder {
    fn store_low(&mut self, index: oer_mmio::Index<4>, word: u32) {
        self.record(Operation::StoreLow(index.get(), word));
    }

    fn store_high(&mut self, index: oer_mmio::Index<4>, bytes: u16) {
        self.record(Operation::StoreHigh(index.get(), bytes));
    }

    fn enable_receive_policy(&mut self, index: oer_mmio::Index<4>) {
        self.record(Operation::EnableReceivePolicy(index.get()));
    }
}

#[test]
fn the_low_store_the_high_store_and_the_enable_are_three_ordered_operations() {
    let mut recorder = Recorder::default();
    super::program(&mut recorder, MacInterface::Context2, ADDRESS);
    assert_eq!(
        recorder.operations,
        [
            Some(Operation::StoreLow(2, 0x3322_1102)),
            Some(Operation::StoreHigh(2, 0x5544)),
            Some(Operation::EnableReceivePolicy(2)),
            None,
        ]
    );
}

#[test]
fn the_enable_is_a_read_modify_write_of_the_high_word() {
    let mut words = memory();
    let mut block = WifiMacInterfaceAddress::host(&mut words).expect("memory holds the block");
    super::InterfaceAddressPort::enable_receive_policy(&mut block, MacInterface::Station.index());
    drop(block);
    // The enable bit was already set: a read-modify-write keeps every other
    // bit, a store would have replaced them.
    assert_eq!(words, memory());
}

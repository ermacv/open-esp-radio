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

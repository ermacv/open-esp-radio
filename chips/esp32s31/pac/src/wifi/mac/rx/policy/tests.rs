use super::{COLD_RECEIVE_QUEUE_INTERFACES, clear_cold_interface_address};
use crate::generated::wifi_mac_interface_address::{SIZE, WifiMacInterfaceAddress};

#[test]
fn the_cold_policy_clears_the_first_three_interfaces_and_keeps_their_high_bits() {
    let mut words = [u32::MAX; SIZE / 4];
    let mut block = WifiMacInterfaceAddress::host(&mut words).expect("memory holds the block");
    for interface in COLD_RECEIVE_QUEUE_INTERFACES.into_iter().flatten() {
        clear_cold_interface_address(&mut block, interface);
    }
    for interface in [
        crate::MacInterface::Station,
        crate::MacInterface::AccessPoint,
        crate::MacInterface::Context2,
    ] {
        let high = block.read_address_high(interface.index());
        assert_eq!(high.bytes_4_5(), 0, "{interface:?} address bytes cleared");
        assert!(high.rx_policy_enable(), "{interface:?} enable kept");
        assert_ne!(
            high.bits_17_31_opaque().get(),
            0,
            "{interface:?} opaque bits kept"
        );
    }
    drop(block);
    // Queue three has no interface context: the fourth pair and every low
    // word keep their value; each of the first three high words lost only its
    // address bytes.
    let changed: std::vec::Vec<usize> = (0..words.len())
        .filter(|word| words[*word] != u32::MAX)
        .collect();
    assert_eq!(changed, [1, 3, 5]);
}

#[test]
fn queue_three_has_no_interface_context() {
    assert_eq!(
        COLD_RECEIVE_QUEUE_INTERFACES,
        [
            Some(crate::MacInterface::Station),
            Some(crate::MacInterface::AccessPoint),
            Some(crate::MacInterface::Context2),
            None,
        ]
    );
}

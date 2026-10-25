use super::{
    Ieee802154Partition, RadioPartitions, SharedRadioParts, SharedRadioRegisters,
    WifiRadioRegisters,
};

fn wifi_registers(partitions: RadioPartitions) -> (WifiRadioRegisters, super::MacInterruptSetup) {
    let RadioPartitions {
        wifi_mac,
        wifi_interrupts,
        ..
    } = partitions;
    (WifiRadioRegisters::new(wifi_mac), wifi_interrupts)
}

fn shared_registers(partitions: RadioPartitions) -> SharedRadioRegisters {
    let RadioPartitions {
        radio_phy,
        coexistence,
        shared_radio,
        ..
    } = partitions;
    SharedRadioRegisters::new(SharedRadioParts {
        radio_phy,
        coexistence,
        shared_radio,
    })
}

#[test]
fn wifi_register_set_returns_its_mac_partition() {
    let (registers, _interrupts) = wifi_registers(RadioPartitions::for_host_test());
    let _partition = registers.into_partition();
}

#[test]
fn shared_radio_owner_returns_every_shared_partition() {
    let shared = shared_registers(RadioPartitions::for_host_test());
    let SharedRadioParts { .. } = shared.into_parts();
}

#[test]
fn ieee802154_partition_rejoins_the_mac_its_parts_split() {
    let RadioPartitions { ieee802154, .. } = RadioPartitions::for_host_test();
    let Ieee802154Partition { mac, etm, rx_info } = ieee802154;
    let (task, interrupts) = mac.split();
    let _partition = Ieee802154Partition {
        mac: crate::generated::ieee802154_mac::Ieee802154Mac::join(task, interrupts),
        etm,
        rx_info,
    };
}

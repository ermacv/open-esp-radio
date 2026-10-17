use super::{
    BluetoothTaskRegisters, Ieee802154TaskRegisters, RadioPartitions, SharedRadioParts,
    SharedRadioRegisters, WifiRadioRegisters,
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
    let (registers, _interrupts) = wifi_registers(RadioPartitions::for_validation());
    let _partition = registers.into_partition();
}

#[test]
fn bluetooth_register_set_returns_its_controller_partition() {
    let RadioPartitions { bluetooth, .. } = RadioPartitions::for_validation();
    let registers = BluetoothTaskRegisters::new(bluetooth);
    let _partition = registers.into_partition();
}

#[test]
fn shared_radio_owner_returns_every_shared_partition() {
    let shared = shared_registers(RadioPartitions::for_validation());
    let SharedRadioParts { .. } = shared.into_parts();
}

#[test]
fn ieee802154_register_set_reunites_its_interrupt_owner() {
    let RadioPartitions { ieee802154, .. } = RadioPartitions::for_validation();
    let (task, interrupts) = Ieee802154TaskRegisters::new(ieee802154);
    let _partition = task.into_partition(interrupts);
}

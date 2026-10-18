use oer_bluetooth_hci::BluetoothPublicDeviceAddress;

use oer_esp32s31_bluetooth_memory::{
    BlePhyEngineCpuOwned, BlePhyEngineModelAddress, BlePhyEngineStorage,
};

use super::{
    BlePhyInitializationReport, apply_register_init, apply_register_init_then_public_address,
};

fn model_storage() -> BlePhyEngineCpuOwned {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BlePhyEngineStorage::new()));
    let base = BlePhyEngineModelAddress::new(0x2f00_0100)
        .expect("model base uses the controller-SRAM encoding");
    BlePhyEngineStorage::pin_static_model(storage, base)
        .expect("complete model storage fits physical SRAM")
}

#[test]
fn normal_register_profile_is_applied_once_without_releasing_storage() {
    let owner = model_storage();
    let calibration = owner.le_1m_packet_start_calibration();
    let mut calls = 0;

    let report = apply_register_init(&owner, |_| calls += 1);

    assert_eq!(calls, 1);
    assert_eq!(report, BlePhyInitializationReport);
    assert_eq!(owner.le_1m_packet_start_calibration(), calibration);
}

#[test]
fn public_identity_is_published_after_phy_register_initialization() {
    let owner = model_storage();
    let public_address = BluetoothPublicDeviceAddress::from_canonical_bytes([2, 3, 5, 7, 11, 13]);
    let mut operations = std::vec::Vec::new();

    let report = apply_register_init_then_public_address(
        &owner,
        public_address,
        &mut operations,
        |operations, _| operations.push("phy"),
        |operations, address| {
            assert_eq!(address.canonical_bytes(), public_address.canonical_bytes());
            operations.push("public-address");
        },
    );

    assert_eq!(report, BlePhyInitializationReport);
    assert_eq!(operations, ["phy", "public-address"]);
}

#[test]
fn connection_packet_start_normalization_preserves_elapsed_time() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BlePhyEngineStorage::new()));
    let base = BlePhyEngineModelAddress::new(0x2f00_4000)
        .expect("model base uses the controller-SRAM encoding");
    let owner = BlePhyEngineStorage::pin_static_model(storage, base)
        .expect("complete model storage fits physical SRAM");
    let calibration = owner.le_1m_packet_start_calibration();

    let first = calibration.normalize_controller_micros(20_000);
    let second = calibration.normalize_controller_micros(20_017);

    assert_eq!(second.wrapping_sub(first), 17);
}

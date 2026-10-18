use super::{BlePhyEngineModelAddress, BlePhyEngineStorage, BlePhyLe1MPacketStartCalibration};

#[test]
fn failed_binding_returns_the_same_opaque_allocation() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BlePhyEngineStorage::new()));
    let original = core::ptr::from_mut(storage);
    let base = BlePhyEngineModelAddress::new(0x2f07_fffc)
        .expect("model base uses the controller-SRAM encoding");
    let failure = match BlePhyEngineStorage::pin_static_model(storage, base) {
        Ok(_) => panic!("both retained extents cross physical SRAM"),
        Err(failure) => failure,
    };
    let (storage, _) = failure.into_parts();
    assert_eq!(core::ptr::from_mut(storage), original);
}

#[test]
fn le_1m_calibration_preserves_elapsed_controller_time() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BlePhyEngineStorage::new()));
    let base = BlePhyEngineModelAddress::new(0x2f00_0100)
        .expect("model base uses the controller-SRAM encoding");
    let owner = BlePhyEngineStorage::pin_static_model(storage, base)
        .expect("complete model storage fits physical SRAM");
    let calibration = owner.le_1m_packet_start_calibration();

    let first = calibration.normalize_controller_micros(1_000);
    let second = calibration.normalize_controller_micros(1_001);

    assert_ne!(first, 1_000, "the initialized calibration is not zero");
    assert_eq!(calibration, BlePhyLe1MPacketStartCalibration::le_1m());
    assert_eq!(
        first,
        1_000 - BlePhyLe1MPacketStartCalibration::le_1m().capture_delay_micros()
    );
    assert_eq!(second.wrapping_sub(first), 1);
}

#[test]
fn a_controller_reset_restores_the_allocation_image() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BlePhyEngineStorage::new()));
    let base = BlePhyEngineModelAddress::new(0x2f00_0100)
        .expect("model base uses the controller-SRAM encoding");
    let mut owner = BlePhyEngineStorage::pin_static_model(storage, base)
        .expect("complete model storage fits physical SRAM");
    let initial = owner.image();
    owner.emulate_hardware_writes();
    assert_ne!(owner.image(), initial);

    owner.reset_after_controller_reset(
        &oer_esp32s31_hal::bluetooth::BluetoothControllerReset::for_validation(),
    );

    assert_eq!(owner.image(), initial);
}

#[test]
fn the_environment_records_the_retained_btbb_transmit_power_table() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BlePhyEngineStorage::new()));
    let base = BlePhyEngineModelAddress::new(0x2f00_0100)
        .expect("model base uses the controller-SRAM encoding");
    let owner = BlePhyEngineStorage::pin_static_model(storage, base)
        .expect("complete model storage fits physical SRAM");
    let storage = owner.storage.as_ref().get_ref();
    let word = |offset: usize| {
        let bytes: [u8; 4] =
            core::array::from_fn(|index| storage.environment[offset + index].get());
        u32::from_le_bytes(bytes)
    };
    let levels = oer_esp32s31_hal::phy::baseband::TX_POWER_LEVELS_DBM;

    assert_eq!(storage.tx_power_levels_dbm, levels);
    assert_eq!(
        word(super::TX_POWER_LEVELS_POINTER_OFFSET),
        owner.binding().tx_power_levels.address(),
        "the record addresses the table retained by this allocation"
    );
    assert_eq!(
        usize::from(storage.environment[super::TX_POWER_LEVEL_COUNT_OFFSET].get()),
        levels.len()
    );
    assert_eq!(
        word(super::TX_POWER_FIRST_LEVEL_OFFSET) as i32,
        i32::from(levels[0])
    );
    assert_eq!(
        word(super::TX_POWER_LAST_LEVEL_OFFSET) as i32,
        i32::from(levels[levels.len() - 1])
    );
}

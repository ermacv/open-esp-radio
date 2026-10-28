use super::{BlePhyEngineStorage, BlePhyLe1MPacketStartCalibration};
use bytemuck::Zeroable;

#[test]
fn le_1m_calibration_preserves_elapsed_controller_time() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BlePhyEngineStorage::zeroed()));
    let owner = BlePhyEngineStorage::place::<crate::bluetooth::engine::HostDma<0x2f00_0100>>(
        crate::memory::dma_static::Placed::host(storage),
    );
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
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BlePhyEngineStorage::zeroed()));
    let mut owner = BlePhyEngineStorage::place::<crate::bluetooth::engine::HostDma<0x2f00_0100>>(
        crate::memory::dma_static::Placed::host(storage),
    );
    let initial = owner.image();
    owner.emulate_hardware_writes();
    assert_ne!(owner.image(), initial);

    owner
        .reset_after_controller_reset(&crate::bluetooth::BluetoothControllerReset::for_host_test());

    assert_eq!(owner.image(), initial);
}

#[test]
fn the_environment_records_the_retained_btbb_transmit_power_table() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(BlePhyEngineStorage::zeroed()));
    let owner = BlePhyEngineStorage::place::<crate::bluetooth::engine::HostDma<0x2f00_0100>>(
        crate::memory::dma_static::Placed::host(storage),
    );
    let storage = owner.storage.as_ref().get_ref();
    let levels = oer_esp32s31_knowledge::tx_power::TX_POWER_LEVELS_DBM;

    assert_eq!(storage.tx_power_levels_dbm, levels);
    assert_eq!(
        storage.environment.tx_power_levels().read().address(),
        owner.binding().tx_power_levels.get(),
        "the record addresses the table retained by this allocation"
    );
    assert_eq!(
        usize::from(storage.environment.tx_power_level_count().read().value()),
        levels.len()
    );
    assert_eq!(
        storage
            .environment
            .tx_power_first_level()
            .read()
            .dbm_sign_extended() as i32,
        i32::from(levels[0])
    );
    assert_eq!(
        storage
            .environment
            .tx_power_last_level()
            .read()
            .dbm_sign_extended() as i32,
        i32::from(levels[levels.len() - 1])
    );
}

#[test]
#[allow(
    unsafe_code,
    reason = "the HAL DMA-static macro proves this test static placement"
)]
fn zeroed_phy_tables_are_initialized_before_the_environment_is_published() {
    use crate::layouts::*;
    crate::memory::dma_static! {
        static PHY: BlePhyEngineStorage = zeroed in ".dma.bss.bluetooth";
    }
    let placed = PHY.take().expect("the PHY static is unclaimed");
    assert!(PHY.take().is_none(), "the arena has one owner");
    let owner =
        BlePhyEngineStorage::place::<crate::bluetooth::engine::HostDma<0x2f00_0100>>(placed);
    let storage = owner.storage.as_ref().get_ref();
    let frequencies = BlePhyChannelFrequencyOffsets::new(&storage.channel_frequency_offsets_mhz);
    assert_eq!(
        frequencies
            .entry(0)
            .expect("a reviewed LE channel index")
            .value()
            .offset_mhz(),
        2
    );
    assert_eq!(
        frequencies
            .entry(10)
            .expect("a reviewed LE channel index")
            .value()
            .offset_mhz(),
        22
    );
    assert_eq!(
        frequencies
            .entry(11)
            .expect("a reviewed LE channel index")
            .value()
            .offset_mhz(),
        26
    );
    assert_eq!(
        frequencies
            .entry(36)
            .expect("a reviewed LE channel index")
            .value()
            .offset_mhz(),
        76
    );
    assert_eq!(
        frequencies
            .entry(37)
            .expect("a reviewed LE channel index")
            .value()
            .offset_mhz(),
        0
    );
    assert_eq!(
        frequencies
            .entry(38)
            .expect("a reviewed LE channel index")
            .value()
            .offset_mhz(),
        24
    );
    assert_eq!(
        frequencies
            .entry(39)
            .expect("a reviewed LE channel index")
            .value()
            .offset_mhz(),
        78
    );
    let starts = BlePhyPacketStartOffsets::new(&storage.packet_start_offsets_micros);
    assert_eq!(
        [
            starts.le_coded_s2().micros(),
            starts.le_1m().micros(),
            starts.le_2m().micros(),
            starts.le_coded_s8().micros()
        ],
        [336, 40, 24, 336]
    );
    let delays = BlePhyRxAddressDelays::new(&storage.rx_address_delays_micros);
    assert_eq!(
        [
            delays.le_coded_s2().micros(),
            delays.le_1m().micros(),
            delays.le_2m().micros(),
            delays.le_coded_s8().micros()
        ],
        [68, 3, 3, 68]
    );
    assert_eq!(
        storage
            .environment
            .channel_frequency_offsets()
            .read()
            .address(),
        owner.binding.channel_frequency_offsets.get()
    );
    assert_eq!(
        storage.environment.packet_start_offsets().read().address(),
        owner.binding.packet_start_offsets.get()
    );
    assert_eq!(
        storage.environment.rx_address_delays().read().address(),
        owner.binding.rx_address_delays.get()
    );
    assert_eq!(
        owner
            .le_1m_packet_start_calibration()
            .capture_delay_micros(),
        43
    );
}

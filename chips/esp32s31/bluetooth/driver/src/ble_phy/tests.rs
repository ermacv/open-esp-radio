#[test]
fn connection_packet_start_normalization_preserves_elapsed_time() {
    let calibration =
        oer_esp32s31_hal::bluetooth::engine::BlePhyLe1MPacketStartCalibration::le_1m();

    let first = calibration.normalize_controller_micros(20_000);
    let second = calibration.normalize_controller_micros(20_017);

    assert_eq!(second.wrapping_sub(first), 17);
}

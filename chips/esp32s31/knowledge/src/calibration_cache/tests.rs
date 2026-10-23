use super::*;

const GOLDEN: &[u8; 261] = include_bytes!("golden.bin");

fn fixture() -> CalibrationRecord {
    CalibrationRecord {
        schema: 8,
        identity: CalibrationIdentity {
            rf_cal_version: 2712847316,
            base_mac_address: [34, 51, 68, 85, 102, 119],
            mac_extension: 7976,
        },
        common: CommonCalibration {
            temperature: -28277,
            rfpll_reference_temperature: -27778,
            rxcal_reference_temperature: -27279,
            txcal_reference_temperature: -26780,
            power_reference_temperature: -26281,
            sensor_index: 238,
            crystal_selector: 255,
            rc_result: 16,
            filter_dcap: [33, 50, 67, 84, 101],
            rc_calibrated: false,
            dcode: [135, 152, 169, 186, 203, 220, 237, 254],
            i2c_frequency_parameter: 15,
            xtal_duty: [32, 49, 66],
            clear_tone_after_ready: true,
        },
        wifi: WifiCalibration {
            tx_gain_adjustment: 44,
            baseband_calibrated: true,
            pwdet_calibrated: false,
            tx_power_calibrated: true,
            tx_iq_calibrated: false,
            rx_gain_dc_calibrated: true,
            rx_gain_tables_initialized: false,
            rx_saturation_detected: true,
            tx_dco: [
                [43868, 44865, 45862, 46859],
                [47856, 48853, 49850, 50847],
                [51844, 52841, 53838, 54835],
                [55832, 56829, 57826, 58823],
                [59820, 60817, 61814, 62811],
            ],
            tx_reference_codes: [-832, -333],
            tx_capacitance: [98, 115, 132, 149, 166, 183],
            tx_power_curve: [-40, -21, -2],
            tx_power_corrections: [17, 36, 55],
            tx_power_adjustment: 74,
            calibrated_attenuation: 63,
            tx_iq_config: 14224,
            tx_iq_coefficient: 15221,
            rx_iq_coefficients: [16218, 17215, 18212, 19209],
            external_dcode: [182, 199],
            calibration_temperature: 11144,
            calibration_channel: 23197,
            wifi_rx_table_last_index: 250,
            shared_rx_table_last_index: 11,
            wifi_index_dc: [
                [26188, 27185],
                [28182, 29179],
                [30176, 31173],
                [32170, 33167],
                [34164, 35161],
                [36158, 37155],
                [38152, 39149],
                [40146, 41143],
            ],
            wifi_fine_dc: [
                [42140, 43137],
                [44134, 45131],
                [46128, 47125],
                [48122, 49119],
                [50116, 51113],
            ],
            shared_index_dc: [
                [52110, 53107],
                [54104, 55101],
                [56098, 57095],
                [58092, 59089],
                [60086, 61083],
                [62080, 63077],
                [64074, 65071],
                [532, 1529],
                [2526, 3523],
                [4520, 5517],
                [6514, 7511],
            ],
        },
        bluetooth: BluetoothCalibration {
            tx_dc_calibrated: false,
            tx_power_calibrated: true,
            tx_dco: [
                [10502, 11499, 12496, 13493],
                [14490, 15487, 16484, 17481],
                [18478, 19475, 20472, 21469],
            ],
            tx_power_curve: [-18, 1, 20],
            tx_power_corrections: [39, 58, 77],
            tx_power_adjustment: 96,
        },
    }
}

#[test]
fn golden_bytes_preserve_every_field_and_fixed_width_signed_values() {
    let record = fixture();
    assert_eq!(record.encode().expect("schema 8"), *GOLDEN);
    assert_eq!(
        CalibrationRecord::decode(S31_SEMANTIC_V1, GOLDEN),
        Ok(record)
    );
    assert_eq!(record.identity.encode(), GOLDEN[2..14]);
}

#[test]
fn reject_every_truncation_trailing_bytes_format_schema_and_noncanonical_boolean() {
    for length in 0..GOLDEN.len() {
        assert_eq!(
            CalibrationRecord::decode(S31_SEMANTIC_V1, &GOLDEN[..length]),
            Err(CodecError::Length)
        );
    }
    let mut long = [0; 262];
    long[..261].copy_from_slice(GOLDEN);
    assert_eq!(
        CalibrationRecord::decode(S31_SEMANTIC_V1, &long),
        Err(CodecError::Length)
    );
    assert_eq!(
        CalibrationRecord::decode(0, GOLDEN),
        Err(CodecError::Format)
    );
    let mut bad = *GOLDEN;
    bad[..2].copy_from_slice(&9u16.to_le_bytes());
    assert_eq!(
        CalibrationRecord::decode(S31_SEMANTIC_V1, &bad),
        Err(CodecError::Schema)
    );
    assert_eq!(
        CalibrationRecord {
            schema: 9,
            ..fixture()
        }
        .encode(),
        Err(CodecError::Schema)
    );
    for offset in [32, 45, 47, 48, 49, 50, 51, 52, 53, 228, 229] {
        for value in 2..=u8::MAX {
            let mut bad = *GOLDEN;
            bad[offset] = value;
            assert_eq!(
                CalibrationRecord::decode(S31_SEMANTIC_V1, &bad),
                Err(CodecError::Boolean)
            );
        }
    }
}

#[test]
fn structurally_valid_incomplete_or_foreign_records_are_not_silently_repaired() {
    let mut record = fixture();
    record.identity.base_mac_address = [255; 6];
    record.common.rc_calibrated = false;
    record.wifi.baseband_calibrated = false;
    record.wifi.wifi_rx_table_last_index = 255;
    record.bluetooth.tx_dc_calibrated = false;
    assert_eq!(
        CalibrationRecord::decode(S31_SEMANTIC_V1, &record.encode().expect("schema")),
        Ok(record)
    );
}

#[test]
fn canonical_flags_and_integer_extremes_round_trip_without_clamping() {
    for offset in [32, 45, 47, 48, 49, 50, 51, 52, 53, 228, 229] {
        for value in [0, 1] {
            let mut bytes = *GOLDEN;
            bytes[offset] = value;
            let decoded =
                CalibrationRecord::decode(S31_SEMANTIC_V1, &bytes).expect("canonical flag");
            assert_eq!(decoded.encode().expect("schema"), bytes);
        }
    }
    let mut record = fixture();
    record.identity.rf_cal_version = u32::MAX;
    record.common.temperature = i16::MIN;
    record.common.rfpll_reference_temperature = i16::MAX;
    record.wifi.tx_gain_adjustment = i8::MIN;
    record.wifi.tx_power_adjustment = i8::MAX;
    record.wifi.tx_dco = [[u16::MAX; 4]; 5];
    record.wifi.tx_reference_codes = [i16::MIN, i16::MAX];
    record.wifi.tx_iq_config = 0;
    assert_eq!(
        CalibrationRecord::decode(S31_SEMANTIC_V1, &record.encode().expect("schema")),
        Ok(record)
    );
}

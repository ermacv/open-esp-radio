use super::*;

const IDENTITY: crate::calibration::registration::PhyCalibrationIdentity =
    crate::calibration::registration::PhyCalibrationIdentity {
        rf_cal_version: 7,
        base_mac_address: [2, 3, 5, 7, 11, 13],
        mac_extension: 17,
    };

fn complete_calibration_cache() -> PhyCalibrationCache {
    let mut state = PhyState::new(PhyConfig::production());
    state.common.temperature = 23;
    state.common.references.rfpll = 19;
    state.common.references.common = 17;
    state.common.references.transmit = 13;
    state.common.sensor_index = 3;
    state.common.crystal_selector = 2;
    state.common.rc_result = 41;
    state.common.filter_dcap = [2, 3, 4, 5, 6];
    state.common.rc_calibrated = true;
    state.common.dcode = [1, 2, 3, 4, 5, 6, 7, 8];
    state.common.i2c_frequency_parameter = 9;
    state.common.xtal_duty = [10, 11, 12];
    state.common.clear_tone_after_ready = true;
    state.common.calibrated_attenuation = 13;
    state.wifi.tx_gain_adjustment = -3;
    state
        .wifi
        .calibration
        .set(WifiCalibrationStatus::BASEBAND, true);
    state
        .wifi
        .calibration
        .set(WifiCalibrationStatus::PWDET, true);
    state
        .wifi
        .calibration
        .set(WifiCalibrationStatus::TX_POWER, true);
    state
        .wifi
        .calibration
        .set(WifiCalibrationStatus::TX_IQ, true);
    state
        .wifi
        .calibration
        .set(WifiCalibrationStatus::RX_GAIN_DC, true);
    state
        .wifi
        .calibration
        .set(WifiCalibrationStatus::RX_GAIN_TABLES, true);
    state
        .wifi
        .calibration
        .set(WifiCalibrationStatus::RX_SATURATION, true);
    state.wifi.tx_dco = [[14; 4]; 5];
    state.wifi.tx_reference_codes = [15, 16];
    state.wifi.tx_capacitance = [17; 6];
    state.wifi.tx_power_curve = [18, 19, 20];
    state.wifi.tx_power_corrections = [21, 22, 23];
    state.wifi.tx_power_adjustment = 24;
    state.wifi.tx_iq_config = 25;
    state.wifi.tx_iq_coefficient = 26;
    state.wifi.rx_iq_coefficients = [27; 4];
    state.wifi.external_dcode = [28, 29];
    state.wifi.calibration_temperature = 30;
    state.wifi.current_channel = 11;
    state.wifi.wifi_rx_table_last_index = PHY_WIFI_RX_GAIN_LAST_INDEX;
    state.wifi.shared_rx_table_last_index = PHY_SHARED_RX_GAIN_LAST_INDEX;
    state.wifi.wifi_index_dc = [[31; 2]; 8];
    state.wifi.wifi_fine_dc = [[32; 2]; crate::rx::gain_calibration::FINE_CODES];
    state.wifi.shared_index_dc = [[33; 2]; 11];
    state.bluetooth.tx_dc_calibrated = true;
    state.bluetooth.tx_power_calibrated = true;
    state.bluetooth.tx_dco = [[35; 4]; 3];
    state.bluetooth.tx_power_curve = [36, 37, 38];
    state.bluetooth.tx_power_corrections = [39, 40, 41];
    state.bluetooth.tx_power_adjustment = 42;
    state.calibration_cache(IDENTITY)
}

#[test]
fn rfpll_tracking_reference_is_initialized_and_committed_only_on_update() {
    let mut state = PhyState::new(PhyConfig::production());
    state.apply_register_temperature_outcome(
        PhyRegisterTemperatureControl::FULL,
        PhyTemperatureOutcome {
            temperature: 20,
            sensor_index: 3,
            next_dac: 4,
        },
    );
    assert_eq!(
        state.rfpll_tracking_request(None),
        crate::tracking::rfpll::thermal::Request {
            current_temperature: 20,
            reference_temperature: 20,
            threshold_override: None,
            current_channel: 0,
        }
    );

    state.apply_temperature_outcome(PhyTemperatureOutcome {
        temperature: 25,
        sensor_index: 3,
        next_dac: 4,
    });
    state.commit_rfpll_tracking(crate::tracking::rfpll::thermal::Outcome {
        reference_temperature: 99,
        correction: None,
    });
    assert_eq!(
        state.rfpll_tracking_request(Some(7)).reference_temperature,
        20
    );

    state.commit_rfpll_tracking(crate::tracking::rfpll::thermal::Outcome {
        reference_temperature: 25,
        correction: Some(crate::tracking::rfpll::Outcome {
            search: crate::tracking::rfpll::search::Outcome {
                initial_cap: 100,
                selected_cap: 100,
                accepted_samples: 0,
            },
            memory: None,
        }),
    });
    assert_eq!(state.rfpll_tracking_request(None).reference_temperature, 25);
}

#[test]
fn calibration_tracking_references_are_semantic_and_commit_per_branch() {
    let mut state = PhyState::new(PhyConfig::production());
    state.apply_register_temperature_outcome(
        PhyRegisterTemperatureControl::FULL,
        PhyTemperatureOutcome {
            temperature: 20,
            sensor_index: 2,
            next_dac: 15,
        },
    );
    // A cold registration commits the common reference with its RX-gain
    // tables, not at its temperature step.
    assert_eq!(
        state
            .calibration_tracking_parameters(None)
            .common_reference_temperature,
        0
    );
    state.apply_rx_gain_init_outcome(crate::rx::gain::PhyRxGainInitOutcome {
        dc: None,
        generated_tables: true,
        wifi_last_index: 69,
        shared_last_index: 75,
    });
    let initial = state.calibration_tracking_parameters(None);
    assert_eq!(initial.common_reference_temperature, 20);
    assert_eq!(initial.transmit_reference_temperature, 20);
    assert_eq!(initial.transmit_reference_temperature, 20);

    state.apply_calibration_tracking_outcome(PhyCalibrationTrackingOutcome {
        clients: (crate::tracking::parameters::PhyCalibrationTrackClass::Wifi).clients(),
        threshold: 30,
        common_reference_temperature: 50,
        transmit_reference_temperature: 50,

        common_updated: true,
        transmit_updated: true,
        dcode: Some(PhyDcodeOutcome { codes: [7; 8] }),
        rx_gain: Some(crate::rx::gain::PhyRxGainInitOutcome {
            dc: Some(crate::rx::gain_calibration::PhyRxGainDcOutcome {
                quality: Default::default(),
                wifi_index_dc: [[1; 2]; 8],
                wifi_fine_dc: [[2; 2]; crate::rx::gain_calibration::FINE_CODES],
                shared_index_dc: [[3; 2]; 11],
            }),
            generated_tables: true,
            wifi_last_index: 69,
            shared_last_index: 75,
        }),
        channel: Some(crate::channel::PhyChipChannelOutcome {
            channel: 11,
            frequency_mhz: 2_462,
            cbw: 1,
            init_complete: true,
            temperature: PhyTemperatureOutcome {
                temperature: 50,
                sensor_index: 3,
                next_dac: 15,
            },
        }),
        wifi_tx_dc_pwdet: Some(crate::tx::dc_power_detector::PhyTxDcPwdetOutcome {
            dco: [[5; 4]; 3],
            total_measurements: 144,
        }),
        bluetooth_ieee802154_tx_dc_pwdet: None,
    });
    let committed = state.calibration_tracking_parameters(Some(31));
    assert_eq!(committed.threshold_override, Some(31));
    assert_eq!(committed.common_reference_temperature, 50);
    assert_eq!(committed.transmit_reference_temperature, 50);

    assert_eq!(state.common.dcode, [7; 8]);
    assert_eq!(state.wifi.wifi_rx_table_last_index, 69);
    assert_eq!(state.wifi.shared_rx_table_last_index, 75);
    assert_eq!(state.wifi.wifi_index_dc, [[1; 2]; 8]);
    assert_eq!(state.wifi.shared_index_dc, [[3; 2]; 11]);
    assert!(
        state
            .wifi
            .calibration
            .contains(WifiCalibrationStatus::RX_GAIN_DC)
    );
    assert!(
        state
            .wifi
            .calibration
            .contains(WifiCalibrationStatus::RX_GAIN_TABLES)
    );
    assert_eq!(state.current_wifi_channel(), 11);
    assert_eq!(state.common.temperature, 50);
    assert_eq!(state.tx_dc_pwdet_parameters().dco, [[5; 4]; 3]);

    state.apply_calibration_tracking_outcome(PhyCalibrationTrackingOutcome {
        clients: (crate::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154)
            .clients(),
        threshold: 30,
        common_reference_temperature: 60,
        transmit_reference_temperature: 60,

        common_updated: true,
        transmit_updated: true,
        dcode: None,
        rx_gain: None,
        channel: None,
        bluetooth_ieee802154_tx_dc_pwdet: Some(crate::tx::dc_power_detector::PhyTxDcPwdetOutcome {
            dco: [[6; 4]; 3],
            total_measurements: 144,
        }),
        wifi_tx_dc_pwdet: None,
    });
    let rejected = state.calibration_tracking_parameters(None);
    assert_eq!(rejected.common_reference_temperature, 50);
    assert_eq!(rejected.transmit_reference_temperature, 50);
    assert_eq!(state.common.dcode, [7; 8]);

    state.apply_calibration_tracking_outcome(PhyCalibrationTrackingOutcome {
        clients: (crate::tracking::parameters::PhyCalibrationTrackClass::Wifi).clients(),
        threshold: 30,
        common_reference_temperature: 70,
        transmit_reference_temperature: 70,

        common_updated: true,
        transmit_updated: false,
        dcode: Some(PhyDcodeOutcome { codes: [9; 8] }),
        rx_gain: Some(crate::rx::gain::PhyRxGainInitOutcome {
            dc: None,
            generated_tables: true,
            wifi_last_index: 69,
            shared_last_index: 75,
        }),
        channel: Some(crate::channel::PhyChipChannelOutcome {
            channel: 11,
            frequency_mhz: 2_462,
            cbw: 1,
            init_complete: true,
            temperature: PhyTemperatureOutcome {
                temperature: 70,
                sensor_index: 3,
                next_dac: 15,
            },
        }),
        wifi_tx_dc_pwdet: None,
        bluetooth_ieee802154_tx_dc_pwdet: None,
    });
    let rejected_partial_rx = state.calibration_tracking_parameters(None);
    assert_eq!(rejected_partial_rx.common_reference_temperature, 50);
    assert_eq!(state.common.dcode, [7; 8]);
}

#[test]
fn periodic_gain_tracking_commits_only_terminal_runtime_outcomes() {
    let mut state = PhyState::new(PhyConfig::production());
    let immutable_config = state.config;
    state.apply_temperature_outcome(PhyTemperatureOutcome {
        temperature: 25,
        sensor_index: 3,
        next_dac: 4,
    });
    assert_eq!(
        state.tx_power_tracking_parameters(true),
        PhyTxPowerTrackingParameters {
            current_temperature: 25,
            reference_temperature: 0,
            previous_tracking_temperature: 0,
            previous_tracking_gain_base: 0,
            wifi_gain_base: 0,
            bluetooth_ieee802154_gain_base: 0,
            relaxed_threshold: true,
            wifi_gain_publication_skipped: false,
        }
    );

    state.apply_tx_power_tracking_outcome(PhyTxPowerTrackingOutcome {
        class: crate::tracking::parameters::PhyCalibrationTrackClass::BluetoothIeee802154,
        gain_updated: true,
        tracking_temperature: 25,
        tracking_gain_base: 5,
        wifi_gain_base: 0,
        bluetooth_ieee802154_gain_base: 5,
    });
    assert_eq!(state.bluetooth_tx_gain_parameters().base, 5);
    assert_eq!(state.channel_parameters().tx_gain_base, 0);
    assert_eq!(state.config, immutable_config);
    assert_eq!(
        state.tx_power_tracking_parameters(false),
        PhyTxPowerTrackingParameters {
            current_temperature: 25,
            reference_temperature: 0,
            previous_tracking_temperature: 25,
            previous_tracking_gain_base: 5,
            wifi_gain_base: 0,
            bluetooth_ieee802154_gain_base: 5,
            relaxed_threshold: false,
            wifi_gain_publication_skipped: false,
        }
    );

    state.apply_tx_power_tracking_outcome(PhyTxPowerTrackingOutcome {
        class: crate::tracking::parameters::PhyCalibrationTrackClass::Wifi,
        gain_updated: false,
        tracking_temperature: -60,
        tracking_gain_base: -12,
        wifi_gain_base: -12,
        bluetooth_ieee802154_gain_base: -12,
    });
    assert_eq!(state.bluetooth_tx_gain_parameters().base, 5);
    assert_eq!(state.channel_parameters().tx_gain_base, 0);
}

#[test]
fn tracking_gain_regeneration_uses_live_typed_state_and_honors_wifi_skip() {
    let mut state = PhyState::new(PhyConfig::production());
    let mut bluetooth = state.bluetooth_tx_gain_parameters();
    bluetooth.base = (-7_i8) as u8;
    assert_eq!(
        state.bluetooth_ieee802154_tracking_gain_image(-7),
        calculate_bluetooth_tx_gain(bluetooth)
    );

    let parameters = state.channel_parameters();
    let mut wifi = calculate_wifi_tx_gain(PhyWifiTxGainRequest {
        channel: 11,
        calibration_curve: parameters.tx_gain_curve,
        correction: parameters.tx_gain_correction,
        base_and_delta: (5_u8).wrapping_add(parameters.tx_gain_adjustment as u8) as i8,
    });
    wifi.seed = parameters.tx_gain_seed;
    wifi.config = parameters.tx_gain_config;
    assert_eq!(state.wifi_tracking_gain_image(11, 5), Some(wifi));

    state.config.tx_gain_skip_publication = true;
    assert_eq!(state.wifi_tracking_gain_image(11, 5), None);
}

#[test]
fn calibration_gain_regeneration_uses_pending_txdc_seed_before_state_commit() {
    let mut state = PhyState::new(PhyConfig::production());
    let pending = PhyTxDcPwdetOutcome {
        dco: [[0x1234; 4]; 3],
        total_measurements: 144,
    };
    let expected_seed = [0x1234_1234; 6];

    assert_ne!(state.channel_parameters().tx_gain_seed, expected_seed);
    assert_ne!(state.bluetooth_tx_gain_parameters().seed, expected_seed);
    assert_eq!(
        state.wifi_calibration_gain_image(11, pending).unwrap().seed,
        expected_seed
    );
    assert_eq!(
        state
            .bluetooth_ieee802154_calibration_gain_image(pending)
            .seed,
        expected_seed
    );

    state.config.tx_gain_skip_publication = true;
    assert_eq!(state.wifi_calibration_gain_image(11, pending), None);
}

#[test]
fn cache_contains_calibration_but_not_runtime_role_state() {
    let mut calibrated = PhyState::new(PhyConfig::production());
    calibrated.set_dot11p_configuration(1, 4);
    calibrated.set_current_level(9);
    calibrated.set_tx_power_tracking_slow(0);
    calibrated.set_bt_power_tracking(0);
    calibrated.set_ble_channel_base(21);
    calibrated.mark_baseband_calibration_complete();
    calibrated.apply_tx_power_outcome(PhyTxPowerOutcome {
        reference_codes: [80, 120],
        power_curve: [-3, 4, 5],
        point_corrections: [6, -7, 8],
        power_adjustment: -9,
        final_attenuation: 13,
        current_channel: 11,
        calibration_performed: true,
    });

    let cache = calibrated.calibration_cache(IDENTITY);
    let snapshot = cache.snapshot();
    assert!(snapshot.wifi.baseband_calibrated);
    assert!(snapshot.wifi.tx_power_calibrated);
    assert_eq!(snapshot.wifi.calibrated_attenuation, 13);
    assert_eq!(snapshot.wifi.tx_power_curve, [-3, 4, 5]);
    assert_eq!(snapshot.wifi.tx_power_corrections, [6, -7, 8]);
    assert_eq!(snapshot.wifi.tx_power_adjustment, -9);

    let restored = PhyState::new(PhyConfig::esp32s31_default());
    assert_eq!(
        restored.dot11p_configuration(),
        PhyDot11pConfiguration {
            enabled: 0,
            configuration: 0
        }
    );
    assert_eq!(restored.current_level(), 0);
    assert_eq!(restored.tx_power_tracking_slow(), 1);
    assert_eq!(restored.bt_power_tracking(), 1);
    assert_eq!(restored.ble_channel_base(), 0);
    assert!(
        !restored
            .channel_frequency_control()
            .frequency_table_initialized
    );
}

#[test]
fn cache_schema_is_checked_before_artifact_admission() {
    let state = PhyState::new(PhyConfig::production());
    let cache = state.calibration_cache(IDENTITY);
    let mut snapshot = cache.into_snapshot();
    snapshot.schema += 1;
    assert!(PhyCalibrationCache::from_snapshot(snapshot).is_none());
}

#[test]
fn cache_replay_validation_rejects_identity_incomplete_and_invalid_table_shape() {
    let cache = complete_calibration_cache();
    assert_eq!(cache.validate_for_replay(IDENTITY), Ok(()));
    assert_eq!(
        cache.validate_for_replay(crate::calibration::registration::PhyCalibrationIdentity {
            rf_cal_version: IDENTITY.rf_cal_version + 1,
            ..IDENTITY
        }),
        Err(PhyCalibrationCacheError::IdentityMismatch)
    );

    let mut incomplete = cache.into_snapshot();
    incomplete.wifi.tx_iq_calibrated = false;
    assert_eq!(
        PhyCalibrationCache::from_snapshot(incomplete)
            .unwrap()
            .validate_for_replay(IDENTITY),
        Err(PhyCalibrationCacheError::IncompleteCalibration)
    );

    let mut invalid_table = complete_calibration_cache().into_snapshot();
    invalid_table.wifi.shared_rx_table_last_index -= 1;
    assert_eq!(
        PhyCalibrationCache::from_snapshot(invalid_table)
            .unwrap()
            .validate_for_replay(IDENTITY),
        Err(PhyCalibrationCacheError::InvalidRxGainTable)
    );
}

#[test]
fn cached_calibration_restores_products_but_resets_runtime_and_hardware_epoch_state() {
    let cache = complete_calibration_cache();
    let snapshot = *cache.snapshot();
    let config = PhyConfig::production();
    let mut state = PhyState::new(PhyConfig::esp32s31_default());
    state.set_dot11p_configuration(1, 7);
    state.set_current_level(8);
    state.set_bt_power_tracking(0);
    state.set_ble_channel_base(9);
    state.common.registered = true;
    state.common.frequency_table_initialized = true;
    state.wifi.channel_initialized = true;

    state
        .begin_cached_calibration(config, &cache, IDENTITY)
        .unwrap();

    assert_eq!(state.config, config);
    assert_eq!(state.common.temperature, snapshot.common.temperature);
    assert_eq!(
        state.common.references.common,
        snapshot.common.rxcal_reference_temperature
    );
    assert_eq!(
        state.common.references.transmit,
        snapshot.common.txcal_reference_temperature
    );
    assert_eq!(
        state.common.references.rfpll,
        snapshot.common.rfpll_reference_temperature
    );
    assert_eq!(state.common.calibrated_attenuation, 13);
    assert_eq!(state.wifi.tx_dco, snapshot.wifi.tx_dco);
    assert_eq!(state.wifi.wifi_index_dc, snapshot.wifi.wifi_index_dc);
    assert_eq!(state.bluetooth.tx_dco, snapshot.bluetooth.tx_dco);
    assert!(
        state
            .wifi
            .calibration
            .contains(WifiCalibrationStatus::BASEBAND)
    );
    assert!(
        state
            .wifi
            .calibration
            .contains(WifiCalibrationStatus::RX_GAIN_DC)
    );
    assert!(
        !state
            .wifi
            .calibration
            .contains(WifiCalibrationStatus::RX_GAIN_TABLES)
    );
    assert!(!state.common.frequency_table_initialized);
    assert!(!state.wifi.channel_initialized);
    assert!(!state.common.registered);
    assert_eq!(
        state.common.temperature_acquisition,
        crate::tracking::temperature::StoredAcquisition::UNOBSERVED
    );
    assert_eq!(state.dot11p_configuration().enabled, 0);
    assert_eq!(state.current_level(), 0);
    assert_eq!(state.bt_power_tracking(), 1);
    assert_eq!(state.ble_channel_base(), 0);
    assert_eq!(
        state.wifi.tracking_gain_base,
        config.initial_wifi_tx_gain_base
    );
    assert_eq!(
        state.bluetooth.tracking_gain_base,
        config.initial_bluetooth_tx_gain_base
    );
}

#[test]
fn rejected_cache_does_not_modify_live_state() {
    let mut cache = complete_calibration_cache().into_snapshot();
    cache.bluetooth.tx_power_calibrated = false;
    let cache = PhyCalibrationCache::from_snapshot(cache).unwrap();
    let mut state = PhyState::new(PhyConfig::esp32s31_default());
    state.set_current_level(19);
    let before = state.calibration_snapshot(IDENTITY);

    assert_eq!(
        state.begin_cached_calibration(PhyConfig::production(), &cache, IDENTITY),
        Err(PhyCalibrationCacheError::IncompleteCalibration)
    );
    assert_eq!(state.calibration_snapshot(IDENTITY), before);
    assert_eq!(state.current_level(), 19);
}

#[test]
fn runtime_views_are_independent_named_state() {
    let mut state = PhyState::default();
    state.set_dot11p_configuration(1, 0x5a);
    state.set_current_level(0x34);
    state.set_bt_power_tracking(0x12);
    state.set_ble_channel_base(0x56);
    state.set_initialization_parameter(u32::MAX);
    state.set_temperature_tracking_debug(0x78, 0x9a);
    state.set_tx_power_tracking_slow(0xbc);

    assert_eq!(
        state.dot11p_configuration(),
        PhyDot11pConfiguration {
            enabled: 1,
            configuration: 0x5a,
        }
    );
    assert_eq!(state.current_level(), 0x34);
    assert_eq!(state.bt_power_tracking(), 0x12);
    assert_eq!(state.ble_channel_base(), 0x56);
    assert!(state.initialization_parameter());
    assert_eq!(
        state.temperature_tracking_debug(),
        PhyTemperatureTrackingDebug {
            first: 0x78,
            second: 0x9a,
        }
    );
    assert_eq!(state.tx_power_tracking_slow(), 0xbc);
}

#[test]
fn generated_rx_gain_tables_commit_the_latest_temperature_as_common_reference() {
    let mut state = PhyState::default();
    state.common.temperature = 27;
    state.common.references.common = 20;
    let outcome = crate::rx::gain::PhyRxGainInitOutcome {
        dc: None,
        generated_tables: false,
        wifi_last_index: 69,
        shared_last_index: 75,
    };
    state.apply_rx_gain_init_outcome(outcome);
    assert_eq!(state.common.references.common, 20);
    state.apply_rx_gain_init_outcome(crate::rx::gain::PhyRxGainInitOutcome {
        generated_tables: true,
        ..outcome
    });
    assert_eq!(state.common.references.common, 27);
}

#[test]
fn registration_restarts_power_tracking_from_the_power_reference() {
    let temperature = |temperature| PhyTemperatureOutcome {
        temperature,
        sensor_index: 2,
        next_dac: 15,
    };
    let mut state = PhyState::new(PhyConfig::production());
    state.apply_register_temperature_outcome(PhyRegisterTemperatureControl::FULL, temperature(23));
    let power = state.tx_power_tracking_parameters(false);
    assert_eq!(power.reference_temperature, 23);
    assert_eq!(power.previous_tracking_temperature, 23);

    // A registration after the TX-power calibration keeps the reference and
    // restarts tracking from it, whatever the sample.
    state.common.references.power_tracking = 40;
    state
        .wifi
        .calibration
        .set(WifiCalibrationStatus::TX_POWER, true);
    let control = state.register_temperature_control();
    state.apply_register_temperature_outcome(control, temperature(30));
    let power = state.tx_power_tracking_parameters(false);
    assert_eq!(power.reference_temperature, 23);
    assert_eq!(power.previous_tracking_temperature, 23);
}

#[test]
fn rx_table_preparation_updates_both_semantic_indices() {
    let mut state = PhyState::default();
    assert_eq!(
        state.prepare_rx_table_init(),
        PhyRxTableInitParameters {
            parameter_002: 0xbf,
            parameter_121: PHY_RX_TABLE_ENTRY_COUNT,
        }
    );
    assert_eq!(
        state.register_init_parameters(),
        PhyRegisterInitParameters {
            parameter_121: PHY_RX_TABLE_ENTRY_COUNT,
            parameter_120: PHY_RX_TABLE_ENTRY_COUNT,
        }
    );
}

#[test]
fn rx_saturation_is_one_way_and_failures_do_not_commit() {
    let mut state = PhyState::default();
    assert_eq!(state.rx_saturation_parameter_002(), 0xbf);
    assert_eq!(
        state.apply_rx_saturation_outcome(PhyRxSaturationOutcome::CaptureTimedOut),
        Err(PhyRxSaturationOutcome::CaptureTimedOut)
    );
    assert!(!state.rx_gain_dc_parameters().rx_saturation_detected);

    state
        .apply_rx_saturation_outcome(PhyRxSaturationOutcome::Measured {
            saturated_samples: 1,
            samples: 100,
        })
        .unwrap();
    state
        .apply_rx_saturation_outcome(PhyRxSaturationOutcome::Measured {
            saturated_samples: 0,
            samples: 100,
        })
        .unwrap();
    assert!(state.rx_gain_dc_parameters().rx_saturation_detected);
}

#[test]
fn calibration_outputs_have_named_cache_fields() {
    let mut state = PhyState::default();
    state.apply_temperature_outcome(PhyTemperatureOutcome {
        temperature: -37,
        sensor_index: 3,
        next_dac: 11,
    });
    state.apply_dcode_outcome(PhyDcodeOutcome {
        codes: [1, 2, 3, 4, 5, 6, 7, 8],
    });

    let snapshot = state.calibration_cache(IDENTITY).into_snapshot();
    assert_eq!(snapshot.common.temperature, -37);
    assert_eq!(snapshot.common.sensor_index, 3);
    assert_eq!(snapshot.common.dcode, [1, 2, 3, 4, 5, 6, 7, 8]);
}

#[test]
fn wifi_tx_calibration_updates_only_wifi_calibration_data() {
    let mut state = PhyState::default();
    let bluetooth_before = state.bluetooth_tx_dco();
    let outcome = PhyTxDcOutcome {
        dco: [
            [1, 2, 3, 4],
            [5, 6, 7, 8],
            [9, 10, 11, 12],
            [13, 14, 15, 16],
            [17, 18, 19, 20],
        ],
    };
    state.apply_tx_dc_outcome(outcome);
    state.apply_pwdet_outcome(PhyPwdetOutcome {
        reference_codes: [-101, 202],
        calibrated: true,
        measurement_performed: true,
    });

    assert_eq!(
        state.tx_dc_pwdet_parameters().dco,
        [outcome.dco[0], outcome.dco[1], outcome.dco[2]]
    );
    assert_eq!(state.pwdet_parameters().reference_codes, [-101, 202]);
    assert!(state.pwdet_parameters().already_calibrated);
    assert_eq!(state.bluetooth_tx_dco(), bluetooth_before);
}

#[test]
fn rf_prefix_consumes_only_typed_views() {
    let mut state = PhyState::default();
    assert!(!state.rc_calibration_complete());
    assert_eq!(
        state.xtal_duty_parameters(),
        XtalDutyCalibrationParameters {
            restore_duty: 0,
            rf_frequency_offset_base: 0,
            pbus_rx_path_value: 0xbf,
        }
    );
    assert_eq!(
        state.channel_frequency_control(),
        PhyChannelFrequencyInitControl {
            frequency_register_parameter_override: false,
            frequency_table_initialized: false,
            front_end_parameter_bit: true,
        }
    );

    state.apply_rc_calibration(45);
    let snapshot = state.calibration_cache(IDENTITY).into_snapshot();
    assert!(snapshot.common.rc_calibrated);
    assert_ne!(snapshot.common.filter_dcap, [0; 5]);
}

#[test]
fn wifi_gain_adjustment_is_separate_from_bluetooth_attenuation() {
    let mut state = PhyState::new(PhyConfig::production());
    let baseline = state.wifi_tracking_gain_image(13, 127).unwrap();
    state.config.bluetooth_tx_gain_attenuation = 31;
    assert_eq!(state.wifi_tracking_gain_image(13, 127), Some(baseline));
    let bluetooth = state.bluetooth_ieee802154_tracking_gain_image(0);
    state.wifi.tx_gain_adjustment = 1;
    assert_eq!(state.bluetooth_ieee802154_tracking_gain_image(0), bluetooth);
    assert_ne!(state.wifi_tracking_gain_image(13, 127), Some(baseline));
    assert_eq!(state.channel_parameters().tx_gain_adjustment, 1);
    assert_eq!(
        state.calibration_snapshot(IDENTITY).wifi.tx_gain_adjustment,
        1
    );
    let mut expected = calculate_wifi_tx_gain(PhyWifiTxGainRequest {
        channel: 13,
        calibration_curve: state.channel_parameters().tx_gain_curve,
        correction: state.channel_parameters().tx_gain_correction,
        base_and_delta: -128,
    });
    expected.seed = state.channel_parameters().tx_gain_seed;
    expected.config = state.channel_parameters().tx_gain_config;
    assert_eq!(state.wifi_tracking_gain_image(13, 127), Some(expected));
}

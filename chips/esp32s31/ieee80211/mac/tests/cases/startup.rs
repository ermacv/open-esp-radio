use crate::{support::*, *};

#[test]
fn mac_delay_slot_reproduces_vendor_modulo_eleven() {
    assert_eq!(MacDelaySlot::from_random(0).value(), 0);
    assert_eq!(MacDelaySlot::from_random(10).value(), 10);
    assert_eq!(MacDelaySlot::from_random(11).value(), 0);
    assert_eq!(MacDelaySlot::from_random(u32::MAX).value(), 3);
}

#[test]
fn unavailable_calibration_preserves_provenance_even_when_the_field_is_zero() {
    let unavailable = MacSlowClockCalibration::Unavailable;
    let measured_zero = MacSlowClockCalibration::from_osi_value(0);
    assert_ne!(unavailable, measured_zero);
    assert_eq!(unavailable.register_value(), measured_zero.register_value());
}

#[test]
fn cold_mac_init_orders_semantic_hardware_transactions() {
    let clock_trace = Rc::new(RefCell::new(Vec::new()));
    let mut platform = MockPlatform::default();
    let mut mmio = MockMmio {
        cold_handshake_result: Some(Ok(MacColdStartOutcome {
            handshake_samples: 0,
            handshake_observations: 1,
        })),
        cold_start_clock_trace: Some(clock_trace.clone()),
        ..MockMmio::default()
    };

    let station = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
    let access_point = [0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];
    let outcome = initialize_wifi_mac(
        &mut platform,
        &mut mmio,
        MacColdStartConfig {
            handshake_sample_limit: 4,
            station_address: station,
            access_point_address: access_point,
        },
    )
    .unwrap();
    assert_ne!(
        mmio.operations().last(),
        Some(&Operation::ConfigureOpenPromiscuousReceive)
    );
    activate_promiscuous_receive(&mut mmio);

    assert_eq!(outcome.handshake_samples, 0);
    assert_eq!(outcome.handshake_observations, 1);
    assert_eq!(
        *clock_trace.borrow(),
        [
            ColdStartClockEdge::SetWifiMacReset(true),
            ColdStartClockEdge::SetWifiMacReset(false),
        ]
    );
    assert_eq!(
        &mmio.operations()[..3],
        [
            Operation::SetWifiMacReset(true),
            Operation::SetWifiMacReset(false),
            Operation::BeginColdHandshake(4),
        ]
    );
    assert_eq!(
        &mmio.operations()[3..9],
        [
            Operation::InitializeTxRxPrefix,
            Operation::InitializeTxRxCallbacks(MacDelaySlot::from_random(7)),
            Operation::InitializeTxRxSuffix,
            Operation::InitializeColdReceivePolicy,
            Operation::InitializeRxBufferPrefix,
            Operation::InitializeHePrefix,
        ]
    );
    assert!(matches!(
        mmio.operations()[9],
        Operation::InitializeTxPower(_)
    ));
    assert_eq!(
        &mmio.operations()[10..16],
        [
            Operation::InitializeHeSuffix,
            Operation::InitializeLastRxBufferTable,
            Operation::DisablePhyLowRate,
            Operation::InitializeCryptoBypass,
            Operation::InitializeMacAntenna,
            Operation::InitializeHalTail(
                MacInterruptMask::COLD_RX,
                MacSlowClockCalibration::Unavailable,
            ),
        ]
    );
    assert!(matches!(
        mmio.operations()[16],
        Operation::InitializeColdCoex(_)
    ));
    assert_eq!(
        &mmio.operations()[17..],
        [
            Operation::EnableMacInterrupts(MacInterruptMask::COLD_RX),
            Operation::ProgramInterfaceAddress(MacInterface::Station, station),
            Operation::ProgramInterfaceAddress(MacInterface::AccessPoint, access_point),
            Operation::ConfigureOpenPromiscuousReceive,
        ]
    );
    let mut expected_platform = vec![PlatformOperation::MacDelayRandom];
    expected_platform.extend((0..43).map(PlatformOperation::TxPower));
    expected_platform.extend(
        (0..26)
            .filter(|rate| *rate != 4)
            .map(PlatformOperation::TxPower),
    );
    expected_platform.extend([
        PlatformOperation::SlowClockCalibration,
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event15),
        PlatformOperation::CoexPti(MacCoexEvent::Event1),
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event1),
        PlatformOperation::CoexPti(MacCoexEvent::Event1),
        PlatformOperation::CoexPti(MacCoexEvent::Event1),
        PlatformOperation::CoexPti(MacCoexEvent::Event1),
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event3),
        PlatformOperation::CoexPti(MacCoexEvent::Event10),
        PlatformOperation::CoexPti(MacCoexEvent::Event10),
    ]);
    assert_eq!(platform.operations, expected_platform);
}

#[test]
fn cold_mac_handshake_timeout_stops_mac_initialization() {
    let mut platform = MockPlatform::default();
    let mut mmio = MockMmio {
        cold_handshake_result: Some(Err(MacColdStartError::HandshakeTimedOut {
            samples: 2,
            sample_limit: 2,
        })),
        ..MockMmio::default()
    };

    assert_eq!(
        initialize_wifi_mac(
            &mut platform,
            &mut mmio,
            MacColdStartConfig {
                handshake_sample_limit: 2,
                station_address: [0; 6],
                access_point_address: [0; 6],
            },
        ),
        Err(MacColdStartError::HandshakeTimedOut {
            samples: 2,
            sample_limit: 2,
        })
    );
    assert_eq!(
        mmio.operations(),
        [
            Operation::SetWifiMacReset(true),
            Operation::SetWifiMacReset(false),
            Operation::BeginColdHandshake(2),
        ]
    );
}

#[test]
fn sta_link_rx_policy_forwards_one_bssid_transaction() {
    let bssid = [0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e];
    let mut mmio = MockMmio::default();

    configure_sta_link_receive_policy(&mut mmio, bssid);

    assert_eq!(mmio.operations(), &[Operation::ApplyStaLinkPolicy(bssid)]);
}

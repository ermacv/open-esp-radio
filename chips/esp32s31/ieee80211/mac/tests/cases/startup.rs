use crate::{support::*, *};
use oer_time::Clock;

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
fn cold_start_prepares_mac_without_opening_its_core() {
    let clock_trace = Rc::new(RefCell::new(Vec::new()));
    let mut platform = MockPlatform::default();
    let mut mmio = MockMmio {
        cold_handshake_ready_after: Some(0),
        cold_start_clock_trace: Some(clock_trace.clone()),
        ..MockMmio::default()
    };

    let station = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
    let access_point = [0x02, 0xaa, 0xbb, 0xcc, 0xdd, 0xee];
    let clock = oer_time_virtual::VirtualClock::default();
    let outcome = run(
        &clock,
        initialize_wifi_mac(
            &mut platform,
            &mut mmio,
            &clock,
            MacColdStartConfig {
                handshake_deadline: MAC_COLD_HANDSHAKE_DEADLINE,
                station_address: station,
                access_point_address: access_point,
            },
        ),
    )
    .unwrap();
    assert_ne!(
        mmio.operations().last(),
        Some(&Operation::ConfigureOpenPromiscuousReceive)
    );
    activate_promiscuous_receive(&mut mmio);
    assert!(mmio.mac_gates_closed);
    assert!(mmio.runtime_stop_held);

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
        &mmio.operations()[..5],
        [
            Operation::SetWifiMacReset(true),
            Operation::SetWifiMacReset(false),
            Operation::RequestColdHandshake,
            Operation::SampleColdHandshake,
            Operation::FinishColdHandshake,
        ]
    );
    assert_eq!(
        &mmio.operations()[5..12],
        [
            Operation::HoldMacRuntime,
            Operation::InitializeTxRxPrefix,
            Operation::InitializeTxRxCallbacks(MacDelaySlot::from_random(7)),
            Operation::InitializeTxRxSuffix,
            Operation::InitializeColdReceivePolicy,
            Operation::InitializeRxBufferPrefix,
            Operation::InitializeHePrefix,
        ]
    );
    assert!(matches!(
        mmio.operations()[12],
        Operation::InitializeTxPower(_)
    ));
    assert_eq!(
        &mmio.operations()[13..19],
        [
            Operation::InitializeHeSuffix,
            Operation::InitializeLastRxBufferTable,
            Operation::DisablePhyLowRate,
            Operation::InitializeCryptoBypass,
            Operation::InitializeMacAntenna,
            Operation::InitializeHalTail(
                MacInterruptMask::STATION,
                MacSlowClockCalibration::Unavailable,
            ),
        ]
    );
    assert!(matches!(
        mmio.operations()[19],
        Operation::InitializeColdCoex(_)
    ));
    assert_eq!(
        &mmio.operations()[20..],
        [
            Operation::PrepareMacInterrupts(MacInterruptMask::STATION),
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
fn cold_mac_handshake_resamples_every_ten_microseconds_until_ready() {
    let mut platform = MockPlatform::default();
    let mut mmio = MockMmio {
        cold_handshake_ready_after: Some(2),
        ..MockMmio::default()
    };
    let clock = oer_time_virtual::VirtualClock::default();
    let start = clock.now();

    let outcome = run(
        &clock,
        initialize_wifi_mac(
            &mut platform,
            &mut mmio,
            &clock,
            MacColdStartConfig {
                handshake_deadline: MAC_COLD_HANDSHAKE_DEADLINE,
                station_address: [0; 6],
                access_point_address: [0; 6],
            },
        ),
    )
    .unwrap();

    assert_eq!(outcome.handshake_samples, 2);
    assert_eq!(outcome.handshake_observations, 3);
    assert_eq!(
        clock.now().saturating_duration_since(start),
        oer_time::Duration::from_micros(20)
    );
}

#[test]
fn cold_mac_handshake_timeout_stops_mac_initialization() {
    let mut platform = MockPlatform::default();
    let mut mmio = MockMmio::default();
    let clock = oer_time_virtual::VirtualClock::default();
    let deadline = oer_time::Duration::from_micros(30);

    let error = run(
        &clock,
        initialize_wifi_mac(
            &mut platform,
            &mut mmio,
            &clock,
            MacColdStartConfig {
                handshake_deadline: deadline,
                station_address: [0; 6],
                access_point_address: [0; 6],
            },
        ),
    )
    .unwrap_err();

    // Samples at 0, 10, 20 and 30 µs; the one at the deadline ends it.
    assert_eq!(
        error,
        MacColdStartError::HandshakeTimedOut {
            samples: 4,
            deadline,
        }
    );
    let mut expected = vec![
        Operation::SetWifiMacReset(true),
        Operation::SetWifiMacReset(false),
        Operation::RequestColdHandshake,
    ];
    expected.extend([Operation::SampleColdHandshake; 4]);
    assert_eq!(mmio.operations(), expected);
}

#[test]
fn sta_link_rx_policy_forwards_one_bssid_transaction() {
    let bssid = [0xdc, 0x15, 0xc8, 0x54, 0xbc, 0x1e];
    let mut mmio = MockMmio::default();

    configure_sta_link_receive_policy(&mut mmio, bssid);

    assert_eq!(mmio.operations(), &[Operation::ApplyStaLinkPolicy(bssid)]);
}

use crate::{
    AcceptListChange, AdvertisingChannel, AdvertisingChannels, AdvertisingConfiguration,
    AdvertisingEvent, AdvertisingPdu, AdvertisingReception, AdvertisingSetId, CoexistenceLevel,
    ConnectionEvent, ConnectionEventTiming, ConnectionId, DataChannel, DataPdu, DataPduKind,
    LeCapabilities, LeConnectionCapabilities, LeInstant, LePhy, LePhys, LeRadioCapabilities,
    LeWindow, LegacyAdvertisingTypes, LinkAcknowledgement, NoCapabilities, RadioDuration,
    RadioTiming, ScanFilterPolicy, ScanType, ScanWindow, ScannerConfiguration, ScannerId,
    Supported, TestChannel, TestPayloadType, TestPhy, TestReceive, TestTransmit, TxPower, Unplaced,
};

/// LE 1M, a nonconnectable advertiser, a passive scanner and a peripheral
/// with 27-octet payloads, from -24 dBm, one second ahead.
#[derive(Debug, PartialEq)]
enum Peripheral {}

impl LeCapabilities for Peripheral {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        legacy_advertising: LegacyAdvertisingTypes {
            nonconnectable: true,
            responsive: false,
        },
        passive_scanning: true,
        active_scanning: false,
        filter_accept_list: false,
        peripheral_connection: Some(LeConnectionCapabilities {
            max_data_payload: 27,
            link_acknowledgement: LinkAcknowledgement::Hardware,
            max_first_receive_wait: RadioDuration::from_micros(65_473),
            max_receive_wait: RadioDuration::from_micros(131_070),
        }),
        direct_test_mode: true,
        phys: LePhys::LE_1M,
        test_phys: LePhys::LE_1M.with(LePhy::Le2M),
        min_tx_power_dbm: -24,
        max_ahead: RadioDuration::from_micros(1_000_000),
        timing: RadioTiming {
            preparation_lead: RadioDuration::from_micros(500),
            ..RadioTiming::MINIMAL
        },
    };
}

const ADV_NONCONN_IND: [u8; 8] = [0x02, 6, 1, 2, 3, 4, 5, 6];
const ADV_IND: [u8; 8] = [0x00, 6, 1, 2, 3, 4, 5, 6];
/// `ADV_NONCONN_IND` with the RFU header bit set.
const ADV_NONCONN_IND_RFU: [u8; 8] = [0x12, 6, 1, 2, 3, 4, 5, 6];

fn advertising(
    pdu: &'static [u8],
    reception: AdvertisingReception<'static>,
    phy: LePhy,
    dbm: i8,
) -> AdvertisingConfiguration<'static> {
    AdvertisingConfiguration {
        set: AdvertisingSetId::new(0),
        pdu: AdvertisingPdu::new(pdu).unwrap(),
        reception,
        tx_power: TxPower::from_dbm(dbm),
        phy,
    }
}

fn scanner(scan_type: ScanType, filter_policy: ScanFilterPolicy) -> ScannerConfiguration {
    ScannerConfiguration {
        scanner: ScannerId::new(0),
        scan_type,
        filter_policy,
        tx_power: TxPower::from_dbm(0),
        phy: LePhy::Le1M,
    }
}

fn window(start: u64, duration: u64) -> LeWindow {
    LeWindow::new(
        LeInstant::from_micros(start),
        RadioDuration::from_micros(duration),
    )
    .unwrap()
}

fn scan(start: u64, duration: u64) -> ScanWindow {
    ScanWindow {
        scanner: ScannerId::new(0),
        channel: AdvertisingChannel::Channel37,
        window: window(start, duration),
    }
}

fn advertise(anchor: u64, spacing: u64) -> AdvertisingEvent {
    AdvertisingEvent {
        set: AdvertisingSetId::new(0),
        anchor: LeInstant::from_micros(anchor),
        channels: AdvertisingChannels::ALL,
        channel_spacing: RadioDuration::from_micros(spacing),
        coexistence: CoexistenceLevel::Baseline,
    }
}

#[test]
fn an_advertising_set_needs_a_served_pdu_type_reception_phy_and_power() {
    let new = Supported::<Peripheral, _>::new;
    assert!(
        new(advertising(
            &ADV_NONCONN_IND,
            AdvertisingReception::None,
            LePhy::Le1M,
            0
        ))
        .is_ok()
    );
    // The nonconnectable type listens for nothing.
    assert_eq!(
        new(advertising(
            &ADV_NONCONN_IND,
            AdvertisingReception::Report,
            LePhy::Le1M,
            0
        )),
        Err(Unplaced::Unsupported)
    );
    // No responsive type is served here.
    assert_eq!(
        new(advertising(
            &ADV_IND,
            AdvertisingReception::Report,
            LePhy::Le1M,
            0
        )),
        Err(Unplaced::Unsupported)
    );
    // The RFU header bit stays clear.
    assert_eq!(
        new(advertising(
            &ADV_NONCONN_IND_RFU,
            AdvertisingReception::None,
            LePhy::Le1M,
            0
        )),
        Err(Unplaced::Unsupported)
    );
    assert_eq!(
        new(advertising(
            &ADV_NONCONN_IND,
            AdvertisingReception::None,
            LePhy::Le2M,
            0
        )),
        Err(Unplaced::Unsupported)
    );
    // A power above the lowest level rounds down; below it is excluded.
    assert!(
        new(advertising(
            &ADV_NONCONN_IND,
            AdvertisingReception::None,
            LePhy::Le1M,
            40
        ))
        .is_ok()
    );
    assert_eq!(
        new(advertising(
            &ADV_NONCONN_IND,
            AdvertisingReception::None,
            LePhy::Le1M,
            -25
        )),
        Err(Unplaced::Unsupported)
    );
}

#[test]
fn a_scanner_needs_its_type_and_filter() {
    let new = Supported::<Peripheral, _>::new;
    assert!(new(scanner(ScanType::Passive, ScanFilterPolicy::AcceptAll)).is_ok());
    assert!(new(scanner(ScanType::Active, ScanFilterPolicy::AcceptAll)).is_err());
    assert!(new(scanner(ScanType::Passive, ScanFilterPolicy::AcceptListOnly)).is_err());
    assert!(Supported::<Peripheral, _>::new(AcceptListChange::Clear).is_err());
}

#[test]
fn a_timed_value_is_placed_within_the_horizon_of_its_instant() {
    let now = LeInstant::from_micros(10_000);
    let placed = Supported::<Peripheral, _>::placed;
    // The reservation ends 600 us + 1 s - 600 us after now: at the horizon.
    assert!(placed(scan(10_500, 1_000_000 - 500), now).is_ok());
    assert_eq!(
        placed(scan(10_500, 1_000_000 - 499), now),
        Err(Unplaced::BeyondHorizon)
    );
    // A reservation that ends before its instant was placed too late to
    // mean anything.
    assert_eq!(placed(scan(1_000, 100), now), Err(Unplaced::BeyondHorizon));
    // The preparation lead before the epoch's start leaves the epoch.
    assert_eq!(
        placed(scan(100, 100), LeInstant::from_micros(0)),
        Err(Unplaced::OutsideEpoch)
    );
}

#[test]
fn an_advertising_event_reserves_each_channel_from_the_shared_reservation() {
    let now = LeInstant::from_micros(0);
    let placed = Supported::<Peripheral, _>::placed;
    assert!(placed(advertise(1_000, 1_500), now).is_ok());
    // A spacing no longer than the preparation lead leaves no air.
    assert_eq!(
        placed(advertise(1_000, 500), now),
        Err(Unplaced::Unsupported)
    );
    // The last channel's reservation counts: 2 * spacing past the anchor.
    assert_eq!(
        placed(advertise(1_000_000 - 2_000, 1_500), now),
        Err(Unplaced::BeyondHorizon)
    );
}

#[test]
fn data_and_tests_are_bounded_by_the_connection_and_test_capabilities() {
    let pdu = |length| DataPdu::new(DataPduKind::Start, &[0; 255][..length]).unwrap();
    assert!(Supported::<Peripheral, _>::new(pdu(27)).is_ok());
    assert!(Supported::<Peripheral, _>::new(pdu(28)).is_err());
    let test = |phy| TestReceive {
        channel: TestChannel::new(0).unwrap(),
        phy,
        window: window(2_000, 100),
        recurring: false,
        tx_power: TxPower::from_dbm(0),
    };
    let now = LeInstant::from_micros(0);
    assert!(Supported::<Peripheral, _>::placed(test(TestPhy::Le2M), now).is_ok());
    assert!(Supported::<Peripheral, _>::placed(test(TestPhy::LeCodedS8), now).is_err());
}

#[test]
fn nothing_is_supported_without_a_radio() {
    assert!(Supported::<NoCapabilities, _>::new(TxPower::from_dbm(0)).is_err());
    assert!(
        Supported::<NoCapabilities, _>::new(DataPdu::new(DataPduKind::Start, &[]).unwrap())
            .is_err()
    );
}

/// [`Peripheral`] with responsive advertising.
#[derive(Debug, PartialEq)]
enum Responsive {}

impl LeCapabilities for Responsive {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        legacy_advertising: LegacyAdvertisingTypes {
            nonconnectable: false,
            responsive: true,
        },
        ..Peripheral::CAPS
    };
}

const ADV_DIRECT_IND: [u8; 14] = [0x01, 12, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
const ADV_SCAN_IND: [u8; 8] = [0x06, 6, 1, 2, 3, 4, 5, 6];
const SCAN_RSP: [u8; 8] = [0x04, 6, 1, 2, 3, 4, 5, 6];

#[test]
fn each_responsive_type_listens_as_its_pdu_type_says() {
    let new = Supported::<Responsive, _>::new;
    let response = || AdvertisingReception::ScanResponse(AdvertisingPdu::new(&SCAN_RSP).unwrap());
    for pdu in [&ADV_IND, &ADV_SCAN_IND] {
        assert!(new(advertising(pdu, response(), LePhy::Le1M, 0)).is_ok());
        assert!(
            new(advertising(
                pdu,
                AdvertisingReception::Report,
                LePhy::Le1M,
                0
            ))
            .is_ok()
        );
        assert!(new(advertising(pdu, AdvertisingReception::None, LePhy::Le1M, 0)).is_err());
    }
    // A directed advertiser reports and answers nothing.
    assert!(
        new(advertising(
            &ADV_DIRECT_IND,
            AdvertisingReception::Report,
            LePhy::Le1M,
            0
        ))
        .is_ok()
    );
    assert!(new(advertising(&ADV_DIRECT_IND, response(), LePhy::Le1M, 0)).is_err());
    // This backend sends no nonconnectable advertising.
    assert!(
        new(advertising(
            &ADV_NONCONN_IND,
            AdvertisingReception::None,
            LePhy::Le1M,
            0
        ))
        .is_err()
    );
}

fn connection_event(timing: ConnectionEventTiming, priority: u8) -> ConnectionEvent {
    ConnectionEvent {
        connection: ConnectionId::new(0),
        channel: DataChannel::new(0).unwrap(),
        window: window(2_000, 1_000),
        interval: RadioDuration::from_micros(30_000),
        timing,
        priority,
        coexistence: CoexistenceLevel::Baseline,
    }
}

#[test]
fn a_connection_event_waits_within_the_backend_and_keeps_its_priority() {
    let micros = RadioDuration::from_micros;
    let placed = |event| Supported::<Peripheral, _>::placed(event, LeInstant::from_micros(0));
    let first = |transmit_window, timing_guard| ConnectionEventTiming::First {
        transmit_window: micros(transmit_window),
        timing_guard: micros(timing_guard),
    };
    // 65,473 us: the window and a guard on each side.
    assert!(placed(connection_event(first(65_273, 100), 15)).is_ok());
    assert_eq!(
        placed(connection_event(first(65_274, 100), 15)),
        Err(Unplaced::Unsupported)
    );
    // A first event needs a transmit window.
    assert!(placed(connection_event(first(0, 100), 8)).is_err());
    let recurring = |receive_wait| ConnectionEventTiming::Recurring {
        receive_wait: micros(receive_wait),
        widening: micros(16),
    };
    assert!(placed(connection_event(recurring(131_070), 8)).is_ok());
    assert!(placed(connection_event(recurring(131_071), 8)).is_err());
    // The scheduler priority is at most 15.
    assert!(placed(connection_event(recurring(1_000), 16)).is_err());
    // An interval lasts, and with its widening stays within the horizon.
    let with_interval = |interval| ConnectionEvent {
        interval: micros(interval),
        ..connection_event(recurring(1_000), 8)
    };
    assert!(placed(with_interval(0)).is_err());
    assert!(placed(with_interval(1_000_000 - 16)).is_ok());
    assert!(placed(with_interval(1_000_000 - 15)).is_err());
}

#[test]
fn a_test_payload_is_at_most_255_octets() {
    let payload = [0; 256];
    let test = |length: usize| TestTransmit {
        channel: TestChannel::new(0).unwrap(),
        phy: TestPhy::Le1M,
        window: window(2_000, 100),
        tx_power: TxPower::from_dbm(0),
        payload_type: TestPayloadType::new(0).unwrap(),
        payload: &payload[..length],
    };
    let now = LeInstant::from_micros(0);
    assert!(Supported::<Peripheral, _>::placed(test(255), now).is_ok());
    assert!(Supported::<Peripheral, _>::placed(test(256), now).is_err());
}

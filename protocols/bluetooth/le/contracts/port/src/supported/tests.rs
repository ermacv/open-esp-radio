use crate::{
    AdvertisingData, AdvertisingEvent, AdvertisingEventKind, AdvertisingFilterPolicy,
    AdvertisingReception, AdvertisingSet, ChannelSpacing, ConnectionEvent, ConnectionEventKind,
    ConnectionEventTiming, ConnectionPriority, ConnectionSetup, LeCapabilities,
    LeConnectionCapabilities, LeInstant, LeLimits, LePhy, LePhys, LeRadioCapabilities, LeWindow,
    LegacyAdvertisingTypes, LinkAcknowledgement, PduLen, PduTooLong, RadioDuration, RadioTiming,
    ScanFilterPolicy, ScanType, ScanWindow, ScannerSetup, Supported, TestPhy, TestWindow, TxPower,
    Unplaced, dtm::DtmTest,
};
use oer_protocols_bluetooth_le_wire::{
    AccessAddress, AdvertisingChannel, AdvertisingChannels, AdvertisingPdu, CrcInit, DataChannel,
    LeDeviceAddress, LeDeviceAddressKind, TestChannel, dtm::DtmPayloadPattern,
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
        limits: LeLimits::NONE,
    };
}

/// A power under the open test domain's ceiling.
fn power(dbm: i8) -> TxPower {
    let band = oer_protocols_radio_regulatory::TEST_DOMAIN
        .permit(crate::LeBand)
        .unwrap();
    let ceiling = band.tx_ceiling(oer_protocols_radio_regulatory::PowerLimits {
        antenna_gain_mbm: 0,
        chip_maximum_mbm: i32::MAX,
        owner_cap_mbm: None,
    });
    TxPower::within(crate::LeTxCeiling::new(band, ceiling), dbm).unwrap()
}

const ADV_NONCONN_IND: [u8; 8] = [0x02, 6, 1, 2, 3, 4, 5, 6];
const ADV_IND: [u8; 8] = [0x00, 6, 1, 2, 3, 4, 5, 6];
/// `ADV_NONCONN_IND` with the RFU header bit set.
const ADV_NONCONN_IND_RFU: [u8; 8] = [0x12, 6, 1, 2, 3, 4, 5, 6];

const OWN: LeDeviceAddress =
    LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 6], LeDeviceAddressKind::Random);

fn advertising(
    pdu: &'static [u8],
    reception: AdvertisingReception<'static>,
    phy: LePhy,
    dbm: i8,
) -> AdvertisingSet<'static> {
    AdvertisingSet {
        data: AdvertisingData {
            pdu: AdvertisingPdu::new(pdu).unwrap(),
            reception,
        },
        own: OWN,
        peer: None,
        filter_policy: AdvertisingFilterPolicy::AcceptAll,
        tx_power: power(dbm),
        phy,
    }
}

fn scanner(scan_type: ScanType, filter_policy: ScanFilterPolicy) -> ScannerSetup {
    ScannerSetup {
        scan_type,
        filter_policy,
        own: OWN,
        tx_power: power(0),
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
        channel: AdvertisingChannel::Channel37,
        window: window(start, duration),
    }
}

/// The advertising channels 1.5 ms apart.
const SPACING: ChannelSpacing<Peripheral> =
    ChannelSpacing::new(RadioDuration::from_micros(1_500)).unwrap();

fn advertise(anchor: u64) -> AdvertisingEvent<Peripheral> {
    AdvertisingEvent {
        anchor: LeInstant::from_micros(anchor),
        channels: AdvertisingChannels::ALL,
        channel_spacing: SPACING,
        coexistence: AdvertisingEventKind::Ordinary,
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
            20
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
}

#[test]
fn an_advertiser_filters_by_the_accept_list_only_where_the_capabilities_have_one() {
    let new = Supported::<Peripheral, _>::new;
    let with = |filter_policy| AdvertisingSet {
        filter_policy,
        ..advertising(&ADV_NONCONN_IND, AdvertisingReception::None, LePhy::Le1M, 0)
    };
    assert!(new(with(AdvertisingFilterPolicy::AcceptAll)).is_ok());
    for policy in [
        AdvertisingFilterPolicy::ScanAcceptList,
        AdvertisingFilterPolicy::ConnectAcceptList,
        AdvertisingFilterPolicy::BothAcceptList,
    ] {
        assert_eq!(new(with(policy)), Err(Unplaced::Unsupported));
        assert!(Supported::<Listed, _>::new(with(policy)).is_ok());
    }
}

/// [`Peripheral`] with a filter accept list.
enum Listed {}

impl LeCapabilities for Listed {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        filter_accept_list: true,
        ..Peripheral::CAPS
    };
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
    assert!(placed(advertise(1_000), now).is_ok());
    // The last channel's reservation counts: 2 * spacing past the anchor.
    assert_eq!(
        placed(advertise(1_000_000 - 2_000), now).err(),
        Some(Unplaced::BeyondHorizon)
    );
    // A spacing no longer than the preparation lead is not built.
    assert!(ChannelSpacing::<Peripheral>::new(RadioDuration::from_micros(500)).is_none());
}

#[test]
fn a_pdu_and_a_test_are_bounded_by_the_connection_and_test_capabilities() {
    assert_eq!(PduLen::<Peripheral>::new(27).map(|len| len.get()), Ok(27));
    assert_eq!(
        PduLen::<Peripheral>::new(28).map(|len| len.get()),
        Err(PduTooLong {
            octets: 28,
            max: 27
        })
    );
    let receive = |phy| DtmTest::Receive {
        channel: TestChannel::new(0).unwrap(),
        phy,
    };
    assert!(Supported::<Peripheral, _>::new(receive(TestPhy::Le2M)).is_ok());
    assert_eq!(
        Supported::<Peripheral, _>::new(receive(TestPhy::LeCodedS8)),
        Err(Unplaced::Unsupported)
    );
    let transmit = |dbm| DtmTest::Transmit {
        channel: TestChannel::new(0).unwrap(),
        phy: TestPhy::Le1M,
        pattern: DtmPayloadPattern::Prbs9,
        length: 37,
        power: power(dbm),
    };
    assert!(Supported::<Peripheral, _>::new(transmit(0)).is_ok());
    assert_eq!(
        Supported::<Peripheral, _>::new(transmit(-25)),
        Err(Unplaced::Unsupported)
    );
    let now = LeInstant::from_micros(0);
    let test = |start| TestWindow {
        window: window(start, 100),
    };
    assert!(Supported::<Peripheral, _>::placed(test(2_000), now).is_ok());
    assert_eq!(
        Supported::<Peripheral, _>::placed(test(1_000_000), now),
        Err(Unplaced::BeyondHorizon)
    );
}

#[test]
fn a_connection_needs_a_served_phy_and_power() {
    let setup = |phy, dbm| ConnectionSetup {
        access_address: AccessAddress([1, 2, 3, 4]),
        crc_init: CrcInit([1, 2, 3]),
        created_at: LeInstant::from_micros(0),
        tx_power: power(dbm),
        phy,
    };
    assert!(Supported::<Peripheral, _>::new(setup(LePhy::Le1M, 0)).is_ok());
    assert_eq!(
        Supported::<Peripheral, _>::new(setup(LePhy::Le2M, 0)),
        Err(Unplaced::Unsupported)
    );
    assert_eq!(
        Supported::<Peripheral, _>::new(setup(LePhy::Le1M, -25)),
        Err(Unplaced::Unsupported)
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

const PRIORITY: ConnectionPriority = ConnectionPriority::new(8).unwrap();

fn connection_event(timing: ConnectionEventTiming) -> ConnectionEvent {
    ConnectionEvent {
        channel: DataChannel::new(0).unwrap(),
        window: window(2_000, 1_000),
        interval: RadioDuration::from_micros(30_000),
        timing,
        priority: PRIORITY,
        coexistence: ConnectionEventKind::Ordinary,
    }
}

#[test]
fn a_connection_event_waits_within_the_backend() {
    let micros = RadioDuration::from_micros;
    let placed = |event| Supported::<Peripheral, _>::placed(event, LeInstant::from_micros(0));
    let first = |transmit_window, timing_guard| ConnectionEventTiming::First {
        transmit_window: micros(transmit_window),
        timing_guard: micros(timing_guard),
    };
    // 65,473 us: the window and a guard on each side.
    assert!(placed(connection_event(first(65_273, 100))).is_ok());
    assert_eq!(
        placed(connection_event(first(65_274, 100))),
        Err(Unplaced::Unsupported)
    );
    // A first event needs a transmit window.
    assert!(placed(connection_event(first(0, 100))).is_err());
    let recurring = |receive_wait| ConnectionEventTiming::Recurring {
        receive_wait: micros(receive_wait),
        widening: micros(16),
    };
    assert!(placed(connection_event(recurring(131_070))).is_ok());
    assert!(placed(connection_event(recurring(131_071))).is_err());
    // An interval lasts, and with its widening stays within the horizon.
    let with_interval = |interval| ConnectionEvent {
        interval: micros(interval),
        ..connection_event(recurring(1_000))
    };
    assert!(placed(with_interval(0)).is_err());
    assert!(placed(with_interval(1_000_000 - 16)).is_ok());
    assert!(placed(with_interval(1_000_000 - 15)).is_err());
}

#[test]
fn a_power_above_the_regulatory_ceiling_is_not_requestable() {
    let band = oer_protocols_radio_regulatory::TEST_DOMAIN
        .permit(crate::LeBand)
        .unwrap();
    let ceiling = band.tx_ceiling(oer_protocols_radio_regulatory::PowerLimits {
        antenna_gain_mbm: 0,
        chip_maximum_mbm: i32::MAX,
        owner_cap_mbm: Some(500),
    });
    let ceiling = crate::LeTxCeiling::new(band, ceiling);
    assert!(TxPower::within(ceiling, 5).is_ok());
    assert_eq!(
        TxPower::within(ceiling, 6),
        Err(crate::AboveCeiling {
            requested_dbm: 6,
            ceiling_dbm: 5
        })
    );
}

#[test]
fn advertising_capability_policy_preserves_every_header_and_reception_decision() {
    use super::Capability;
    let response = AdvertisingPdu::new(&SCAN_RSP).unwrap();
    for octet in u8::MIN..=u8::MAX {
        let bytes = [octet, 6, 1, 2, 3, 4, 5, 6];
        for reception in [
            AdvertisingReception::None,
            AdvertisingReception::Report,
            AdvertisingReception::ScanResponse(response),
        ] {
            let data = AdvertisingData {
                pdu: AdvertisingPdu::new(&bytes).unwrap(),
                reception,
            };
            let rfu_clear = (octet / 16) % 2 == 0;
            let code = octet % 16;
            let nonconnectable = code == 2 && matches!(reception, AdvertisingReception::None);
            let responsive = matches!(
                (code, reception),
                (
                    0 | 6,
                    AdvertisingReception::Report | AdvertisingReception::ScanResponse(_)
                ) | (1, AdvertisingReception::Report)
            );
            assert_eq!(
                Capability::<Peripheral>::supported(&data),
                rfu_clear && nonconnectable
            );
            assert_eq!(
                Capability::<Responsive>::supported(&data),
                rfu_clear && responsive
            );
        }
    }
}

#[test]
fn no_power_is_supported_without_a_radio() {
    assert_eq!(
        Supported::<crate::NoCapabilities, _>::new(power(0)),
        Err(Unplaced::Unsupported)
    );
}

#[test]
fn an_accept_list_holds_at_most_the_capabilities_entries() {
    /// [`Peripheral`] with a list of two.
    enum Two {}
    impl LeCapabilities for Two {
        const CAPS: LeRadioCapabilities = LeRadioCapabilities {
            filter_accept_list: true,
            limits: LeLimits {
                accept_list: 2,
                ..LeLimits::NONE
            },
            ..Peripheral::CAPS
        };
    }
    let entries = [OWN; 3];
    let list = |count| crate::AcceptEntries::new(&entries[..count]);
    assert!(Supported::<Two, _>::new(list(2)).is_ok());
    assert_eq!(
        Supported::<Two, _>::new(list(3)).err(),
        Some(Unplaced::BeyondCapacity)
    );
    assert!(Supported::<Peripheral, _>::new(list(0)).is_ok());
    assert_eq!(
        Supported::<Peripheral, _>::new(list(1)).err(),
        Some(Unplaced::BeyondCapacity)
    );
}

use crate::{
    AdvertisingConfiguration, AdvertisingPdu, AdvertisingReception, AdvertisingSetId, ConnectionId,
    DataPdu, DataPduKind, EventId, LeCapabilities, LeInstant, LePhy, LePhys, LeRadioCapabilities,
    LeWindow, LegacyAdvertisingTypes, RadioDuration, RadioRequest, RadioTiming, Supported,
    TestChannel, TestPayloadType, TestPhy, TestTransmit, TxPower,
};

/// LE 1M responsive advertising, data PDUs of 251 octets and Direct Test
/// Mode, one second ahead.
#[derive(Debug, PartialEq)]
enum Caps {}

impl LeCapabilities for Caps {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        legacy_advertising: LegacyAdvertisingTypes {
            nonconnectable: true,
            responsive: true,
        },
        peripheral_connection: Some(crate::LeConnectionCapabilities {
            max_data_payload: 251,
            link_acknowledgement: crate::LinkAcknowledgement::Hardware,
            max_first_receive_wait: RadioDuration::from_micros(65_473),
            max_receive_wait: RadioDuration::from_micros(131_070),
        }),
        direct_test_mode: true,
        phys: LePhys::LE_1M,
        test_phys: LePhys::LE_1M,
        min_tx_power_dbm: -24,
        max_ahead: RadioDuration::from_micros(1_000_000),
        timing: RadioTiming::MINIMAL,
        ..LeRadioCapabilities::NONE
    };
}

const ADV_IND: [u8; 39] = {
    let mut pdu = [7; 39];
    pdu[0] = 0x00;
    pdu[1] = 37;
    pdu
};
const SCAN_RSP: [u8; 8] = [0x04, 6, 1, 2, 3, 4, 5, 6];

/// Each request that carries bytes, at the longest its check admits.
fn requests(payload: &[u8]) -> [RadioRequest<'_, Caps>; 3] {
    let configuration = AdvertisingConfiguration {
        set: AdvertisingSetId::new(1),
        pdu: AdvertisingPdu::new(&ADV_IND).unwrap(),
        reception: AdvertisingReception::ScanResponse(AdvertisingPdu::new(&SCAN_RSP).unwrap()),
        tx_power: TxPower::from_dbm(0),
        phy: LePhy::Le1M,
    };
    let test = TestTransmit {
        id: EventId::new(9),
        channel: TestChannel::new(19).unwrap(),
        phy: TestPhy::Le1M,
        window: LeWindow::new(
            LeInstant::from_micros(10_000),
            RadioDuration::from_micros(2_120),
        )
        .unwrap(),
        tx_power: TxPower::from_dbm(0),
        payload_type: TestPayloadType::new(0).unwrap(),
        payload,
    };
    [
        RadioRequest::ConfigureAdvertising(Supported::new(configuration).unwrap()),
        RadioRequest::Transmit {
            connection: ConnectionId::new(0),
            pdu: Supported::new(DataPdu::new(DataPduKind::Start, &payload[..251]).unwrap())
                .unwrap(),
        },
        RadioRequest::TestTransmit(Supported::placed(test, LeInstant::from_micros(0)).unwrap()),
    ]
}

#[test]
fn an_owned_request_borrows_back_the_request_it_copied() {
    let payload = [0x5a; 255];
    for request in requests(&payload) {
        let owned = request.to_owned();
        assert_eq!(owned.as_request(), request);
    }
}

#[test]
fn an_owned_request_outlives_the_bytes_it_copied() {
    let owned = {
        let payload = vec_of(255);
        let [_, transmit, _] = requests(&payload);
        transmit.to_owned()
    };
    let RadioRequest::Transmit { pdu, .. } = owned.as_request() else {
        panic!("a transmit stays a transmit")
    };
    assert_eq!(pdu.get().payload(), &[0xa5; 251][..]);
}

fn vec_of(len: usize) -> std::vec::Vec<u8> {
    std::vec![0xa5; len]
}

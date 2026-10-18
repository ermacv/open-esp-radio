use std::vec::Vec;

use bt_hci::cmd::{Opcode, OpcodeGroup};
use oer_bluetooth_hci::{
    BluetoothPublicDeviceAddress, HciCommandPacket, LeControllerBootstrapConfig, LeRandomSource,
    LeRandomUnavailable,
};
use oer_bluetooth_radio::{
    AcceptListChange, AdvertisingChannel, AdvertisingChannels, AdvertisingEvent,
    AdvertisingReception, ConnectionAllowances, ConnectionConfiguration, ConnectionEvent,
    DataPduKind, EventId, EventResult, LeInstant, RadioDuration, RadioOutcome, RadioRequest,
    RadioTiming, ReceivedPdu, RequestError, ScanFilterPolicy, ScanType, ScanWindow,
};

use crate::{LeController, LeControllerConfig, LeVersionInformation, PLANNING_SLACK, RadioWork};

mod accept_list;
mod capabilities;
mod connection;
mod connections;

pub(super) const TIMING: RadioTiming = RadioTiming {
    preparation_lead: RadioDuration::from_micros(300),
    admission_guard: RadioDuration::from_micros(200),
    connection: ConnectionAllowances {
        local_sleep_clock_ppm: 500,
        widening_jitter: RadioDuration::from_micros(63),
        receive_guard: RadioDuration::from_micros(10),
        receive_tail: RadioDuration::from_micros(2),
        boundary_guard: oer_bluetooth_radio::NonZeroRadioDuration::from_micros(
            core::num::NonZeroU64::MIN,
        ),
        first_event_guard: RadioDuration::from_micros(16),
        event_length: RadioDuration::from_micros(5047),
        first_event_length: RadioDuration::from_micros(5155),
    },
};
/// The capabilities the core plans for in these tests: what the
/// ESP32-S31 serves, with [`TIMING`].
#[derive(Debug, PartialEq)]
pub(super) enum Caps {}

impl oer_bluetooth_radio::LeCapabilities for Caps {
    const CAPS: oer_bluetooth_radio::LeRadioCapabilities =
        oer_bluetooth_radio::LeRadioCapabilities {
            legacy_advertising: oer_bluetooth_radio::LegacyAdvertisingTypes {
                nonconnectable: true,
                responsive: true,
            },
            passive_scanning: true,
            active_scanning: true,
            filter_accept_list: true,
            peripheral_connection: Some(oer_bluetooth_radio::LeConnectionCapabilities {
                max_data_payload: 255,
                link_acknowledgement: oer_bluetooth_radio::LinkAcknowledgement::Hardware,
                max_first_receive_wait: RadioDuration::from_micros(65_473),
                max_receive_wait: RadioDuration::from_micros(131_070),
            }),
            direct_test_mode: true,
            phys: oer_bluetooth_radio::LePhys::LE_1M,
            test_phys: oer_bluetooth_radio::LePhys::ALL,
            min_tx_power_dbm: -24,
            max_ahead: RadioDuration::from_micros(1 << 30),
            timing: TIMING,
        };
}

const SUCCESS: u8 = 0x00;
const UNKNOWN_CONNECTION: u8 = 0x02;
const HARDWARE_FAILURE: u8 = 0x03;
const DISALLOWED: u8 = 0x0c;
const UNSUPPORTED: u8 = 0x11;

const RESET: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0003);
const SET_EVENT_MASK: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0001);
const DISCONNECT: Opcode = Opcode::new(OpcodeGroup::LINK_CONTROL, 0x0006);
const SET_RANDOM_ADDRESS: Opcode = Opcode::new(OpcodeGroup::LE, 0x0005);
const SET_ADV_PARAMS: Opcode = Opcode::new(OpcodeGroup::LE, 0x0006);
const SET_ADV_DATA: Opcode = Opcode::new(OpcodeGroup::LE, 0x0008);
const SET_ADV_ENABLE: Opcode = Opcode::new(OpcodeGroup::LE, 0x000a);
const SET_SCAN_PARAMS: Opcode = Opcode::new(OpcodeGroup::LE, 0x000b);
const SET_SCAN_ENABLE: Opcode = Opcode::new(OpcodeGroup::LE, 0x000c);
const LE_RAND: Opcode = Opcode::new(OpcodeGroup::LE, 0x0018);
const TRANSMITTER_TEST: Opcode = Opcode::new(OpcodeGroup::LE, 0x001e);
const TEST_END: Opcode = Opcode::new(OpcodeGroup::LE, 0x001f);

/// 100 ms advertising interval.
const ADV_INTERVAL_UNITS: u16 = 160;

const OUTPUT: usize = 12;

fn config() -> LeControllerConfig {
    LeControllerConfig {
        bootstrap: LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
            251,
            4,
        )
        .unwrap(),
        version: Some(LeVersionInformation::new(0x0d, 0xffff, 1)),
    }
}

struct FixedRandom;

impl LeRandomSource for FixedRandom {
    fn random_bytes(&self) -> Result<[u8; 8], LeRandomUnavailable> {
        Ok([1, 2, 3, 4, 5, 6, 7, 8])
    }
}

static RANDOM: FixedRandom = FixedRandom;

/// Owned view of one radio request.
#[derive(Clone, Debug, PartialEq)]
enum Request {
    ConfigureAdvertising(Vec<u8>),
    ConfigureConnectable(Vec<u8>, Vec<u8>),
    ConfigureDirected(Vec<u8>),
    OpenConnection(ConnectionConfiguration),
    ConnectionEvent(ConnectionEvent),
    Transmit(DataPduKind, Vec<u8>),
    CloseConnection,
    Advertise(AdvertisingEvent),
    RemoveAdvertising,
    ConfigureScanner(ScanType, ScanFilterPolicy),
    FilterAcceptList(AcceptListChange),
    Scan(ScanWindow),
    RemoveScanner,
    TestTransmit(EventId),
    TestReceive(EventId),
    EndTest,
    Cancel(EventId),
}

impl<K: oer_bluetooth_radio::LeCapabilities> From<RadioWork<'_, K>> for Request {
    fn from(work: RadioWork<'_, K>) -> Self {
        let request = match work {
            RadioWork::Submit(request) => request,
            RadioWork::Cancel(id) => return Self::Cancel(id),
        };
        match request {
            RadioRequest::ConfigureAdvertising(configuration) => {
                let configuration = *configuration.get();
                match configuration.reception {
                    AdvertisingReception::ScanResponse(response) => Self::ConfigureConnectable(
                        configuration.pdu.bytes().to_vec(),
                        response.bytes().to_vec(),
                    ),
                    AdvertisingReception::Report => {
                        Self::ConfigureDirected(configuration.pdu.bytes().to_vec())
                    }
                    AdvertisingReception::None => {
                        Self::ConfigureAdvertising(configuration.pdu.bytes().to_vec())
                    }
                }
            }
            RadioRequest::OpenConnection(configuration) => {
                Self::OpenConnection(*configuration.get())
            }
            RadioRequest::ConnectionEvent(event) => Self::ConnectionEvent(*event.get()),
            RadioRequest::Transmit { pdu, .. } => {
                Self::Transmit(pdu.get().kind(), pdu.get().payload().to_vec())
            }
            RadioRequest::CloseConnection(_) => Self::CloseConnection,
            RadioRequest::Advertise(event) => Self::Advertise(*event.get()),
            RadioRequest::RemoveAdvertising(_) => Self::RemoveAdvertising,
            RadioRequest::ConfigureScanner(configuration) => Self::ConfigureScanner(
                configuration.get().scan_type,
                configuration.get().filter_policy,
            ),
            RadioRequest::FilterAcceptList(change) => Self::FilterAcceptList(*change.get()),
            RadioRequest::Scan(window) => Self::Scan(*window.get()),
            RadioRequest::RemoveScanner(_) => Self::RemoveScanner,
            RadioRequest::TestTransmit(test) => Self::TestTransmit(test.get().id),
            RadioRequest::TestReceive(test) => Self::TestReceive(test.get().id),
            RadioRequest::EndTest => Self::EndTest,
        }
    }
}

/// A Controller of `C` connections and an output of `O` packets under
/// test.
struct Bench<const C: usize, const O: usize, K: oer_bluetooth_radio::LeCapabilities = Caps> {
    core: LeController<'static, K, O, C>,
    now: u64,
}

/// The single-connection Controller.
type Harness = Bench<1, OUTPUT>;

/// Output of a two-connection Controller: room for both connections' events
/// in flight and the next one.
const PAIR_OUTPUT: usize = 32;

/// The two-connection Controller.
type Pair = Bench<2, PAIR_OUTPUT>;

impl<const C: usize, const O: usize, K: oer_bluetooth_radio::LeCapabilities> Bench<C, O, K> {
    fn new() -> Self {
        Self {
            core: LeController::new(config(), Some(&RANDOM)),
            now: 10_000,
        }
    }

    /// A Controller after Reset with LE Meta events enabled.
    fn configured() -> Self {
        let mut harness = Self::new();
        assert_eq!(harness.command(RESET, &[]), Some(SUCCESS));
        // The default mask plus LE Meta (bit 61).
        let mask = [0xff, 0xff, 0xff, 0xff, 0xff, 0x1f, 0x00, 0x20];
        assert_eq!(harness.command(SET_EVENT_MASK, &mask), Some(SUCCESS));
        harness
    }

    fn send(&mut self, opcode: Opcode, parameters: &[u8]) {
        self.core
            .command(HciCommandPacket::new(opcode, parameters))
            .unwrap();
    }

    /// Send one command and return the status of its immediate response.
    fn command(&mut self, opcode: Opcode, parameters: &[u8]) -> Option<u8> {
        self.send(opcode, parameters);
        self.status_of(opcode)
    }

    /// The status of the queued response to `opcode`, if one is queued.
    fn status_of(&mut self, opcode: Opcode) -> Option<u8> {
        let packets = self.drain();
        let mut status = None;
        for packet in packets {
            match packet[0] {
                0x0e => {
                    assert_eq!(u16::from_le_bytes([packet[3], packet[4]]), opcode.to_raw());
                    status = Some(packet[5]);
                }
                0x0f => {
                    assert_eq!(u16::from_le_bytes([packet[4], packet[5]]), opcode.to_raw());
                    status = Some(packet[2]);
                }
                _ => panic!("unexpected event {packet:?}"),
            }
        }
        status
    }

    fn drain(&mut self) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        while let Some(packet) = self.core.front() {
            packets.push(packet.as_bytes().to_vec());
            self.core.pop();
        }
        packets
    }

    /// Ask for one request and answer it with `result`.
    fn step_with(
        &mut self,
        result: Result<(), RequestError>,
    ) -> Result<Option<Request>, crate::EpochExhausted> {
        let request = self
            .core
            .next_request(LeInstant::from_micros(self.now))?
            .map(Request::from);
        match request {
            Some(Request::Cancel(_)) => self
                .core
                .cancel_done(result.map_err(|_| oer_bluetooth_radio::CancelError::NotRunning)),
            Some(_) => self.core.request_done(result),
            None => {}
        }
        Ok(request)
    }

    fn step(&mut self) -> Result<Option<Request>, crate::EpochExhausted> {
        self.step_with(Ok(()))
    }

    fn end(&mut self, id: EventId) {
        self.core.outcome(RadioOutcome::EventEnded {
            id,
            result: EventResult::NotExecuted,
        });
    }

    fn earliest(&self) -> u64 {
        self.now
            + (TIMING.preparation_lead.as_micros()
                + TIMING.admission_guard.as_micros()
                + PLANNING_SLACK.as_micros())
    }

    fn advertise(&mut self, data: &[u8]) {
        let parameters = nonconnectable_parameters();
        assert_eq!(self.command(SET_ADV_PARAMS, &parameters), Some(SUCCESS));
        assert_eq!(self.command(SET_ADV_DATA, &adv_data(data)), Some(SUCCESS));
        assert_eq!(self.command(SET_ADV_ENABLE, &[1]), None);
        assert!(!self.core.is_command_ready());
        assert!(matches!(
            self.step().unwrap(),
            Some(Request::ConfigureAdvertising(_))
        ));
        assert_eq!(self.status_of(SET_ADV_ENABLE), Some(SUCCESS));
    }

    fn scan(&mut self, interval_units: u16, window_units: u16, filter_duplicates: bool) {
        self.scan_as(false, interval_units, window_units, filter_duplicates);
    }

    fn scan_as(
        &mut self,
        active: bool,
        interval_units: u16,
        window_units: u16,
        filter_duplicates: bool,
    ) {
        self.scan_with(
            active,
            ScanFilterPolicy::AcceptAll,
            interval_units,
            window_units,
            filter_duplicates,
        );
    }

    fn scan_with(
        &mut self,
        active: bool,
        filter_policy: ScanFilterPolicy,
        interval_units: u16,
        window_units: u16,
        filter_duplicates: bool,
    ) {
        let mut parameters = [0; 7];
        parameters[0] = u8::from(active);
        parameters[6] = u8::from(filter_policy == ScanFilterPolicy::AcceptListOnly);
        parameters[1..3].copy_from_slice(&interval_units.to_le_bytes());
        parameters[3..5].copy_from_slice(&window_units.to_le_bytes());
        assert_eq!(self.command(SET_SCAN_PARAMS, &parameters), Some(SUCCESS));
        assert_eq!(
            self.command(SET_SCAN_ENABLE, &[1, u8::from(filter_duplicates)]),
            None
        );
        assert_eq!(
            self.step().unwrap(),
            Some(Request::ConfigureScanner(
                if active {
                    ScanType::Active
                } else {
                    ScanType::Passive
                },
                filter_policy
            ))
        );
        assert_eq!(self.status_of(SET_SCAN_ENABLE), Some(SUCCESS));
    }
}

/// ADV_NONCONN_IND every 100 ms on all primary channels from the public
/// address.
fn nonconnectable_parameters() -> [u8; 15] {
    let mut parameters = [0; 15];
    parameters[0..2].copy_from_slice(&ADV_INTERVAL_UNITS.to_le_bytes());
    parameters[2..4].copy_from_slice(&ADV_INTERVAL_UNITS.to_le_bytes());
    parameters[4] = 0x03;
    parameters[13] = 0x07;
    parameters
}

fn adv_data(data: &[u8]) -> [u8; 32] {
    let mut parameters = [0; 32];
    parameters[0] = data.len() as u8;
    parameters[1..1 + data.len()].copy_from_slice(data);
    parameters
}

fn advertising_pdu(address: [u8; 6], data: &[u8]) -> Vec<u8> {
    let mut pdu = std::vec![0x02 | 0x40, (6 + data.len()) as u8];
    pdu.extend_from_slice(&address);
    pdu.extend_from_slice(data);
    pdu
}

#[test]
fn radio_commands_wait_for_reset_and_connection_commands_find_no_connection() {
    let mut harness = Harness::new();
    assert_eq!(harness.command(SET_ADV_ENABLE, &[1]), Some(DISALLOWED));
    assert_eq!(harness.command(SET_SCAN_ENABLE, &[1, 0]), Some(DISALLOWED));
    assert_eq!(
        harness.command(TRANSMITTER_TEST, &[0, 37, 0]),
        Some(DISALLOWED)
    );
    assert_eq!(harness.command(RESET, &[]), Some(SUCCESS));
    assert_eq!(
        harness.command(DISCONNECT, &[0x40, 0x00, 0x13]),
        Some(UNKNOWN_CONNECTION)
    );
    assert!(!harness.core.wants_radio());
}

#[test]
fn le_rand_uses_the_random_source_or_is_unknown_without_one() {
    let mut harness = Harness::configured();
    harness.send(LE_RAND, &[]);
    let packets = harness.drain();
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0][5], SUCCESS);
    assert_eq!(&packets[0][6..14], &[1, 2, 3, 4, 5, 6, 7, 8]);

    let mut core = LeController::<Caps, OUTPUT, 1>::new(config(), None);
    core.command(HciCommandPacket::new(LE_RAND, &[])).unwrap();
    assert_eq!(core.front().unwrap().as_bytes()[5], 0x01);
}

#[test]
fn nonconnectable_advertising_places_events_one_interval_plus_delay_apart() {
    let mut harness = Harness::configured();
    let data = [0x02, 0x01, 0x06];
    harness.advertise(&data);

    let Some(Request::Advertise(first)) = harness.step().unwrap() else {
        panic!("first advertising event");
    };
    assert_eq!(first.anchor.as_micros(), harness.earliest());
    assert_eq!(first.channels, AdvertisingChannels::ALL);
    let air = (6 + data.len() as u64) * 8 + 80;
    assert_eq!(
        first.channel_spacing.as_micros(),
        TIMING.preparation_lead.as_micros() + air
    );
    // One event at a time.
    assert_eq!(harness.step().unwrap(), None);

    harness.end(first.id);
    let Some(Request::Advertise(second)) = harness.step().unwrap() else {
        panic!("second advertising event");
    };
    let gap = second.anchor.as_micros() - first.anchor.as_micros();
    let interval = u64::from(ADV_INTERVAL_UNITS) * 625;
    assert!((interval..=interval + 10_000).contains(&gap), "gap {gap}");
    assert_ne!(second.id, first.id);
}

#[test]
fn advertising_pdu_carries_the_public_address_and_host_data() {
    let mut harness = Harness::configured();
    let parameters = nonconnectable_parameters();
    harness.command(SET_ADV_PARAMS, &parameters);
    harness.command(SET_ADV_DATA, &adv_data(&[0xaa, 0xbb]));
    harness.send(SET_ADV_ENABLE, &[1]);
    let Some(Request::ConfigureAdvertising(pdu)) = harness.step().unwrap() else {
        panic!("configuration");
    };
    assert_eq!(pdu, std::vec![0x02, 8, 6, 5, 4, 3, 2, 1, 0xaa, 0xbb]);
}

#[test]
fn connectable_advertising_needs_entropy_and_a_set_random_address() {
    // Defaults describe connectable undirected advertising.
    let mut core = LeController::<Caps, OUTPUT, 1>::new(config(), None);
    core.command(HciCommandPacket::new(RESET, &[])).unwrap();
    core.pop();
    core.command(HciCommandPacket::new(SET_ADV_ENABLE, &[1]))
        .unwrap();
    assert_eq!(core.front().unwrap().as_bytes()[5], UNSUPPORTED);

    let mut harness = Harness::configured();
    let mut parameters = nonconnectable_parameters();
    parameters[5] = 0x01; // random own address
    harness.command(SET_ADV_PARAMS, &parameters);
    assert_eq!(harness.command(SET_ADV_ENABLE, &[1]), Some(0x12));
    assert!(!harness.core.wants_radio());
}

#[test]
fn a_refused_configuration_completes_enable_with_hardware_failure() {
    let mut harness = Harness::configured();
    let parameters = nonconnectable_parameters();
    harness.command(SET_ADV_PARAMS, &parameters);
    harness.send(SET_ADV_ENABLE, &[1]);
    harness.step_with(Err(RequestError::NoInstance)).unwrap();
    assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(HARDWARE_FAILURE));
    assert!(!harness.core.wants_radio());
}

#[test]
fn disable_cancels_the_event_then_removes_the_set_before_completing() {
    let mut harness = Harness::configured();
    harness.advertise(&[]);
    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("advertising event");
    };
    assert_eq!(harness.command(SET_ADV_ENABLE, &[0]), None);
    assert_eq!(harness.step().unwrap(), Some(Request::Cancel(event.id)));
    assert_eq!(harness.step().unwrap(), None);
    assert_eq!(harness.status_of(SET_ADV_ENABLE), None);
    harness.end(event.id);
    assert_eq!(harness.step().unwrap(), Some(Request::RemoveAdvertising));
    assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));
    assert!(!harness.core.wants_radio());
    // Disabling again has no effect.
    assert_eq!(harness.command(SET_ADV_ENABLE, &[0]), Some(SUCCESS));
}

#[test]
fn advertising_data_updates_while_advertising_reconfigure_the_set() {
    let mut harness = Harness::configured();
    harness.advertise(&[0x01]);
    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("advertising event");
    };
    assert_eq!(
        harness.command(SET_ADV_DATA, &adv_data(&[0x02, 0x03])),
        None
    );
    assert_eq!(harness.step().unwrap(), Some(Request::Cancel(event.id)));
    harness.end(event.id);
    assert_eq!(harness.step().unwrap(), Some(Request::RemoveAdvertising));
    let Some(Request::ConfigureAdvertising(pdu)) = harness.step().unwrap() else {
        panic!("new configuration");
    };
    assert_eq!(&pdu[8..], &[0x02, 0x03]);
    assert_eq!(harness.status_of(SET_ADV_DATA), Some(SUCCESS));
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::Advertise(_))
    ));

    // Parameters change only while disabled.
    assert_eq!(
        harness.command(SET_ADV_PARAMS, &nonconnectable_parameters()),
        Some(DISALLOWED)
    );
    assert_eq!(
        harness.command(SET_RANDOM_ADDRESS, &[1, 2, 3, 4, 5, 0xc0]),
        Some(DISALLOWED)
    );
}

#[test]
fn passive_scanning_rotates_channels_and_reports_advertisements() {
    let mut harness = Harness::configured();
    // 100 ms interval, 50 ms window.
    harness.scan(160, 80, false);
    let mut channels = Vec::new();
    for round in 0..4 {
        let Some(Request::Scan(window)) = harness.step().unwrap() else {
            panic!("scan window {round}");
        };
        assert_eq!(window.window.duration().as_micros(), 50_000);
        channels.push(window.channel);
        if round == 0 {
            assert_eq!(window.window.start().as_micros(), harness.earliest());
            let pdu = advertising_pdu([9, 8, 7, 6, 5, 4], &[0x02, 0x01, 0x06]);
            for _ in 0..2 {
                harness.core.outcome(RadioOutcome::Received {
                    id: window.id,
                    pdu: ReceivedPdu {
                        pdu: &pdu,
                        rssi_dbm: -40,
                        captured_at: None,
                    },
                });
            }
            let reports = harness.drain();
            assert_eq!(reports.len(), 2);
            let report = &reports[0];
            assert_eq!(&report[..5], &[0x3e, 15, 0x02, 1, 0x03]);
            assert_eq!(report[5], 0x01);
            assert_eq!(&report[6..12], &[9, 8, 7, 6, 5, 4]);
            assert_eq!(&report[12..16], &[3, 0x02, 0x01, 0x06]);
            assert_eq!(report[16], (-40i8) as u8);
        }
        harness.end(window.id);
    }
    assert_eq!(
        channels,
        [
            AdvertisingChannel::Channel37,
            AdvertisingChannel::Channel38,
            AdvertisingChannel::Channel39,
            AdvertisingChannel::Channel37,
        ]
    );
}

#[test]
fn only_an_active_scanner_reports_scan_responses() {
    let mut scan_rsp = std::vec![0x04 | 0x40, 6 + 3];
    scan_rsp.extend_from_slice(&[1, 2, 3, 4, 5, 0xc6]);
    scan_rsp.extend_from_slice(&[2, 0x09, b'x']);
    for active in [false, true] {
        let mut harness = Harness::configured();
        harness.scan_as(active, 160, 80, false);
        let Some(Request::Scan(window)) = harness.step().unwrap() else {
            panic!("scan window");
        };
        harness.core.outcome(RadioOutcome::Received {
            id: window.id,
            pdu: ReceivedPdu {
                pdu: &scan_rsp,
                rssi_dbm: -30,
                captured_at: None,
            },
        });
        let reports = harness.drain();
        if active {
            assert_eq!(reports.len(), 1);
            // LE Advertising Report of a SCAN_RSP from a random address.
            assert_eq!(&reports[0][..6], &[0x3e, 15, 0x02, 1, 0x04, 0x01]);
            assert_eq!(&reports[0][6..12], &[1, 2, 3, 4, 5, 0xc6]);
        } else {
            assert!(reports.is_empty());
        }
    }
}

#[test]
fn duplicate_filtering_and_masked_events_suppress_reports() {
    let mut harness = Harness::configured();
    harness.scan(160, 80, true);
    let Some(Request::Scan(window)) = harness.step().unwrap() else {
        panic!("scan window");
    };
    let pdu = advertising_pdu([1, 1, 1, 1, 1, 1], &[0x01]);
    fn received(id: EventId, pdu: &[u8]) -> RadioOutcome<'_> {
        RadioOutcome::Received {
            id,
            pdu: ReceivedPdu {
                pdu,
                rssi_dbm: -60,
                captured_at: None,
            },
        }
    }
    harness.core.outcome(received(window.id, &pdu));
    harness.core.outcome(received(window.id, &pdu));
    assert_eq!(harness.drain().len(), 1);
    // Changed data is a new advertisement.
    let changed = advertising_pdu([1, 1, 1, 1, 1, 1], &[0x02]);
    harness.core.outcome(received(window.id, &changed));
    assert_eq!(harness.drain().len(), 1);
    // A PDU of another event is not this window's.
    harness.core.outcome(RadioOutcome::Received {
        id: EventId::new(window.id.get() + 100),
        pdu: ReceivedPdu {
            pdu: &advertising_pdu([2; 6], &[]),
            rssi_dbm: -60,
            captured_at: None,
        },
    });
    assert!(harness.drain().is_empty());

    // Without LE Meta events nothing is reported.
    let mut quiet = Harness::new();
    quiet.command(RESET, &[]);
    quiet.scan(160, 80, false);
    let Some(Request::Scan(window)) = quiet.step().unwrap() else {
        panic!("scan window");
    };
    quiet.core.outcome(RadioOutcome::Received {
        id: window.id,
        pdu: ReceivedPdu {
            pdu: &pdu,
            rssi_dbm: -60,
            captured_at: None,
        },
    });
    assert!(quiet.drain().is_empty());
}

#[test]
fn scan_windows_leave_room_for_the_next_advertising_event() {
    let mut harness = Harness::configured();
    harness.advertise(&[]);
    // Continuous scanning: window equals interval.
    harness.scan(160, 160, false);
    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("advertising event");
    };
    let Some(Request::Scan(window)) = harness.step().unwrap() else {
        panic!("scan window");
    };
    let event_air = TIMING.preparation_lead.as_micros() * 2 + (3 * 80 + 6 * 8 * 3);
    let event_end = event.anchor.as_micros() + event_air;
    assert!(
        window.window.start().as_micros() >= event_end + TIMING.preparation_lead.as_micros(),
        "the window starts after the event and its own lead"
    );
    // The window ends before the reservation of the next event, which starts
    // at least one interval after this one.
    let next_reservation = event.anchor.as_micros() + u64::from(ADV_INTERVAL_UNITS) * 625
        - TIMING.preparation_lead.as_micros();
    assert!(window.window.end().as_micros() <= next_reservation + 10_000);
    assert!(window.window.end().as_micros() > event_end);

    harness.end(event.id);
    let Some(Request::Advertise(next)) = harness.step().unwrap() else {
        panic!("next advertising event");
    };
    let next_reservation = next.anchor.as_micros() - TIMING.preparation_lead.as_micros();
    assert!(window.window.end().as_micros() <= next_reservation);
}

#[test]
fn direct_test_mode_runs_alone_and_test_end_drains_then_releases() {
    let mut harness = Harness::configured();
    assert_eq!(
        harness.command(TRANSMITTER_TEST, &[0, 37, 0]),
        Some(SUCCESS)
    );
    let Some(Request::TestTransmit(id)) = harness.step().unwrap() else {
        panic!("test transmit");
    };
    assert_eq!(harness.command(SET_ADV_ENABLE, &[1]), Some(DISALLOWED));
    assert_eq!(harness.command(SET_SCAN_ENABLE, &[1, 0]), Some(DISALLOWED));

    harness.send(TEST_END, &[]);
    assert!(!harness.core.is_command_ready());
    assert_eq!(harness.step().unwrap(), Some(Request::Cancel(id)));
    harness.end(id);
    assert_eq!(harness.step().unwrap(), Some(Request::EndTest));
    let packets = harness.drain();
    assert_eq!(packets.len(), 1);
    assert_eq!(&packets[0][3..8], &[0x1f, 0x20, SUCCESS, 0, 0]);
    assert!(!harness.core.wants_radio());
}

#[test]
fn advertising_and_scanning_refuse_direct_test_mode() {
    let mut harness = Harness::configured();
    harness.scan(160, 80, false);
    assert_eq!(
        harness.command(TRANSMITTER_TEST, &[0, 37, 0]),
        Some(DISALLOWED)
    );
}

#[test]
fn reset_stops_every_role_before_it_completes() {
    let mut harness = Harness::configured();
    harness.advertise(&[]);
    harness.scan(160, 80, false);
    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("advertising event");
    };
    let Some(Request::Scan(window)) = harness.step().unwrap() else {
        panic!("scan window");
    };
    assert_eq!(harness.command(RESET, &[]), None);
    assert_eq!(harness.step().unwrap(), Some(Request::Cancel(event.id)));
    assert_eq!(harness.step().unwrap(), Some(Request::Cancel(window.id)));
    harness.end(event.id);
    harness.end(window.id);
    assert_eq!(harness.step().unwrap(), Some(Request::RemoveAdvertising));
    assert_eq!(harness.status_of(RESET), None);
    assert_eq!(harness.step().unwrap(), Some(Request::RemoveScanner));
    assert_eq!(harness.status_of(RESET), Some(SUCCESS));
    assert!(!harness.core.wants_radio());
    // The configuration returned to its defaults: connectable again.
    assert_eq!(harness.command(SET_ADV_ENABLE, &[1]), None);
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::ConfigureConnectable(_, _))
    ));
}

#[test]
fn data_length_commands_report_the_maximum_and_keep_the_suggestion_until_reset() {
    const READ_SUGGESTED: Opcode = Opcode::new(OpcodeGroup::LE, 0x0023);
    const WRITE_SUGGESTED: Opcode = Opcode::new(OpcodeGroup::LE, 0x0024);
    const READ_MAXIMUM: Opcode = Opcode::new(OpcodeGroup::LE, 0x002f);
    const SET_DATA_LENGTH: Opcode = Opcode::new(OpcodeGroup::LE, 0x0022);
    let mut harness = Harness::configured();
    harness.send(READ_MAXIMUM, &[]);
    assert_eq!(
        harness.drain(),
        [std::vec![
            0x0e, 12, 1, 0x2f, 0x20, SUCCESS, 251, 0, 0x48, 8, 251, 0, 0x48, 8
        ]]
    );
    let suggested = |harness: &mut Harness| {
        harness.send(READ_SUGGESTED, &[]);
        let packets = harness.drain();
        assert_eq!(packets.len(), 1);
        packets[0][5..].to_vec()
    };
    assert_eq!(suggested(&mut harness), [SUCCESS, 27, 0, 0x48, 1]);
    assert_eq!(
        harness.command(WRITE_SUGGESTED, &[251, 0, 0x48, 8]),
        Some(SUCCESS)
    );
    assert_eq!(suggested(&mut harness), [SUCCESS, 251, 0, 0x48, 8]);
    assert_eq!(
        harness.command(WRITE_SUGGESTED, &[26, 0, 0x48, 8]),
        Some(0x12)
    );
    assert_eq!(
        harness.command(SET_DATA_LENGTH, &[0, 0, 251, 0, 0x48, 8]),
        Some(UNKNOWN_CONNECTION)
    );
    assert_eq!(harness.command(RESET, &[]), Some(SUCCESS));
    assert_eq!(suggested(&mut harness), [SUCCESS, 27, 0, 0x48, 1]);
}

#[test]
fn inactive_and_control_only_work_do_not_require_a_future_anchor() {
    let mut harness = Harness::configured();
    harness.now = u64::MAX;
    assert_eq!(harness.step(), Ok(None));
    harness.advertise(&[1]);
    assert!(matches!(
        harness.step(),
        Err(crate::EpochExhausted {
            role: crate::PlanningRole::Advertising,
            calculation: crate::PlanningCalculation::Admission,
            ..
        })
    ));
    harness.send(SET_ADV_ENABLE, &[0]);
    assert_eq!(harness.step(), Ok(Some(Request::RemoveAdvertising)));
    assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));
}

#[test]
fn failed_advertising_geometry_preserves_identity_randomness_and_continuation() {
    let mut harness = Harness::configured();
    harness.advertise(&[1]);
    harness.now = u64::MAX - 1_000;
    assert!(matches!(
        harness.step(),
        Err(crate::EpochExhausted {
            role: crate::PlanningRole::Advertising,
            calculation: crate::PlanningCalculation::LatestAnchor,
            ..
        })
    ));
    // Compare the first and second validated plans with an untouched epoch.
    harness.now = 10_000;
    let mut reference = Harness::configured();
    reference.advertise(&[1]);
    let first = harness.step().unwrap();
    assert_eq!(first, reference.step().unwrap());
    let Some(Request::Advertise(event)) = first else {
        panic!("first event")
    };
    harness.end(event.id);
    reference.end(event.id);
    assert_eq!(harness.step().unwrap(), reference.step().unwrap());
}

#[test]
fn test_end_remains_available_after_fatal_test_planning() {
    let mut harness = Harness::configured();
    assert_eq!(
        harness.command(TRANSMITTER_TEST, &[0, 37, 0]),
        Some(SUCCESS)
    );
    harness.now = u64::MAX;
    assert!(matches!(
        harness.step(),
        Err(crate::EpochExhausted {
            role: crate::PlanningRole::DirectTest,
            ..
        })
    ));
    // No request was admitted, so ending this retained test needs no radio anchor.
    assert_eq!(harness.command(TEST_END, &[]), Some(SUCCESS));
    assert_eq!(harness.step(), Ok(None));
}

#[test]
fn mixed_role_planning_failure_keeps_the_admitted_identity_and_control_path() {
    let mut harness = Harness::configured();
    harness.advertise(&[]);
    harness.scan(160, 160, false);
    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("advertising event")
    };
    let Some(Request::Scan(window)) = harness.step().unwrap() else {
        panic!("scan window")
    };
    harness.end(window.id);
    harness.now = u64::MAX;
    let failure = harness.step().unwrap_err();
    assert_eq!(failure.role, crate::PlanningRole::Scanning);
    assert_eq!(failure.calculation, crate::PlanningCalculation::Admission);
    assert_eq!(harness.step(), Err(failure));
    assert!(harness.core.is_command_ready());
    harness.send(SET_ADV_ENABLE, &[0]);
    assert_eq!(harness.step().unwrap(), Some(Request::Cancel(event.id)));
    harness.end(event.id);
    assert_eq!(harness.step().unwrap(), Some(Request::RemoveAdvertising));
    assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));
    harness.send(SET_SCAN_ENABLE, &[0, 0]);
    assert_eq!(harness.step().unwrap(), Some(Request::RemoveScanner));
    assert_eq!(harness.status_of(SET_SCAN_ENABLE), Some(SUCCESS));
    assert_eq!(harness.step(), Ok(None));
}

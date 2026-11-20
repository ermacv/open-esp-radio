//! A consumer of the contract at compile level: a port that does nothing
//! behind the trait, and the thin runs walked through the signatures as
//! they are written. It asserts no behaviour of a port; it shows that every
//! right has a path from the install to the release.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test: its harness polls with a no-op waker and fails by panicking"
)]

use std::collections::VecDeque;

use core::task::{Context, Waker};

use oer_protocols_bluetooth_le_contracts_port::{
    AcceptEntries, AcceptList, AdvEvent, AdvPlace, Advertiser, AdvertisingData, AdvertisingEvent,
    AdvertisingEventKind, AdvertisingFilterPolicy, AdvertisingReception, AdvertisingSet,
    ChannelSpacing, ConnEvent, ConnPlace, Connection, ConnectionEvent, ConnectionEventKind,
    ConnectionEventTiming, ConnectionPriority, ConnectionSetup, Cut, EventEnd, EventPlace,
    Executed, Facts, Install, LeCapabilities, LeConnectionCapabilities, LeInstant, LeLimits, LePhy,
    LePhys, LePort, LeRadioCapabilities, LeRounds, LeWindow, LegacyAdvertisingTypes,
    LinkAcknowledgement, PduHandle, PduLen, PortKey, PortPools, Published, RadioDuration,
    RadioTiming, Received, Rest, Retired, RoleRef, RoleRights, RoundSeal, ScanEvent, ScanPlace,
    ScanWindow, Scanner, ScannerSetup, StartPoll, Starting, StopFrom, StopPoll, StopRight, Stopped,
    Stopping, Supported, Test, TestEvent, TestPlace, TestWindow, TxEnd, TxPlace, TxPower,
    dtm::DtmTest,
};
use oer_protocols_bluetooth_le_wire::{
    AccessAddress, AdvertisingChannels, AdvertisingPdu, CrcInit, DataChannel, DataPduKind,
    LeDeviceAddress, LeDeviceAddressKind,
};
use oer_protocols_radio_port::{Mint, branded};

/// A nonconnectable advertiser and a peripheral connection.
enum Caps {}

impl LeCapabilities for Caps {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        legacy_advertising: LegacyAdvertisingTypes {
            nonconnectable: true,
            responsive: false,
        },
        peripheral_connection: Some(LeConnectionCapabilities {
            max_data_payload: 27,
            link_acknowledgement: LinkAcknowledgement::Hardware,
            max_first_receive_wait: RadioDuration::from_micros(65_473),
            max_receive_wait: RadioDuration::from_micros(131_070),
        }),
        phys: LePhys::LE_1M,
        min_tx_power_dbm: -24,
        max_ahead: RadioDuration::from_micros(1_000_000),
        timing: RadioTiming {
            preparation_lead: RadioDuration::from_micros(100),
            ..RadioTiming::MINIMAL
        },
        limits: LeLimits {
            advertising_sets: 1,
            connections: 1,
            connection_events: 2,
            transmit_places: 1,
            receive_nodes: 2,
            accept_list: 2,
        },
        ..LeRadioCapabilities::NONE
    };
}

/// The geometry: one set, no scanner, one connection of two events, one
/// transmit place and two receive nodes, no test.
type Pools = PortPools<Caps, 1, 0, 1, 2, 1, 2, 0>;

/// A port that does nothing: every event it is given ends executed at the
/// next round, a connection's event with one reception, and every PDU of a
/// chain is transmitted.
struct Idle<'id> {
    key: PortKey<'id, 2, 1>,
    ended: VecDeque<(EventPlace<'id>, EventEnd)>,
    chain: VecDeque<TxPlace<'id>>,
    transmitted: VecDeque<(TxEnd, TxPlace<'id>)>,
    receptions: VecDeque<u16>,
    current: VecDeque<u16>,
    node: [u8; 29],
}

impl<'id> Idle<'id> {
    fn new(key: PortKey<'id, 2, 1>) -> Self {
        Self {
            key,
            ended: VecDeque::new(),
            chain: VecDeque::new(),
            transmitted: VecDeque::new(),
            receptions: VecDeque::new(),
            current: VecDeque::new(),
            node: [0; 29],
        }
    }

    fn end(&mut self, place: EventPlace<'id>) -> Published {
        self.ended.push_back((
            place,
            EventEnd::Executed(Executed {
                anchor: None,
                exchanged: 0,
                cut: Cut::Whole,
                peer: None,
            }),
        ));
        Published
    }

    fn view(&self) -> Received<'_> {
        Received {
            header: [0, 0],
            payload: &self.node[2..],
            rssi_dbm: 0,
            captured_at: None,
        }
    }
}

impl<'id> LeRounds<'id> for Idle<'id> {
    type Node = u16;

    fn round_open(&mut self, _: &RoundSeal) -> LeInstant {
        while let Some(place) = self.chain.pop_front() {
            self.transmitted.push_back((TxEnd::Transmitted, place));
        }
        LeInstant::from_micros(0)
    }

    fn round_next(&mut self, _: &RoundSeal) -> Option<(EventPlace<'id>, EventEnd)> {
        let (place, end) = self.ended.pop_front()?;
        self.current.clear();
        if matches!(place, EventPlace::Connection(_)) {
            self.current.extend(self.receptions.pop_front());
        }
        Some((place, end))
    }

    fn round_received(&mut self, _: &RoundSeal) -> Option<u16> {
        self.current.pop_front()
    }

    fn round_view(&self, _: &RoundSeal, _: &u16) -> Received<'_> {
        self.view()
    }

    fn round_take(&mut self, _: &RoundSeal, node: u16) -> PduHandle<'id> {
        self.key.hold(node)
    }

    fn round_held(&self, _: &RoundSeal, _: &PduHandle<'id>) -> Received<'_> {
        self.view()
    }

    fn round_release(&mut self, _: &RoundSeal, handle: PduHandle<'id>) {
        let node = self.key.release_pdu(handle);
        self.receptions.push_back(node);
    }

    fn round_transmitted(&mut self, _: &RoundSeal) -> Option<(TxEnd, TxPlace<'id>)> {
        self.transmitted.pop_front()
    }

    fn round_close(&mut self, _: &RoundSeal) -> Rest {
        Rest::Quiet
    }
}

impl<'id> LePort<'id, 1, 0, 1, 2, 1, 2, 0> for Idle<'id> {
    type Caps = Caps;
    type Released<'a>
        = ()
    where
        Self: 'a;

    fn start(&mut self, stopped: Stopped<'id>) -> Starting<'id> {
        self.key.start(stopped)
    }

    fn poll_start(&mut self, starting: Starting<'id>, _: &mut Context<'_>) -> StartPoll<'id> {
        StartPoll::Running(self.key.started(starting))
    }

    fn stop(&mut self, _: &mut StopRight<'id>, from: StopFrom<'id>) -> Stopping<'id> {
        self.key.stop(from)
    }

    fn poll_stop(&mut self, stopping: Stopping<'id>, _: &mut Context<'_>) -> StopPoll<'id> {
        StopPoll::Stopped(self.key.stopped(stopping))
    }

    fn retire(
        &mut self,
        stopped: Stopped<'id>,
        roles: RoleRights<'id, 1, 0, 1, 0>,
    ) -> Retired<'id> {
        self.key.retire(stopped, roles)
    }

    fn release(&mut self, retired: Retired<'id>, right: StopRight<'id>) {
        self.key.release(retired, right);
    }

    fn open_advertising(
        &mut self,
        place: AdvPlace<'id>,
        _: &Supported<Caps, AdvertisingSet<'_>>,
    ) -> (Advertiser<'id>, AdvEvent<'id>) {
        self.key.open_advertising(place)
    }

    fn rewrite_advertising(
        &mut self,
        _: &mut Advertiser<'id>,
        _: &mut AdvEvent<'id>,
        _: &Supported<Caps, AdvertisingData<'_>>,
    ) {
    }

    fn close_advertising(&mut self, role: Advertiser<'id>, home: AdvEvent<'id>) -> AdvPlace<'id> {
        self.key.close_advertising(role, home)
    }

    fn open_scanner(
        &mut self,
        place: ScanPlace<'id>,
        _: &Supported<Caps, ScannerSetup>,
    ) -> (Scanner<'id>, ScanEvent<'id>) {
        self.key.open_scanner(place)
    }

    fn close_scanner(&mut self, role: Scanner<'id>, home: ScanEvent<'id>) -> ScanPlace<'id> {
        self.key.close_scanner(role, home)
    }

    fn open_connection(
        &mut self,
        place: ConnPlace<'id>,
        _: &Supported<Caps, ConnectionSetup>,
    ) -> (Connection<'id>, [ConnEvent<'id>; 2], [TxPlace<'id>; 1]) {
        self.receptions.extend([0, 1]);
        self.key.open_connection(place)
    }

    fn close_connection(
        &mut self,
        role: Connection<'id>,
        events: [ConnEvent<'id>; 2],
        tx: [TxPlace<'id>; 1],
    ) -> ConnPlace<'id> {
        self.receptions.clear();
        self.key.close_connection(role, events, tx)
    }

    fn open_test(
        &mut self,
        place: TestPlace<'id>,
        _: &Supported<Caps, DtmTest>,
    ) -> (Test<'id>, TestEvent<'id>) {
        self.key.open_test(place)
    }

    fn close_test(&mut self, role: Test<'id>, home: TestEvent<'id>) -> TestPlace<'id> {
        self.key.close_test(role, home)
    }

    fn write_accept_list(
        &mut self,
        _: &mut AcceptList<'id>,
        _: &Supported<Caps, AcceptEntries<'_>>,
    ) {
    }

    fn publish_advertising(
        &mut self,
        place: AdvEvent<'id>,
        _: &Supported<Caps, AdvertisingEvent<Caps>>,
    ) -> Published {
        self.end(EventPlace::Advertising(place))
    }

    fn publish_scan(
        &mut self,
        place: ScanEvent<'id>,
        _: &Supported<Caps, ScanWindow>,
    ) -> Published {
        self.end(EventPlace::Scan(place))
    }

    fn publish_connection(
        &mut self,
        place: ConnEvent<'id>,
        _: &Supported<Caps, ConnectionEvent>,
    ) -> Published {
        self.end(EventPlace::Connection(place))
    }

    fn publish_test(
        &mut self,
        place: TestEvent<'id>,
        _: &Supported<Caps, TestWindow>,
    ) -> Published {
        self.end(EventPlace::Test(place))
    }

    fn withdraw(&mut self, _: RoleRef<'_, 'id>) {}

    fn write_pdu<F: FnOnce(&mut [u8]) -> PduLen<Caps>>(
        &mut self,
        _: &mut TxPlace<'id>,
        _: DataPduKind,
        fill: F,
    ) {
        let mut payload = [0; 27];
        let _length = fill(&mut payload);
    }

    fn submit(&mut self, place: TxPlace<'id>) {
        self.chain.push_back(place);
    }

    fn alarm(&mut self, _: Option<LeInstant>) {}

    fn poll_facts(&mut self, _: &mut Context<'_>) -> Facts {
        Facts::default()
    }
}

/// A power under the open test domain's ceiling.
fn power() -> TxPower {
    let band = oer_protocols_radio_regulatory::TEST_DOMAIN
        .permit(oer_protocols_bluetooth_le_contracts_port::LeBand)
        .unwrap();
    let ceiling = band.tx_ceiling(oer_protocols_radio_regulatory::PowerLimits {
        antenna_gain_mbm: 0,
        chip_maximum_mbm: i32::MAX,
        owner_cap_mbm: None,
    });
    TxPower::within(
        oer_protocols_bluetooth_le_contracts_port::LeTxCeiling::new(band, ceiling),
        0,
    )
    .unwrap()
}

const ADV_NONCONN_IND: [u8; 8] = [0x02, 6, 1, 2, 3, 4, 5, 6];
const OWN: LeDeviceAddress =
    LeDeviceAddress::from_wire_bytes([1, 2, 3, 4, 5, 6], LeDeviceAddressKind::Random);
const SPACING: ChannelSpacing<Caps> =
    ChannelSpacing::new(RadioDuration::from_micros(1_500)).unwrap();
const PRIORITY: ConnectionPriority = ConnectionPriority::new(8).unwrap();

fn window(start: u64) -> LeWindow {
    LeWindow::new(
        LeInstant::from_micros(start),
        RadioDuration::from_micros(1_000),
    )
    .unwrap()
}

/// Run `body` with a fresh installation: the port built from its key, the
/// machine's rights beside it.
fn installed(
    body: impl for<'id> FnOnce(
        &mut Idle<'id>,
        Stopped<'id>,
        StopRight<'id>,
        RoleRights<'id, 1, 0, 1, 0>,
    ),
) {
    let future = core::pin::pin!(branded(async |mint: Mint<'_, Pools>| {
        let Install {
            key,
            stopped,
            right,
            roles,
        } = mint.into_pools();
        let mut port = Idle::new(key);
        body(&mut port, stopped, right, roles);
    }));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(core::future::Future::poll(future, &mut cx).is_ready());
}

/// The machine's start and stop around `run`, then the retirement and the
/// release.
fn life<'id>(
    port: &mut Idle<'id>,
    stopped: Stopped<'id>,
    mut right: StopRight<'id>,
    roles: RoleRights<'id, 1, 0, 1, 0>,
    run: impl FnOnce(&mut Idle<'id>, RoleRights<'id, 1, 0, 1, 0>) -> RoleRights<'id, 1, 0, 1, 0>,
) {
    let mut cx = Context::from_waker(Waker::noop());
    let starting = port.start(stopped);
    let StartPoll::Running(running) = port.poll_start(starting, &mut cx) else {
        panic!("the idle port starts at once");
    };
    let roles = run(port, roles);
    let stopping = port.stop(&mut right, StopFrom::Running(running));
    let StopPoll::Stopped(stopped) = port.poll_stop(stopping, &mut cx) else {
        panic!("the idle port stops at once");
    };
    let retired = port.retire(stopped, roles);
    port.release(retired, right);
}

#[test]
fn an_advertising_set_runs_from_the_install_to_the_release() {
    installed(|port, stopped, right, roles| {
        life(port, stopped, right, roles, |port, roles| {
            let RoleRights {
                advertising: [place],
                scanners,
                connections,
                tests,
                accept_list,
            } = roles;
            let set = Supported::new(AdvertisingSet {
                data: AdvertisingData {
                    pdu: AdvertisingPdu::new(&ADV_NONCONN_IND).unwrap(),
                    reception: AdvertisingReception::None,
                },
                own: OWN,
                peer: None,
                filter_policy: AdvertisingFilterPolicy::AcceptAll,
                tx_power: power(),
                phy: LePhy::Le1M,
            })
            .unwrap();
            let (role, home) = port.open_advertising(place, &set);
            let event = Supported::placed(
                AdvertisingEvent {
                    anchor: LeInstant::from_micros(2_000),
                    channels: AdvertisingChannels::ALL,
                    channel_spacing: SPACING,
                    coexistence: AdvertisingEventKind::Ordinary,
                },
                LeInstant::from_micros(0),
            )
            .unwrap();
            let Published = port.publish_advertising(home, &event);
            let mut round = port.round();
            let ended = round.ended().unwrap();
            let EventPlace::Advertising(home) = ended.place() else {
                panic!("the advertising event's place comes back");
            };
            assert_eq!(round.close(), Rest::Quiet);
            let place = port.close_advertising(role, home);
            RoleRights {
                advertising: [place],
                scanners,
                connections,
                tests,
                accept_list,
            }
        });
    });
}

#[test]
fn a_connection_runs_from_the_install_to_the_release() {
    installed(|port, stopped, right, roles| {
        life(port, stopped, right, roles, |port, roles| {
            let RoleRights {
                advertising,
                scanners,
                connections: [place],
                tests,
                mut accept_list,
            } = roles;
            let link = Supported::new(ConnectionSetup {
                access_address: AccessAddress([1, 2, 3, 4]),
                crc_init: CrcInit([1, 2, 3]),
                created_at: LeInstant::from_micros(0),
                tx_power: power(),
                phy: LePhy::Le1M,
            })
            .unwrap();
            let (role, [first, second], [mut tx]) = port.open_connection(place, &link);
            port.write_pdu(&mut tx, DataPduKind::Start, |payload| {
                payload[0] = 1;
                PduLen::new(1).unwrap()
            });
            port.submit(tx);
            let event = |start| {
                Supported::placed(
                    ConnectionEvent {
                        channel: DataChannel::new(0).unwrap(),
                        window: window(start),
                        interval: RadioDuration::from_micros(30_000),
                        timing: ConnectionEventTiming::Recurring {
                            receive_wait: RadioDuration::from_micros(1_000),
                            widening: RadioDuration::from_micros(16),
                        },
                        priority: PRIORITY,
                        coexistence: ConnectionEventKind::Ordinary,
                    },
                    LeInstant::from_micros(0),
                )
                .unwrap()
            };
            let Published = port.publish_connection(first, &event(2_000));
            let Published = port.publish_connection(second, &event(32_000));

            // A round with both ends, the PDU's end and a reception held.
            let mut round = port.round();
            let mut held = Vec::new();
            let mut homes = Vec::new();
            while let Some(mut ended) = round.ended() {
                assert!(matches!(ended.end(), EventEnd::Executed(_)));
                while let Some(pdu) = ended.received() {
                    let _bytes = pdu.view().payload;
                    held.push(pdu.take());
                }
                let EventPlace::Connection(home) = ended.place() else {
                    panic!("a connection event's place comes back");
                };
                homes.push(home);
            }
            assert_eq!(
                (homes.len(), held.len()),
                (2, 2),
                "both ends and their receptions"
            );
            let (end, tx) = round.transmitted().unwrap();
            assert_eq!(end, TxEnd::Transmitted);
            assert_eq!(round.close(), Rest::Quiet);
            assert!(port.receptions.is_empty(), "both nodes are held");

            // A later round reads the held PDUs and releases them.
            let mut round = port.round();
            for handle in held {
                let _bytes = round.pdu(&handle).payload;
                round.release(handle);
            }
            let Rest::Quiet = round.close() else {
                panic!("nothing more");
            };
            assert_eq!(port.receptions.len(), 2, "both nodes went back");

            let listed = [OWN];
            let entries = Supported::new(AcceptEntries::new(&listed)).unwrap();
            port.write_accept_list(&mut accept_list, &entries);
            port.withdraw(RoleRef::Connection(&role));
            let [first, second]: [ConnEvent<'_>; 2] = homes.try_into().ok().unwrap();
            let place = port.close_connection(role, [first, second], [tx]);
            RoleRights {
                advertising,
                scanners,
                connections: [place],
                tests,
                accept_list,
            }
        });
    });
}

#[test]
fn a_forged_installation_holds_the_rights_a_mint_gives() {
    let Install {
        key,
        stopped,
        right,
        roles,
    } = oer_protocols_bluetooth_le_contracts_port::rights::forge::<1, 0, 1, 2, 1, 0>();
    let mut port = Idle::new(key);
    life(&mut port, stopped, right, roles, |_, roles| roles);
}

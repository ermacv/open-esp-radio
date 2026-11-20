#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::unreachable,
    clippy::indexing_slicing,
    reason = "the suite's checks are assertions over a test's port"
)]

//! The conformance suite of the Bluetooth LE port's contract: properties 1
//! to 7 of `design/ble-port-contract-2026-11-19.md`, section 7, as checks
//! over any port.
//!
//! A port's test implements [`Harness`] (the port, the hardware's levers a
//! test pulls, the values the port's calls take, and what the hardware
//! programmed and holds) and runs each property with its installation's
//! rights ([`Rights`]). The suite keeps the rights in its own places and
//! compares what the harness gives for equality only; it reads no
//! bookkeeping of the port's. The model port's tests run every property
//! over `oer-protocols-bluetooth-le-testing-model`; a chip's port runs the
//! same functions over its hardware.

extern crate std;

use core::task::{Context, Waker};
use std::vec::Vec;

use oer_protocols_bluetooth_le_contracts_port::{
    AcceptEntries, AcceptList, AdvEvent, AdvPlace, Advertiser, AdvertisingEvent,
    AdvertisingEventKind, AdvertisingSet, ChannelSpacing, ConnEvent, ConnPlace, Connection,
    ConnectionEvent, ConnectionEventKind, ConnectionEventTiming, ConnectionPriority,
    ConnectionSetup, Cut, EventEnd, EventPlace, Facts, LeCapabilities, LeInstant, LePort, LeWindow,
    MoreData, NotExecuted, PduHandle, PduLen, RadioDuration, Rest, RoleRef, RoleRights, Running,
    ScanEvent, ScanPlace, ScanWindow, Scanner, ScannerSetup, Signal, StartFailure, StartPoll,
    Starting, StopFrom, StopPoll, StopRight, Stopped, Supported, Test, TestEvent, TestPlace,
    TestWindow, TxEnd, TxPlace, dtm::DtmTest,
};
use oer_protocols_bluetooth_le_wire::{
    AdvertisingChannel, AdvertisingChannels, DataChannel, DataPduKind,
};

/// A role of the port.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Role {
    /// An advertising set.
    Advertising,
    /// The scanner.
    Scan,
    /// A connection.
    Connection,
    /// The test session.
    Test,
}

/// One event place of one instance of a role, as its place names it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EventId {
    /// Its role.
    pub role: Role,
    /// Its instance.
    pub instance: u8,
    /// Its slot: a connection's event place; 0 for the other roles.
    pub slot: u8,
}

impl EventId {
    /// The event a place names.
    pub fn of(place: &EventPlace<'_>) -> Self {
        let (role, instance, slot) = match place {
            EventPlace::Advertising(place) => (Role::Advertising, place.index(), 0),
            EventPlace::Scan(place) => (Role::Scan, place.index(), 0),
            EventPlace::Connection(place) => (Role::Connection, place.index(), place.slot()),
            EventPlace::Test(place) => (Role::Test, place.index(), 0),
        };
        Self {
            role,
            instance,
            slot,
        }
    }
}

/// Whether a peer acknowledges the head of a connection's chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ack {
    /// It acknowledges.
    Acknowledge,
    /// It withholds its acknowledgement.
    Withhold,
}

/// Whether a peer's packets reach the port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Heard {
    /// They do.
    Heard,
    /// They are lost.
    Lost,
}

/// What the peer of a role does in its next event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Peer {
    /// A scanner's or an initiator's request to an advertising event,
    /// header first.
    Request(Vec<u8>),
    /// The advertisers a scan window hears, each header first.
    Advertisements(Vec<Vec<u8>>),
    /// The central of a connection event.
    Central {
        /// Whether it is heard.
        heard: Heard,
        /// Whether it acknowledges the chain's head.
        ack: Ack,
        /// The PDU it sends, header first; empty for nothing.
        sends: Vec<u8>,
    },
    /// The central goes on past the event's window: the event is cut with
    /// `exchanged` packets before the cut.
    Overrun {
        /// The packets before the cut.
        exchanged: u8,
    },
    /// A tester's packets with a good CRC in a receiver test's window.
    Packets(u8),
}

/// A port under the suite, with the levers of its hardware, of the
/// geometry `A` advertising sets, `S` scanners, `C` connections of `E`
/// event places, `T` transmit places and `R` receive nodes, and `X` test
/// sessions.
pub trait Harness<
    'id,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>
{
    /// The port's capabilities.
    type Caps: LeCapabilities;
    /// The port.
    type Port: LePort<'id, A, S, C, E, T, R, X, Caps = Self::Caps>;
    /// What the hardware was asked to program in a run.
    type Record: Eq + core::fmt::Debug;
    /// What the hardware holds that a stop takes down.
    type State: Eq + core::fmt::Debug;

    /// The port.
    fn port(&mut self) -> &mut Self::Port;
    /// The hardware's clock.
    fn now(&self) -> LeInstant;
    /// Move the hardware's clock to `to`.
    fn advance(&mut self, to: LeInstant);
    /// What the peer of `role`'s `instance` does in its next event.
    fn peer(&mut self, role: Role, instance: u8, peer: Peer);
    /// The hardware does not run the listed event `event`.
    fn displace(&mut self, event: EventId);
    /// The anomalies the backend may indicate.
    fn anomalies(&self) -> usize;
    /// The hardware sees the anomaly `index`.
    fn raise(&mut self, index: usize);
    /// The next start fails by `failure`.
    fn fail_next_start(&mut self, failure: StartFailure);
    /// What the hardware was asked to program in the last run, ended by
    /// its stop.
    fn record(&self) -> Self::Record;
    /// What the hardware holds.
    fn state(&self) -> Self::State;
    /// An advertising set the port serves.
    fn advertising_set(&self) -> Supported<Self::Caps, AdvertisingSet<'static>>;
    /// A scanner the port serves.
    fn scanner(&self) -> Supported<Self::Caps, ScannerSetup>;
    /// A connection the port serves.
    fn link(&self) -> Supported<Self::Caps, ConnectionSetup>;
    /// A test the port serves.
    fn test(&self) -> Supported<Self::Caps, DtmTest>;
    /// An advertising event's channel spacing the port serves.
    fn spacing(&self) -> ChannelSpacing<Self::Caps>;
}

/// The rights the suite runs an installation with: what its mint gave the
/// machine.
pub struct Rights<'id, const A: usize, const S: usize, const C: usize, const X: usize> {
    /// The radio is stopped.
    pub stopped: Stopped<'id>,
    /// The stop's right.
    pub right: StopRight<'id>,
    /// The roles' rights.
    pub roles: RoleRights<'id, A, S, C, X>,
}

/// A context whose waker does nothing: a pending answer is a failure of
/// the property that met it.
fn cx() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

/// The suite's places of the installation's rights while it runs.
struct Places<'id> {
    stopped: Option<Stopped<'id>>,
    running: Option<Running<'id>>,
    right: StopRight<'id>,
    advertising: Vec<AdvPlace<'id>>,
    scanners: Vec<ScanPlace<'id>>,
    connections: Vec<ConnPlace<'id>>,
    tests: Vec<TestPlace<'id>>,
    accept_list: AcceptList<'id>,
}

impl<'id> Places<'id> {
    fn of<const A: usize, const S: usize, const C: usize, const X: usize>(
        rights: Rights<'id, A, S, C, X>,
    ) -> Self {
        let Rights {
            stopped,
            right,
            roles,
        } = rights;
        let RoleRights {
            advertising,
            scanners,
            connections,
            tests,
            accept_list,
        } = roles;
        Self {
            stopped: Some(stopped),
            running: None,
            right,
            advertising: advertising.into(),
            scanners: scanners.into(),
            connections: connections.into(),
            tests: tests.into(),
            accept_list,
        }
    }

    fn advertising(&mut self, instance: u8) -> AdvPlace<'id> {
        let at = self
            .advertising
            .iter()
            .position(|place| place.index() == instance)
            .expect("the set's place is home");
        self.advertising.remove(at)
    }

    fn connection(&mut self, instance: u8) -> ConnPlace<'id> {
        let at = self
            .connections
            .iter()
            .position(|place| place.index() == instance)
            .expect("the connection's place is home");
        self.connections.remove(at)
    }

    fn roles<const A: usize, const S: usize, const C: usize, const X: usize>(
        self,
    ) -> (Stopped<'id>, StopRight<'id>, RoleRights<'id, A, S, C, X>) {
        let Self {
            stopped,
            running,
            right,
            mut advertising,
            mut scanners,
            mut connections,
            mut tests,
            accept_list,
        } = self;
        assert!(running.is_none(), "the radio is stopped");
        advertising.sort_by_key(AdvPlace::index);
        scanners.sort_by_key(ScanPlace::index);
        connections.sort_by_key(ConnPlace::index);
        tests.sort_by_key(TestPlace::index);
        (
            stopped.expect("the radio is stopped"),
            right,
            RoleRights {
                advertising: home(advertising, "every set's place is home"),
                scanners: home(scanners, "the scanner's place is home"),
                connections: home(connections, "every connection's place is home"),
                tests: home(tests, "the test's place is home"),
                accept_list,
            },
        )
    }
}

/// The rights of `rights` back in their array, all of them.
fn home<T, const N: usize>(rights: Vec<T>, what: &str) -> [T; N] {
    match rights.try_into() {
        Ok(home) => home,
        Err(_) => panic!("{what}"),
    }
}

/// What one round gave, read in the order the round gave it.
struct Gave<'id> {
    /// The ends, each with its event, its place and its receptions'
    /// payloads.
    ends: Vec<(EventId, EventEnd, Vec<Vec<u8>>)>,
    places: Vec<EventPlace<'id>>,
    transmitted: Vec<(TxEnd, TxPlace<'id>)>,
    held: Vec<PduHandle<'id>>,
    rest: Rest,
}

impl<'id> Gave<'id> {
    fn ids(&self) -> Vec<(EventId, EventEnd)> {
        self.ends.iter().map(|(id, end, _)| (*id, *end)).collect()
    }

    fn tx(&self) -> Vec<(TxEnd, u8, u8)> {
        self.transmitted
            .iter()
            .map(|(end, place)| (*end, place.index(), place.slot()))
            .collect()
    }
}

/// Whether a round's reader takes the receptions it reads.
#[derive(Clone, Copy, PartialEq)]
enum Take {
    /// Read and take every reception.
    All,
    /// Read every reception, take none.
    None,
    /// Read no reception.
    Unread,
}

/// The suite's view of a harness.
struct Run<
    'h,
    'id,
    H,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
> {
    h: &'h mut H,
    places: Places<'id>,
}

impl<
    'h,
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
> Run<'h, 'id, H, A, S, C, E, T, R, X>
{
    fn new(h: &'h mut H, rights: Rights<'id, A, S, C, X>) -> Self {
        Self {
            h,
            places: Places::of(rights),
        }
    }

    fn port(&mut self) -> &mut H::Port {
        self.h.port()
    }

    fn at(&self, micros: u64) -> LeInstant {
        LeInstant::from_micros(self.h.now().as_micros() + micros)
    }

    fn advance(&mut self, micros: u64) {
        let to = self.at(micros);
        self.h.advance(to);
    }

    fn start(&mut self) {
        let stopped = self.places.stopped.take().expect("the radio is stopped");
        let starting = self.h.port().start(stopped);
        match self.h.port().poll_start(starting, &mut cx()) {
            StartPoll::Running(running) => self.places.running = Some(running),
            StartPoll::Pending(_) | StartPoll::Stopped(..) => panic!("the start runs the radio"),
        }
    }

    fn starting(&mut self) -> Starting<'id> {
        let stopped = self.places.stopped.take().expect("the radio is stopped");
        self.h.port().start(stopped)
    }

    fn stop_from(&mut self, from: StopFrom<'id>) {
        let mut stopping = self.h.port().stop(&mut self.places.right, from);
        let stopped = loop {
            match self.h.port().poll_stop(stopping, &mut cx()) {
                StopPoll::Stopped(stopped) => break stopped,
                StopPoll::Pending(again) => stopping = again,
            }
        };
        self.places.stopped = Some(stopped);
    }

    fn stop(&mut self) {
        let running = self.places.running.take().expect("the radio runs");
        self.stop_from(StopFrom::Running(running));
    }

    fn retire(self) {
        let Self { h, places } = self;
        let (stopped, right, roles) = places.roles::<A, S, C, X>();
        let retired = h.port().retire(stopped, roles);
        let _released = h.port().release(retired, right);
    }

    /// One round, read whole.
    fn round(&mut self, take: Take) -> Gave<'id> {
        let mut gave = Gave {
            ends: Vec::new(),
            places: Vec::new(),
            transmitted: Vec::new(),
            held: Vec::new(),
            rest: Rest::Quiet,
        };
        let mut round = self.h.port().round();
        while let Some(mut ended) = round.ended() {
            let end = ended.end();
            let mut payloads = Vec::new();
            while take != Take::Unread
                && let Some(pdu) = ended.received()
            {
                payloads.push(pdu.view().payload.to_vec());
                if take == Take::All {
                    gave.held.push(pdu.take());
                }
            }
            let place = ended.place();
            gave.ends.push((EventId::of(&place), end, payloads));
            gave.places.push(place);
        }
        while let Some(end) = round.transmitted() {
            gave.transmitted.push(end);
        }
        gave.rest = round.close();
        gave
    }

    /// Release `held` in one round; answer the payloads read before.
    fn release(&mut self, held: Vec<PduHandle<'id>>) -> Vec<Vec<u8>> {
        let mut round = self.h.port().round();
        let mut payloads = Vec::new();
        for handle in held {
            payloads.push(round.pdu(&handle).payload.to_vec());
            round.release(handle);
        }
        let _rest = round.close();
        payloads
    }

    fn advertising_event(&self, micros: u64) -> Supported<H::Caps, AdvertisingEvent<H::Caps>> {
        Supported::placed(
            AdvertisingEvent {
                anchor: self.at(micros),
                channels: AdvertisingChannels::ALL,
                channel_spacing: self.h.spacing(),
                coexistence: AdvertisingEventKind::Ordinary,
            },
            self.h.now(),
        )
        .expect("an advertising event the port serves")
    }

    fn connection_event(&self, micros: u64) -> Supported<H::Caps, ConnectionEvent> {
        const PRIORITY: ConnectionPriority = match ConnectionPriority::new(8) {
            Some(priority) => priority,
            None => panic!("8 is a priority"),
        };
        Supported::placed(
            ConnectionEvent {
                channel: DataChannel::new(0).expect("a data channel"),
                window: window(self.at(micros), 2_000),
                interval: RadioDuration::from_micros(30_000),
                timing: ConnectionEventTiming::Recurring {
                    receive_wait: RadioDuration::from_micros(1_000),
                    widening: RadioDuration::from_micros(16),
                },
                priority: PRIORITY,
                coexistence: ConnectionEventKind::Ordinary,
            },
            self.h.now(),
        )
        .expect("a connection event the port serves")
    }

    fn scan_window(&self, micros: u64) -> Supported<H::Caps, ScanWindow> {
        Supported::placed(
            ScanWindow {
                channel: AdvertisingChannel::Channel37,
                window: window(self.at(micros), 5_000),
            },
            self.h.now(),
        )
        .expect("a scan window the port serves")
    }

    fn test_window(&self, micros: u64) -> Supported<H::Caps, TestWindow> {
        Supported::placed(
            TestWindow {
                window: window(self.at(micros), 1_000),
            },
            self.h.now(),
        )
        .expect("a test window the port serves")
    }

    fn open_set(&mut self, instance: u8) -> (Advertiser<'id>, AdvEvent<'id>) {
        let place = self.places.advertising(instance);
        let set = self.h.advertising_set();
        self.h.port().open_advertising(place, &set)
    }

    fn close_set(&mut self, role: Advertiser<'id>, home: AdvEvent<'id>) {
        let place = self.h.port().close_advertising(role, home);
        self.places.advertising.push(place);
    }

    fn open_link(
        &mut self,
        instance: u8,
    ) -> (Connection<'id>, [ConnEvent<'id>; E], [TxPlace<'id>; T]) {
        let place = self.places.connection(instance);
        let link = self.h.link();
        self.h.port().open_connection(place, &link)
    }

    fn close_link(
        &mut self,
        role: Connection<'id>,
        events: Vec<ConnEvent<'id>>,
        tx: Vec<TxPlace<'id>>,
    ) {
        let mut events = events;
        let mut tx = tx;
        events.sort_by_key(ConnEvent::slot);
        tx.sort_by_key(TxPlace::slot);
        let place = self.h.port().close_connection(
            role,
            home(events, "every event place of the connection is home"),
            home(tx, "every transmit place of the connection is home"),
        );
        self.places.connections.push(place);
    }

    fn publish_set(&mut self, home: AdvEvent<'id>, micros: u64) {
        let event = self.advertising_event(micros);
        let _published = self.h.port().publish_advertising(home, &event);
    }

    fn publish_link(&mut self, home: ConnEvent<'id>, micros: u64) {
        let event = self.connection_event(micros);
        let _published = self.h.port().publish_connection(home, &event);
    }

    fn submit(&mut self, mut place: TxPlace<'id>, byte: u8) {
        self.h
            .port()
            .write_pdu(&mut place, DataPduKind::Start, |payload| {
                payload[0] = byte;
                PduLen::new(1).expect("one octet fits")
            });
        self.h.port().submit(place);
    }

    fn facts(&mut self) -> Facts {
        self.h.port().poll_facts(&mut cx())
    }
}

fn window(start: LeInstant, micros: u64) -> LeWindow {
    LeWindow::new(start, RadioDuration::from_micros(micros)).expect("a window in the epoch")
}

fn adv_places(places: Vec<EventPlace<'_>>) -> Vec<AdvEvent<'_>> {
    places
        .into_iter()
        .map(|place| match place {
            EventPlace::Advertising(place) => place,
            other => panic!("an advertising event's place, not {other:?}"),
        })
        .collect()
}

fn conn_places(places: Vec<EventPlace<'_>>) -> Vec<ConnEvent<'_>> {
    places
        .into_iter()
        .map(|place| match place {
            EventPlace::Connection(place) => place,
            other => panic!("a connection event's place, not {other:?}"),
        })
        .collect()
}

fn executed(end: EventEnd) -> oer_protocols_bluetooth_le_contracts_port::Executed {
    match end {
        EventEnd::Executed(executed) => executed,
        other => panic!("an executed event, not {other:?}"),
    }
}

/// A data channel PDU of `payload`, header first, as a peer sends it.
pub fn data(payload: &[u8]) -> Vec<u8> {
    let mut pdu = std::vec![0x02, u8::try_from(payload.len()).expect("a short payload")];
    pdu.extend_from_slice(payload);
    pdu
}

fn central(sends: &[u8], ack: Ack) -> Peer {
    Peer::Central {
        heard: Heard::Heard,
        ack,
        sends: sends.to_vec(),
    }
}

const WITHDRAWN: EventEnd = EventEnd::NotExecuted(NotExecuted::Withdrawn);

/// One run of every role the geometry has, from its opening to its closing:
/// each instance an event, a connection's chain a PDU, a received PDU taken
/// and released.
fn each_role<
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>(
    run: &mut Run<'_, 'id, H, A, S, C, E, T, R, X>,
) {
    for instance in 0..A {
        let instance = u8::try_from(instance).unwrap();
        let (role, home) = run.open_set(instance);
        run.publish_set(home, 2_000);
        run.advance(10_000);
        let gave = run.round(Take::None);
        let [home] = home_of(adv_places(gave.places));
        run.close_set(role, home);
    }
    for place in core::mem::take(&mut run.places.scanners) {
        let scanner = run.h.scanner();
        let (role, home) = run.port().open_scanner(place, &scanner);
        let window = run.scan_window(2_000);
        let _published = run.port().publish_scan(home, &window);
        run.advance(10_000);
        let gave = run.round(Take::None);
        let [EventPlace::Scan(home)] = home_of(gave.places) else {
            panic!("the scanner's place");
        };
        let place = run.port().close_scanner(role, home);
        run.places.scanners.push(place);
    }
    for instance in 0..C {
        let instance = u8::try_from(instance).unwrap();
        let (role, events, tx) = run.open_link(instance);
        let mut tx: Vec<_> = tx.into();
        let mut out = Vec::new();
        if let Some(place) = tx.pop() {
            run.submit(place, 1);
        }
        run.h.peer(
            Role::Connection,
            instance,
            central(&data(&[9]), Ack::Acknowledge),
        );
        for (number, event) in events.into_iter().enumerate() {
            run.publish_link(event, 2_000 + 30_000 * u64::try_from(number).unwrap());
        }
        run.advance(2_000 + 30_000 * u64::try_from(E).unwrap());
        let gave = run.round(Take::All);
        out.extend(conn_places(gave.places));
        tx.extend(gave.transmitted.into_iter().map(|(_, place)| place));
        let _payloads = run.release(gave.held);
        run.port().withdraw(RoleRef::Connection(&role));
        let rest = run.round(Take::None);
        out.extend(conn_places(rest.places));
        tx.extend(rest.transmitted.into_iter().map(|(_, place)| place));
        run.close_link(role, out, tx);
    }
    for place in core::mem::take(&mut run.places.tests) {
        let test = run.h.test();
        let (role, home) = run.port().open_test(place, &test);
        let window = run.test_window(2_000);
        let _published = run.port().publish_test(home, &window);
        run.advance(10_000);
        let gave = run.round(Take::None);
        let [EventPlace::Test(home)] = home_of(gave.places) else {
            panic!("the test's place");
        };
        let place = run.port().close_test(role, home);
        run.places.tests.push(place);
    }
}

fn home_of<T, const N: usize>(places: Vec<T>) -> [T; N] {
    home(places, "every place of the instance came back")
}

/// **1. Every right is in exactly one place**: a run of every role and its
/// stop hand every right back to the suite's places, the accept list's
/// right is lent and comes back, and a retirement takes them whole.
pub fn every_right_is_in_exactly_one_place<
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>(
    harness: &mut H,
    rights: Rights<'id, A, S, C, X>,
) {
    let mut run = Run::new(harness, rights);
    run.start();
    let entries = Supported::new(AcceptEntries::new(&[])).expect("an empty list is served");
    run.h
        .port()
        .write_accept_list(&mut run.places.accept_list, &entries);
    each_role(&mut run);
    run.stop();
    run.retire();
}

/// **2. An event ends once, as what happened**: `Executed` when it ran, cut
/// when its exchange did not fit, `Late` when published past its start less
/// the guard, `Displaced` when the hardware did not run it, `Withdrawn` by
/// a withdraw or a stop before it ran, `Indeterminate` when the stop came
/// while it ran; and no end comes twice.
pub fn an_event_ends_once_as_what_happened<
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>(
    harness: &mut H,
    rights: Rights<'id, A, S, C, X>,
) {
    assert!(
        A >= 1 && C >= 1,
        "the property runs an advertising set and a connection"
    );
    let mut run = Run::new(harness, rights);
    run.start();
    let set = EventId {
        role: Role::Advertising,
        instance: 0,
        slot: 0,
    };
    let (role, home) = run.open_set(0);
    // Executed.
    run.publish_set(home, 2_000);
    run.advance(10_000);
    let gave = run.round(Take::None);
    assert_eq!(gave.ends.len(), 1);
    let ran = executed(gave.ends[0].1);
    assert_eq!(ran.cut, Cut::Whole);
    let [home] = home_of(adv_places(gave.places));
    assert!(run.round(Take::None).ends.is_empty(), "an end comes once");
    // Late: its reservation starts before now.
    run.publish_set(home, 1);
    let gave = run.round(Take::None);
    assert_eq!(
        gave.ids(),
        [(set, EventEnd::NotExecuted(NotExecuted::Late))]
    );
    let [home] = home_of(adv_places(gave.places));
    // Displaced.
    run.publish_set(home, 2_000);
    run.h.displace(set);
    run.advance(10_000);
    let gave = run.round(Take::None);
    assert_eq!(
        gave.ids(),
        [(set, EventEnd::NotExecuted(NotExecuted::Displaced))]
    );
    let [home] = home_of(adv_places(gave.places));
    // Withdrawn by a withdraw.
    run.publish_set(home, 2_000);
    run.port().withdraw(RoleRef::Advertiser(&role));
    let gave = run.round(Take::None);
    assert_eq!(gave.ids(), [(set, WITHDRAWN)]);
    let [home] = home_of(adv_places(gave.places));
    run.advance(10_000);
    assert!(
        run.round(Take::None).ends.is_empty(),
        "a withdrawn event ends once"
    );
    // Cut.
    let (link, events, tx) = run.open_link(0);
    let mut events: Vec<_> = events.into();
    run.h
        .peer(Role::Connection, 0, Peer::Overrun { exchanged: 3 });
    let first = events.remove(0);
    run.publish_link(first, 2_000);
    run.advance(10_000);
    let gave = run.round(Take::None);
    let cut = executed(gave.ends[0].1);
    assert_eq!(
        (cut.cut, cut.exchanged),
        (Cut::Cut, 3),
        "cut with what was exchanged before"
    );
    events.extend(conn_places(gave.places));
    run.close_link(link, events, tx.into());
    // Withdrawn by a stop before it ran; Indeterminate by a stop while it ran.
    run.publish_set(home, 2_000);
    run.stop();
    let gave = run.round(Take::None);
    assert_eq!(gave.ids(), [(set, WITHDRAWN)]);
    let [home] = home_of(adv_places(gave.places));
    run.start();
    run.publish_set(home, 2_000);
    run.advance(3_000);
    run.stop();
    let gave = run.round(Take::None);
    assert_eq!(gave.ids(), [(set, EventEnd::Indeterminate)]);
    let [home] = home_of(adv_places(gave.places));
    run.advance(10_000);
    assert!(run.round(Take::None).ends.is_empty(), "an end comes once");
    run.close_set(role, home);
    run.retire();
}

/// **2'. Events end in the air's order**, whatever the order of their
/// publication.
pub fn events_end_in_the_airs_order<
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>(
    harness: &mut H,
    rights: Rights<'id, A, S, C, X>,
) {
    assert!(
        C >= 1 && E >= 2,
        "the property runs a connection of two events"
    );
    let mut run = Run::new(harness, rights);
    run.start();
    let (link, events, tx) = run.open_link(0);
    let mut events: Vec<_> = events.into();
    let first = events.remove(0);
    let second = events.remove(0);
    // The later event published first.
    run.publish_link(first, 40_000);
    run.publish_link(second, 2_000);
    run.advance(50_000);
    let gave = run.round(Take::None);
    assert_eq!(
        gave.ends
            .iter()
            .map(|(id, _, _)| id.slot)
            .collect::<Vec<_>>(),
        [1, 0],
        "the earlier event ends first"
    );
    events.extend(conn_places(gave.places));
    run.close_link(link, events, tx.into());
    run.stop();
    run.retire();
}

/// **2''. A withdraw is of one instance**: its events not begun and its
/// chain end; another instance's stay out and end as they run.
pub fn a_withdraw_is_of_one_instance<
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>(
    harness: &mut H,
    rights: Rights<'id, A, S, C, X>,
) {
    assert!(C >= 2 && T >= 1, "the property runs two connections");
    let mut run = Run::new(harness, rights);
    run.start();
    let mut links = Vec::new();
    for instance in 0..2u8 {
        let (link, events, tx) = run.open_link(instance);
        let mut events: Vec<_> = events.into();
        let mut tx: Vec<_> = tx.into();
        run.submit(tx.remove(0), instance);
        run.publish_link(events.remove(0), 2_000 + 3_000 * u64::from(instance));
        links.push((link, events, tx));
    }
    let (first, _, _) = &links[0];
    run.port().withdraw(RoleRef::Connection(first));
    let gave = run.round(Take::None);
    assert_eq!(
        gave.ids(),
        [(
            EventId {
                role: Role::Connection,
                instance: 0,
                slot: 0,
            },
            WITHDRAWN
        )]
    );
    assert_eq!(
        gave.tx(),
        [(TxEnd::Withdrawn, 0, 0)],
        "the first's chain only"
    );
    links[0].1.extend(conn_places(gave.places));
    links[0]
        .2
        .extend(gave.transmitted.into_iter().map(|(_, p)| p));
    run.h
        .peer(Role::Connection, 1, central(&[], Ack::Acknowledge));
    run.advance(10_000);
    let gave = run.round(Take::None);
    assert_eq!(gave.ends.len(), 1);
    assert!(matches!(
        gave.ends[0],
        (EventId { instance: 1, .. }, EventEnd::Executed(_), _)
    ));
    assert_eq!(
        gave.tx(),
        [(TxEnd::Transmitted, 1, 0)],
        "the second's chain runs"
    );
    links[1].1.extend(conn_places(gave.places));
    links[1]
        .2
        .extend(gave.transmitted.into_iter().map(|(_, p)| p));
    for (link, events, tx) in links {
        run.close_link(link, events, tx);
    }
    run.stop();
    run.retire();
}

/// **7. The chain leaves in the order submitted, a PDU once, when
/// acknowledged**; a withheld acknowledgement and a lost packet end
/// nothing.
pub fn the_chain_leaves_in_the_order_submitted<
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>(
    harness: &mut H,
    rights: Rights<'id, A, S, C, X>,
) {
    assert!(
        C >= 1 && E >= 2 && T >= 3,
        "the property runs a chain of three"
    );
    let mut run = Run::new(harness, rights);
    run.start();
    let (link, events, tx) = run.open_link(0);
    let mut events: Vec<_> = events.into();
    let mut tx: Vec<_> = tx.into();
    for byte in 0..3 {
        run.submit(tx.remove(0), byte);
    }
    let lost = Peer::Central {
        heard: Heard::Lost,
        ack: Ack::Acknowledge,
        sends: Vec::new(),
    };
    let none: Vec<(TxEnd, u8, u8)> = Vec::new();
    let sent = |slot| std::vec![(TxEnd::Transmitted, 0, slot)];
    for (peer, expected, chained) in [
        (central(&[], Ack::Withhold), none.clone(), 3),
        (lost.clone(), none.clone(), 3),
        (central(&[], Ack::Acknowledge), sent(0), 2),
        (central(&[], Ack::Withhold), none.clone(), 2),
        (lost, none.clone(), 2),
        (central(&[], Ack::Acknowledge), sent(1), 1),
        (central(&[], Ack::Acknowledge), sent(2), 0),
        (central(&[], Ack::Acknowledge), none.clone(), 0),
    ] {
        run.h.peer(Role::Connection, 0, peer.clone());
        run.publish_link(events.remove(0), 2_000);
        run.advance(10_000);
        let gave = run.round(Take::None);
        assert_eq!(
            gave.tx(),
            expected,
            "an event ends the chain's head only when its central acknowledges it ({peer:?})"
        );
        events.extend(conn_places(gave.places));
        tx.extend(gave.transmitted.into_iter().map(|(_, p)| p));
        assert_eq!(
            tx.len(),
            T - chained,
            "the chain is as it was after {peer:?}"
        );
    }
    run.close_link(link, events, tx);
    run.stop();
    run.retire();
}

/// **6. A received PDU is in one place**: read in its event's round, taken
/// and held across rounds, its node back at the close of the round of its
/// release; with no node free the peer's PDU is not acknowledged and comes
/// again; a PDU held across a stop is read and released after it.
pub fn a_received_pdu_is_in_one_place<
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>(
    harness: &mut H,
    rights: Rights<'id, A, S, C, X>,
) {
    assert!(
        C >= 1 && R >= 1,
        "the property runs a connection with nodes"
    );
    let mut run = Run::new(harness, rights);
    run.start();
    let (link, events, tx) = run.open_link(0);
    let mut events: Vec<_> = events.into();
    let one_event = |run: &mut Run<'_, 'id, H, A, S, C, E, T, R, X>,
                     events: &mut Vec<ConnEvent<'id>>,
                     peer: Peer,
                     take: Take| {
        run.h.peer(Role::Connection, 0, peer);
        run.publish_link(events.remove(0), 2_000);
        run.advance(10_000);
        let gave = run.round(take);
        let ends = gave.ends.clone();
        events.extend(conn_places(gave.places));
        (ends, gave.held)
    };
    // Receptions read and not taken, and receptions not read, are back by
    // the close of their round: twice the chain's nodes, each event's PDU
    // finds a node.
    for (byte, take) in (0..2 * R)
        .map(|byte| (byte, Take::None))
        .chain((0..2 * R).map(|byte| (byte, Take::Unread)))
    {
        let byte = u8::try_from(byte).unwrap();
        let (ends, taken) = one_event(
            &mut run,
            &mut events,
            central(&data(&[byte]), Ack::Withhold),
            take,
        );
        assert!(taken.is_empty());
        if take == Take::None {
            assert_eq!(ends[0].2, [std::vec![byte]], "a node was free for it");
        }
    }
    let (ends, _) = one_event(
        &mut run,
        &mut events,
        central(&data(&[0x77]), Ack::Withhold),
        Take::None,
    );
    assert_eq!(
        ends[0].2,
        [std::vec![0x77]],
        "unread receptions went back at their close"
    );
    // Fill every node, each PDU read in its event's round and held.
    let mut held = Vec::new();
    for byte in 0..R {
        let byte = u8::try_from(byte).unwrap();
        let (ends, taken) = one_event(
            &mut run,
            &mut events,
            central(&data(&[byte]), Ack::Withhold),
            Take::All,
        );
        assert_eq!(ends[0].2, [std::vec![byte]], "read in its event's round");
        held.extend(taken);
    }
    // No node free: the peer's PDU is neither taken nor acknowledged.
    let (ends, taken) = one_event(
        &mut run,
        &mut events,
        central(&data(&[0xee]), Ack::Withhold),
        Take::All,
    );
    assert!(ends[0].2.is_empty(), "no node free: nothing taken");
    assert_eq!(executed(ends[0].1).peer, Some(MoreData::More));
    assert!(taken.is_empty());
    // Held across rounds and read after them; the release frees one node.
    let last = held.pop().expect("a held PDU");
    let read = run.release(std::vec![last]);
    assert_eq!(
        read,
        [std::vec![u8::try_from(R - 1).unwrap()]],
        "held across rounds, in place"
    );
    let (ends, taken) = one_event(&mut run, &mut events, Peer::Packets(0), Take::All);
    assert_eq!(
        ends[0].2,
        [std::vec![0xee]],
        "sent again once a node is free"
    );
    held.extend(taken);
    // The receptions of one event in the air's order.
    if S >= 1 && R >= 2 {
        let place = run.places.scanners.pop().expect("the scanner's place");
        let scanner = run.h.scanner();
        let (role, home) = run.h.port().open_scanner(place, &scanner);
        run.h.peer(
            Role::Scan,
            0,
            Peer::Advertisements(std::vec![std::vec![0x02, 1, 1], std::vec![0x02, 1, 2]]),
        );
        let window = run.scan_window(2_000);
        let _published = run.h.port().publish_scan(home, &window);
        run.advance(10_000);
        let gave = run.round(Take::None);
        assert_eq!(
            gave.ends[0].2,
            [std::vec![1], std::vec![2]],
            "in the air's order"
        );
        let [EventPlace::Scan(home)] = home_of(gave.places) else {
            panic!("the scanner's place");
        };
        let place = run.h.port().close_scanner(role, home);
        run.places.scanners.push(place);
    }
    // Held across a stop: read and released after it.
    run.stop();
    let mut expected: Vec<Vec<u8>> = (0..R - 1)
        .map(|byte| std::vec![u8::try_from(byte).unwrap()])
        .collect();
    expected.push(std::vec![0xee]);
    let read = run.release(held);
    assert_eq!(
        read, expected,
        "every held PDU read in place after the stop"
    );
    run.start();
    let rest = run.round(Take::None);
    events.extend(conn_places(rest.places));
    run.close_link(link, events, tx.into());
    run.stop();
    run.retire();
}

/// **5. The indication holds until the stop**: after each anomaly the facts
/// say the port needs a stop, with no payload; a round gives no event's end
/// and no transmit end; the scheduler runs nothing; the stop's rounds then
/// give everything.
pub fn the_indication_holds_until_the_stop<
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>(
    harness: &mut H,
    rights: Rights<'id, A, S, C, X>,
) {
    assert!(C >= 1 && E >= 2 && T >= 1, "the property runs a connection");
    let mut run = Run::new(harness, rights);
    for anomaly in 0..run.h.anomalies() {
        run.start();
        let (link, events, tx) = run.open_link(0);
        let mut events: Vec<_> = events.into();
        let mut tx: Vec<_> = tx.into();
        // An event that ended and a PDU acknowledged before the anomaly, and
        // an event listed after it.
        run.submit(tx.remove(0), 1);
        run.h
            .peer(Role::Connection, 0, central(&[], Ack::Acknowledge));
        run.publish_link(events.remove(0), 2_000);
        run.advance(10_000);
        run.publish_link(events.remove(0), 2_000);
        run.h.raise(anomaly);
        let facts = run.facts();
        assert_eq!(facts.needs_stop, Signal::Raised, "anomaly {anomaly}");
        let gave = run.round(Take::None);
        assert!(gave.ends.is_empty(), "no end under the anomaly {anomaly}");
        assert!(
            gave.transmitted.is_empty(),
            "no transmit end under the anomaly {anomaly}"
        );
        run.advance(10_000);
        run.stop();
        assert_eq!(run.facts().needs_stop, Signal::Quiet);
        let gave = run.round(Take::None);
        assert_eq!(gave.ends.len(), 2, "the stop's rounds give everything");
        assert!(matches!(gave.ends[0].1, EventEnd::Executed(_)));
        assert_eq!(
            gave.ends[1].1, WITHDRAWN,
            "the scheduler ran nothing under the anomaly"
        );
        assert_eq!(gave.tx(), [(TxEnd::Transmitted, 0, 0)]);
        events.extend(conn_places(gave.places));
        tx.extend(gave.transmitted.into_iter().map(|(_, p)| p));
        run.close_link(link, events, tx);
    }
    run.retire();
}

/// **4. A stop ends everything**: from a running radio and from a start in
/// progress; with an event listed, one begun and a chain; after each
/// anomaly; and an alarm armed before a stop raises no fact after it.
pub fn a_stop_ends_everything<
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>(
    harness: &mut H,
    rights: Rights<'id, A, S, C, X>,
) {
    assert!(C >= 1 && E >= 2 && T >= 1, "the property runs a connection");
    let mut run = Run::new(harness, rights);
    // From a start in progress: the state is as before the start.
    let before = run.h.state();
    let starting = run.starting();
    run.stop_from(StopFrom::Starting(starting));
    assert!(run.round(Take::None).ends.is_empty());
    assert_eq!(
        run.h.state(),
        before,
        "a stop of a start in progress ends it"
    );
    for anomaly in [None].into_iter().chain((0..run.h.anomalies()).map(Some)) {
        run.start();
        let (link, events, tx) = run.open_link(0);
        let mut events: Vec<_> = events.into();
        let mut tx: Vec<_> = tx.into();
        run.submit(tx.remove(0), 1);
        run.h.peer(Role::Connection, 0, central(&[], Ack::Withhold));
        run.publish_link(events.remove(0), 2_000);
        run.publish_link(events.remove(0), 40_000);
        run.advance(3_000);
        let alarm = run.at(5_000);
        run.port().alarm(Some(alarm));
        if let Some(anomaly) = anomaly {
            run.h.raise(anomaly);
        }
        run.stop();
        let gave = run.round(Take::None);
        assert_eq!(
            gave.ends.iter().map(|(_, end, _)| *end).collect::<Vec<_>>(),
            [EventEnd::Indeterminate, WITHDRAWN],
            "the begun one indeterminate, the listed one withdrawn ({anomaly:?})"
        );
        assert_eq!(gave.tx(), [(TxEnd::Withdrawn, 0, 0)]);
        assert_eq!(gave.rest, Rest::Quiet);
        run.advance(10_000);
        assert_eq!(
            run.facts(),
            Facts::default(),
            "no fact after the stop ({anomaly:?})"
        );
        events.extend(conn_places(gave.places));
        tx.extend(gave.transmitted.into_iter().map(|(_, p)| p));
        run.close_link(link, events, tx);
    }
    run.retire();
}

/// **3. A second run is as the first**: what the hardware programs in a
/// run after a stop equals the first run's, and its state after each stop
/// equals its state before the first start: after one role's run, after
/// every role's, after each anomaly, after a failed start, and over eight
/// runs in a row.
pub fn a_second_run_is_as_the_first<
    'id,
    H: Harness<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>(
    harness: &mut H,
    rights: Rights<'id, A, S, C, X>,
) {
    let mut run = Run::new(harness, rights);
    let before = run.h.state();
    let each = |run: &mut Run<'_, 'id, H, A, S, C, E, T, R, X>| {
        run.advance(1_000_000);
        run.start();
        each_role(run);
        run.stop();
        assert_eq!(
            run.h.state(),
            before,
            "a stop leaves the state before the first start"
        );
        run.h.record()
    };
    let first = each(&mut run);
    assert_eq!(each(&mut run), first, "a second run is as the first");
    for anomaly in 0..run.h.anomalies() {
        run.advance(1_000_000);
        run.start();
        if A >= 1 {
            let (role, home) = run.open_set(0);
            run.publish_set(home, 2_000);
            run.h.raise(anomaly);
            run.stop();
            let gave = run.round(Take::None);
            let [home] = home_of(adv_places(gave.places));
            run.close_set(role, home);
        } else {
            run.h.raise(anomaly);
            run.stop();
        }
        assert_eq!(
            run.h.state(),
            before,
            "the stop of anomaly {anomaly} leaves the state"
        );
        assert_eq!(
            each(&mut run),
            first,
            "a run after anomaly {anomaly} is as the first"
        );
    }
    run.h.fail_next_start(StartFailure::Phy);
    let stopped = run.places.stopped.take().expect("stopped");
    let starting = run.h.port().start(stopped);
    match run.h.port().poll_start(starting, &mut cx()) {
        StartPoll::Stopped(stopped, failure) => {
            assert_eq!(failure, StartFailure::Phy);
            run.places.stopped = Some(stopped);
        }
        StartPoll::Pending(_) | StartPoll::Running(_) => panic!("the start fails"),
    }
    assert_eq!(run.h.state(), before, "a failed start leaves the state");
    for anomaly in 0..run.h.anomalies() {
        run.h.fail_next_start(StartFailure::Bound);
        let starting = run.starting();
        run.h.raise(anomaly);
        match run.h.port().poll_start(starting, &mut cx()) {
            StartPoll::Stopped(stopped, failure) => {
                assert_eq!(failure, StartFailure::Bound);
                run.places.stopped = Some(stopped);
            }
            StartPoll::Pending(_) | StartPoll::Running(_) => panic!("the start fails"),
        }
        assert_eq!(
            run.facts(),
            Facts::default(),
            "a failed start takes down anomaly {anomaly}"
        );
        assert_eq!(
            run.h.state(),
            before,
            "a failed start takes everything down"
        );
        assert_eq!(
            each(&mut run),
            first,
            "a run after a failed start with anomaly {anomaly}"
        );
    }
    assert_eq!(
        each(&mut run),
        first,
        "a run after a failed start is as the first"
    );
    for _ in 0..8 {
        assert_eq!(each(&mut run), first, "eight runs in a row");
    }
    run.retire();
}

/// **2. No call returns an error**: the port's calls, as the trait declares
/// them, answer outcomes and rights.
pub fn no_call_returns_an_error<
    'id,
    P: LePort<'id, A, S, C, E, T, R, X>,
    const A: usize,
    const S: usize,
    const C: usize,
    const E: usize,
    const T: usize,
    const R: usize,
    const X: usize,
>() {
    let _: fn(&mut P, Stopped<'id>) -> Starting<'id> = P::start;
    let _: fn(&mut P, Starting<'id>, &mut Context<'_>) -> StartPoll<'id> = P::poll_start;
    let _: fn(
        &mut P,
        &mut StopRight<'id>,
        StopFrom<'id>,
    ) -> oer_protocols_bluetooth_le_contracts_port::Stopping<'id> = P::stop;
    let _: fn(
        &mut P,
        oer_protocols_bluetooth_le_contracts_port::Stopping<'id>,
        &mut Context<'_>,
    ) -> StopPoll<'id> = P::poll_stop;
    let _: fn(
        &mut P,
        Stopped<'id>,
        RoleRights<'id, A, S, C, X>,
    ) -> oer_protocols_bluetooth_le_contracts_port::Retired<'id> = P::retire;
    let _: fn(
        &mut P,
        AdvPlace<'id>,
        &Supported<P::Caps, AdvertisingSet<'_>>,
    ) -> (Advertiser<'id>, AdvEvent<'id>) = P::open_advertising;
    let _: fn(&mut P, Advertiser<'id>, AdvEvent<'id>) -> AdvPlace<'id> = P::close_advertising;
    let _: fn(
        &mut P,
        ScanPlace<'id>,
        &Supported<P::Caps, ScannerSetup>,
    ) -> (Scanner<'id>, ScanEvent<'id>) = P::open_scanner;
    let _: fn(
        &mut P,
        ConnPlace<'id>,
        &Supported<P::Caps, ConnectionSetup>,
    ) -> (Connection<'id>, [ConnEvent<'id>; E], [TxPlace<'id>; T]) = P::open_connection;
    let _: fn(&mut P, TestPlace<'id>, &Supported<P::Caps, DtmTest>) -> (Test<'id>, TestEvent<'id>) =
        P::open_test;
    let _: fn(
        &mut P,
        AdvEvent<'id>,
        &Supported<P::Caps, AdvertisingEvent<P::Caps>>,
    ) -> oer_protocols_bluetooth_le_contracts_port::Published = P::publish_advertising;
    let _: fn(
        &mut P,
        ConnEvent<'id>,
        &Supported<P::Caps, ConnectionEvent>,
    ) -> oer_protocols_bluetooth_le_contracts_port::Published = P::publish_connection;
    let _: fn(&mut P, RoleRef<'_, 'id>) = P::withdraw;
    let _: fn(&mut P, TxPlace<'id>) = P::submit;
    let _: fn(&mut P, Option<LeInstant>) = P::alarm;
    let _: fn(&mut P, &mut Context<'_>) -> Facts = P::poll_facts;
}

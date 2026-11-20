//! The port's calls (`design/ble-port-contract-2026-11-19.md`, sections 4
//! and 5): the radio's life, the roles' openings and closings, the
//! publications, the transmit chain, a round, the alarm and the facts.
//!
//! No call answers an error: section 8 of the contract is the whole list of
//! answers, and each is a value of this module.

use core::{marker::PhantomData, task::Context};

use oer_protocols_bluetooth_le_wire::DataPduKind;

use crate::{
    AcceptEntries, AdvertisingData, AdvertisingEvent, AdvertisingSet, ConnectionEvent,
    ConnectionSetup, LeCapabilities, LeInstant, ScanWindow, ScannerSetup, Supported, TestWindow,
    dtm::DtmTest,
    rights::{
        AcceptList, AdvEvent, AdvPlace, Advertiser, ConnEvent, ConnPlace, Connection, EventPlace,
        PduHandle, Retired, RoleRef, RoleRights, Running, ScanEvent, ScanPlace, Scanner, Starting,
        StopFrom, StopRight, Stopped, Stopping, Test, TestEvent, TestPlace, TxPlace,
    },
};

/// Why a start did not make the radio run. The port took down what it
/// began before it answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartFailure {
    /// The PHY's step failed.
    Phy,
    /// The shared radio's owner refused what the start asked of it.
    SharedOwner,
    /// The start did not end within its bound.
    Bound,
}

/// A start's poll.
#[must_use = "a poll's answer holds the radio's right"]
#[derive(Debug)]
pub enum StartPoll<'id> {
    /// Still in progress; the port's facts wake.
    Pending(Starting<'id>),
    /// The radio runs.
    Running(Running<'id>),
    /// The start failed and the radio is stopped.
    Stopped(Stopped<'id>, StartFailure),
}

/// A stop's poll. A stop always ends.
#[must_use = "a poll's answer holds the radio's right"]
#[derive(Debug)]
pub enum StopPoll<'id> {
    /// Still in progress; the port's facts wake.
    Pending(Stopping<'id>),
    /// The radio is stopped.
    Stopped(Stopped<'id>),
}

/// What a publication answers: nothing. The event's end comes in a round,
/// whatever it is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Published;

/// Whether one signal of the port came in a poll: one fact of the poll.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Signal {
    /// It did not.
    #[default]
    Quiet,
    /// It did.
    Raised,
}

/// What the port's signals held when its caller polled them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Facts {
    /// The hardware finished something a round reads.
    pub finished: Signal,
    /// The alarm passed.
    pub alarm: Signal,
    /// The port needs a stop: one indication with no payload, latched
    /// until the stop that follows.
    pub needs_stop: Signal,
}

/// Whether an executed event's exchange ran to its end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cut {
    /// It did.
    Whole,
    /// The event was cut with its exchange incomplete.
    Cut,
}

/// The More Data bit of the peer's last data channel PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoreData {
    /// The peer had nothing more.
    Done,
    /// The peer had more to send.
    More,
}

/// What is known of an event that ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Executed {
    /// The captured anchor, when the hardware has one.
    pub anchor: Option<LeInstant>,
    /// The packets exchanged: for a test's event, the packets sent, or
    /// received with a good CRC, in its window.
    pub exchanged: u8,
    /// Whether it was cut with its exchange incomplete.
    pub cut: Cut,
    /// The More Data bit of the peer's last data channel PDU; absent when
    /// the event received none.
    pub peer: Option<MoreData>,
}

/// Why an event did not run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotExecuted {
    /// Published later than its start less the backend's guard: nothing
    /// was listed.
    Late,
    /// Given up by a withdraw or by a stop before it ran.
    Withdrawn,
    /// The hardware did not run it.
    Displaced,
}

/// How an event ended: one sum, whatever the role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventEnd {
    /// The event ran.
    Executed(Executed),
    /// The event did not run.
    NotExecuted(NotExecuted),
    /// The scheduler was stopped while it may have been running.
    Indeterminate,
}

/// How a PDU of a connection's chain ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxEnd {
    /// The peer acknowledged it.
    Transmitted,
    /// It was given up by a withdraw or by a stop.
    Withdrawn,
}

/// What comes after a round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rest {
    /// Nothing until the port's signal.
    Quiet,
    /// Another round: more was finished meanwhile.
    Round,
    /// Nothing until this instant.
    At(LeInstant),
}

/// A received PDU, read in place in its node. Only a PDU whose reception
/// was good crosses; one with an error of its own is dropped and counted
/// below the port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Received<'a> {
    /// The two-octet header.
    pub header: [u8; 2],
    /// The payload after the header.
    pub payload: &'a [u8],
    /// Receive strength.
    pub rssi_dbm: i8,
    /// The on-air start, when the hardware captured one.
    pub captured_at: Option<LeInstant>,
}

/// A data channel PDU's payload length, at most what the capabilities `K`
/// carry.
pub struct PduLen<K: LeCapabilities> {
    octets: u8,
    caps: PhantomData<fn() -> K>,
}

/// A payload longer than the capabilities carry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PduTooLong {
    /// The length asked.
    pub octets: usize,
    /// The longest the capabilities carry.
    pub max: u8,
}

impl<K: LeCapabilities> PduLen<K> {
    /// The longest payload `K` carries.
    pub const MAX: u8 = match K::CAPS.peripheral_connection {
        Some(connection) => connection.max_data_payload,
        None => panic!("a data channel PDU needs capabilities that serve connections"),
    };

    /// `octets`, when `K` carries a payload that long.
    ///
    /// # Errors
    ///
    /// The length is longer than the capabilities carry.
    pub const fn new(octets: usize) -> Result<Self, PduTooLong> {
        let max = Self::MAX;
        if octets > max as usize {
            return Err(PduTooLong { octets, max });
        }
        Ok(Self {
            // At most `max`: one octet.
            octets: octets as u8,
            caps: PhantomData,
        })
    }

    /// The length.
    pub const fn get(&self) -> u8 {
        self.octets
    }
}

/// The seal of a round's parts: only a [`Round`] has one, so a round's
/// reads and releases are written only inside a round's borrow.
#[derive(Debug)]
pub struct RoundSeal(());

/// The parts of a round a port gives, called only by [`Round`], [`Ended`]
/// and [`Pdu`] with their seal.
pub trait LeRounds<'id> {
    /// What the port names one reception of a round by, while it is read.
    type Node;

    /// Sample the round's instant, at its open.
    fn round_open(&mut self, seal: &RoundSeal) -> LeInstant;
    /// The next event that ended, in the order they ended, with its place.
    fn round_next(&mut self, seal: &RoundSeal) -> Option<(EventPlace<'id>, EventEnd)>;
    /// The next reception of the event `round_next` gave last, in the air's
    /// order.
    fn round_received(&mut self, seal: &RoundSeal) -> Option<Self::Node>;
    /// A reception of this round, in place.
    fn round_view(&self, seal: &RoundSeal, node: &Self::Node) -> Received<'_>;
    /// Hold a reception of this round: its node stays out of the hardware
    /// until its release.
    fn round_take(&mut self, seal: &RoundSeal, node: Self::Node) -> PduHandle<'id>;
    /// A held PDU, in place.
    fn round_held(&self, seal: &RoundSeal, handle: &PduHandle<'id>) -> Received<'_>;
    /// Release a held PDU: its node goes back to the hardware at the close.
    fn round_release(&mut self, seal: &RoundSeal, handle: PduHandle<'id>);
    /// The next PDU of a chain that ended, with its place.
    fn round_transmitted(&mut self, seal: &RoundSeal) -> Option<(TxEnd, TxPlace<'id>)>;
    /// Close the round: what was not taken goes back to the hardware.
    fn round_close(&mut self, seal: &RoundSeal) -> Rest;
}

/// Everything the cores read of what the hardware finished, inside one
/// borrow of the port.
#[must_use = "a round is closed to give back what it did not take"]
pub struct Round<'r, 'id, P: LeRounds<'id>> {
    port: &'r mut P,
    seal: RoundSeal,
    now: LeInstant,
    brand: PhantomData<fn(&'id ()) -> &'id ()>,
}

impl<'r, 'id, P: LeRounds<'id>> Round<'r, 'id, P> {
    /// Open a round of `port`: what a port's [`LePort::round`] answers. The
    /// port's geometry is checked against its capabilities here, at build.
    pub fn open<
        const ADV: usize,
        const SCAN: usize,
        const CONN: usize,
        const EVENTS: usize,
        const TX: usize,
        const RX: usize,
        const TEST: usize,
    >(
        port: &'r mut P,
    ) -> Self
    where
        P: LePort<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>,
    {
        const {
            crate::geometry::Geometry::<
                <P as LePort<'id, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>>::Caps,
                ADV,
                SCAN,
                CONN,
                EVENTS,
                TX,
                RX,
                TEST,
            >::CHECKED
        };
        let seal = RoundSeal(());
        let now = port.round_open(&seal);
        Self {
            port,
            seal,
            now,
            brand: PhantomData,
        }
    }

    /// The round's instant, sampled once at its open.
    pub const fn now(&self) -> LeInstant {
        self.now
    }

    /// The next event that ended, in the order they ended.
    pub fn ended(&mut self) -> Option<Ended<'_, 'id, P>> {
        let (place, end) = self.port.round_next(&self.seal)?;
        Some(Ended {
            port: &mut *self.port,
            seal: &self.seal,
            place,
            end,
        })
    }

    /// The next PDU of a chain that ended: `Transmitted` or `Withdrawn`,
    /// with its place, which names its connection.
    pub fn transmitted(&mut self) -> Option<(TxEnd, TxPlace<'id>)> {
        self.port.round_transmitted(&self.seal)
    }

    /// A held PDU, in place.
    pub fn pdu(&self, handle: &PduHandle<'id>) -> Received<'_> {
        self.port.round_held(&self.seal, handle)
    }

    /// Release a held PDU: its node goes back to the hardware at the close.
    pub fn release(&mut self, handle: PduHandle<'id>) {
        self.port.round_release(&self.seal, handle);
    }

    /// Close the round: what comes next.
    pub fn close(self) -> Rest {
        self.port.round_close(&self.seal)
    }
}

/// One event that ended, read inside its round.
#[must_use = "an ended event gives its place back"]
pub struct Ended<'a, 'id, P: LeRounds<'id>> {
    port: &'a mut P,
    seal: &'a RoundSeal,
    place: EventPlace<'id>,
    end: EventEnd,
}

impl<'a, 'id, P: LeRounds<'id>> Ended<'a, 'id, P> {
    /// How it ended.
    pub const fn end(&self) -> EventEnd {
        self.end
    }

    /// Its receptions, in the air's order.
    pub fn received(&mut self) -> Option<Pdu<'_, 'id, P>> {
        let node = self.port.round_received(self.seal)?;
        Some(Pdu {
            port: &mut *self.port,
            seal: self.seal,
            node,
        })
    }

    /// Its place back, typed by its role; its receptions not read go back
    /// at the round's close.
    pub fn place(self) -> EventPlace<'id> {
        self.place
    }
}

/// One reception of an ended event, read in place.
#[must_use = "a reception not taken goes back at the round's close"]
pub struct Pdu<'a, 'id, P: LeRounds<'id>> {
    port: &'a mut P,
    seal: &'a RoundSeal,
    node: P::Node,
}

impl<'id, P: LeRounds<'id>> Pdu<'_, 'id, P> {
    /// The PDU, in place.
    pub fn view(&self) -> Received<'_> {
        self.port.round_view(self.seal, &self.node)
    }

    /// Hold it: its node stays out of the hardware until its release.
    pub fn take(self) -> PduHandle<'id> {
        self.port.round_take(self.seal, self.node)
    }
}

/// A chip's Bluetooth LE port, of the geometry `ADV`, `SCAN`, `CONN`,
/// `EVENTS`, `TX`, `RX`, `TEST` (`design/ble-port-contract-2026-11-19.md`,
/// section 2).
///
/// A call that needs a state takes its right; no call answers an error.
pub trait LePort<
    'id,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
>: LeRounds<'id> + Sized
{
    /// The port's capabilities.
    type Caps: LeCapabilities;
    /// What a release gives back: the hardware the installation held.
    type Released<'a>
    where
        Self: 'a;

    /// Begin a start of the stopped radio.
    fn start(&mut self, stopped: Stopped<'id>) -> Starting<'id>;
    /// Poll a start.
    fn poll_start(&mut self, starting: Starting<'id>, cx: &mut Context<'_>) -> StartPoll<'id>;
    /// Begin the radio's stop from a running radio or a start.
    fn stop(&mut self, right: &mut StopRight<'id>, from: StopFrom<'id>) -> Stopping<'id>;
    /// Poll a stop. It always ends in [`Stopped`].
    fn poll_stop(&mut self, stopping: Stopping<'id>, cx: &mut Context<'_>) -> StopPoll<'id>;
    /// Retire the stopped installation with every role's place home. It
    /// cannot refuse.
    fn retire(
        &mut self,
        stopped: Stopped<'id>,
        roles: RoleRights<'id, ADV, SCAN, CONN, TEST>,
    ) -> Retired<'id>;
    /// Release the retired installation, with the stop's right.
    fn release(&mut self, retired: Retired<'id>, right: StopRight<'id>) -> Self::Released<'_>;

    /// Open an advertising set in its place.
    fn open_advertising(
        &mut self,
        place: AdvPlace<'id>,
        set: &Supported<Self::Caps, AdvertisingSet<'_>>,
    ) -> (Advertiser<'id>, AdvEvent<'id>);
    /// Write an advertising set's data between its events.
    fn rewrite_advertising(
        &mut self,
        role: &mut Advertiser<'id>,
        home: &mut AdvEvent<'id>,
        data: &Supported<Self::Caps, AdvertisingData<'_>>,
    );
    /// Close an advertising set, with its event's place home.
    fn close_advertising(&mut self, role: Advertiser<'id>, home: AdvEvent<'id>) -> AdvPlace<'id>;

    /// Open the scanner in its place.
    fn open_scanner(
        &mut self,
        place: ScanPlace<'id>,
        scan: &Supported<Self::Caps, ScannerSetup>,
    ) -> (Scanner<'id>, ScanEvent<'id>);
    /// Close the scanner, with its event's place home.
    fn close_scanner(&mut self, role: Scanner<'id>, home: ScanEvent<'id>) -> ScanPlace<'id>;

    /// Open a connection in its place.
    fn open_connection(
        &mut self,
        place: ConnPlace<'id>,
        link: &Supported<Self::Caps, ConnectionSetup>,
    ) -> (
        Connection<'id>,
        [ConnEvent<'id>; EVENTS],
        [TxPlace<'id>; TX],
    );
    /// Close a connection, with every place of it home.
    fn close_connection(
        &mut self,
        role: Connection<'id>,
        events: [ConnEvent<'id>; EVENTS],
        tx: [TxPlace<'id>; TX],
    ) -> ConnPlace<'id>;

    /// Open the test session in its place.
    fn open_test(
        &mut self,
        place: TestPlace<'id>,
        test: &Supported<Self::Caps, DtmTest>,
    ) -> (Test<'id>, TestEvent<'id>);
    /// Close the test session, with its event's place home.
    fn close_test(&mut self, role: Test<'id>, home: TestEvent<'id>) -> TestPlace<'id>;

    /// Write the whole filter accept list.
    fn write_accept_list(
        &mut self,
        list: &mut AcceptList<'id>,
        entries: &Supported<Self::Caps, AcceptEntries<'_>>,
    );

    /// Publish an advertising set's event. Its place comes back with its
    /// end in a round.
    fn publish_advertising(
        &mut self,
        place: AdvEvent<'id>,
        event: &Supported<Self::Caps, AdvertisingEvent<Self::Caps>>,
    ) -> Published;
    /// Publish the scanner's window.
    fn publish_scan(
        &mut self,
        place: ScanEvent<'id>,
        window: &Supported<Self::Caps, ScanWindow>,
    ) -> Published;
    /// Publish a connection's event.
    fn publish_connection(
        &mut self,
        place: ConnEvent<'id>,
        event: &Supported<Self::Caps, ConnectionEvent>,
    ) -> Published;
    /// Publish the test session's window.
    fn publish_test(
        &mut self,
        place: TestEvent<'id>,
        window: &Supported<Self::Caps, TestWindow>,
    ) -> Published;
    /// Ask the hardware to give up every event of the role that is out, and
    /// a connection's chain. Each ends in a round as whatever happened.
    fn withdraw(&mut self, role: RoleRef<'_, 'id>);

    /// Write a PDU into its transmit place: `fill` writes the payload and
    /// answers its length.
    fn write_pdu<F: FnOnce(&mut [u8]) -> PduLen<Self::Caps>>(
        &mut self,
        place: &mut TxPlace<'id>,
        llid: DataPduKind,
        fill: F,
    );
    /// Append the PDU of its place to its connection's chain.
    fn submit(&mut self, place: TxPlace<'id>);

    /// Open a round.
    fn round(&mut self) -> Round<'_, 'id, Self> {
        Round::open(self)
    }

    /// Arm the port's one alarm, or disarm it.
    fn alarm(&mut self, at: Option<LeInstant>);
    /// Poll the port's signals.
    fn poll_facts(&mut self, cx: &mut Context<'_>) -> Facts;
}

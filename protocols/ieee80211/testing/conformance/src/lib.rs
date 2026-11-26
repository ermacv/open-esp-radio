#![expect(
    clippy::cast_possible_truncation,
    reason = "the port type and the conformance suite"
)]
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

//! The conformance suite of the IEEE 802.11 port's contract: the eight
//! properties of `design/port-contract-2026-11-16.md`, section 6, as checks
//! over any port.
//!
//! A port's test implements [`Harness`] (the port, the hardware's levers
//! a test pulls, and what the hardware programmed and holds) and runs each
//! property with its installation's rights. The reference model's tests
//! run them over `oer-protocols-ieee80211-testing-model`; a chip's port runs
//! the same checks over its hardware model.

extern crate std;

use core::task::{Context, Poll, Waker};
use oer_protocols_ieee802_wire::{Eui48, IndividualAddress};
use std::vec::Vec;

use core::num::NonZeroU8;
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    AckHeader, HeaderSpec, Protection, RetryPlan, RetryStage, SizedFrame, TxFrame,
};
use oer_protocols_ieee80211_contracts_port::{
    AmpduRate, ArenaOutcome, CloseGate, CommandPlace, CommandPoll, DataPort, DataRights, FrameSet,
    GateClosed, GateOpen, GroupCommand, GroupEnd, HEADROOM, HeaderLen, IdleArena, InCommand,
    MpduLength, PairwiseCommand, PairwiseEnd, PhyRate, PortRate, PortSink, Ppdu, QueueRight,
    RadioPort, ReceivePort, Round, Settled, SingleOutcome, SinglePlace, StartPoll, StopFrom,
    StopPoll, StopRight, Stopped, TbttEvent, TxFacts, TxOutcome, ValueCommand, ValueEnd,
};
use oer_protocols_ieee80211_wire::frame::MacHeader;
use oer_protocols_ieee80211_wire::phy::{
    HtGuardInterval, HtMcs, HtRate, LegacyRate, PpduBandwidth,
};

/// The instant of every call of the suite.
pub const NOW: Instant = Instant::from_micros(0);

/// A port under the suite, with the levers of its hardware.
pub trait Harness<
    'id,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const DATA: usize,
>
{
    /// The port.
    type Port: ReceivePort<'id>
        + DataPort<'id, Data = DataRights<'id, SINGLES, GROUPS, AGREEMENTS, DATA>>;
    /// What the hardware was asked to program in a run.
    type Record: Eq + core::fmt::Debug;
    /// What the hardware holds that a stop takes down.
    type State: Eq + core::fmt::Debug;

    /// The port.
    fn port(&mut self) -> &mut Self::Port;
    /// The channel a run starts on.
    fn channel(&self) -> <Self::Port as RadioPort<'id>>::Channel;
    /// The network's memory of an MSDU body of `len` octets.
    fn memory(&mut self, len: u16) -> <Self::Port as DataPort<'id>>::Memory;
    /// A link of the station.
    fn link(&self) -> <Self::Port as DataPort<'id>>::Link;
    /// Land a received MPDU on the ring.
    fn land(&mut self, bytes: &[u8]);
    /// The anomalies the backend may indicate.
    fn anomalies(&self) -> usize;
    /// The hardware sees the anomaly `index`.
    fn raise(&mut self, index: usize);
    /// Keep the MAC busy: the next stop's last resort runs.
    fn hold_busy(&mut self);
    /// Keep the next frames on the air until [`Self::release_air`].
    fn hold_air(&mut self);
    /// Let the air end what it holds.
    fn release_air(&mut self);
    /// What the hardware was asked to program since the last start.
    fn record(&self) -> Self::Record;
    /// What the hardware holds.
    fn state(&self) -> Self::State;
    /// The port's own octets of its transmit slots (in front of the
    /// headroom, behind a body and its MIC's room), as the port checked
    /// them after each write and at each publication.
    fn port_octets(&self) -> PortOctets;
}

/// The port's own octets of its transmit slots, as the port finds them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortOctets {
    /// As the port wrote them, at every check.
    Whole,
    /// Written by someone else at a check: the engine or the network wrote
    /// outside the headroom and the body.
    Overwritten,
}

/// The rights the suite runs an installation with.
pub struct Rights<
    'id,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
    const DATA: usize,
> {
    /// The radio is stopped.
    pub stopped: Option<Stopped<'id>>,
    /// The stop's right.
    pub right: StopRight<'id>,
    /// The data's rights, while the data is stopped.
    pub data: Option<DataRights<'id, SINGLES, GROUPS, AGREEMENTS, DATA>>,
    /// The gate, closed.
    pub gate: Option<GateClosed<'id>>,
}

/// A sink that keeps the singles' ends.
struct Ends<'id> {
    singles: Vec<(SingleOutcome<(), ()>, SinglePlace<'id>, QueueRight<'id>)>,
}

impl<'id> PortSink<'id, (), ()> for Ends<'id> {
    fn single_ended(
        &mut self,
        outcome: SingleOutcome<(), ()>,
        place: SinglePlace<'id>,
        queue: QueueRight<'id>,
    ) {
        self.singles.push((outcome, place, queue));
    }

    fn tbtt(&mut self, _event: TbttEvent) {}
}

fn cx() -> Context<'static> {
    Context::from_waker(Waker::noop())
}

/// A QoS Data MPDU header of 26 octets.
/// A QoS Data header from the access point to the station.
const QOS_DATA: [u8; 26] = [
    0x88, 0x01, 0, 0, 2, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 2, 0, 0, 0, 0,
];

fn header(headroom: &mut [u8; HEADROOM]) -> HeaderLen {
    // Flush against the body.
    headroom[HEADROOM - QOS_DATA.len()..].copy_from_slice(&QOS_DATA);
    HeaderLen::new(QOS_DATA.len()).unwrap()
}

/// A received QoS Data MPDU with a body of `len` octets.
pub fn data_mpdu(len: usize) -> Vec<u8> {
    let mut bytes = std::vec![0_u8; QOS_DATA.len() + len];
    bytes[..QOS_DATA.len()].copy_from_slice(&QOS_DATA);
    bytes
}

/// Start the stopped radio and confirm it.
fn run<'id, H, const S: usize, const G: usize, const A: usize, const D: usize>(
    harness: &mut H,
    rights: &mut Rights<'id, S, G, A, D>,
) -> oer_protocols_ieee80211_contracts_port::Running<'id>
where
    H: Harness<'id, S, G, A, D>,
{
    let channel = harness.channel();
    let stopped = rights.stopped.take().expect("the radio is stopped");
    let starting = harness.port().start(stopped, channel, NOW);
    match harness.port().poll_start(starting, &mut cx(), NOW) {
        StartPoll::Running(running) => running,
        StartPoll::Pending(_) | StartPoll::Stopped(..) => panic!("the start runs the radio"),
    }
}

/// Stop the running radio: it always ends in `Stopped`.
fn stop<'id, H, const S: usize, const G: usize, const A: usize, const D: usize>(
    harness: &mut H,
    rights: &mut Rights<'id, S, G, A, D>,
    running: oer_protocols_ieee80211_contracts_port::Running<'id>,
) where
    H: Harness<'id, S, G, A, D>,
{
    let mut stopping = harness
        .port()
        .stop(&mut rights.right, StopFrom::Running(running));
    let stopped = loop {
        match harness.port().poll_stop(stopping, &mut cx(), NOW) {
            StopPoll::Stopped(stopped) => break stopped,
            StopPoll::Pending(again) => stopping = again,
        }
    };
    rights.stopped = Some(stopped);
}

/// Run a command of one kind to its end: its poll ends only with that
/// kind's end, and gives the place back.
fn run_command<'id, H, C, K, E, const S: usize, const G: usize, const A: usize, const D: usize>(
    harness: &mut H,
    place: CommandPlace<'id>,
    command: C,
    begin: impl Fn(&mut H::Port, CommandPlace<'id>, C) -> InCommand<'id, K>,
    poll: impl Fn(&mut H::Port, InCommand<'id, K>, &mut Ends<'id>) -> CommandPoll<'id, K, E>,
) -> (CommandPlace<'id>, E)
where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let mut flight = begin(harness.port(), place, command);
    let mut ends = Ends {
        singles: Vec::new(),
    };
    for _ in 0..16 {
        match poll(harness.port(), flight, &mut ends) {
            CommandPoll::Ended(place, end) => return (place, end),
            CommandPoll::Pending(again) => flight = again,
        }
        harness.port().serve(
            TxFacts::DUE,
            oer_protocols_ieee80211_contracts_port::Signal::Quiet,
            NOW,
            &mut ends,
        );
    }
    panic!("a command ends within its bound");
}

/// Close the gate.
fn close_gate<'id, H, const S: usize, const G: usize, const A: usize, const D: usize>(
    harness: &mut H,
    place: CommandPlace<'id>,
    open: GateOpen<'id>,
) -> (CommandPlace<'id>, GateClosed<'id>)
where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    run_command(
        harness,
        place,
        CloseGate(open),
        |port, place, command| port.begin_close(place, command, NOW),
        |port, flight, ends| port.poll_close(flight, NOW, ends),
    )
}

/// A pairwise key command.
fn pairwise_command<'id, H, const S: usize, const G: usize, const A: usize, const D: usize>(
    harness: &mut H,
    place: CommandPlace<'id>,
    command: PairwiseCommand<'id>,
) -> (CommandPlace<'id>, PairwiseEnd<'id>)
where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    run_command(
        harness,
        place,
        command,
        |port, place, command| port.begin_pairwise(place, command, NOW),
        |port, flight, ends| port.poll_pairwise(flight, NOW, ends),
    )
}

/// A group key command.
fn group_command<'id, H, const S: usize, const G: usize, const A: usize, const D: usize>(
    harness: &mut H,
    place: CommandPlace<'id>,
    command: GroupCommand<'id>,
) -> (CommandPlace<'id>, GroupEnd<'id>)
where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    run_command(
        harness,
        place,
        command,
        |port, place, command| port.begin_group(place, command, NOW),
        |port, flight, ends| port.poll_group(flight, NOW, ends),
    )
}

/// A command of values.
fn value_command<'id, H, const S: usize, const G: usize, const A: usize, const D: usize>(
    harness: &mut H,
    place: CommandPlace<'id>,
    command: ValueCommand<<H::Port as RadioPort<'id>>::Caps>,
) -> (CommandPlace<'id>, ValueEnd)
where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    run_command(
        harness,
        place,
        command,
        |port, place, command| port.begin_value(place, command, NOW),
        |port, flight, ends| port.poll_value(flight, NOW, ends),
    )
}

/// The data path's settings over the port, taken down by the data's stop
/// with their own commands: a pairwise key and a group key installed,
/// then removed, the gate opened, then closed.
fn data_run<'id, H, const S: usize, const G: usize, const A: usize, const D: usize>(
    harness: &mut H,
    rights: &mut Rights<'id, S, G, A, D>,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let gate = rights.gate.take().expect("the gate is closed");
    let open: GateOpen<'id> = harness.port().open_gate(gate);
    let DataRights {
        arenas,
        singles,
        data,
        queues,
        command: place,
        pairwise,
        groups,
        agreements,
    } = rights.data.take().expect("the data's rights are home");
    let peer =
        oer_protocols_ieee802_wire::IndividualAddress::new(Eui48::new([2, 0, 0, 0, 0, 2])).unwrap();
    let (place, end) = pairwise_command(
        harness,
        place,
        PairwiseCommand::Install {
            slot: pairwise,
            peer,
            key: oer_protocols_ieee80211_contracts_security::Ccmp128Key::new(&[1; 16]),
            spp: oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
        },
    );
    let PairwiseEnd::Installed(installed) = end else {
        panic!("the pairwise key installs");
    };
    let (place, end) = pairwise_command(harness, place, PairwiseCommand::Remove(installed));
    let PairwiseEnd::Empty(pairwise) = end else {
        panic!("the pairwise key is removed");
    };
    // A group key's install and removal give its slot back.
    let mut groups = Vec::from(groups);
    let first = groups.remove(0);
    let (place, end) = group_command(
        harness,
        place,
        GroupCommand::Install {
            slot: first,
            key_id: oer_protocols_ieee80211_wire::ccmp::GroupKeyId::new(1).unwrap(),
            key: oer_protocols_ieee80211_contracts_security::Ccmp128Key::new(&[2; 16]),
        },
    );
    let GroupEnd::Installed(installed) = end else {
        panic!("the group key installs");
    };
    let (place, end) = group_command(harness, place, GroupCommand::Remove(installed));
    let GroupEnd::Empty(first) = end else {
        panic!("the group key is removed");
    };
    groups.insert(0, first);
    let groups = home(groups, "every group slot is back");
    // A command of values ends done.
    let (place, end) = value_command(
        harness,
        place,
        ValueCommand::SetBssColor(oer_protocols_ieee80211_wire::he::BssColor::ZERO),
    );
    assert_eq!(end, ValueEnd::Done, "a command of values takes effect");
    let (command, closed) = close_gate(harness, place, open);
    rights.gate = Some(closed);
    rights.data = Some(DataRights {
        arenas,
        singles,
        data,
        queues,
        command,
        pairwise,
        groups,
        agreements,
    });
}

/// **1. Every right is in exactly one place**: a run, its data path and its
/// stop hand every right back to the suite's places, and a retirement takes
/// them whole.
pub fn every_right_is_in_exactly_one_place<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, S, G, A, D>,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    data_run(harness, &mut rights);
    stop(harness, &mut rights, running);
    retire_whole(harness, rights);
}

/// The rights of `rights` back in their array, all of them.
fn home<T, const N: usize>(rights: Vec<T>, what: &str) -> [T; N] {
    match rights.try_into() {
        Ok(home) => home,
        Err(_) => panic!("{what}"),
    }
}

/// Retire and release a stopped installation: every right is home, the gate
/// closed.
fn retire_whole<'id, H, const S: usize, const G: usize, const A: usize, const D: usize>(
    harness: &mut H,
    rights: Rights<'id, S, G, A, D>,
) where
    H: Harness<'id, S, G, A, D>,
{
    let units = match harness.port().poll_units_home(&mut cx()) {
        Poll::Ready(units) => units,
        Poll::Pending => panic!("no unit is out"),
    };
    let Rights {
        stopped,
        right,
        data,
        gate,
    } = rights;
    assert!(gate.is_some(), "the gate is closed at the stop's end");
    let retired = harness.port().retire(
        stopped.expect("the radio is stopped"),
        data.expect("every data right is home"),
        units,
    );
    let _released = harness.port().release(retired, right);
}

/// **3. A second run is as the first**: what the hardware programs in a run
/// after a stop equals what it programmed in the first, and its state after
/// each stop equals its state before the first start; also after a run in
/// which each anomaly was raised, and after a stop whose last resort ran.
pub fn a_second_run_is_as_the_first<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, S, G, A, D>,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    // The gate is closed at each start and opened only on a running radio;
    // the data's stop takes down the keys and closes the gate with their
    // own commands, so each run's data path is part of what is compared.
    let before = harness.state();
    let running = run(harness, &mut rights);
    data_run(harness, &mut rights);
    let first = harness.record();
    stop(harness, &mut rights, running);
    assert_eq!(
        harness.state(),
        before,
        "a stop leaves the state before its start"
    );
    for anomaly in 0..harness.anomalies() {
        let running = run(harness, &mut rights);
        data_run(harness, &mut rights);
        assert_eq!(harness.record(), first, "each run programs as the first");
        harness.raise(anomaly);
        stop(harness, &mut rights, running);
        assert_eq!(
            harness.state(),
            before,
            "the indication is clear after the stop of anomaly {anomaly}"
        );
    }
    let running = run(harness, &mut rights);
    data_run(harness, &mut rights);
    harness.hold_busy();
    stop(harness, &mut rights, running);
    assert_eq!(
        harness.state(),
        before,
        "the last resort leaves the state before the start"
    );
    for _ in 0..8 {
        let running = run(harness, &mut rights);
        data_run(harness, &mut rights);
        assert_eq!(
            harness.record(),
            first,
            "many runs in a row are as the first"
        );
        stop(harness, &mut rights, running);
        assert_eq!(harness.state(), before);
    }
}

/// An individual Probe Request of 24 octets at 6 Mb/s, for a single.
fn single_frame<'id, P: DataPort<'id>>(port: &P) -> (TxFrame<'id, P::Caps>, MpduLength<P::Caps>) {
    // A Probe Request: management, subtype 4, to an individual receiver.
    const PROBE_REQUEST: [u8; 24] = [
        0x40, 0, 0, 0, 2, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 1, 2, 0, 0, 0, 0, 2, 0, 0,
    ];
    let header = MacHeader::decode(&PROBE_REQUEST)
        .and_then(|header| HeaderSpec::from_mac_header(header, IndividualAddress::new))
        .and_then(AckHeader::new)
        .expect("a Probe Request's header");
    let length = MpduLength::new(24).expect("an MPDU's length");
    let rate = port
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .expect("the port sends 6 Mb/s");
    let plan = RetryPlan::new(RetryStage {
        rate,
        transmissions: NonZeroU8::MIN,
        protection: Protection::None,
    });
    (
        TxFrame::Individual {
            frame: SizedFrame::new(header, length).expect("a sized frame"),
            plan,
        },
        length,
    )
}

/// **4. A stop ends** in `Stopped` with everything that was on the air
/// ended through the sink, and every single's place and queue back.
pub fn a_stop_ends_everything_on_the_air<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, S, G, A, D>,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    let gate = rights.gate.take().expect("the gate is closed");
    let open = harness.port().open_gate(gate);
    let data = rights.data.take().expect("the data's rights are home");
    let mut singles: Vec<_> = data.singles.into_iter().collect();
    let mut queues: Vec<_> = data.queues.into_iter().collect();
    let mut place = singles.remove(0);
    let queue = queues.remove(0);
    let (frame, length) = single_frame(harness.port());
    harness.port().write_single(&mut place, |out| {
        out[..length.get()].fill(0);
        length
    });
    harness.hold_air();
    harness
        .port()
        .publish_single(place, queue, &open, &frame, NOW);
    stop(harness, &mut rights, running);
    let mut ends = Ends {
        singles: Vec::new(),
    };
    harness.port().serve(
        TxFacts::DUE,
        oer_protocols_ieee80211_contracts_port::Signal::Quiet,
        NOW,
        &mut ends,
    );
    assert_eq!(
        ends.singles.len(),
        1,
        "the single on the air ends through the sink"
    );
    let (outcome, place, queue) = ends.singles.pop().unwrap();
    assert!(
        matches!(outcome, TxOutcome::Withdrawn(_)),
        "a stop's end of a single is a withdrawal: {outcome:?}"
    );
    harness.release_air();
    singles.insert(0, place);
    queues.insert(0, queue);
    let (command, closed) = close_gate(harness, data.command, open);
    rights.gate = Some(closed);
    rights.data = Some(DataRights {
        arenas: data.arenas,
        singles: home(singles, "every place is back"),
        data: data.data,
        queues: home(queues, "every queue's right is back"),
        command,
        pairwise: data.pairwise,
        groups: data.groups,
        agreements: data.agreements,
    });
    retire_whole(harness, rights);
}

/// **5. The indication**: an anomaly raises "the port needs a stop" in the
/// port's facts, with no payload, until the stop that follows.
pub fn the_indication_holds_until_the_stop<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, S, G, A, D>,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    for anomaly in 0..harness.anomalies() {
        let running = run(harness, &mut rights);
        assert!(!harness.port().poll_facts(&mut cx()).needs_stop.is_raised());
        harness.raise(anomaly);
        assert!(harness.port().poll_facts(&mut cx()).needs_stop.is_raised());
        assert!(
            harness.port().poll_facts(&mut cx()).needs_stop.is_raised(),
            "the indication is latched"
        );
        stop(harness, &mut rights, running);
        assert!(!harness.port().poll_facts(&mut cx()).needs_stop.is_raised());
    }
}

/// **6. A unit is in one of** the ring, a staging place (staged, taken or
/// lent), gone: a round hands what the ring held, a unit not taken goes
/// back, a lent one comes home when the network drops it.
pub fn a_unit_is_in_one_place<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, S, G, A, D>,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    harness.land(&data_mpdu(40));
    harness.land(&data_mpdu(40));
    let mut round = Round::open(harness.port(), NOW);
    let first = round.staged().expect("the ring held two units");
    let handle = first.take();
    let Some(_second) = round.staged() else {
        panic!("the ring held two units");
    };
    assert!(round.staged().is_none());
    let lendable = round
        .unit(&handle)
        .unwrap()
        .lendable()
        .expect("a unit can be lent");
    let lent = round.lend(handle, lendable);
    let _ = round.close();
    assert!(
        harness.port().poll_units_home(&mut cx()).is_pending(),
        "a lent unit keeps the network's units out"
    );
    drop(lent);
    let round = Round::open(harness.port(), NOW);
    assert_eq!(round.reclaimed(), 1, "the lent unit came home");
    let _ = round.close();
    assert!(harness.port().poll_units_home(&mut cx()).is_ready());
    stop(harness, &mut rights, running);
}

/// **7. An aggregate's prefix**: a publication at a rate that admits less
/// than the arena holds sends the longest prefix, one frame at least, and
/// What one publication of an arena of `frames` frames did: the frames
/// that went and whether the outcome says one MPDU went.
struct Publication {
    sent: FrameSet,
    single: bool,
}

/// Fill an arena with `frames` frames of `body` octets, publish it as
/// `how` makes it, end it and settle it keeping nothing; the frames that
/// did not go stay in place with no attempt spent.
fn publication<'id, H, const S: usize, const G: usize, const A: usize, const D: usize>(
    harness: &mut H,
    mut rights: Rights<'id, S, G, A, D>,
    length: MpduLength<<H::Port as RadioPort<'id>>::Caps>,
    body: u16,
    frames: u8,
    how: impl FnOnce(&H::Port) -> Ppdu<'id, <H::Port as RadioPort<'id>>::Caps>,
) -> Publication
where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    let gate = rights.gate.take().expect("the gate is closed");
    let open = harness.port().open_gate(gate);
    let mut data = rights.data.take().expect("the data's rights are home");
    let [idle, other] = data.arenas;
    let link = harness.link();
    let tid = oer_protocols_ieee80211_wire::qos::Tid::constant::<0>();
    let mut arena = harness.port().open(idle, &link, tid);
    for _ in 0..frames {
        let memory = harness.memory(body);
        let place = harness
            .port()
            .place(&mut arena, length)
            .expect("the arena has slots");
        place.write(memory, header);
    }
    assert_eq!(
        harness.port_octets(),
        PortOctets::Whole,
        "the writes leave the port's octets in front of the headroom and behind the body whole"
    );
    let how = how(harness.port());
    let on_air = harness.port().publish(arena, how, &open, NOW);
    assert_eq!(
        harness.port_octets(),
        PortOctets::Whole,
        "the port's octets are whole at the publication"
    );
    let sent = on_air.sent();
    assert!(!sent.is_empty(), "one frame goes at least");
    assert_eq!(
        sent,
        FrameSet::prefix(sent.len() as u8),
        "the sent frames are a prefix"
    );
    let ended = harness
        .port()
        .aggregate_ended(TxFacts::DUE, &on_air, NOW)
        .expect("the arena ended");
    assert_eq!(ended.sent, sent);
    let single = matches!(ended.outcome, ArenaOutcome::Single(_));
    let mut home = 0;
    let settled = harness
        .port()
        .settle(on_air, FrameSet::EMPTY, &mut |_memory| home += 1);
    assert_eq!(home, sent.len(), "the sent frames go home");
    let idle = match settled {
        Settled::Kept(arena) => {
            assert_eq!(
                u32::from(arena.frames()),
                u32::from(frames) - sent.len(),
                "no attempt of the rest was spent"
            );
            harness.port().withdraw(arena, &mut |_memory| {})
        }
        Settled::Idle(idle) => {
            assert_eq!(sent.len(), u32::from(frames), "every frame went");
            idle
        }
    };
    data.arenas = [idle, other];
    let (place, closed) = close_gate(harness, data.command, open);
    data.command = place;
    rights.gate = Some(closed);
    rights.data = Some(data);
    stop(harness, &mut rights, running);
    retire_whole(harness, rights);
    Publication { sent, single }
}

/// The plan of one frame alone: one transmission at 6 Mb/s.
fn alone<'id, P: DataPort<'id>>(port: &P) -> RetryPlan<PortRate<'id, P::Caps>> {
    let rate = port
        .rate(PhyRate::Legacy(LegacyRate::Ofdm6M))
        .expect("the port sends 6 Mb/s");
    RetryPlan::new(RetryStage {
        rate,
        transmissions: NonZeroU8::MIN,
        protection: Protection::None,
    })
}

/// An aggregate at HT `mcs`, a frame alone at 6 Mb/s.
fn aggregate<'id, P: DataPort<'id>>(port: &P, mcs: u8) -> Ppdu<'id, P::Caps> {
    Ppdu::Aggregate {
        rate: ht_rate(port, mcs),
        spacing: oer_protocols_ieee80211_wire::ht::MinimumMpduStartSpacing::NoRestriction,
        protection: Protection::None,
        alone: alone(port),
    }
}

/// **7. A publication sends what its rate admits**: an aggregate at a
/// rate that admits less than the arena holds sends the longest prefix, as
/// an A-MPDU, and what did not go is in place with its attempts unchanged.
/// `frames` frames of `body` octets must hold more than the rate admits
/// and more than one frame of it.
pub fn a_publication_sends_the_prefix_its_rate_admits<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    rights: Rights<'id, S, G, A, D>,
    length: MpduLength<<H::Port as RadioPort<'id>>::Caps>,
    body: u16,
    frames: u8,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let went = publication(harness, rights, length, body, frames, |port| {
        aggregate(port, 0)
    });
    assert!(
        u32::from(frames) > went.sent.len(),
        "the rate admits less than the arena holds"
    );
    assert!(went.sent.len() > 1, "the prefix is an aggregate");
    assert!(!went.single, "an aggregate's outcome is an aggregate's");
}

/// **The port's octets are the port's**: the engine and the network
/// write nothing in front of the headroom or behind a body and its MIC's
/// room. Three frames of `body` octets, the longest the port's slots hold,
/// are written and published; the port finds its own octets whole after
/// the writes and at the publication.
pub fn the_ports_octets_stay_the_ports<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    rights: Rights<'id, S, G, A, D>,
    length: MpduLength<<H::Port as RadioPort<'id>>::Caps>,
    body: u16,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let went = publication(harness, rights, length, body, 3, |port| {
        Ppdu::Single(alone(port))
    });
    assert_eq!(went.sent, FrameSet::prefix(1), "the first frame goes");
    assert_eq!(harness.port_octets(), PortOctets::Whole);
}

/// **7. A single publication** sends the arena's first frame as one MPDU
/// and leaves the rest in place with no attempt spent.
pub fn a_single_publication_sends_the_first_frame<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    rights: Rights<'id, S, G, A, D>,
    length: MpduLength<<H::Port as RadioPort<'id>>::Caps>,
    body: u16,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let went = publication(harness, rights, length, body, 3, |port| {
        Ppdu::Single(alone(port))
    });
    assert_eq!(went.sent, FrameSet::prefix(1), "the first frame goes");
    assert!(went.single, "its outcome is a single's");
}

/// **7. An aggregate of one frame ends as a single**: at HT `mcs`, a rate
/// that admits one frame of the arena's two, that frame goes alone by its
/// plan and the outcome says one MPDU went. `body` octets must exceed half
/// of what the rate admits.
pub fn an_aggregate_of_one_frame_ends_single<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    rights: Rights<'id, S, G, A, D>,
    length: MpduLength<<H::Port as RadioPort<'id>>::Caps>,
    body: u16,
    mcs: u8,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let went = publication(harness, rights, length, body, 2, |port| {
        aggregate(port, mcs)
    });
    assert_eq!(went.sent, FrameSet::prefix(1), "one frame is admitted");
    assert!(went.single, "what went is one MPDU");
}

/// HT `mcs` at 20 MHz with the long guard interval.
fn ht_rate<'id, P: DataPort<'id>>(port: &P, mcs: u8) -> AmpduRate<'id, P::Caps> {
    let rate = PhyRate::Ht(
        HtRate::new(
            HtMcs::new(mcs).unwrap(),
            PpduBandwidth::Mhz20,
            HtGuardInterval::Normal,
        )
        .unwrap(),
    );
    let rate = port.rate(rate).expect("the port sends the HT rate");
    port.ampdu_rate(rate)
        .expect("the port aggregates at the HT rate")
}

/// **8. A round's port calls are bounded**: with a full ring, a round
/// hands as many units as one service staged, and services the ring once.
pub fn a_round_services_the_ring_once<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, S, G, A, D>,
    places: usize,
    services: impl Fn(&H) -> u32,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    for _ in 0..places + 2 {
        harness.land(&data_mpdu(20));
    }
    let before = services(harness);
    let mut round = Round::open(harness.port(), NOW);
    let mut handed = 0;
    while let Some(_unit) = round.staged() {
        handed += 1;
    }
    let _ = round.close();
    assert_eq!(handed, places, "a round hands what one service staged");
    assert_eq!(
        services(harness) - before,
        1,
        "a round services the ring once"
    );
    stop(harness, &mut rights, running);
}

/// **6. A frame that is no frame goes back at its staging**: a unit that
/// is neither a data, a management nor a control frame is counted and
/// returned to the ring; the round hands the others.
pub fn an_unclassifiable_frame_goes_back_at_its_staging<
    'id,
    H,
    const S: usize,
    const G: usize,
    const A: usize,
    const D: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, S, G, A, D>,
    dropped: impl Fn(&H) -> u32,
) where
    H: Harness<'id, S, G, A, D>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    // Protocol version 3: no frame of this standard.
    harness.land(&[0xff; 24]);
    harness.land(&data_mpdu(20));
    let before = dropped(harness);
    let mut round = Round::open(harness.port(), NOW);
    let mut handed = 0;
    while let Some(_unit) = round.staged() {
        handed += 1;
    }
    let _ = round.close();
    assert_eq!(handed, 1, "the data frame is handed");
    assert_eq!(dropped(harness) - before, 1, "the other is counted");
    stop(harness, &mut rights, running);
}

/// **2. No call returns an error of the port**: the data path's calls, as
/// the trait declares them, answer outcomes; this function compiles only
/// while they do.
pub fn no_call_returns_an_error<'id, P: DataPort<'id>>() {
    let _: fn(
        &mut P,
        IdleArena<'id>,
        &P::Link,
        oer_protocols_ieee80211_wire::qos::Tid,
    ) -> oer_protocols_ieee80211_contracts_port::Arena<'id> = P::open;
}

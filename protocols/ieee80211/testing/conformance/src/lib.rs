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
    AccessPoint, AmpduRate, ArenaOutcome, CloseGate, CommandPlace, CommandPoll, DataPort, FrameSet,
    GateClosed, GateOpen, GroupCommand, GroupEnd, HEADROOM, HeaderLen, IdleChain, InCommand,
    InterfaceLayout, InterfaceRights, InterfaceRole, Layout, LayoutRights, MpduLength,
    PairwiseCommand, PairwiseEnd, PhyRate, PortRate, PortSink, Ppdu, QueueRight, RadioLayout,
    RadioPort, RadioRights, ReceiveCommand, ReceiveEnd, ReceivePort, Round, Settled, SingleOutcome,
    StartPoll, Station, StationReceive, StopFrom, StopPoll, StopRight, Stopped, TbttEvent, TxFacts,
    TxOutcome, ValueCommand, ValueEnd,
};
use oer_protocols_ieee80211_contracts_port::{
    AckHeader, HeaderSpec, Protection, RetryPlan, RetryStage, SizedFrame, TxFrame,
};
use oer_protocols_ieee80211_wire::frame::MacHeader;
use oer_protocols_ieee80211_wire::phy::{
    HtGuardInterval, HtMcs, HtRate, LegacyRate, PpduBandwidth,
};

/// The instant of every call of the suite.
pub const NOW: Instant = Instant::from_micros(0);

/// A port under the suite, with the levers of its hardware.
pub trait Harness<'id, L: Layout> {
    /// The port, of the layout `L`.
    type Port: ReceivePort<'id> + DataPort<'id, Layout = L>;
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
    /// A link of the interface `I`.
    fn link<I: InterfaceRole>(&self) -> <Self::Port as DataPort<'id>>::Link<I>;
    /// Land a received MPDU on the ring.
    fn land(&mut self, bytes: &[u8]);
    /// The anomalies the backend may indicate.
    fn anomalies(&self) -> usize;
    /// The hardware sees the anomaly `index`.
    fn raise(&mut self, index: usize);
    /// Keep the MAC busy: the next stop's last resort runs.
    fn hold_busy(&mut self);
    /// Let the next command expire at its bound.
    fn expire_next(&mut self);
    /// Keep the next frames on the air until [`Self::release_air`].
    fn hold_air(&mut self);
    /// Let the air end what it holds.
    fn release_air(&mut self);
    /// The chains the hardware queue of `category` carries now.
    fn chains_on(&self, category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory) -> usize;
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

/// The layout the suite runs: the radio's `CHAINS` chains of `SLOTS`, and
/// a station alone, with `SINGLES` single places, one pairwise key place, `GROUPS` group key slots and
/// `AGREEMENTS` agreements' slots.
pub type StationLayout<
    const CHAINS: usize,
    const SLOTS: usize,
    const SINGLES: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> = RadioLayout<CHAINS, SLOTS, (InterfaceLayout<Station, SINGLES, 1, GROUPS, AGREEMENTS>,)>;

/// The station's rights of the suite's layout.
pub type StationRights<'id, const SINGLES: usize, const GROUPS: usize, const AGREEMENTS: usize> =
    InterfaceRights<'id, Station, SINGLES, 1, GROUPS, AGREEMENTS>;

/// The layout of the suite's access point alone: `SINGLES` single places,
/// `PAIRWISE` pairwise key places, a client each, `GROUPS` group key slots
/// and `AGREEMENTS` agreements' slots.
pub type AccessPointLayout<
    const CHAINS: usize,
    const SLOTS: usize,
    const SINGLES: usize,
    const PAIRWISE: usize,
    const GROUPS: usize,
    const AGREEMENTS: usize,
> = RadioLayout<
    CHAINS,
    SLOTS,
    (InterfaceLayout<AccessPoint, SINGLES, PAIRWISE, GROUPS, AGREEMENTS>,),
>;

/// The rights the suite runs an installation of the layout `L` with.
pub struct Rights<'id, L: Layout> {
    /// The radio is stopped.
    pub stopped: Option<Stopped<'id>>,
    /// The stop's right.
    pub right: StopRight<'id>,
    /// The radio's and the interfaces' rights, while the data is stopped.
    pub data: Option<LayoutRights<'id, L>>,
    /// The gate, closed.
    pub gate: Option<GateClosed<'id>>,
}

/// A sink that keeps the singles' ends, each as its interface's.
struct Ends<'id, L: Layout> {
    singles: Vec<(SingleOutcome<(), ()>, L::Single<'id>, QueueRight<'id>)>,
}

impl<'id, L: Layout> Ends<'id, L> {
    const fn new() -> Self {
        Self {
            singles: Vec::new(),
        }
    }
}

impl<'id, L: Layout> PortSink<'id, L, (), ()> for Ends<'id, L> {
    fn single_ended(
        &mut self,
        outcome: SingleOutcome<(), ()>,
        place: L::Single<'id>,
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
fn run<'id, H, L: Layout>(
    harness: &mut H,
    rights: &mut Rights<'id, L>,
) -> oer_protocols_ieee80211_contracts_port::Running<'id>
where
    H: Harness<'id, L>,
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
fn stop<'id, H, L: Layout>(
    harness: &mut H,
    rights: &mut Rights<'id, L>,
    running: oer_protocols_ieee80211_contracts_port::Running<'id>,
) where
    H: Harness<'id, L>,
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
fn run_command<'id, H, C, K, E, L: Layout>(
    harness: &mut H,
    place: CommandPlace<'id>,
    command: C,
    begin: impl Fn(&mut H::Port, CommandPlace<'id>, C) -> InCommand<'id, K>,
    poll: impl Fn(&mut H::Port, InCommand<'id, K>, &mut Ends<'id, L>) -> CommandPoll<'id, K, E>,
) -> (CommandPlace<'id>, E)
where
    H: Harness<'id, L>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let mut flight = begin(harness.port(), place, command);
    let mut ends = Ends::new();
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
fn close_gate<'id, H, L: Layout>(
    harness: &mut H,
    place: CommandPlace<'id>,
    open: GateOpen<'id>,
) -> (CommandPlace<'id>, GateClosed<'id>)
where
    H: Harness<'id, L>,
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

/// A receive rule's command.
fn receive_command<'id, H, L: Layout, I: InterfaceRole>(
    harness: &mut H,
    place: CommandPlace<'id>,
    command: ReceiveCommand<'id, I, <H::Port as RadioPort<'id>>::Caps>,
) -> (CommandPlace<'id>, ReceiveEnd<'id, I>)
where
    H: Harness<'id, L>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    run_command(
        harness,
        place,
        command,
        |port, place, command| port.begin_receive(place, command, NOW),
        |port, flight, ends| port.poll_receive(flight, NOW, ends),
    )
}

/// A pairwise key command.
fn pairwise_command<'id, H, L: Layout, I: InterfaceRole>(
    harness: &mut H,
    place: CommandPlace<'id>,
    command: PairwiseCommand<'id, I>,
) -> (CommandPlace<'id>, PairwiseEnd<'id, I>)
where
    H: Harness<'id, L>,
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
fn group_command<'id, H, L: Layout, I: InterfaceRole>(
    harness: &mut H,
    place: CommandPlace<'id>,
    command: GroupCommand<'id, I>,
) -> (CommandPlace<'id>, GroupEnd<'id, I>)
where
    H: Harness<'id, L>,
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
fn value_command<'id, H, L: Layout>(
    harness: &mut H,
    place: CommandPlace<'id>,
    command: ValueCommand<<H::Port as RadioPort<'id>>::Caps>,
) -> (CommandPlace<'id>, ValueEnd)
where
    H: Harness<'id, L>,
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
fn data_run<
    'id,
    H,
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    rights: &mut Rights<'id, StationLayout<C, N, S, G, A>>,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let gate = rights.gate.take().expect("the gate is closed");
    let open: GateOpen<'id> = harness.port().open_gate(gate);
    let (
        RadioRights {
            chains,
            ones,
            queues,
            command: place,
        },
        StationRights {
            singles,
            data,
            pairwise: [pairwise],
            groups,
            agreements,
            receive,
        },
    ) = rights.data.take().expect("the data's rights are home");
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
    rights.data = Some((
        RadioRights {
            chains,
            ones,
            queues,
            command,
        },
        StationRights {
            singles,
            data,
            pairwise: [pairwise],
            groups,
            agreements,
            receive,
        },
    ));
}

/// **1. Every right is in exactly one place**: a run, its data path and its
/// stop hand every right back to the suite's places, and a retirement takes
/// them whole.
pub fn every_right_is_in_exactly_one_place<
    'id,
    H,
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
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
fn retire_whole<'id, H, L: Layout>(harness: &mut H, rights: Rights<'id, L>)
where
    H: Harness<'id, L>,
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

/// **The receive rule is a right**: the station's rule is set with its
/// own address, changed and ended, each end giving the right back, set or
/// off; a setting that expires gives it back off and leaves the hardware as
/// before it, and a change that expires gives it back off with the rule
/// cleared. `rule` and `other` are two rules the port admits.
pub fn the_receive_rule_is_a_right<
    'id,
    H,
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
    address: oer_protocols_ieee802_wire::IndividualAddress,
    rule: StationReceive<<H::Port as RadioPort<'id>>::Caps>,
    other: StationReceive<<H::Port as RadioPort<'id>>::Caps>,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    let (radio, station) = rights.data.take().expect("the rights are home");
    let off_state = harness.state();
    // A setting that expires: the right comes back off, the hardware as
    // before it.
    harness.expire_next();
    let (place, ReceiveEnd::Off(off)) = receive_command(
        harness,
        radio.command,
        ReceiveCommand::Set {
            off: station.receive,
            address,
            rule,
        },
    ) else {
        panic!("an expired setting gives the right back off");
    };
    assert_eq!(
        harness.state(),
        off_state,
        "an expired setting leaves the hardware as before it"
    );
    let (place, ReceiveEnd::Receiving(receiving)) =
        receive_command(harness, place, ReceiveCommand::Set { off, address, rule })
    else {
        panic!("the rule is set");
    };
    let set_state = harness.state();
    assert_ne!(
        set_state, off_state,
        "the hardware holds the address and the rule"
    );
    let (place, ReceiveEnd::Receiving(receiving)) = receive_command(
        harness,
        place,
        ReceiveCommand::Change {
            receiving,
            rule: other,
        },
    ) else {
        panic!("the rule is changed");
    };
    let (place, ReceiveEnd::Off(off)) =
        receive_command(harness, place, ReceiveCommand::End(receiving))
    else {
        panic!("the rule ends off");
    };
    assert_eq!(
        harness.state(),
        off_state,
        "an ended rule leaves the hardware as before its setting"
    );
    // A change that expires: the right comes back off, the port having
    // cleared the address and the rule.
    let (place, ReceiveEnd::Receiving(receiving)) =
        receive_command(harness, place, ReceiveCommand::Set { off, address, rule })
    else {
        panic!("the rule is set again");
    };
    harness.expire_next();
    let (command, ReceiveEnd::Off(off)) = receive_command(
        harness,
        place,
        ReceiveCommand::Change {
            receiving,
            rule: other,
        },
    ) else {
        panic!("an expired change gives the right back off");
    };
    assert_eq!(
        harness.state(),
        off_state,
        "an expired change leaves the rule cleared"
    );
    rights.data = Some((
        RadioRights { command, ..radio },
        InterfaceRights {
            receive: off,
            ..station
        },
    ));
    stop(harness, &mut rights, running);
    retire_whole(harness, rights);
}

/// **A client's key goes alone**: an access point's pairwise key places,
/// a client each, are installed and removed each on its own. Two clients'
/// keys are installed, each for its own client, and the first client's is
/// removed: the hardware then holds what it holds with the second client's
/// key installed alone in its place, which a harness's state names by its
/// peer; the second's is removed in its turn, and every place comes home.
/// `PAIRWISE` is two at least.
pub fn a_clients_key_goes_alone<
    'id,
    H,
    const C: usize,
    const N: usize,
    const S: usize,
    const P: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, AccessPointLayout<C, N, S, P, G, A>>,
) where
    H: Harness<'id, AccessPointLayout<C, N, S, P, G, A>>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    const {
        if P < 2 {
            panic!("two clients' keys need two pairwise places");
        }
    }
    let client = |last: u8| {
        oer_protocols_ieee802_wire::IndividualAddress::new(Eui48::new([2, 0, 0, 0, 0, last]))
            .unwrap()
    };
    let install = |slot, last: u8| PairwiseCommand::<'id, AccessPoint>::Install {
        slot,
        peer: client(last),
        key: oer_protocols_ieee80211_contracts_security::Ccmp128Key::new(&[last; 16]),
        spp: oer_protocols_ieee80211_wire::security::Spp::NotNegotiated,
    };
    let running = run(harness, &mut rights);
    let (radio, access_point) = rights.data.take().expect("the rights are home");
    let mut slots: Vec<_> = access_point.pairwise.into_iter().collect();
    let second = slots.remove(1);
    let first = slots.remove(0);
    let empty = harness.state();
    let (place, PairwiseEnd::Installed(first)) =
        pairwise_command(harness, radio.command, install(first, 1))
    else {
        panic!("the first client's key installs");
    };
    let first_alone = harness.state();
    let (place, PairwiseEnd::Installed(second)) =
        pairwise_command(harness, place, install(second, 2))
    else {
        panic!("the second client's key installs");
    };
    assert_eq!(
        (first.index(), second.index()),
        (0, 1),
        "each key in its own place"
    );
    assert_ne!(
        harness.state(),
        first_alone,
        "the second key is the hardware's too"
    );
    let (place, PairwiseEnd::Empty(first)) =
        pairwise_command(harness, place, PairwiseCommand::Remove(first))
    else {
        panic!("the first client's key is removed");
    };
    let second_left = harness.state();
    let (place, PairwiseEnd::Empty(second)) =
        pairwise_command(harness, place, PairwiseCommand::Remove(second))
    else {
        panic!("the second client's key is removed");
    };
    assert_eq!(
        harness.state(),
        empty,
        "both keys removed, the hardware holds none"
    );
    let (place, PairwiseEnd::Installed(second)) =
        pairwise_command(harness, place, install(second, 2))
    else {
        panic!("the second client's key installs alone");
    };
    assert_eq!(
        harness.state(),
        second_left,
        "the removal of the first client's key left the second client's as it was"
    );
    let (command, PairwiseEnd::Empty(second)) =
        pairwise_command(harness, place, PairwiseCommand::Remove(second))
    else {
        panic!("the second client's key is removed");
    };
    assert_eq!(harness.state(), empty);
    slots.insert(0, first);
    slots.insert(1, second);
    rights.data = Some((
        RadioRights { command, ..radio },
        InterfaceRights {
            pairwise: home(slots, "every pairwise place is back"),
            ..access_point
        },
    ));
    stop(harness, &mut rights, running);
    retire_whole(harness, rights);
}

/// **3. A second run is as the first**: what the hardware programs in a run
/// after a stop equals what it programmed in the first, and its state after
/// each stop equals its state before the first start; also after a run in
/// which each anomaly was raised, and after a stop whose last resort ran.
pub fn a_second_run_is_as_the_first<
    'id,
    H,
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
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
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    let gate = rights.gate.take().expect("the gate is closed");
    let open = harness.port().open_gate(gate);
    let (radio, station) = rights.data.take().expect("the data's rights are home");
    let mut singles: Vec<_> = station.singles.into_iter().collect();
    let mut queues: Vec<_> = radio.queues.into_iter().collect();
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
    let mut ends = Ends::new();
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
    let (command, closed) = close_gate(harness, radio.command, open);
    rights.gate = Some(closed);
    rights.data = Some((
        RadioRights {
            queues: home(queues, "every queue's right is back"),
            command,
            ..radio
        },
        StationRights {
            singles: home(singles, "every place is back"),
            ..station
        },
    ));
    retire_whole(harness, rights);
}

/// **5. The indication**: an anomaly raises "the port needs a stop" in the
/// port's facts, with no payload, until the stop that follows.
pub fn the_indication_holds_until_the_stop<
    'id,
    H,
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
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
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
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
fn publication<
    'id,
    H,
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
    length: MpduLength<<H::Port as RadioPort<'id>>::Caps>,
    body: u16,
    frames: u8,
    how: impl FnOnce(&H::Port) -> Ppdu<'id, <H::Port as RadioPort<'id>>::Caps>,
) -> Publication
where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    let gate = rights.gate.take().expect("the gate is closed");
    let open = harness.port().open_gate(gate);
    let (data, station) = rights.data.take().expect("the data's rights are home");
    let mut chains: Vec<_> = data.chains.into_iter().collect();
    let mut queues: Vec<_> = data.queues.into_iter().collect();
    let idle = chains.remove(0);
    // QUEUES' order: voice, video, best effort, background.
    let best_effort = queues.remove(2);
    let link = harness.link::<Station>();
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
    let on_air = harness.port().publish(arena, best_effort, how, &open, NOW);
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
        .ended(TxFacts::DUE, &on_air, NOW)
        .expect("the arena ended");
    assert_eq!(ended.sent, sent);
    let single = matches!(ended.outcome, ArenaOutcome::Single(_));
    let mut went_home = 0;
    let (settled, best_effort) = harness
        .port()
        .settle(on_air, FrameSet::EMPTY, &mut |_memory| went_home += 1);
    assert_eq!(
        best_effort.category(),
        oer_protocols_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
        "the settlement gives back the right of the chain's queue"
    );
    queues.insert(2, best_effort);
    assert_eq!(went_home, sent.len(), "the sent frames go home");
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
    chains.insert(0, idle);
    let (command, closed) = close_gate(harness, data.command, open);
    rights.gate = Some(closed);
    rights.data = Some((
        RadioRights {
            chains: home(chains, "every chain is back"),
            ones: data.ones,
            queues: home(queues, "every queue's right is back"),
            command,
        },
        station,
    ));
    stop(harness, &mut rights, running);
    retire_whole(harness, rights);
    Publication { sent, single }
}

/// A chain of `frames` frames of `body` octets, opened from `idle` and
/// filled.
fn filled<'id, H, L: Layout, const S: usize>(
    harness: &mut H,
    idle: IdleChain<'id, S>,
    frames: u8,
    body: u16,
) -> oer_protocols_ieee80211_contracts_port::Chain<'id, S>
where
    H: Harness<'id, L>,
{
    let link = harness.link::<Station>();
    let tid = oer_protocols_ieee80211_wire::qos::Tid::constant::<0>();
    let mut chain = harness.port().open(idle, &link, tid);
    let length = MpduLength::new(usize::from(body) + QOS_DATA.len()).expect("an MPDU's length");
    for _ in 0..frames {
        let memory = harness.memory(body);
        let place = harness
            .port()
            .place(&mut chain, length)
            .expect("the chain has descriptors");
        place.write(memory, header);
    }
    chain
}

/// The best effort chain and the voice chain of one, published together,
/// then ended through `end` (the air released, or a stop), and settled:
/// each chain's frames home and its queue's right back, every chain and
/// every right in its place again.
fn two_queues<'id, H, L: Layout, const C: usize, const N: usize>(
    harness: &mut H,
    radio: RadioRights<'id, C, N>,
    open: &GateOpen<'id>,
    end: impl FnOnce(&mut H),
) -> RadioRights<'id, C, N>
where
    H: Harness<'id, L>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    use oer_protocols_ieee80211_wire::qos::WmmAccessCategory as Ac;
    let mut chains: Vec<_> = radio.chains.into_iter().collect();
    let mut ones: Vec<_> = radio.ones.into_iter().collect();
    let mut queues: Vec<_> = radio.queues.into_iter().collect();
    // QUEUES' order: voice, video, best effort, background.
    let best_effort = queues.remove(2);
    let voice = queues.remove(0);
    // Two frames, one of which stays, where the long chains hold two.
    let long_frames = u8::try_from(N.min(2)).expect("two frames are a byte");
    let long = filled(harness, chains.remove(0), long_frames, 100);
    let one = filled(harness, ones.remove(0), 1, 40);
    harness.hold_air();
    let how = Ppdu::Single(alone(harness.port()));
    let long = harness.port().publish(long, best_effort, how, open, NOW);
    let how = Ppdu::Single(alone(harness.port()));
    let one = harness.port().publish(one, voice, how, open, NOW);
    assert_eq!(
        [Ac::Voice, Ac::Video, Ac::BestEffort, Ac::Background].map(|ac| harness.chains_on(ac)),
        [1, 0, 1, 0],
        "each chain is on the queue it was published on"
    );
    end(harness);
    assert!(
        harness.port().ended(TxFacts::DUE, &long, NOW).is_some(),
        "the long chain ended"
    );
    assert!(
        harness.port().ended(TxFacts::DUE, &one, NOW).is_some(),
        "the chain of one ended"
    );
    let mut went_home = 0;
    let (long, long_queue) = harness
        .port()
        .settle(long, FrameSet::EMPTY, &mut |_memory| went_home += 1);
    let (one, one_queue) = harness
        .port()
        .settle(one, FrameSet::EMPTY, &mut |_memory| went_home += 1);
    assert_eq!(
        (long_queue.category(), one_queue.category()),
        (Ac::BestEffort, Ac::Voice),
        "each settlement gives back its own queue's right"
    );
    assert_eq!(went_home, 2, "each settlement sent its sent frame home");
    // A long chain's second frame did not go: it stays, with no attempt
    // spent, until a withdrawal sends it home.
    let long = match long {
        Settled::Kept(chain) => {
            assert_eq!(
                chain.frames(),
                long_frames - 1,
                "the frame that did not go stays"
            );
            harness
                .port()
                .withdraw(chain, &mut |_memory| went_home += 1)
        }
        Settled::Idle(idle) => {
            assert_eq!(long_frames, 1, "a chain whose frames all went is idle");
            idle
        }
    };
    let Settled::Idle(one) = one else {
        panic!("the chain of one is idle");
    };
    assert_eq!(
        went_home,
        1 + usize::from(long_frames),
        "every frame's memory came home"
    );
    assert_eq!(
        [Ac::Voice, Ac::Video, Ac::BestEffort, Ac::Background].map(|ac| harness.chains_on(ac)),
        [0; 4],
        "no queue carries a chain after the settlements"
    );
    chains.insert(0, long);
    ones.insert(0, one);
    queues.insert(0, one_queue);
    queues.insert(2, long_queue);
    RadioRights {
        chains: home(chains, "every chain is back"),
        ones: home(ones, "every chain of one is back"),
        queues: home(queues, "every queue's right is back"),
        command: radio.command,
    }
}

/// **A chain takes its queue's right**: a chain on best effort and a chain
/// of one on voice are on the air together, each on the queue it was
/// published on; each ends and its settlement returns its own queue's
/// right, its frames' memory home.
pub fn chains_on_two_queues_end_each_with_its_queue<
    'id,
    H,
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    let gate = rights.gate.take().expect("the gate is closed");
    let open = harness.port().open_gate(gate);
    let (radio, station) = rights.data.take().expect("the rights are home");
    let radio = two_queues(harness, radio, &open, |harness| harness.release_air());
    let (command, closed) = close_gate(harness, radio.command, open);
    rights.gate = Some(closed);
    rights.data = Some((RadioRights { command, ..radio }, station));
    stop(harness, &mut rights, running);
    retire_whole(harness, rights);
}

/// **A stop returns every right and every handle**: with a chain on best
/// effort and a chain of one on voice on the air, the stop ends both
/// withdrawn; their settlements return each queue's right and every
/// frame's memory, and a retirement takes every right.
pub fn a_stop_returns_every_chain_and_queue<
    'id,
    H,
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
    H::Port: DataPort<'id, Ack = (), Denial = ()>,
{
    let running = run(harness, &mut rights);
    let gate = rights.gate.take().expect("the gate is closed");
    let open = harness.port().open_gate(gate);
    let (radio, station) = rights.data.take().expect("the rights are home");
    let right = &mut rights.right;
    let mut stopped = None;
    let radio = two_queues(harness, radio, &open, |harness| {
        let mut stopping = harness.port().stop(right, StopFrom::Running(running));
        stopped = Some(loop {
            match harness.port().poll_stop(stopping, &mut cx(), NOW) {
                StopPoll::Stopped(stopped) => break stopped,
                StopPoll::Pending(again) => stopping = again,
            }
        });
    });
    rights.stopped = stopped;
    harness.release_air();
    let (command, closed) = close_gate(harness, radio.command, open);
    rights.gate = Some(closed);
    rights.data = Some((RadioRights { command, ..radio }, station));
    retire_whole(harness, rights);
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
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    rights: Rights<'id, StationLayout<C, N, S, G, A>>,
    length: MpduLength<<H::Port as RadioPort<'id>>::Caps>,
    body: u16,
    frames: u8,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
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
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    rights: Rights<'id, StationLayout<C, N, S, G, A>>,
    length: MpduLength<<H::Port as RadioPort<'id>>::Caps>,
    body: u16,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
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
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    rights: Rights<'id, StationLayout<C, N, S, G, A>>,
    length: MpduLength<<H::Port as RadioPort<'id>>::Caps>,
    body: u16,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
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
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    rights: Rights<'id, StationLayout<C, N, S, G, A>>,
    length: MpduLength<<H::Port as RadioPort<'id>>::Caps>,
    body: u16,
    mcs: u8,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
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
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
    places: usize,
    services: impl Fn(&H) -> u32,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
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
    const C: usize,
    const N: usize,
    const S: usize,
    const G: usize,
    const A: usize,
>(
    harness: &mut H,
    mut rights: Rights<'id, StationLayout<C, N, S, G, A>>,
    dropped: impl Fn(&H) -> u32,
) where
    H: Harness<'id, StationLayout<C, N, S, G, A>>,
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
        IdleChain<'id, 1>,
        &P::Link<Station>,
        oer_protocols_ieee80211_wire::qos::Tid,
    ) -> oer_protocols_ieee80211_contracts_port::Chain<'id, 1> = P::open::<Station, 1>;
}

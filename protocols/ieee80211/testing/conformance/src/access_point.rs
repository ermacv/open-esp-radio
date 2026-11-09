//! Cases of the access-point role ([`AccessPointPort`]) that every backend
//! serving it passes.
//!
//! A backend's tests implement [`AccessPointHarness`] over its port and run
//! each case of [`check`]. The harness holds the tokens of the access point
//! and the station of its current and of its earlier installation, makes
//! the frames and commands a case names, and takes every terminal back; a
//! case calls the port itself for the operation it checks and judges what
//! the port returned and what the harness observed of the air. The cases
//! check what the contract promises of every backend:
//!
//! - an access-point frame goes out as the access point's on the shared port
//!   ([`Case::SharedPort`]);
//! - a station configuration neither withdraws nor waits for the access
//!   point's frames ([`Case::StationConfigureApart`]), and an access-point
//!   frame on the air holds only the access point's configuration
//!   ([`Case::ConfigureBarrier`]);
//! - a beacon goes out on its queue and its terminal returns the permission
//!   ([`Case::BeaconPermission`]); a stopped radio refuses a beacon with its
//!   permission at once ([`Case::StoppedBeacon`]); a beacon not published by
//!   its deadline ends `Expired` with every owner ([`Case::BeaconExpires`]);
//! - every token of an earlier installation is refused at its acceptance
//!   point with its owners ([`Case::StaleConfigure`], [`Case::StaleBeacon`],
//!   [`Case::StaleInstall`], [`Case::StaleRemoval`], [`Case::StaleFrameKey`]);
//! - the access point's keys install and remove on its own permissions
//!   ([`Case::Keys`]);
//! - the access point stops alone beside a running station
//!   ([`Case::StopsAlone`]); its held command holds no station command and
//!   comes back refused `Expired` with its owners past its bound, the port
//!   running on and stopping ([`Case::HeldCommandApart`]); an expired key
//!   removal undoes its barrier: the key stays installed and in use
//!   ([`Case::ExpiredRemoval`]); a stop
//!   returns a held command, which the stopped port refuses
//!   ([`Case::StopReturnsHeld`]);
//! - the shared stop ends the beacon and the access point's interface
//!   ([`Case::SharedStop`]);
//! - the same two stops over ordinary access-point frames, no beacon
//!   among them ([`Case::StopsAloneFrames`], [`Case::SharedStopFrames`]):
//!   a backend that admits no beacon runs these.

use core::task::{Context, Poll, Waker};

use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_contracts_port::{
    AccessPointCommand, AccessPointCommandKind, AccessPointPort, AccessPointProgress,
    AccessPointRefusal, CommandProgress, DataPort, PortCommand, PortSink, SubmitRefusal,
    SupportedChannel, TsfControl, TxFacts, VifRole, WithdrawCause,
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

use crate::data_port::PROGRESS_STEPS;

/// An access-point command of the port `P`.
pub type AccessPointCommandOf<'id, P> =
    AccessPointCommand<'id, <P as DataPort<'id>>::Types, <P as DataPort<'id>>::Caps>;

/// How far the port `P` took an access-point command.
pub type AccessPointProgressOf<'id, P> =
    AccessPointProgress<'id, <P as DataPort<'id>>::Types, <P as DataPort<'id>>::Caps>;

/// A station command of the port `P`.
pub type PortCommandOf<'id, P> =
    PortCommand<'id, <P as DataPort<'id>>::Types, <P as DataPort<'id>>::Caps>;

/// The beacon's queue, from the contract's own policy.
pub const BEACON_QUEUE: WmmAccessCategory =
    oer_protocols_ieee80211_contracts_port::BEACON_ACCESS_CATEGORY;

/// The queue of the access point's data in these cases, apart from the
/// beacon's and the station's.
pub const ACCESS_POINT_QUEUE: WmmAccessCategory = WmmAccessCategory::Video;

/// The queue of the station's data in these cases.
pub const STATION_QUEUE: WmmAccessCategory = WmmAccessCategory::BestEffort;

/// Which installation issued a token an operation names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Issued {
    /// The port's current installation.
    Current,
    /// The installation before it, which [`AccessPointHarness::reinstall`]
    /// retired.
    Earlier,
}

/// What an access-point configuration asks its interface to receive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Receive {
    /// Nothing for the access point.
    Off,
    /// Its BSS's members.
    Bss,
}

/// How a transmission ended, as the cases tell its ends apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum End {
    /// Its solicited response came, or it solicited none.
    Delivered,
    /// It was withdrawn before its plan ended.
    Withdrawn(WithdrawCause),
    /// It aged before it was published.
    Expired,
    /// Any other end.
    Other,
}

/// One transmission terminal the harness took from the sink, its owners
/// back with the harness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ended {
    /// A single MPDU of the ticket.
    Frame(u8, End),
    /// The beacon of the ticket; its permission is back with the harness.
    Beacon(u8, End),
}

/// One publication the fixture's air saw, in publication order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sent {
    /// The interface it went out on.
    pub role: VifRole,
    /// Whether it went out under an installed key.
    pub protected: bool,
}

/// What the fixture's hardware does for the access point's interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Interface {
    /// It receives the BSS's frames.
    pub receives: bool,
    /// Its TSF runs.
    pub tsf_running: bool,
}

/// An access-point command terminal the harness took from the sink, its
/// owners back with the harness, by the installation that issued them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Terminal {
    /// A configuration took effect; its permission is back.
    Configured(u32),
    /// A pairwise key is installed; the harness holds it.
    PairwiseInstalled(u32),
    /// The group key is installed; the harness holds it and its permission.
    GroupInstalled(u32),
    /// A pairwise key is removed; its slot and association are back.
    PairwiseRemoved(u32),
    /// The group key is removed; its slot and permission are back.
    GroupRemoved(u32),
}

/// A backend fixture of the access-point role: its port, the sink of its
/// terminals, the tokens of both roles of its current and earlier
/// installation, and the air and time around it.
pub trait AccessPointHarness<'id> {
    /// The backend's port.
    type Port: AccessPointPort<'id>;
    /// The sink the harness keeps the port's terminals and frames in.
    type Sink: PortSink<'id, <Self::Port as DataPort<'id>>::Types>;
    /// The transmission terminals of one [`Self::take_ends`], in order.
    type Ends: AsRef<[Ended]>;
    /// The publications the air saw, in order.
    type Sent: AsRef<[Sent]>;

    /// The port and the sink, borrowed together for one call.
    fn parts(&mut self) -> (&mut Self::Port, &mut Self::Sink);
    /// The fixture's virtual time.
    fn now(&self) -> Instant;
    /// Move virtual time on by `by`.
    fn advance(&mut self, by: Duration);
    /// A waker for the port's polls.
    fn waker(&self) -> Waker;
    /// The channel the port starts on.
    fn channel(&self) -> SupportedChannel<<Self::Port as DataPort<'id>>::Caps>;
    /// Let the fixture's hardware make the progress it owes (a start's or
    /// a stop's answer), without touching the port.
    fn settle(&mut self);
    /// Stop the started port, retire it and install its next installation,
    /// not started: the tokens it held become [`Issued::Earlier`].
    ///
    /// # Errors
    ///
    /// [`Violation::Fixture`] when the port does not stop or reinstall.
    fn reinstall(&mut self) -> Result<(), Violation>;
    /// How long the backend lets an access-point command stay held before
    /// its named fault.
    fn command_bound(&self) -> Duration;
    /// How long the backend lets a published frame stay on its queue.
    fn publication_bound(&self) -> Duration;

    /// Keep the published frames of `category` from completing (`true`), or
    /// let them complete (`false`).
    fn hold(&mut self, category: WmmAccessCategory, hold: bool);
    /// Keep the port's group-key mutations at their barrier (`true`), or let
    /// them pass (`false`).
    fn hold_group_commands(&mut self, hold: bool);

    /// An access-point data frame to the fixture's peer on `category`,
    /// under the pairwise key of `key` when given, admitted by the port.
    fn access_point_frame(
        &mut self,
        key: Option<Issued>,
        category: WmmAccessCategory,
    ) -> Option<<Self::Port as DataPort<'id>>::Admitted>;
    /// A station data frame on `category`, under the station's installed
    /// key when `keyed`, admitted by the port.
    fn station_frame(
        &mut self,
        category: WmmAccessCategory,
        keyed: bool,
    ) -> Option<<Self::Port as DataPort<'id>>::Admitted>;
    /// A beacon under the beacon permission `of`, which must be published
    /// by `deadline`, admitted by the port. `None` while the harness does
    /// not hold that permission.
    fn beacon(
        &mut self,
        of: Issued,
        deadline: Instant,
    ) -> Option<<Self::Port as DataPort<'id>>::Admitted>;
    /// The ticket of an admitted attempt, which its terminal names.
    fn ticket(attempt: &<Self::Port as DataPort<'id>>::Admitted) -> u8;
    /// Take every transmission terminal from the sink, returning tickets,
    /// buffers and a beacon's permission to the harness. Other terminals
    /// stay in the sink, in order.
    fn take_ends(&mut self) -> Self::Ends;
    /// Whether the harness holds the beacon permission `of`.
    fn holds_beacon(&self, of: Issued) -> bool;

    /// The station's receive configuration command.
    fn station_configure(&mut self) -> Option<PortCommandOf<'id, Self::Port>>;
    /// Take the station's configuration terminal from the sink: whether it
    /// was there.
    fn take_station_configured(&mut self) -> bool;
    /// Install the station's pairwise key through the port and keep it:
    /// whether it was installed.
    fn install_station_key(&mut self) -> bool;

    /// The access point's configuration under the configure permission
    /// `of`.
    fn configure(
        &mut self,
        of: Issued,
        receive: Receive,
        tsf: Option<TsfControl>,
    ) -> Option<AccessPointCommandOf<'id, Self::Port>>;
    /// A pairwise installation for the fixture's peer under the association
    /// `association` in a vacant slot of `slot`.
    fn install_pairwise(
        &mut self,
        association: Issued,
        slot: Issued,
    ) -> Option<AccessPointCommandOf<'id, Self::Port>>;
    /// The group installation under the association `association`, the group
    /// permission `permission`, in a vacant slot of `slot`.
    fn install_group(
        &mut self,
        association: Issued,
        permission: Issued,
        slot: Issued,
    ) -> Option<AccessPointCommandOf<'id, Self::Port>>;
    /// The removal of the installed pairwise key `key` under `association`.
    fn remove_pairwise(
        &mut self,
        association: Issued,
        key: Issued,
    ) -> Option<AccessPointCommandOf<'id, Self::Port>>;
    /// The removal of the installed group key `key` with the group
    /// permission `permission`.
    fn remove_group(
        &mut self,
        permission: Issued,
        key: Issued,
    ) -> Option<AccessPointCommandOf<'id, Self::Port>>;
    /// Take the first access-point command terminal from the sink, its
    /// owners back with the harness.
    fn take_terminal(&mut self) -> Option<Terminal>;
    /// Take back the owners of a command the port returned.
    fn restore(&mut self, command: AccessPointCommandOf<'id, Self::Port>);

    /// The publications the air saw so far.
    fn sent(&self) -> Self::Sent;
    /// What the hardware does for the access point's interface now.
    fn interface(&self) -> Interface;
    /// The key material writes the hardware has seen.
    fn key_writes(&self) -> usize;
    /// Whether every ticket and buffer of the current installation is back
    /// with the harness.
    fn idle(&self) -> bool;
}

/// One case of the suite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// An access-point frame goes out as the access point's.
    SharedPort,
    /// A station configuration neither withdraws nor waits for
    /// access-point frames.
    StationConfigureApart,
    /// An access-point frame on the air holds only the access point's
    /// configuration.
    ConfigureBarrier,
    /// A beacon goes out on its queue; its terminal returns the permission,
    /// which admits the next beacon.
    BeaconPermission,
    /// A stopped radio refuses a beacon and returns its permission at once.
    StoppedBeacon,
    /// A beacon not published by its deadline ends expired with every owner.
    BeaconExpires,
    /// An earlier installation's configure permission is refused with it.
    StaleConfigure,
    /// A beacon sealed before a reinstallation is refused at submit.
    StaleBeacon,
    /// Every key installation naming an earlier token is refused.
    StaleInstall,
    /// Every key removal naming an earlier token is refused.
    StaleRemoval,
    /// A frame under an earlier installation's key is refused at submit.
    StaleFrameKey,
    /// The access point's keys install and remove on its own permissions.
    Keys,
    /// The access point stops alone beside a running station.
    StopsAlone,
    /// A held access-point command holds no station command; past its bound
    /// it comes back refused `Expired` with its owners and the port stops.
    HeldCommandApart,
    /// A group key removal held past its bound comes back `Expired`; the key
    /// stays installed and removes later.
    ExpiredRemoval,
    /// A stop returns a held command, which the stopped port refuses.
    StopReturnsHeld,
    /// The shared stop ends the beacon and the access point's interface.
    SharedStop,
    /// The access point stops alone beside a running station, its frames
    /// ordinary access-point frames: the one on the air ends as the air
    /// answers, the one queued behind it withdrawn.
    StopsAloneFrames,
    /// The shared stop ends the work of both roles once, an access-point
    /// frame on the air among it, and the access point's interface.
    SharedStopFrames,
}

impl Case {
    /// Every case, in suite order.
    pub const ALL: [Self; 19] = [
        Self::SharedPort,
        Self::StationConfigureApart,
        Self::ConfigureBarrier,
        Self::BeaconPermission,
        Self::StoppedBeacon,
        Self::BeaconExpires,
        Self::StaleConfigure,
        Self::StaleBeacon,
        Self::StaleInstall,
        Self::StaleRemoval,
        Self::StaleFrameKey,
        Self::Keys,
        Self::StopsAlone,
        Self::HeldCommandApart,
        Self::ExpiredRemoval,
        Self::StopReturnsHeld,
        Self::SharedStop,
        Self::StopsAloneFrames,
        Self::SharedStopFrames,
    ];
}

/// What a case found wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// The port did not start or stop within the case's steps.
    Lifecycle,
    /// The fixture could not set up the case.
    Fixture,
    /// The port faulted where the case expects none.
    Fault,
    /// A transmission ended otherwise than the case expects.
    Ends,
    /// A frame went out on the wrong interface or protection, or a refused
    /// one went out.
    Air,
    /// A command ran early, never ran, or was held where it should run.
    Barrier,
    /// A refusal named another reason, or lost or changed an owner.
    Refusal,
    /// An owner was not returned.
    Owners,
    /// The access point's interface is not as the case expects.
    Interface,
    /// The access point's address or the station's changed.
    Address,
}

/// Run `case` on a fresh fixture.
///
/// # Errors
///
/// What the case found wrong.
pub fn check<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    case: Case,
) -> Result<(), Violation> {
    match case {
        Case::SharedPort => shared_port(harness),
        Case::StationConfigureApart => station_configure_apart(harness),
        Case::ConfigureBarrier => configure_barrier(harness),
        Case::BeaconPermission => beacon_permission(harness),
        Case::StoppedBeacon => stopped_beacon(harness),
        Case::BeaconExpires => beacon_expires(harness),
        Case::StaleConfigure => stale_configure(harness),
        Case::StaleBeacon => stale_beacon(harness),
        Case::StaleInstall => stale_install(harness),
        Case::StaleRemoval => stale_removal(harness),
        Case::StaleFrameKey => stale_frame_key(harness),
        Case::Keys => keys(harness),
        Case::StopsAlone => stops_alone(harness, Behind::Beacon),
        Case::HeldCommandApart => held_command_apart(harness),
        Case::ExpiredRemoval => expired_removal(harness),
        Case::StopReturnsHeld => stop_returns_held(harness),
        Case::SharedStop => shared_stop(harness, Behind::Beacon),
        Case::StopsAloneFrames => stops_alone(harness, Behind::Frame),
        Case::SharedStopFrames => shared_stop(harness, Behind::Frame),
    }
}

/// Start the port, settling the fixture between polls.
///
/// # Errors
///
/// [`Violation::Lifecycle`] when it does not start.
pub fn start<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    let channel = harness.channel();
    for _ in 0..PROGRESS_STEPS {
        let waker = harness.waker();
        let now = harness.now();
        let (port, _) = harness.parts();
        match port.poll_start(&mut Context::from_waker(&waker), channel, now) {
            Poll::Ready(Ok(())) => return Ok(()),
            Poll::Ready(Err(_)) => return Err(Violation::Lifecycle),
            Poll::Pending => harness.settle(),
        }
    }
    Err(Violation::Lifecycle)
}

/// Serve the transmit path once at the fixture's time.
pub fn service<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    harness.settle();
    let waker = harness.waker();
    let now = harness.now();
    let (port, sink) = harness.parts();
    let facts = port.poll_facts(&mut Context::from_waker(&waker));
    port.service_tx(facts.tx.unwrap_or(TxFacts::DUE), facts.tbtt, now, sink)
        .map(|_| ())
        .map_err(|_| Violation::Fault)
}

/// Submit an admitted attempt the port must accept; its ticket.
pub fn submit<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    attempt: Option<<H::Port as DataPort<'id>>::Admitted>,
) -> Result<u8, Violation> {
    let attempt = attempt.ok_or(Violation::Fixture)?;
    let ticket = H::ticket(&attempt);
    let now = harness.now();
    let (port, sink) = harness.parts();
    port.submit(attempt, now, sink)
        .map_err(|_| Violation::Refusal)?;
    Ok(ticket)
}

/// Run an access-point command once.
pub fn run<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    command: Option<AccessPointCommandOf<'id, H::Port>>,
) -> Result<AccessPointProgressOf<'id, H::Port>, Violation> {
    let command = command.ok_or(Violation::Fixture)?;
    let now = harness.now();
    let (port, sink) = harness.parts();
    port.access_point_command(command, now, sink)
        .map_err(|_| Violation::Fault)
}

/// Run an access-point command the port must finish with nothing of the
/// access point on the air: a hold at a barrier the port itself passes (a
/// receive cut) runs again after the port's next service.
pub fn done<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    command: Option<AccessPointCommandOf<'id, H::Port>>,
) -> Result<(), Violation> {
    let mut command = command;
    for _ in 0..PROGRESS_STEPS {
        match run(harness, command.take())? {
            AccessPointProgress::Done => return Ok(()),
            AccessPointProgress::Held(held) => {
                command = Some(held);
                service(harness)?;
            }
            AccessPointProgress::Refused(refused, _) => {
                harness.restore(refused);
                return Err(Violation::Barrier);
            }
        }
    }
    if let Some(command) = command {
        harness.restore(command);
    }
    Err(Violation::Barrier)
}

/// Run an access-point command the port must hold at its barrier.
fn held<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    command: Option<AccessPointCommandOf<'id, H::Port>>,
) -> Result<AccessPointCommandOf<'id, H::Port>, Violation> {
    match run(harness, command)? {
        AccessPointProgress::Held(command) => Ok(command),
        AccessPointProgress::Refused(command, _) => {
            harness.restore(command);
            Err(Violation::Barrier)
        }
        AccessPointProgress::Done => Err(Violation::Barrier),
    }
}

/// Run an access-point command the port must refuse for a token of the
/// earlier installation; its owners go back to the harness.
fn refused_stale<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    command: Option<AccessPointCommandOf<'id, H::Port>>,
    kind: AccessPointCommandKind,
) -> Result<(), Violation> {
    match run(harness, command)? {
        AccessPointProgress::Refused(
            command,
            AccessPointRefusal::StalePermission {
                issued,
                installation,
            },
        ) => {
            let right = command.kind() == kind && issued < installation;
            harness.restore(command);
            if right {
                Ok(())
            } else {
                Err(Violation::Refusal)
            }
        }
        AccessPointProgress::Refused(command, _) | AccessPointProgress::Held(command) => {
            harness.restore(command);
            Err(Violation::Refusal)
        }
        AccessPointProgress::Done => Err(Violation::Refusal),
    }
}

/// Run a station command the port must finish at once.
pub fn station_command<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    command: Option<PortCommandOf<'id, H::Port>>,
) -> Result<Option<PortCommandOf<'id, H::Port>>, Violation> {
    let command = command.ok_or(Violation::Fixture)?;
    let now = harness.now();
    let (port, sink) = harness.parts();
    match port
        .command(command, now, sink)
        .map_err(|_| Violation::Fault)?
    {
        CommandProgress::Done => Ok(None),
        CommandProgress::Held(command) => Ok(Some(command)),
        CommandProgress::Refused(..) => Err(Violation::Refusal),
    }
}

/// Configure the station and take its terminal, serving the port while
/// its own barrier holds it.
fn configure_station<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    let mut command = harness.station_configure();
    if command.is_none() {
        return Err(Violation::Fixture);
    }
    for _ in 0..PROGRESS_STEPS {
        if command.is_some() {
            command = station_command(harness, command)?;
        }
        if command.is_none() && harness.take_station_configured() {
            return Ok(());
        }
        service(harness)?;
    }
    Err(Violation::Barrier)
}

/// Take the transmission terminals and compare them with `expected` in
/// order.
fn ends<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    expected: &[Ended],
) -> Result<(), Violation> {
    if harness.take_ends().as_ref() == expected {
        Ok(())
    } else {
        Err(Violation::Ends)
    }
}

/// Take the next access-point command terminal, which must be `expected`.
fn terminal<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    expected: Terminal,
) -> Result<(), Violation> {
    if harness.take_terminal() == Some(expected) {
        Ok(())
    } else {
        Err(Violation::Owners)
    }
}

/// The last publication the air saw.
fn last_sent<'id, H: AccessPointHarness<'id>>(harness: &H) -> Option<Sent> {
    harness.sent().as_ref().last().copied()
}

/// Whether the air saw no publication at all.
fn nothing_sent<'id, H: AccessPointHarness<'id>>(harness: &H) -> bool {
    harness.sent().as_ref().is_empty()
}

/// An instant that keeps a beacon fresh for the whole case.
fn far<'id, H: AccessPointHarness<'id>>(harness: &H) -> Instant {
    harness.now().saturating_add(Duration::from_secs(3_600))
}

fn shared_port<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    let (station, access_point) = {
        let (port, _) = harness.parts();
        (port.address(), port.access_point_address())
    };
    if station == access_point {
        return Err(Violation::Address);
    }
    let frame = harness.access_point_frame(None, ACCESS_POINT_QUEUE);
    let ticket = submit(harness, frame)?;
    service(harness)?;
    let sent = harness.sent();
    if sent.as_ref()
        != [Sent {
            role: VifRole::AccessPoint,
            protected: false,
        }]
    {
        return Err(Violation::Air);
    }
    ends(harness, &[Ended::Frame(ticket, End::Delivered)])?;
    let (port, _) = harness.parts();
    if port.address() != station || port.access_point_address() != access_point {
        return Err(Violation::Address);
    }
    Ok(())
}

fn station_configure_apart<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
) -> Result<(), Violation> {
    start(harness)?;
    // A station frame on the air holds the station's configuration; a
    // second station frame waits behind an access-point frame on the air in
    // another queue.
    harness.hold(STATION_QUEUE, true);
    let first = harness.station_frame(STATION_QUEUE, false);
    let first = submit(harness, first)?;
    let waiting = WmmAccessCategory::Background;
    harness.hold(waiting, true);
    let blocker = harness.access_point_frame(None, waiting);
    let blocker = submit(harness, blocker)?;
    service(harness)?;
    let queued = harness.station_frame(waiting, false);
    let queued = submit(harness, queued)?;
    let configure = harness.station_configure();
    let mut held = station_command(harness, configure)?;
    // An access-point frame submitted behind the station's configuration
    // goes out while it waits.
    let frame = harness.access_point_frame(None, ACCESS_POINT_QUEUE);
    let frame = submit(harness, frame)?;
    service(harness)?;
    let ended = harness.take_ends();
    let ended = ended.as_ref();
    let mut withdrawn = ended
        .iter()
        .any(|end| matches!(end, Ended::Frame(ticket, End::Withdrawn(_)) if *ticket == queued));
    if !ended.contains(&Ended::Frame(frame, End::Delivered))
        || ended.len() != 1 + usize::from(withdrawn)
    {
        return Err(Violation::Ends);
    }
    if harness.take_station_configured() {
        return Err(Violation::Barrier);
    }
    // The air answers: the station's frame ends, the access point's frame
    // ahead of the queued station frame ends, the queued one ends
    // withdrawn, never published, and the configuration takes effect.
    harness.hold(STATION_QUEUE, false);
    harness.hold(waiting, false);
    let (mut first_ended, mut blocker_ended, mut configured) = (false, false, false);
    for _ in 0..PROGRESS_STEPS {
        service(harness)?;
        if held.is_some() {
            held = station_command(harness, held)?;
        }
        for end in harness.take_ends().as_ref() {
            match end {
                Ended::Frame(ticket, End::Delivered) if *ticket == first => first_ended = true,
                Ended::Frame(ticket, End::Delivered) if *ticket == blocker => blocker_ended = true,
                Ended::Frame(ticket, End::Withdrawn(_)) if *ticket == queued => withdrawn = true,
                _ => return Err(Violation::Ends),
            }
        }
        if held.is_none() && harness.take_station_configured() {
            configured = true;
        }
        if first_ended && blocker_ended && withdrawn && configured {
            return Ok(());
        }
    }
    Err(Violation::Barrier)
}

fn configure_barrier<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    harness.hold(ACCESS_POINT_QUEUE, true);
    let frame = harness.access_point_frame(None, ACCESS_POINT_QUEUE);
    let frame = submit(harness, frame)?;
    service(harness)?;
    // The station configures and sends while the access point's frame is
    // out.
    configure_station(harness)?;
    let station = harness.station_frame(STATION_QUEUE, false);
    let station = submit(harness, station)?;
    service(harness)?;
    ends(harness, &[Ended::Frame(station, End::Delivered)])?;
    let configure = harness.configure(Issued::Current, Receive::Bss, None);
    let command = held(harness, configure)?;
    if harness.take_terminal().is_some() {
        return Err(Violation::Barrier);
    }
    harness.hold(ACCESS_POINT_QUEUE, false);
    service(harness)?;
    ends(harness, &[Ended::Frame(frame, End::Delivered)])?;
    done(harness, Some(command))?;
    let installation = {
        let (port, _) = harness.parts();
        port_generation(port)
    };
    terminal(harness, Terminal::Configured(installation))?;
    if !harness.interface().receives {
        return Err(Violation::Interface);
    }
    Ok(())
}

/// The generation the port's tokens carry, read from nothing but its own
/// refusals: the cases compare generations only by order, so the current
/// installation's is the larger. A fixture's first installation is 0 and
/// its next 1; this reads it from the harness's own terminal.
fn port_generation<'id, P: DataPort<'id>>(_: &P) -> u32 {
    0
}

fn beacon_permission<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    // The beacon's queue is busy with an access-point frame: the beacon
    // waits while another queue's frame goes.
    harness.hold(BEACON_QUEUE, true);
    let busy = harness.access_point_frame(None, BEACON_QUEUE);
    let busy = submit(harness, busy)?;
    let deadline = far(harness);
    let beacon = harness.beacon(Issued::Current, deadline);
    let beacon = submit(harness, beacon)?;
    let other = harness.access_point_frame(None, ACCESS_POINT_QUEUE);
    let other = submit(harness, other)?;
    service(harness)?;
    ends(harness, &[Ended::Frame(other, End::Delivered)])?;
    if harness.holds_beacon(Issued::Current) {
        return Err(Violation::Owners);
    }
    harness.hold(BEACON_QUEUE, false);
    for _ in 0..2 {
        service(harness)?;
    }
    ends(
        harness,
        &[
            Ended::Frame(busy, End::Delivered),
            Ended::Beacon(beacon, End::Delivered),
        ],
    )?;
    if last_sent(harness).map(|sent| sent.role) != Some(VifRole::AccessPoint) {
        return Err(Violation::Air);
    }
    // The returned permission admits the next beacon.
    let deadline = far(harness);
    let again = harness.beacon(Issued::Current, deadline);
    let again = submit(harness, again)?;
    service(harness)?;
    ends(harness, &[Ended::Beacon(again, End::Delivered)])?;
    if harness.holds_beacon(Issued::Current) {
        Ok(())
    } else {
        Err(Violation::Owners)
    }
}

fn stopped_beacon<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    let deadline = far(harness);
    let beacon = harness
        .beacon(Issued::Current, deadline)
        .ok_or(Violation::Fixture)?;
    let ticket = H::ticket(&beacon);
    let now = harness.now();
    let (port, sink) = harness.parts();
    if port.submit(beacon, now, sink) != Err(SubmitRefusal::NotEnabled) {
        return Err(Violation::Refusal);
    }
    ends(
        harness,
        &[Ended::Beacon(
            ticket,
            End::Withdrawn(WithdrawCause::Disabled),
        )],
    )?;
    if harness.holds_beacon(Issued::Current) {
        Ok(())
    } else {
        Err(Violation::Owners)
    }
}

fn beacon_expires<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    // A held access-point frame keeps the beacon's queue busy past the
    // beacon's deadline, within the frame's own publication bound.
    let fresh = Duration::from_micros(harness.publication_bound().as_micros() / 2);
    harness.hold(BEACON_QUEUE, true);
    let busy = harness.access_point_frame(None, BEACON_QUEUE);
    let busy = submit(harness, busy)?;
    let deadline = harness.now().saturating_add(fresh);
    let beacon = harness.beacon(Issued::Current, deadline);
    let beacon = submit(harness, beacon)?;
    service(harness)?;
    if !harness.take_ends().as_ref().is_empty() {
        return Err(Violation::Ends);
    }
    harness.advance(fresh);
    service(harness)?;
    // The beacon never went out: it ends expired, its permission back,
    // while the frame ahead of it stays on the air.
    ends(harness, &[Ended::Beacon(beacon, End::Expired)])?;
    if !harness.holds_beacon(Issued::Current) {
        return Err(Violation::Owners);
    }
    harness.hold(BEACON_QUEUE, false);
    service(harness)?;
    ends(harness, &[Ended::Frame(busy, End::Delivered)])?;
    let beacons = harness
        .sent()
        .as_ref()
        .iter()
        .filter(|sent| sent.role == VifRole::AccessPoint)
        .count();
    if beacons != 1 {
        return Err(Violation::Air);
    }
    if harness.idle() {
        Ok(())
    } else {
        Err(Violation::Owners)
    }
}

fn stale_configure<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    harness.reinstall()?;
    start(harness)?;
    let before = harness.interface();
    let configure = harness.configure(Issued::Earlier, Receive::Bss, Some(TsfControl::Restart));
    refused_stale(harness, configure, AccessPointCommandKind::Configure)?;
    if harness.interface() != before {
        return Err(Violation::Interface);
    }
    if harness.take_terminal().is_some() {
        return Err(Violation::Owners);
    }
    // The current installation's permission configures.
    let configure = harness.configure(Issued::Current, Receive::Bss, None);
    done(harness, configure)?;
    match harness.take_terminal() {
        Some(Terminal::Configured(generation)) if generation > 0 => Ok(()),
        _ => Err(Violation::Owners),
    }
}

fn stale_beacon<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    harness.reinstall()?;
    start(harness)?;
    // A beacon of the current installation's ticket and buffer under the
    // earlier installation's beacon permission.
    let deadline = far(harness);
    let sealed = harness
        .beacon(Issued::Earlier, deadline)
        .ok_or(Violation::Fixture)?;
    let ticket = H::ticket(&sealed);
    let now = harness.now();
    let (port, sink) = harness.parts();
    match port.submit(sealed, now, sink) {
        Err(SubmitRefusal::StalePermission {
            issued,
            installation,
        }) if issued < installation => {}
        _ => return Err(Violation::Refusal),
    }
    ends(
        harness,
        &[Ended::Beacon(
            ticket,
            End::Withdrawn(WithdrawCause::StalePermission),
        )],
    )?;
    if !harness.holds_beacon(Issued::Earlier) {
        return Err(Violation::Owners);
    }
    if !nothing_sent(harness) {
        return Err(Violation::Air);
    }
    // The current installation's beacon goes.
    let deadline = far(harness);
    let current = harness.beacon(Issued::Current, deadline);
    let current = submit(harness, current)?;
    service(harness)?;
    ends(harness, &[Ended::Beacon(current, End::Delivered)])?;
    if harness.holds_beacon(Issued::Current) {
        Ok(())
    } else {
        Err(Violation::Owners)
    }
}

fn stale_install<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    harness.reinstall()?;
    start(harness)?;
    let writes = harness.key_writes();
    use AccessPointCommandKind::{InstallGroup, InstallPairwise};
    use Issued::{Current, Earlier};
    // An earlier slot under the current association, and a current slot
    // under the earlier one.
    let install = harness.install_pairwise(Current, Earlier);
    refused_stale(harness, install, InstallPairwise)?;
    let install = harness.install_pairwise(Earlier, Current);
    refused_stale(harness, install, InstallPairwise)?;
    // The earlier group permission with a current slot, and the current
    // permission with an earlier slot. A refused installation's owners go
    // back to the harness, which need not install from them again.
    let install = harness.install_group(Current, Earlier, Current);
    refused_stale(harness, install, InstallGroup)?;
    let install = harness.install_group(Current, Current, Earlier);
    refused_stale(harness, install, InstallGroup)?;
    if harness.take_terminal().is_some() {
        return Err(Violation::Owners);
    }
    if harness.key_writes() != writes {
        return Err(Violation::Refusal);
    }
    Ok(())
}

/// Install the access point's pairwise and group keys of the current
/// installation and take their terminals.
fn install_keys<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    installation: u32,
) -> Result<(), Violation> {
    let install = harness.install_pairwise(Issued::Current, Issued::Current);
    done(harness, install)?;
    terminal(harness, Terminal::PairwiseInstalled(installation))?;
    let install = harness.install_group(Issued::Current, Issued::Current, Issued::Current);
    done(harness, install)?;
    terminal(harness, Terminal::GroupInstalled(installation))
}

/// Remove the access point's group and pairwise keys of the current
/// installation and take their terminals.
fn remove_keys<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    installation: u32,
) -> Result<(), Violation> {
    let remove = harness.remove_group(Issued::Current, Issued::Current);
    done(harness, remove)?;
    terminal(harness, Terminal::GroupRemoved(installation))?;
    let remove = harness.remove_pairwise(Issued::Current, Issued::Current);
    done(harness, remove)?;
    terminal(harness, Terminal::PairwiseRemoved(installation))
}

fn stale_removal<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    install_keys(harness, 0)?;
    harness.reinstall()?;
    start(harness)?;
    install_keys(harness, 1)?;
    use AccessPointCommandKind::{RemoveGroup, RemovePairwise};
    use Issued::{Current, Earlier};
    // Each removal names exactly one token of the earlier installation and
    // is refused for it with every owner back.
    let remove = harness.remove_pairwise(Earlier, Current);
    refused_stale(harness, remove, RemovePairwise)?;
    let remove = harness.remove_pairwise(Current, Earlier);
    refused_stale(harness, remove, RemovePairwise)?;
    let remove = harness.remove_group(Earlier, Current);
    refused_stale(harness, remove, RemoveGroup)?;
    let remove = harness.remove_group(Current, Earlier);
    refused_stale(harness, remove, RemoveGroup)?;
    if harness.take_terminal().is_some() {
        return Err(Violation::Owners);
    }
    // The current tokens still remove the current keys.
    remove_keys(harness, 1)
}

fn stale_frame_key<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    let install = harness.install_pairwise(Issued::Current, Issued::Current);
    done(harness, install)?;
    terminal(harness, Terminal::PairwiseInstalled(0))?;
    harness.reinstall()?;
    start(harness)?;
    // A frame of the current installation's ticket and buffer under the
    // earlier installation's key.
    let sealed = harness
        .access_point_frame(Some(Issued::Earlier), ACCESS_POINT_QUEUE)
        .ok_or(Violation::Fixture)?;
    let ticket = H::ticket(&sealed);
    let now = harness.now();
    let (port, sink) = harness.parts();
    match port.submit(sealed, now, sink) {
        Err(SubmitRefusal::StalePermission {
            issued,
            installation,
        }) if issued < installation => {}
        _ => return Err(Violation::Refusal),
    }
    ends(
        harness,
        &[Ended::Frame(
            ticket,
            End::Withdrawn(WithdrawCause::StalePermission),
        )],
    )?;
    if nothing_sent(harness) {
        Ok(())
    } else {
        Err(Violation::Air)
    }
}

fn keys<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    install_keys(harness, 0)?;
    // A protected frame under the access point's key goes out under it.
    let frame = harness.access_point_frame(Some(Issued::Current), ACCESS_POINT_QUEUE);
    let frame = submit(harness, frame)?;
    service(harness)?;
    if last_sent(harness)
        != Some(Sent {
            role: VifRole::AccessPoint,
            protected: true,
        })
    {
        return Err(Violation::Air);
    }
    ends(harness, &[Ended::Frame(frame, End::Delivered)])?;
    // Both removals return their slots, the group permission and the
    // association.
    remove_keys(harness, 0)
}

/// What a stop case puts in the beacon's queue besides an ordinary frame:
/// a beacon, or an ordinary access-point frame in its place.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Behind {
    Beacon,
    Frame,
}

fn stops_alone<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    behind: Behind,
) -> Result<(), Violation> {
    start(harness)?;
    // The station runs: configured, its key installed, its frames out.
    configure_station(harness)?;
    if !harness.install_station_key() {
        return Err(Violation::Fixture);
    }
    // The access point runs: configured, TSF restarted, its keys in.
    let configure = harness.configure(Issued::Current, Receive::Bss, Some(TsfControl::Restart));
    done(harness, configure)?;
    terminal(harness, Terminal::Configured(0))?;
    if harness.interface()
        != (Interface {
            receives: true,
            tsf_running: true,
        })
    {
        return Err(Violation::Interface);
    }
    install_keys(harness, 0)?;
    // An access-point frame under its key on the air in the beacon's
    // queue, a beacon (or a second frame) queued behind it, a station frame
    // on the air.
    harness.hold(BEACON_QUEUE, true);
    let ap_frame = harness.access_point_frame(Some(Issued::Current), BEACON_QUEUE);
    let ap_frame = submit(harness, ap_frame)?;
    let queued_behind = match behind {
        Behind::Beacon => {
            let deadline = far(harness);
            harness.beacon(Issued::Current, deadline)
        }
        Behind::Frame => harness.access_point_frame(Some(Issued::Current), BEACON_QUEUE),
    };
    let queued_behind = submit(harness, queued_behind)?;
    harness.hold(STATION_QUEUE, true);
    let station_on_air = harness.station_frame(STATION_QUEUE, true);
    let station_on_air = submit(harness, station_on_air)?;
    service(harness)?;
    if !harness.take_ends().as_ref().is_empty() {
        return Err(Violation::Ends);
    }
    // A station frame waits in a free queue as the access point stops.
    let station_queued = harness.station_frame(WmmAccessCategory::Background, false);
    let station_queued = submit(harness, station_queued)?;
    // The access point stops: its configuration Off waits for its frame on
    // the air; the station's queued frame goes out meanwhile.
    let configure = harness.configure(Issued::Current, Receive::Off, None);
    let command = held(harness, configure)?;
    service(harness)?;
    ends(harness, &[Ended::Frame(station_queued, End::Delivered)])?;
    // The station is not held by the access point's stop: its frame ends, a
    // new one goes, and its own command runs.
    harness.hold(STATION_QUEUE, false);
    service(harness)?;
    ends(harness, &[Ended::Frame(station_on_air, End::Delivered)])?;
    let station_next = harness.station_frame(STATION_QUEUE, true);
    let station_next = submit(harness, station_next)?;
    service(harness)?;
    ends(harness, &[Ended::Frame(station_next, End::Delivered)])?;
    // The access point's frame ends; its configuration completes.
    harness.hold(BEACON_QUEUE, false);
    let mut command = Some(command);
    let mut finished = false;
    for _ in 0..PROGRESS_STEPS {
        service(harness)?;
        if let Some(held) = command.take() {
            match run(harness, Some(held))? {
                AccessPointProgress::Done => finished = true,
                AccessPointProgress::Held(held) => command = Some(held),
                AccessPointProgress::Refused(refused, _) => {
                    harness.restore(refused);
                    return Err(Violation::Barrier);
                }
            }
        }
        if finished {
            break;
        }
    }
    if !finished {
        return Err(Violation::Barrier);
    }
    // Its frame ended; what was queued behind it ended withdrawn, never
    // published, a beacon with its permission.
    let ended = harness.take_ends();
    let ended = ended.as_ref();
    let behind_ended = ended.iter().any(|end| match (behind, end) {
        (Behind::Beacon, Ended::Beacon(ticket, End::Withdrawn(_)))
        | (Behind::Frame, Ended::Frame(ticket, End::Withdrawn(_))) => *ticket == queued_behind,
        _ => false,
    });
    if ended.len() != 2 || !behind_ended || !ended.contains(&Ended::Frame(ap_frame, End::Delivered))
    {
        return Err(Violation::Ends);
    }
    if !harness.holds_beacon(Issued::Current) {
        return Err(Violation::Owners);
    }
    terminal(harness, Terminal::Configured(0))?;
    if harness.interface().receives {
        return Err(Violation::Interface);
    }
    // Its keys come off, its TSF stops last, with its configure permission.
    remove_keys(harness, 0)?;
    let configure = harness.configure(Issued::Current, Receive::Off, Some(TsfControl::Stop));
    done(harness, configure)?;
    terminal(harness, Terminal::Configured(0))?;
    if harness.interface().tsf_running {
        return Err(Violation::Interface);
    }
    // The station keeps working on its key after the access point left.
    let station_after = harness.station_frame(STATION_QUEUE, true);
    let station_after = submit(harness, station_after)?;
    service(harness)?;
    if last_sent(harness)
        != Some(Sent {
            role: VifRole::Station,
            protected: true,
        })
    {
        return Err(Violation::Air);
    }
    ends(harness, &[Ended::Frame(station_after, End::Delivered)])?;
    if harness.idle() {
        Ok(())
    } else {
        Err(Violation::Owners)
    }
}

fn held_command_apart<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    harness.hold_group_commands(true);
    let install = harness.install_group(Issued::Current, Issued::Current, Issued::Current);
    let install = held(harness, install)?;
    // The station's command runs meanwhile.
    configure_station(harness)?;
    // Past its bound the held installation did not run: it comes back
    // with its owners and the port runs on.
    expire(harness, install, AccessPointCommandKind::InstallGroup)?;
    if harness.take_terminal().is_some() {
        return Err(Violation::Barrier);
    }
    harness.hold_group_commands(false);
    stop(harness)
}

/// Let a held command pass the port's bound: it must come back refused
/// `Expired`, its owners back with the harness.
fn expire<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    command: AccessPointCommandOf<'id, H::Port>,
    kind: AccessPointCommandKind,
) -> Result<(), Violation> {
    let bound = harness.command_bound();
    harness.advance(bound);
    let now = harness.now();
    let (port, sink) = harness.parts();
    match port.access_point_command(command, now, sink) {
        Err(_) => Err(Violation::Fault),
        Ok(AccessPointProgress::Refused(command, AccessPointRefusal::Expired))
            if command.kind() == kind =>
        {
            harness.restore(command);
            Ok(())
        }
        Ok(AccessPointProgress::Held(command) | AccessPointProgress::Refused(command, _)) => {
            harness.restore(command);
            Err(Violation::Refusal)
        }
        Ok(AccessPointProgress::Done) => Err(Violation::Barrier),
    }
}

/// The end of the one frame a case let the air keep past a bound: any end,
/// its owners back.
fn one_end<'id, H: AccessPointHarness<'id>>(harness: &mut H, ticket: u8) -> Result<(), Violation> {
    for _ in 0..PROGRESS_STEPS {
        service(harness)?;
        let ends = harness.take_ends();
        match ends.as_ref() {
            [] => {
                let now = harness.now();
                let (port, _) = harness.parts();
                if let Some(at) = port.deadline()
                    && at > now
                {
                    harness.advance(at.saturating_duration_since(now));
                }
            }
            [Ended::Frame(ended, _)] if *ended == ticket => return Ok(()),
            _ => return Err(Violation::Ends),
        }
    }
    Err(Violation::Ends)
}

/// A held access-point configuration past the port's bound, its barrier an
/// access-point frame the air keeps, comes back refused `Expired` with its
/// permission; it runs again from its own new admission once the frame
/// ended. Not in [`Case::ALL`]: it holds for a backend whose command call
/// never ends a publication (it ends them in its service). A backend that
/// ends a held frame by its publication bound within the call runs the
/// configuration instead, as the model does.
///
/// # Errors
///
/// What the case found wrong.
pub fn expired_configure<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
) -> Result<(), Violation> {
    start(harness)?;
    harness.hold(ACCESS_POINT_QUEUE, true);
    let frame = harness.access_point_frame(None, ACCESS_POINT_QUEUE);
    let frame = submit(harness, frame)?;
    service(harness)?;
    let configure = harness.configure(Issued::Current, Receive::Bss, None);
    let command = held(harness, configure)?;
    expire(harness, command, AccessPointCommandKind::Configure)?;
    if harness.take_terminal().is_some() || harness.interface().receives {
        return Err(Violation::Barrier);
    }
    harness.hold(ACCESS_POINT_QUEUE, false);
    one_end(harness, frame)?;
    let configure = harness.configure(Issued::Current, Receive::Bss, None);
    done(harness, configure)?;
    terminal(harness, Terminal::Configured(0))?;
    if !harness.interface().receives {
        return Err(Violation::Interface);
    }
    stop(harness)
}

/// A pairwise key removal held past the port's bound behind a frame under
/// the key that the air keeps comes back refused `Expired` with its owners;
/// the key still protects the access point's frames and removes later. Not
/// in [`Case::ALL`], for the reason [`expired_configure`] names.
///
/// # Errors
///
/// What the case found wrong.
pub fn expired_pairwise_removal<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
) -> Result<(), Violation> {
    start(harness)?;
    install_keys(harness, 0)?;
    harness.hold(ACCESS_POINT_QUEUE, true);
    let frame = harness.access_point_frame(Some(Issued::Current), ACCESS_POINT_QUEUE);
    let frame = submit(harness, frame)?;
    service(harness)?;
    let remove = harness.remove_pairwise(Issued::Current, Issued::Current);
    let remove = held(harness, remove)?;
    expire(harness, remove, AccessPointCommandKind::RemovePairwise)?;
    if harness.take_terminal().is_some() {
        return Err(Violation::Barrier);
    }
    harness.hold(ACCESS_POINT_QUEUE, false);
    one_end(harness, frame)?;
    // The key is active again: a frame under it goes out protected.
    let frame = harness.access_point_frame(Some(Issued::Current), ACCESS_POINT_QUEUE);
    let frame = submit(harness, frame)?;
    service(harness)?;
    if last_sent(harness)
        != Some(Sent {
            role: VifRole::AccessPoint,
            protected: true,
        })
    {
        return Err(Violation::Air);
    }
    ends(harness, &[Ended::Frame(frame, End::Delivered)])?;
    remove_keys(harness, 0)?;
    stop(harness)
}

fn expired_removal<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    install_keys(harness, 0)?;
    // The group key's removal waits for the air's group commands.
    harness.hold_group_commands(true);
    let remove = harness.remove_group(Issued::Current, Issued::Current);
    let remove = held(harness, remove)?;
    expire(harness, remove, AccessPointCommandKind::RemoveGroup)?;
    harness.hold_group_commands(false);
    if harness.take_terminal().is_some() {
        return Err(Violation::Barrier);
    }
    // Both keys are still installed: their removals run now.
    remove_keys(harness, 0)?;
    stop(harness)
}

/// Stop the port and confirm the stop within the case's steps, moving
/// time to the port's deadline while it waits.
pub fn stop<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    for _ in 0..PROGRESS_STEPS {
        let waker = harness.waker();
        let now = harness.now();
        let (port, _) = harness.parts();
        match port.poll_stop(&mut Context::from_waker(&waker), now) {
            Poll::Ready(Ok(())) => return Ok(()),
            Poll::Ready(Err(_)) => return Err(Violation::Lifecycle),
            Poll::Pending => {
                service(harness)?;
                let now = harness.now();
                let (port, _) = harness.parts();
                if let Some(at) = port.deadline()
                    && at > now
                {
                    harness.advance(at.saturating_duration_since(now));
                }
            }
        }
    }
    Err(Violation::Lifecycle)
}

fn stop_returns_held<'id, H: AccessPointHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    harness.hold_group_commands(true);
    let install = harness.install_group(Issued::Current, Issued::Current, Issued::Current);
    let install = held(harness, install)?;
    stop(harness)?;
    // The stopped port refuses the installation with its owners; no
    // terminal was owed.
    match run(harness, Some(install))? {
        AccessPointProgress::Refused(command, AccessPointRefusal::NotEnabled)
            if command.kind() == AccessPointCommandKind::InstallGroup =>
        {
            harness.restore(command);
        }
        AccessPointProgress::Refused(command, _) | AccessPointProgress::Held(command) => {
            harness.restore(command);
            return Err(Violation::Refusal);
        }
        AccessPointProgress::Done => return Err(Violation::Refusal),
    }
    if harness.take_terminal().is_some() || !harness.take_ends().as_ref().is_empty() {
        return Err(Violation::Owners);
    }
    Ok(())
}

fn shared_stop<'id, H: AccessPointHarness<'id>>(
    harness: &mut H,
    behind: Behind,
) -> Result<(), Violation> {
    start(harness)?;
    let configure = harness.configure(Issued::Current, Receive::Bss, Some(TsfControl::Restart));
    done(harness, configure)?;
    terminal(harness, Terminal::Configured(0))?;
    // A beacon (or an access-point frame) on the air and a station frame
    // queued behind a busy queue when the shared owner stops.
    harness.hold(BEACON_QUEUE, true);
    let on_air = match behind {
        Behind::Beacon => {
            let deadline = far(harness);
            harness.beacon(Issued::Current, deadline)
        }
        Behind::Frame => harness.access_point_frame(None, BEACON_QUEUE),
    };
    let on_air = submit(harness, on_air)?;
    harness.hold(STATION_QUEUE, true);
    let busy = harness.station_frame(STATION_QUEUE, false);
    let busy = submit(harness, busy)?;
    let queued = harness.station_frame(STATION_QUEUE, false);
    let queued = submit(harness, queued)?;
    service(harness)?;
    let waker = harness.waker();
    let now = harness.now();
    {
        let (port, _) = harness.parts();
        if port
            .poll_stop(&mut Context::from_waker(&waker), now)
            .is_ready()
        {
            return Err(Violation::Lifecycle);
        }
    }
    // What is on the air ends as the air answers; what waits behind it ends
    // withdrawn, never published.
    harness.hold(BEACON_QUEUE, false);
    harness.hold(STATION_QUEUE, false);
    stop(harness)?;
    let ended = harness.take_ends();
    let ended = ended.as_ref();
    let on_air_ended = ended.iter().any(|end| match (behind, end) {
        (Behind::Beacon, Ended::Beacon(ticket, _)) | (Behind::Frame, Ended::Frame(ticket, _)) => {
            *ticket == on_air
        }
        _ => false,
    });
    let busy_ended = ended
        .iter()
        .any(|end| matches!(end, Ended::Frame(ticket, _) if *ticket == busy));
    let queued_ended = ended.contains(&Ended::Frame(
        queued,
        End::Withdrawn(WithdrawCause::Disabled),
    ));
    if ended.len() != 3 || !on_air_ended || !busy_ended || !queued_ended {
        return Err(Violation::Ends);
    }
    if !harness.holds_beacon(Issued::Current) {
        return Err(Violation::Owners);
    }
    if harness.interface()
        != (Interface {
            receives: false,
            tsf_running: false,
        })
    {
        return Err(Violation::Interface);
    }
    // The access point's later commands are refused with their owners.
    let configure = harness.configure(Issued::Current, Receive::Off, None);
    match run(harness, configure)? {
        AccessPointProgress::Refused(command, AccessPointRefusal::NotEnabled)
            if command.kind() == AccessPointCommandKind::Configure =>
        {
            harness.restore(command);
            Ok(())
        }
        AccessPointProgress::Refused(command, _) | AccessPointProgress::Held(command) => {
            harness.restore(command);
            Err(Violation::Refusal)
        }
        AccessPointProgress::Done => Err(Violation::Refusal),
    }
}

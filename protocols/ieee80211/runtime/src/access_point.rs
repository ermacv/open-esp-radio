//! The access point's runner: one task that owns the data port and serves
//! the access-point role over it with the role's engine
//! (`oer_protocols_ieee80211_core_roles_ap::engine`).
//!
//! The runner starts the radio, configures the access point's interface,
//! installs the group key and restarts the TSF, in that order (the
//! archive's start order); it then beacons at each TBTT, answers the
//! management requests the engine takes, runs each peer's four-way
//! handshake and carries the authorized peers' data to and from its network
//! end ([`network`]). A stop tears every peer down (Disassociation and
//! Deauthentication, its key off), closes the receive policy, removes the
//! group key, stops the TSF and stops the radio; the runner then returns
//! every owner it holds.
//!
//! # Handshake keys
//!
//! A verified Message 2 that derived a new PTK installs the peer's pairwise
//! key first; Message 3 goes once the key's terminal came, so the
//! station's Message 4, protected or not, is readable. The controlled port
//! stays closed until a verified Message 4. A retried Message 3 or a
//! repeated Message 2 of the same PTK installs nothing again; a new PTK of
//! a peer whose key is in or going in fails the handshake.
//!
//! # Beacons
//!
//! A beacon is submitted at its TBTT with the next TBTT as its deadline, a
//! freshness bound: one not published by then ends expired, and a beacon
//! that ends in any way is not sent again: the next TBTT's beacon replaces
//! it. The beacon's Timestamp is written at its preparation
//! ([`BeaconTimestamp::Caller`]): the runner's time since the TSF restart;
//! the deadline says nothing of its accuracy on the air. The runner needs
//! that rule: over a port whose capabilities declare no `Caller` rule (none,
//! or only [`BeaconTimestamp::Transmitter`], which the runner does not
//! support) a run ends at once, before the radio starts,
//! [`ApEnd::Unsupported`] with every owner in the runner; a composition
//! checks [`serves_beacons`] before it starts the radio at all.
//!
//! # Waiting
//!
//! A turn that moved something (an owner, a unit, a terminal or a command
//! taken) yields to the executor once and turns again. An idle turn yields
//! and lets its sources decide: the port's facts, a deadline, a frame of
//! the network while a ticket and a buffer are free, or a stop not yet
//! accepted. After [`IDLE_REPEATS`] idle turns in a row the runner has
//! settled: deadlines it found due no longer wake it, and the very facts
//! it just served, when the port still reports them as it registers the
//! runner's waker, stay held for the next turn (never dropped) until a wake
//! of any source comes.
//!
//! # Owners
//!
//! Every owner the runner lends the port has a fixed home it comes back
//! to: the runner is the port's sink, and a terminal puts its owners home
//! during the port's call. What a terminal asks of the port (a Message 1
//! or a Message 3 to send) waits as plain data until the call returned; a
//! command is built from the homes' owners only when it is issued, and the
//! one the port holds at its barrier waits in its own place. A unit a
//! receive cut hands over is read and recycled once the call returned. An
//! owner the port returns without the runner having lent it has no home:
//! the run ends on that break, the owner among the runner's ([`Kept`]).
//!
//! # Ends
//!
//! Every end returns the runner with the port and every owner
//! ([`ApEnd`]): a stop, a refused start, an unconfirmed stop, a port fault
//! with its cause (a command it did not take among the owners,
//! [`ApRunner::take_returned`]), a refused command (the command among the
//! owners too) and a broken contract of the runner's own.
//! A run that ended before its stop was confirmed goes on with
//! [`ApRunner::finish_stop`], which watches the stop on the port's facts and
//! deadline, never asking for it again, and returns once the port
//! confirmed it: only then are the owners free of the hardware. A frame of
//! the air the engine refuses changes nothing; a frame the runner has no
//! ticket or buffer for does not go, which its peer's own retry covers.

pub mod network;

use core::future::poll_fn;
use core::num::NonZeroU8;
use core::task::{Context, Poll};

use embassy_futures::select::{Either, Either4, select, select4};
use oer_portable_time::{Instant, Timer};
use oer_protocols_ieee80211_contracts_port::{
    AccessPoint, AccessPointCapabilities, AccessPointCommand, AccessPointCommands, AccessPointPort,
    AccessPointProgress, AccessPointReceive, AccessPointRefusal, Attempt, AttemptKey,
    BeaconAttempt, BeaconPermission, BeaconStamp, BeaconTimestamp, CommandFailed,
    ConfigurePermission, Configured, ConnectionFrame, DataPort, Facts, GroupKeyId, Input,
    InstalledGroup, InstalledPairwise, KeyTypes, LowerMacTerminal, NumberedKey, PhyRate, PortSink,
    PortTypes, Protection, ReceiveAccounting, ReceiveFilter, RecycleRefused, Removed, RemovedKey,
    RetryPlan, RetryStage, RxCryptoStatus, RxEvidence, RxMetadata, RxPull, SingleEnd,
    StopUnconfirmed, SupportedChannel, SupportedFilter, SupportedPower, SupportedTsfControl,
    TbttEvent, TerminalAccess, TsfControl, TxBufferClass, TxBufferMemory, TxBufferPool,
    TxCompleted, TxFacts, TxMpdu, TxOperation, TxOutcome, TxPower, TxProtection, TxSource,
    Unconsumed, UnconsumedRefused,
};
use oer_protocols_ieee80211_core_roles_ap::{
    ApEapolTransmit, ApEapolTxContext, ApGroupKeyInstallation, ApGroupTransmitKey,
    beacon::{ApBeaconTxContext, ApBeaconTxOutcome},
    engine::{
        AP_EAPOL_CAPACITY, ApAfter, ApDue, ApEapol, ApEngine, ApEngineError, ApFrame, ApFrameKind,
        ApPeerDisconnectKind, ApReceived, ApSppScope as SppScope, ApTeardown, port_frame,
    },
};
use oer_protocols_ieee80211_core_roles_sta::data::WmmAccessCategory;
use oer_protocols_ieee80211_core_upper_mac::{
    ccmp::{CcmpPacketNumberStep, InstalledCcmpTxKey, PendingCcmpTxKey},
    receive::Integrity,
};

use network::ApNetworkEnd;

/// The tickets the runner holds at most.
pub const AP_TICKETS: usize = 8;

/// The transmit buffers the runner holds at most.
pub const AP_BUFFERS: usize = 8;

/// The peers whose pairwise keys the runner holds at most.
pub const AP_KEYS: usize = oer_protocols_ieee80211_core_roles_ap::AP_MAX_CLIENTS;

/// The received units one turn takes at most: the turn's bound.
const RECEIVE_BUDGET: usize = 16;

/// The network frames one turn carries at most.
const TRANSMIT_BUDGET: usize = 8;

/// The received units the runner holds at once: one at a time.
const RECEIVE_ROOM: u8 = 1;

/// The scratch a management response is written into before it is copied
/// into a port buffer.
const SCRATCH: usize = 512;

/// The Key ID of the group key the BSS announces.
pub const GROUP_KEY_ID: u8 = 1;

/// [`GROUP_KEY_ID`] as a KeyID, checked at compile time.
const GROUP_KEY: GroupKeyId = match GroupKeyId::new(GROUP_KEY_ID) {
    Some(key_id) => key_id,
    None => panic!("the group KeyID is one of 1 to 3"),
};

/// The types of the port `P`.
type Ty<'id, P> = <P as DataPort<'id>>::Types;
/// The capabilities of the port `P`.
type Caps<'id, P> = <P as DataPort<'id>>::Caps;
/// The access point's keys of the port `P`.
type Keys<'id, P> = <Ty<'id, P> as PortTypes<'id>>::AccessPointKeys;
/// A ticket of the port `P`.
type Ticket<'id, P> = oer_protocols_radio_port::Slot<
    'id,
    <Ty<'id, P> as PortTypes<'id>>::TicketKind,
    <Ty<'id, P> as PortTypes<'id>>::TicketPath,
>;
/// A transmit buffer of the class `C` of the port `P`.
type Buffer<'id, P, C> = <Ty<'id, P> as TxBufferPool<'id, C>>::Buffer;
/// A sent transmit buffer of the class `C` of the port `P`.
type Sent<'id, P, C> = <Ty<'id, P> as TxBufferPool<'id, C>>::Sent;
/// The named fault of the port `P`.
pub type PortFault<'id, P> = <P as DataPort<'id>>::Fault;
/// An access-point command of the port `P`.
type Command<'id, P> = AccessPointCommand<'id, Ty<'id, P>, Caps<'id, P>>;
/// The access point's configure permission on the port `P`.
type Configure<'id, P> = ConfigurePermission<
    'id,
    AccessPoint,
    <Ty<'id, P> as PortTypes<'id>>::AccessPointCommandPath,
    <Ty<'id, P> as PortTypes<'id>>::AccessPointConfigureContext,
>;
/// A pairwise installation in flight, by its peer.
type Installing<'id, P> = (
    [u8; 6],
    PendingCcmpTxKey<<Keys<'id, P> as KeyTypes<'id>>::Identity>,
);
/// An installed pairwise key and its packet numbers, by its peer.
type Installed<'id, P> = (
    [u8; 6],
    InstalledCcmpTxKey<<Keys<'id, P> as KeyTypes<'id>>::PairwiseKey>,
);
/// The beacon permission of the port `P`.
type Beacon<'id, P> = BeaconPermission<'id, <Ty<'id, P> as PortTypes<'id>>::BeaconPath>;

/// The group key the port installed, as the GTK's packet-number owner
/// names it for Message 3.
pub struct ApGroupKey<G> {
    key: G,
    installation: ApGroupKeyInstallation,
}

impl<G> ApGroupTransmitKey for ApGroupKey<G> {
    fn installation(&self) -> ApGroupKeyInstallation {
        self.installation
    }
}

/// The owners the runner starts with: one installation's tokens of the
/// access point, its share of the tickets and transmit buffers, and the
/// terminal access.
pub struct ApResources<'id, P: DataPort<'id>, C: TxBufferClass>
where
    Ty<'id, P>: TxBufferPool<'id, C>,
{
    /// The tickets.
    pub tickets: heapless::Vec<Ticket<'id, P>, AP_TICKETS>,
    /// The terminal access of the tickets.
    pub access: TerminalAccess<
        'id,
        <Ty<'id, P> as PortTypes<'id>>::TicketKind,
        <Ty<'id, P> as PortTypes<'id>>::TicketPath,
    >,
    /// The transmit buffers.
    pub buffers: heapless::Vec<Buffer<'id, P, C>, AP_BUFFERS>,
    /// The access point's configure permission, with its context.
    pub configure: Configure<'id, P>,
    /// Its association start.
    pub association: <Keys<'id, P> as KeyTypes<'id>>::AssociationStart,
    /// Its vacant pairwise slots.
    pub pairwise: heapless::Vec<<Keys<'id, P> as KeyTypes<'id>>::PairwiseSlot, AP_KEYS>,
    /// Its vacant group slot.
    pub group: <Keys<'id, P> as KeyTypes<'id>>::GroupSlot,
    /// Its group permission.
    pub group_permission: <Keys<'id, P> as KeyTypes<'id>>::GroupPermission,
    /// The installation's beacon permission.
    pub beacon: Beacon<'id, P>,
}

/// The access point's policy values: the composition's choice.
#[derive(Clone, Copy, Debug)]
pub struct ApPolicy {
    /// The rate of management frames, EAPOL and group data: the BSS's
    /// lowest basic rate.
    pub management_rate: PhyRate,
    /// The transmissions of an individually addressed management frame.
    pub management_transmissions: NonZeroU8,
    /// The transmissions of an individually addressed data MSDU.
    pub data_transmissions: NonZeroU8,
    /// The packet-number step of the access point's keys.
    pub packet_numbers: CcmpPacketNumberStep,
    /// The group key's generation the BSS announces in Message 3.
    pub group_generation: u32,
}

/// Fresh handshake values: a new ANonce for each association.
pub trait ApNonces {
    /// A fresh ANonce, or `None` once the source is exhausted.
    fn nonce(&mut self) -> Option<[u8; 32]>;
}

/// What the port's capabilities lack for the access point to serve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApUnsupported {
    /// The port declares no beacon Timestamp rule the runner writes
    /// ([`BeaconTimestamp::Caller`]): it would run without beacons.
    BeaconTimestamp,
}

/// Whether a port of the capabilities `K` serves the runner's beacons: it
/// declares the [`BeaconTimestamp::Caller`] rule. A port that declares only
/// [`BeaconTimestamp::Transmitter`] needs that rule's support in the runner
/// first.
///
/// # Errors
///
/// What `K` lacks.
pub const fn serves_beacons<K: AccessPointCapabilities>() -> Result<(), ApUnsupported> {
    match BeaconStamp::<K>::new(BeaconTimestamp::Caller) {
        Some(_) => Ok(()),
        None => Err(ApUnsupported::BeaconTimestamp),
    }
}

/// Why the runner ended.
#[derive(Debug)]
pub enum ApEnd<F> {
    /// The port's capabilities do not serve the access point: the run
    /// ended before it asked the radio to start, every owner in the
    /// runner.
    Unsupported(ApUnsupported),
    /// The access point stopped and the radio with it; every owner is back
    /// in the runner.
    Stopped,
    /// The radio did not start, for the port's cause; every owner is in
    /// the runner.
    StartRefused(oer_protocols_ieee80211_contracts_port::StartRefused<F>),
    /// The radio's stop is not confirmed yet, as the port observed it: the
    /// runner holds what the hardware may still use, and
    /// [`ApRunner::finish_stop`] watches the stop on until the port
    /// confirms it.
    StopUnconfirmed(StopUnconfirmed),
    /// The port reported its named fault; every owner is in the runner, a
    /// command the port gave back beside it among them
    /// ([`ApRunner::take_returned`]).
    Faulted(F),
    /// A terminal named an owner the runner did not send or one whose home
    /// was taken, refused whole and kept by the port, or the engine refused
    /// one of the runner's own steps; every other owner is in the runner,
    /// the ones without a place among its kept ([`ApRunner::take_kept`]).
    Contract(ApContract),
    /// The port refused one of the access point's commands for its
    /// refusal; every owner is in the runner, the refused command among
    /// them ([`ApRunner::take_returned`]).
    Refused(AccessPointRefusal),
    /// The port refused a received unit's recycle for its count: the unit
    /// is in the runner ([`ApRunner::take_unrecycled`]), every other owner
    /// in its place. The runner returns no unit more, its stop
    /// ([`ApRunner::finish_stop`]) included.
    Unrecycled(ReceiveAccounting),
    /// A port call returned an item it could neither give the runner nor
    /// keep: the item is in the runner ([`ApRunner::take_unkept`]), every
    /// other owner in its place. The runner makes no port call that takes
    /// owners while it holds it, its stop's watch included: a stop not yet
    /// confirmed stays unconfirmed, the owners with the runner.
    Unkept,
}

/// Why a step of the runner ended the run.
enum Halt<'id, P, C, F>
where
    P: AccessPointPort<'id>,
    C: TxBufferClass,
    Ty<'id, P>: TxBufferPool<'id, C>,
{
    Faulted(F),
    Contract(ApContract),
    /// The step found no home for an owner of its operation: it stopped
    /// there, with every owner of the operation it still held, whole.
    Kept(ApContract, Kept<'id, P, C>),
    /// The port refused a unit's recycle, which the runner holds
    /// ([`ApRunner::take_unrecycled`]).
    Unrecycled(ReceiveAccounting),
    /// The port refused a command, which the runner holds
    /// ([`ApRunner::take_returned`]).
    Refused(AccessPointRefusal),
    /// A port call returned an item it could not keep, which the runner
    /// holds ([`ApRunner::take_unkept`]).
    Unkept,
}

impl<'id, P, C, F> From<Stray<'id, P, C>> for Halt<'id, P, C, F>
where
    P: AccessPointPort<'id>,
    C: TxBufferClass,
    Ty<'id, P>: TxBufferPool<'id, C>,
{
    fn from((contract, kept): Stray<'id, P, C>) -> Self {
        Self::Kept(contract, kept)
    }
}

/// The owners of one operation that a step of the runner stopped on,
/// whole: every owner the operation still held when one of them found no
/// home of the runner's. The step stops there; what the operation had
/// already put home stays home. A refused submission's owners the runner
/// cannot use, a buffer whose memory the port still holds, or a terminal's
/// owners whose homes [`Homes::admit`] found free and a home then refused
/// (excluded: nothing runs between the check and the take). A terminal
/// whose owners have no home is refused whole, and the port keeps it.
///
/// The first one ends the run ([`ApEnd::Contract`]): a step returns it in
/// its halt, a terminal's in the port's call leaves it in the homes, which
/// then refuse every terminal whole until it is taken. So a run returns
/// two at most, one of each, and no new run starts while one is out
/// ([`ApRunner::take_kept`]).
pub enum Kept<'id, P, C>
where
    P: AccessPointPort<'id>,
    C: TxBufferClass,
    Ty<'id, P>: TxBufferPool<'id, C>,
{
    /// A single MPDU's completion, taken apart: its ticket found no place;
    /// its buffer as the port returned it.
    Completion(Ticket<'id, P>, <Ty<'id, P> as PortTypes<'id>>::AnyTxBuffer),
    /// A completion's buffer of no class of the runner's; its ticket is
    /// home.
    AnyBuffer(<Ty<'id, P> as PortTypes<'id>>::AnyTxBuffer),
    /// A buffer without a place, or whose memory the port still holds.
    Buffer(Buffer<'id, P, C>),
    /// A refused submission whose ticket found no place, with the owner its
    /// source returned.
    Submission(Ticket<'id, P>, Source<'id, P, C>),
    /// A refused fresh submission's source that came back a retransmission;
    /// its ticket is home.
    Sent(<Ty<'id, P> as TxBufferPool<'id, C>>::Sent),
    /// A pairwise removal's slot without a place, with its association.
    PairwiseRemoved(
        <Keys<'id, P> as KeyTypes<'id>>::PairwiseSlot,
        <Keys<'id, P> as KeyTypes<'id>>::Association,
    ),
    /// The group installation's key without a pending installation, with
    /// its permission.
    GroupInstalled(
        <Keys<'id, P> as KeyTypes<'id>>::GroupKey,
        <Keys<'id, P> as KeyTypes<'id>>::GroupPermission,
    ),
    /// The group key the pending installation refused, both returned, with
    /// the permission.
    ForeignGroup(
        oer_protocols_ieee80211_core_upper_mac::ccmp::ForeignCcmpTxKey<
            <Keys<'id, P> as KeyTypes<'id>>::GroupSlotIdentity,
            <Keys<'id, P> as KeyTypes<'id>>::GroupKey,
        >,
        <Keys<'id, P> as KeyTypes<'id>>::GroupPermission,
    ),
    /// A pairwise slot without a place.
    PairwiseSlot(<Keys<'id, P> as KeyTypes<'id>>::PairwiseSlot),
    /// A pairwise key no installation named, or whose removal found no
    /// place.
    PairwiseKey(<Keys<'id, P> as KeyTypes<'id>>::PairwiseKey),
    /// The pairwise key the peer's pending installation refused: both, as
    /// the installation returned them.
    ForeignPairwise(
        [u8; 6],
        oer_protocols_ieee80211_core_upper_mac::ccmp::ForeignCcmpTxKey<
            <Keys<'id, P> as KeyTypes<'id>>::Identity,
            <Keys<'id, P> as KeyTypes<'id>>::PairwiseKey,
        >,
    ),
    /// An installed pairwise key, with its peer, without a place among the
    /// keys.
    Installed(Installed<'id, P>),
}

/// The owner a refused submission's source returns.
pub enum Source<'id, P, C>
where
    P: AccessPointPort<'id>,
    C: TxBufferClass,
    Ty<'id, P>: TxBufferPool<'id, C>,
{
    /// The fresh buffer it was given.
    Fresh(Buffer<'id, P, C>),
    /// The port's retransmission, which a fresh source never becomes.
    Sent(<Ty<'id, P> as TxBufferPool<'id, C>>::Sent),
}

/// A terminal of the access point whose owners' homes
/// [`Homes::admit`] found free.
enum Admitted<'id, P>
where
    P: AccessPointPort<'id>,
{
    Single(oer_protocols_ieee80211_contracts_port::SingleCompleted<'id, Ty<'id, P>>),
    Beacon(
        oer_protocols_ieee80211_contracts_port::SingleCompleted<'id, Ty<'id, P>>,
        Beacon<'id, P>,
    ),
    Configured(Configure<'id, P>),
    GroupInstalled(
        <Keys<'id, P> as KeyTypes<'id>>::GroupKey,
        <Keys<'id, P> as KeyTypes<'id>>::GroupPermission,
    ),
    PairwiseInstalled(<Keys<'id, P> as KeyTypes<'id>>::PairwiseKey),
    PairwiseRemoved(
        <Keys<'id, P> as KeyTypes<'id>>::PairwiseSlot,
        <Keys<'id, P> as KeyTypes<'id>>::Association,
    ),
    GroupRemoved(
        <Keys<'id, P> as KeyTypes<'id>>::GroupSlot,
        <Keys<'id, P> as KeyTypes<'id>>::GroupPermission,
    ),
}

/// What the runner found broken of its own contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApContract {
    /// A terminal of a ticket that carried nothing of the runner's.
    UnknownTicket(u8),
    /// A buffer of another class came back.
    BufferClass,
    /// A key terminal named a key the runner did not install.
    ForeignKey,
    /// The engine refused a step of the runner's own.
    Engine(ApEngineError),
    /// The runner had no owner a command of its own needs.
    CommandRefused,
    /// A terminal returned a single owner whose home was not empty.
    Permission,
    /// A receive cut handed over more units in one call than the runner
    /// reads after it.
    CutUnits,
}

/// The access point's state, as the application reads it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ApStatus {
    /// The access point beacons.
    pub running: bool,
    /// Its authorized peers.
    pub authorized: u8,
    /// Its associated peers, authorized or not.
    pub associated: u8,
    /// Beacons that ended delivered.
    pub beacons: u32,
    /// Beacons that ended expired, not published by their deadline.
    pub beacons_expired: u32,
    /// Steps the engine refused for a peer's input or a peer that left: a
    /// handshake's transmission end, a teardown of a peer already gone.
    pub refused: u32,
    /// Received MSDUs the network queue had no room for.
    pub rx_dropped: u32,
}

/// What the application and the runner share: the stop request and the
/// status.
pub struct ApShared {
    stop: core::cell::Cell<bool>,
    stop_waker: oer_portable_sync::PriorityWaker,
    status: core::cell::Cell<ApStatus>,
    status_waker: oer_portable_sync::PriorityWaker,
}

impl ApShared {
    pub const fn new() -> Self {
        Self {
            stop: core::cell::Cell::new(false),
            stop_waker: oer_portable_sync::PriorityWaker::new(),
            status: core::cell::Cell::new(ApStatus {
                running: false,
                authorized: 0,
                associated: 0,
                beacons: 0,
                beacons_expired: 0,
                refused: 0,
                rx_dropped: 0,
            }),
            status_waker: oer_portable_sync::PriorityWaker::new(),
        }
    }

    /// Ask the runner to stop the access point.
    pub fn request_stop(&self) {
        self.stop.set(true);
        self.stop_waker.wake();
    }

    /// The access point's status now.
    pub fn status(&self) -> ApStatus {
        self.status.get()
    }

    /// Wait for a status other than `seen`.
    pub async fn changed(&self, seen: ApStatus) -> ApStatus {
        poll_fn(|cx| {
            self.status_waker.register(cx.waker());
            let status = self.status.get();
            if status == seen {
                Poll::Pending
            } else {
                Poll::Ready(status)
            }
        })
        .await
    }

    /// Wait until the application asks the access point to stop.
    pub async fn stop_requested(&self) {
        poll_fn(|cx| self.poll_stop(cx)).await;
    }

    fn poll_stop(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.stop_waker.register(cx.waker());
        if self.stop.get() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    fn publish(&self, status: ApStatus) {
        if self.status.get() != status {
            self.status.set(status);
            self.status_waker.wake();
        }
    }
}

impl Default for ApShared {
    fn default() -> Self {
        Self::new()
    }
}

/// What a transmission was, for its terminal.
enum Purpose {
    /// A management response, then `after`.
    Response(ApAfter),
    /// An EAPOL-Key frame of a handshake.
    Eapol(ApEapolTxContext),
    /// A step of a peer's teardown.
    Teardown(ApTeardown, ApPeerDisconnectKind),
    /// A data MSDU.
    Data,
    /// The beacon.
    Beacon(ApBeaconTxContext),
}

/// The idle turns in a row after which the runner waits only for a source
/// other than the deadlines it found due and the facts it just served.
pub(crate) const IDLE_REPEATS: u8 = 4;

/// The facts of both polls.
fn union(held: Facts, taken: Facts) -> Facts {
    Facts {
        tx: match (held.tx, taken.tx) {
            (Some(held), Some(taken)) => Some(held.union(taken)),
            (held, taken) => held.or(taken),
        },
        tbtt: held.tbtt || taken.tbtt,
        rx: held.rx || taken.rx,
    }
}

/// What a turn of the runner can change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Mark {
    phase: Phase,
    status: ApStatus,
    owners: [usize; 8],
    network: bool,
    /// Units, terminals and commands the runner took from the port.
    taken: u32,
}

/// What the runner is doing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// The receive configuration waits for its terminal.
    Configuring,
    /// The group key's installation waits for its terminal.
    InstallingGroup,
    /// The TSF restart waits for its terminal.
    StartingTsf,
    /// The access point serves its BSS.
    Running,
    /// The stop tears the peers down.
    TearingDown,
    /// The stop closed the receive policy, waiting for its terminal.
    ClosingReceive,
    /// The stop removes the group key.
    RemovingGroup,
    /// The stop stops the TSF.
    StoppingTsf,
    /// The stop stops the radio.
    StoppingRadio,
}

/// The units of a receive cut one port call hands over at most.
const CUT_UNITS: usize = 16;

/// A unit a receive cut of the port `P` handed over, with its metadata.
type CutUnit<'id, P> = (
    <Ty<'id, P> as PortTypes<'id>>::RxBuffer,
    <Ty<'id, P> as PortTypes<'id>>::RxMeta,
);

/// A Message 3 held until its peer's pairwise key is installed.
struct HeldMessage3 {
    peer: [u8; 6],
    message3: ApEapolTransmit<AP_EAPOL_CAPACITY>,
    /// The key is in: it goes once the port's call returned.
    ready: bool,
}

/// Every owner of the run but the port and its command issuer, each in its
/// fixed home: the port's sink. A terminal returns its owners into the
/// homes they left at once, during the port's call; what it asks of the
/// port (a frame to send, a command to issue) waits here as plain data
/// until the call returned. A command is built from the homes' owners only
/// when it is issued.
struct Homes<'id, 's, 'n, P, C, S, X>
where
    P: AccessPointPort<'id>,
    C: TxBufferClass,
    Ty<'id, P>: TxBufferPool<'id, C>,
    S: SppScope,
{
    engine: ApEngine<'s, S>,
    policy: ApPolicy,
    channel: SupportedChannel<Caps<'id, P>>,
    network: ApNetworkEnd<'n>,
    context: <Ty<'id, P> as PortTypes<'id>>::Context,
    nonces: X,
    tickets: heapless::Vec<Ticket<'id, P>, AP_TICKETS>,
    access: TerminalAccess<
        'id,
        <Ty<'id, P> as PortTypes<'id>>::TicketKind,
        <Ty<'id, P> as PortTypes<'id>>::TicketPath,
    >,
    buffers: heapless::Vec<Buffer<'id, P, C>, AP_BUFFERS>,
    purposes: [Option<Purpose>; 32],
    configure: Option<Configure<'id, P>>,
    /// The access point's association, home while no pairwise removal
    /// holds it.
    association: Option<<Keys<'id, P> as KeyTypes<'id>>::Association>,
    pairwise: heapless::Vec<<Keys<'id, P> as KeyTypes<'id>>::PairwiseSlot, AP_KEYS>,
    /// Peers whose pairwise installation waits for the association.
    installs: heapless::Deque<[u8; 6], AP_KEYS>,
    /// Message 3 of each peer whose pairwise key is going in: it goes once
    /// the key is installed.
    held_message3: heapless::Vec<HeldMessage3, AP_KEYS>,
    /// Installed pairwise keys waiting for their removal.
    removals: heapless::Deque<<Keys<'id, P> as KeyTypes<'id>>::PairwiseKey, AP_KEYS>,
    /// The ANonce the next association takes; none once the source ran
    /// out, and then no association is admitted.
    nonce: Option<[u8; 32]>,
    group_slot: Option<<Keys<'id, P> as KeyTypes<'id>>::GroupSlot>,
    group_permission: Option<<Keys<'id, P> as KeyTypes<'id>>::GroupPermission>,
    group_key: [u8; 16],
    group_pending: Option<PendingCcmpTxKey<<Keys<'id, P> as KeyTypes<'id>>::GroupSlotIdentity>>,
    group: Option<InstalledCcmpTxKey<ApGroupKey<<Keys<'id, P> as KeyTypes<'id>>::GroupKey>>>,
    installing: heapless::Vec<Installing<'id, P>, AP_KEYS>,
    keys: heapless::Vec<Installed<'id, P>, AP_KEYS>,
    /// Teardowns waiting for a ticket, in order.
    teardowns: heapless::Deque<(ApTeardown, ApPeerDisconnectKind), AP_KEYS>,
    beacon: Option<Beacon<'id, P>>,
    phase: Phase,
    status: ApStatus,
    scratch: [u8; SCRATCH],
    stop: bool,
    /// The command the port holds at its barrier: the next issue runs it
    /// again before any other.
    held: Option<Command<'id, P>>,
    /// The command the port gave back unrun: refused, or beside its
    /// fault. The runner never issues it again.
    returned: Option<Command<'id, P>>,
    /// The owners of a terminal taken apart that found no home of the
    /// runner's, written once: while it holds them, every terminal is
    /// refused whole.
    kept: Option<Stray<'id, P, C>>,
    /// The owners a step stopped on ([`Halt::Kept`]), written once at the
    /// run's end: no run starts while it holds them.
    stopped_on: Option<Stray<'id, P, C>>,
    /// The unit whose recycle the port refused. The first refusal ends the
    /// run and the runner returns no unit after it, so it is the only one.
    unrecycled: Option<RecycleRefused<<Ty<'id, P> as PortTypes<'id>>::RxBuffer>>,
    /// The item a port call could neither give the homes nor keep. The
    /// runner makes no port call that takes owners while it holds it, its
    /// stop's watch included, so it is the only one.
    unkept: Option<oer_protocols_ieee80211_contracts_port::Unkept<'id, Ty<'id, P>>>,
    /// The attempt a submission returned unaccepted beside the unkept item.
    unaccepted: Option<<P as DataPort<'id>>::Admitted>,
    /// The first break of the runner's contract a terminal showed: the run
    /// ends with it once the port's call returned.
    broken: Option<ApContract>,
    /// Peers whose association response was delivered: their handshake
    /// begins once the port's call returned.
    begins: heapless::Deque<[u8; 6], AP_TICKETS>,
    /// Units a receive cut handed over: read and recycled once the port's
    /// call returned, before anything else.
    cut: heapless::Deque<CutUnit<'id, P>, CUT_UNITS>,
    /// The units a receive cut handed over: a cut that moves is progress.
    cut_units: u32,
    /// The instant of the port's call the terminals belong to.
    now: Instant,
    /// The run ended on a port fault: the stop takes it back first.
    faulted: bool,
    /// Units, terminals and commands taken from the port, counted for the
    /// progress of a turn.
    taken: u32,
}

impl<'id, 's, 'n, P, C, S, X> PortSink<'id, Ty<'id, P>> for Homes<'id, 's, 'n, P, C, S, X>
where
    P: AccessPointPort<'id>,
    C: TxBufferClass,
    Ty<'id, P>: TxBufferPool<'id, C>,
    S: SppScope,
    <Keys<'id, P> as KeyTypes<'id>>::GroupSlotIdentity: PartialEq,
    <Keys<'id, P> as KeyTypes<'id>>::Identity: PartialEq,
{
    fn terminal(
        &mut self,
        terminal: LowerMacTerminal<'id, Ty<'id, P>>,
    ) -> Result<(), LowerMacTerminal<'id, Ty<'id, P>>> {
        // Owners of a terminal taken apart wait without a home: every
        // terminal after them stays with the port, whole.
        if self.kept.is_some() {
            return Err(terminal);
        }
        // Every home the terminal's owners go to is checked before any is
        // taken: a terminal with a home taken, or with an owner the runner
        // never lent, is refused whole. The port keeps it, and the run ends
        // on the break.
        let admitted = self.admit(terminal).map_err(|(terminal, contract)| {
            self.broken.get_or_insert(contract);
            terminal
        })?;
        self.taken = self.taken.wrapping_add(1);
        if let Err((contract, kept)) = self.terminal_home(admitted) {
            // The terminal's operation stopped on an owner without a home;
            // the place was empty, checked above.
            self.broken.get_or_insert(contract);
            self.kept = Some((contract, kept));
        }
        Ok(())
    }
    fn tbtt(&mut self, _: TbttEvent) {}
    fn received(
        &mut self,
        frame: <Ty<'id, P> as PortTypes<'id>>::RxBuffer,
        meta: <Ty<'id, P> as PortTypes<'id>>::RxMeta,
    ) -> Result<
        (),
        (
            <Ty<'id, P> as PortTypes<'id>>::RxBuffer,
            <Ty<'id, P> as PortTypes<'id>>::RxMeta,
        ),
    > {
        // A receive cut's unit precedes the mutation's terminal: the runner
        // reads and recycles it as soon as the port's call returned. More
        // than the runner stages is its contract broken: the run ends, and
        // the port keeps the unit.
        self.cut.push_back((frame, meta)).inspect_err(|_| {
            self.broken.get_or_insert(ApContract::CutUnits);
        })?;
        self.cut_units = self.cut_units.wrapping_add(1);
        Ok(())
    }
}

/// The owner a refused source returns: the fresh buffer it was given, or
/// the port's retransmission, which a fresh source never becomes.
fn returned<'id, P, C, F, O, K, R>(
    source: TxSource<Buffer<'id, P, C>, Sent<'id, P, C>, F, O, K, R>,
) -> Source<'id, P, C>
where
    P: AccessPointPort<'id>,
    C: TxBufferClass,
    Ty<'id, P>: TxBufferPool<'id, C>,
{
    match source {
        TxSource::Fresh { buffer, .. } => Source::Fresh(buffer),
        TxSource::Retransmission { mpdu, .. } => Source::Sent(mpdu),
    }
}

/// The owners an operation stopped on, with the break they show.
type Stray<'id, P, C> = (ApContract, Kept<'id, P, C>);

/// The integrity evidence of a received frame's metadata.
fn integrity<M: RxMetadata>(meta: &M) -> Integrity {
    match meta.common().crypto {
        RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified)
        | RxEvidence::ProtocolValidated(RxCryptoStatus::DecryptedAndIntegrityVerified) => {
            Integrity::DecryptedAndVerified
        }
        _ => Integrity::Unverified,
    }
}

/// The access point's runner over the port `P`, its command issuer `I` and
/// the buffer class `C`.
pub struct ApRunner<'id, 's, 'n, P, I, C, S, X>
where
    P: AccessPointPort<'id>,
    I: AccessPointCommands<'id, Types = Ty<'id, P>, Caps = Caps<'id, P>>,
    C: TxBufferClass,
    Ty<'id, P>: TxBufferPool<'id, C>,
    S: SppScope,
{
    port: P,
    commands: I,
    homes: Homes<'id, 's, 'n, P, C, S, X>,
}

/// What woke a wait of the runner.
enum Woke {
    /// The port's facts.
    Facts(Facts),
    /// The radio's stop ended, confirmed or not.
    Stopped(Result<(), StopUnconfirmed>),
    /// A deadline, the network, the stop request or the yield.
    Other,
}

impl<'id, 's, 'n, P, C, S, X> Homes<'id, 's, 'n, P, C, S, X>
where
    P: AccessPointPort<'id>,
    C: TxBufferClass,
    Ty<'id, P>: TxBufferPool<'id, C>,
    S: SppScope,
    <Keys<'id, P> as KeyTypes<'id>>::GroupSlotIdentity: PartialEq,
    <Keys<'id, P> as KeyTypes<'id>>::Identity: PartialEq,
{
    /// What a turn can change: equal marks before and after a turn mean
    /// it made no progress.
    fn mark(&self) -> Mark {
        Mark {
            phase: self.phase,
            status: self.status,
            owners: [
                self.tickets.len(),
                self.buffers.len(),
                usize::from(self.held.is_some()),
                self.teardowns.len(),
                self.installs.len(),
                self.installing.len(),
                self.keys.len(),
                self.removals.len(),
            ],
            network: self.network.waiting(),
            taken: self.taken.wrapping_add(self.cut_units),
        }
    }

    /// The engine's next work, while the access point serves.
    fn engine_deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Running => self.engine.deadline(),
            _ => None,
        }
    }

    /// Count a step the engine refused.
    fn refuse(&mut self) {
        self.status.refused = self.status.refused.saturating_add(1);
    }

    /// Stop: tear every peer down, then close the interface.
    fn begin_stop(&mut self) -> Result<(), Stray<'id, P, C>> {
        self.phase = Phase::TearingDown;
        self.status.running = false;
        self.network.set_link(false);
        while let Some(teardown) = self.engine.stopping_peer() {
            self.teardown(teardown)?;
        }
        self.advance_stop();
        Ok(())
    }

    /// Queue a teardown of `peer`.
    fn teardown(&mut self, teardown: ApTeardown) -> Result<(), Stray<'id, P, C>> {
        let kind = if teardown.associated {
            ApPeerDisconnectKind::Disassociation
        } else {
            ApPeerDisconnectKind::Deauthentication
        };
        // A full queue keeps the earlier teardowns; this peer's key and
        // state end as the earlier ones do, without its frames.
        if self.teardowns.push_back((teardown, kind)).is_err() {
            self.finish_teardown(teardown.peer)?;
        }
        Ok(())
    }

    /// Move the stop on: once every peer is gone and its key off, close the
    /// receive policy.
    fn advance_stop(&mut self) {
        if self.phase != Phase::TearingDown {
            return;
        }
        let teardown_out = self
            .purposes
            .iter()
            .any(|purpose| matches!(purpose, Some(Purpose::Teardown(..))));
        if !self.teardowns.is_empty()
            || teardown_out
            || !self.installing.is_empty()
            || !self.keys.is_empty()
            || !self.removals.is_empty()
            || !self.installs.is_empty()
            || self.association.is_none()
        {
            return;
        }
        self.phase = Phase::ClosingReceive;
    }

    /// The next command the phase and the homes' owners make, built from
    /// those owners now; none while its owners are out. A teardown on the
    /// way that stops on an owner without a home stops it.
    fn next_command<I>(
        &mut self,
        commands: &mut I,
    ) -> Result<Option<Command<'id, P>>, Stray<'id, P, C>>
    where
        I: AccessPointCommands<'id, Types = Ty<'id, P>, Caps = Caps<'id, P>>,
    {
        if matches!(self.phase, Phase::Running | Phase::TearingDown) {
            return self.key_command(commands);
        }
        Ok(match self.phase {
            Phase::Configuring => self.configure_interface(Some(ReceiveFilter::BSS_MEMBER), None),
            Phase::InstallingGroup => self.install_group(commands),
            Phase::StartingTsf => {
                self.configure_interface(Some(ReceiveFilter::BSS_MEMBER), Some(TsfControl::Restart))
            }
            Phase::ClosingReceive => self.configure_interface(None, None),
            Phase::RemovingGroup => self.remove_group(),
            Phase::StoppingTsf => self.configure_interface(None, Some(TsfControl::Stop)),
            Phase::Running | Phase::TearingDown | Phase::StoppingRadio => None,
        })
    }

    /// A configuration of the access point's interface: its BSS's receive
    /// rules with `filter`, or none, and its TSF control.
    fn configure_interface(
        &mut self,
        filter: Option<ReceiveFilter>,
        tsf: Option<TsfControl>,
    ) -> Option<Command<'id, P>> {
        self.configure.as_ref()?;
        let receive = match filter {
            Some(filter) => match SupportedFilter::new(filter) {
                Some(filter) => AccessPointReceive::Bss { filter },
                None => {
                    self.broken.get_or_insert(ApContract::CommandRefused);
                    return None;
                }
            },
            None => AccessPointReceive::Off,
        };
        let tsf = match tsf {
            Some(control) => match SupportedTsfControl::new(control) {
                Some(control) => Some(control),
                None => {
                    self.broken.get_or_insert(ApContract::CommandRefused);
                    return None;
                }
            },
            None => None,
        };
        let permission = self.configure.take()?;
        Some(AccessPointCommand::Configure {
            permission,
            receive,
            tsf,
        })
    }

    /// The group key's installation, once.
    fn install_group<I>(&mut self, commands: &mut I) -> Option<Command<'id, P>>
    where
        I: AccessPointCommands<'id, Types = Ty<'id, P>, Caps = Caps<'id, P>>,
    {
        if self.group_pending.is_some() || self.group.is_some() {
            return None;
        }
        let key_id = GROUP_KEY;
        let (Some(association), Some(_), Some(_)) = (
            self.association.as_ref(),
            self.group_slot.as_ref(),
            self.group_permission.as_ref(),
        ) else {
            return None;
        };
        let (slot, permission) = (self.group_slot.take()?, self.group_permission.take()?);
        self.group_pending = Some(PendingCcmpTxKey::issued(
            <Keys<'id, P> as KeyTypes<'id>>::vacant_group_slot_identity(&slot),
            self.policy.packet_numbers,
        ));
        Some(commands.install_access_point_group(
            association,
            permission,
            slot,
            key_id,
            oer_protocols_ieee80211_contracts_security::Ccmp128Key::new(&self.group_key),
        ))
    }

    /// The group key's removal, or, with no group key in, the TSF's stop.
    fn remove_group(&mut self) -> Option<Command<'id, P>> {
        if self.group.is_none() {
            // No group key went in: the TSF stops at once.
            self.phase = Phase::StoppingTsf;
            return self.configure_interface(None, Some(TsfControl::Stop));
        }
        self.group_permission.as_ref()?;
        let (group, permission) = (self.group.take()?, self.group_permission.take()?);
        Some(AccessPointCommand::RemoveGroup {
            permission,
            key: group.into_key().key,
        })
    }

    /// A pairwise installation the association allows while it is home, or
    /// one removal, which holds it until its terminal.
    fn key_command<I>(
        &mut self,
        commands: &mut I,
    ) -> Result<Option<Command<'id, P>>, Stray<'id, P, C>>
    where
        I: AccessPointCommands<'id, Types = Ty<'id, P>, Caps = Caps<'id, P>>,
    {
        while self.association.is_some()
            && !self.installing.is_full()
            && let Some(peer) = self.installs.pop_front()
        {
            let Ok((key, peer_address)) = self.engine.pairwise_key(peer) else {
                // The peer left meanwhile.
                continue;
            };
            let Some(slot) = self.pairwise.pop() else {
                // No slot is vacant: the peer cannot be authorized.
                if let Ok(teardown) = self.engine.fail_handshake(peer) {
                    self.teardown(teardown)?;
                }
                continue;
            };
            let Some(association) = self.association.as_ref() else {
                // Excluded by the loop's condition: the slot goes back to the
                // place it just left.
                return self
                    .pairwise
                    .push(slot)
                    .map(|()| None)
                    .map_err(|slot| (ApContract::Permission, Kept::PairwiseSlot(slot)));
            };
            let (command, identity) =
                commands.install_access_point_pairwise(association, slot, peer_address, key);
            // Its place was checked: `installing` holds a slot's
            // installation for each of its AP_KEYS places.
            if self
                .installing
                .push((
                    peer,
                    PendingCcmpTxKey::issued(identity, self.policy.packet_numbers),
                ))
                .is_err()
            {
                self.broken.get_or_insert(ApContract::ForeignKey);
            }
            return Ok(Some(command));
        }
        if !self.removals.is_empty()
            && let Some(association) = self.association.take()
        {
            match self.removals.pop_front() {
                Some(key) => {
                    return Ok(Some(AccessPointCommand::RemovePairwise {
                        association,
                        key,
                    }));
                }
                // Excluded by the check above: the association goes back to
                // the place it just left.
                None => self.association = Some(association),
            }
        }
        Ok(None)
    }

    /// `terminal`, once every home of its owners is free and each owner is
    /// one the runner lent, read by borrow before anything is taken; refused,
    /// the terminal whole with the break it shows.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "a refusal returns the terminal whole, by value"
    )]
    fn admit(
        &self,
        terminal: LowerMacTerminal<'id, Ty<'id, P>>,
    ) -> Result<Admitted<'id, P>, (LowerMacTerminal<'id, Ty<'id, P>>, ApContract)> {
        let refusal = match &terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) => {
                self.admits_completed(completed)
            }
            LowerMacTerminal::TxCompleted(TxCompleted::Beacon { completed, .. }) => {
                if self.beacon.is_some() {
                    Err(ApContract::Permission)
                } else {
                    self.admits_completed(completed)
                }
            }
            LowerMacTerminal::Configured(Configured::AccessPoint(_)) => match self.configure {
                None => Ok(()),
                Some(_) => Err(ApContract::Permission),
            },
            LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint { key, .. }) => {
                match &self.group_pending {
                    _ if self.group_permission.is_some() => Err(ApContract::Permission),
                    Some(pending) if pending.names_group::<Keys<'id, P>>(key) => Ok(()),
                    _ => Err(ApContract::ForeignKey),
                }
            }
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(key)) => {
                // Its key goes to the installed keys, or, its peer gone, to
                // the removals: both places are checked.
                if self
                    .installing
                    .iter()
                    .any(|(_, pending)| pending.names_pairwise::<Keys<'id, P>>(key))
                    && !self.keys.is_full()
                    && !self.removals.is_full()
                {
                    Ok(())
                } else {
                    Err(ApContract::ForeignKey)
                }
            }
            LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Pairwise { .. })) => {
                if self.association.is_some() {
                    Err(ApContract::Permission)
                } else if self.pairwise.is_full() {
                    Err(ApContract::ForeignKey)
                } else {
                    Ok(())
                }
            }
            LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group { .. })) => {
                if self.group_slot.is_some() || self.group_permission.is_some() {
                    Err(ApContract::Permission)
                } else {
                    Ok(())
                }
            }
            // The station's terminals never come to the access point's
            // runner: no station command goes.
            _ => Err(ApContract::UnknownTicket(u8::MAX)),
        };
        if let Err(contract) = refusal {
            return Err((terminal, contract));
        }
        match terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) => {
                Ok(Admitted::Single(completed))
            }
            LowerMacTerminal::TxCompleted(TxCompleted::Beacon {
                completed,
                permission,
            }) => Ok(Admitted::Beacon(completed, permission)),
            LowerMacTerminal::Configured(Configured::AccessPoint(permission)) => {
                Ok(Admitted::Configured(permission))
            }
            LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint {
                key,
                permission,
            }) => Ok(Admitted::GroupInstalled(key, permission)),
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(key)) => {
                Ok(Admitted::PairwiseInstalled(key))
            }
            LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Pairwise {
                slot,
                association,
            })) => Ok(Admitted::PairwiseRemoved(slot, association)),
            LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
                slot,
                permission,
            })) => Ok(Admitted::GroupRemoved(slot, permission)),
            other => Err((other, ApContract::UnknownTicket(u8::MAX))),
        }
    }

    /// Whether a single MPDU's owners have their homes: its ticket names a
    /// purpose the runner gave it, a ticket's place is free, and its buffer
    /// comes back of the runner's class with a buffer's place free.
    fn admits_completed(
        &self,
        completed: &oer_protocols_ieee80211_contracts_port::SingleCompleted<'id, Ty<'id, P>>,
    ) -> Result<(), ApContract> {
        let index = completed.ticket_index();
        if self
            .purposes
            .get(usize::from(index))
            .is_none_or(Option::is_none)
        {
            return Err(ApContract::UnknownTicket(index));
        }
        if self.tickets.is_full() {
            return Err(ApContract::UnknownTicket(index));
        }
        if <Ty<'id, P> as TxBufferPool<'id, C>>::completed_class(completed).is_none()
            || self.buffers.is_full()
        {
            return Err(ApContract::BufferClass);
        }
        Ok(())
    }

    /// One terminal [`Self::admit`] admitted: its owners go home, each to
    /// the home `admit` found free. A home that refuses one stops the
    /// terminal's operation there, with every owner it still holds.
    fn terminal_home(&mut self, terminal: Admitted<'id, P>) -> Result<(), Stray<'id, P, C>> {
        match terminal {
            Admitted::Single(completed) => self.completed(completed),
            Admitted::Beacon(completed, permission) => {
                // `admit` found the permission's home empty.
                self.beacon = Some(permission);
                self.completed(completed)
            }
            Admitted::Configured(permission) => {
                // `admit` found the permission's home empty.
                self.configure = Some(permission);
                self.configured();
                Ok(())
            }
            Admitted::GroupInstalled(key, permission) => self.group_installed(key, permission),
            Admitted::PairwiseInstalled(key) => self.pairwise_installed(key),
            Admitted::PairwiseRemoved(slot, association) => {
                if let Err(slot) = self.pairwise.push(slot) {
                    // Excluded by `admit`: the place was free.
                    return Err((
                        ApContract::ForeignKey,
                        Kept::PairwiseRemoved(slot, association),
                    ));
                }
                // `admit` found the association's home empty.
                self.association = Some(association);
                self.advance_stop();
                Ok(())
            }
            Admitted::GroupRemoved(slot, permission) => {
                // `admit` found both homes empty.
                self.group_slot = Some(slot);
                self.group_permission = Some(permission);
                if self.phase == Phase::RemovingGroup {
                    self.phase = Phase::StoppingTsf;
                }
                Ok(())
            }
        }
    }

    /// The group key is installed: the TSF restarts last. [`Self::admit`]
    /// found it the pending installation's and the permission's home empty.
    fn group_installed(
        &mut self,
        key: <Keys<'id, P> as KeyTypes<'id>>::GroupKey,
        permission: <Keys<'id, P> as KeyTypes<'id>>::GroupPermission,
    ) -> Result<(), Stray<'id, P, C>> {
        let Some(pending) = self.group_pending.take() else {
            // Excluded by `admit`: the installation was pending.
            return Err((
                ApContract::ForeignKey,
                Kept::GroupInstalled(key, permission),
            ));
        };
        let installation = ApGroupKeyInstallation {
            key_id: GROUP_KEY,
            generation: self.policy.group_generation,
        };
        match pending
            .installed_group::<Keys<'id, P>, _>(key, |key| ApGroupKey { key, installation })
        {
            Ok(installed) => {
                self.group_permission = Some(permission);
                self.group = Some(installed);
                self.phase = Phase::StartingTsf;
                Ok(())
            }
            // Excluded by `admit`: the key is the pending installation's.
            Err(foreign) => Err((
                ApContract::ForeignKey,
                Kept::ForeignGroup(foreign, permission),
            )),
        }
    }

    /// A peer's pairwise key is installed: its Message 3 goes once the
    /// port's call returned. [`Self::admit`] found it one installation's.
    fn pairwise_installed(
        &mut self,
        key: <Keys<'id, P> as KeyTypes<'id>>::PairwiseKey,
    ) -> Result<(), Stray<'id, P, C>> {
        let Some(at) = self
            .installing
            .iter()
            .position(|(_, pending)| pending.names_pairwise::<Keys<'id, P>>(&key))
        else {
            // Excluded by `admit`.
            return Err((ApContract::ForeignKey, Kept::PairwiseKey(key)));
        };
        let (peer, pending) = self.installing.swap_remove(at);
        let installed = match pending.installed::<Keys<'id, P>>(key) {
            Ok(installed) => installed,
            // Excluded by the position's own check: the installation and the
            // key come back both, as it returned them.
            Err(foreign) => {
                return Err((ApContract::ForeignKey, Kept::ForeignPairwise(peer, foreign)));
            }
        };
        if self.engine.service().peer_status(peer).is_none()
            || self.engine.pairwise_installed(peer).is_err()
        {
            // The peer left meanwhile: its key comes off, and its Message 3
            // does not go.
            self.held_message3.retain(|held| held.peer != peer);
            if let Err(key) = self.removals.push_back(installed.into_key()) {
                // Excluded by `admit`: a removal's place was free.
                return Err((ApContract::ForeignKey, Kept::PairwiseKey(key)));
            }
            return Ok(());
        }
        if let Err(installed) = self.keys.push((peer, installed)) {
            // Excluded by `admit`: a key's place was free.
            return Err((ApContract::ForeignKey, Kept::Installed(installed)));
        }
        // The key reads the station's Message 4 now, protected or not; the
        // controlled port stays closed until Message 4 is verified.
        for held in self.held_message3.iter_mut() {
            if held.peer == peer {
                held.ready = true;
            }
        }
        Ok(())
    }

    /// The access point's configuration took effect: the next step of the
    /// start or the stop.
    fn configured(&mut self) {
        match self.phase {
            // The group key goes in before the TSF starts.
            Phase::Configuring => self.phase = Phase::InstallingGroup,
            Phase::StartingTsf => {
                self.phase = Phase::Running;
                self.engine.tsf_started(self.now);
                self.status.running = true;
                self.network.set_link(true);
            }
            // Its keys come off: every pairwise key went with its peer.
            Phase::ClosingReceive => self.phase = Phase::RemovingGroup,
            Phase::StoppingTsf => {
                self.engine.tsf_stopped();
                self.phase = Phase::StoppingRadio;
            }
            _ => {}
        }
    }

    /// A single MPDU's terminal: its ticket and buffer home, then what its
    /// purpose asks.
    fn completed(
        &mut self,
        completed: oer_protocols_ieee80211_contracts_port::SingleCompleted<'id, Ty<'id, P>>,
    ) -> Result<(), Stray<'id, P, C>> {
        let now = self.now;
        let parts = completed.into_parts(&mut self.access);
        let index = parts.ticket.index();
        let outcome = parts.outcome;
        let (buffer, _) = parts.payload.into_parts();
        // Each home below was found free by `admit`, and the buffer of the
        // runner's class.
        if let Err(ticket) = self.tickets.push(parts.ticket) {
            return Err((
                ApContract::UnknownTicket(index),
                Kept::Completion(ticket, buffer),
            ));
        }
        match <Ty<'id, P> as TxBufferPool<'id, C>>::class(buffer) {
            Ok(buffer) => self.home_buffer(buffer)?,
            Err(buffer) => match <Ty<'id, P> as TxBufferPool<'id, C>>::sent(buffer) {
                Ok(sent) => {
                    self.home_buffer(<Ty<'id, P> as TxBufferPool<'id, C>>::renew(sent).0)?;
                }
                Err(other) => return Err((ApContract::BufferClass, Kept::AnyBuffer(other))),
            },
        }
        let delivered = matches!(outcome, TxOutcome::Ended(SingleEnd::Delivered { .. }));
        let Some(purpose) = self
            .purposes
            .get_mut(usize::from(index))
            .and_then(Option::take)
        else {
            // Excluded by `admit`: the ticket named a purpose.
            self.broken.get_or_insert(ApContract::UnknownTicket(index));
            return Ok(());
        };
        match purpose {
            Purpose::Response(ApAfter::BeginWpa2 { peer }) if delivered && !self.stop => {
                // Its handshake begins once the port's call returned; a
                // ticket's terminal adds one at most.
                if self.begins.push_back(peer).is_err() {
                    self.refuse();
                }
            }
            Purpose::Response(_) | Purpose::Data => {}
            Purpose::Eapol(context) => {
                // A context the handshake no longer names returns as
                // superseded; a refusal counts, and the handshake's own
                // timeout ends it.
                if self.engine.eapol_sent(context, delivered, now).is_err() {
                    self.refuse();
                }
            }
            Purpose::Teardown(teardown, ApPeerDisconnectKind::Disassociation) => {
                if self
                    .teardowns
                    .push_front((teardown, ApPeerDisconnectKind::Deauthentication))
                    .is_err()
                {
                    self.finish_teardown(teardown.peer)?;
                }
            }
            Purpose::Teardown(teardown, ApPeerDisconnectKind::Deauthentication) => {
                self.finish_teardown(teardown.peer)?;
            }
            Purpose::Beacon(context) => {
                let beacon = match outcome {
                    TxOutcome::Ended(SingleEnd::Delivered { .. }) => {
                        self.status.beacons = self.status.beacons.saturating_add(1);
                        ApBeaconTxOutcome::Delivered
                    }
                    TxOutcome::Expired { .. } => {
                        self.status.beacons_expired = self.status.beacons_expired.saturating_add(1);
                        ApBeaconTxOutcome::Unsent
                    }
                    TxOutcome::Withdrawn(_) => ApBeaconTxOutcome::Withdrawn,
                    TxOutcome::Indeterminate(_) => ApBeaconTxOutcome::Indeterminate,
                    TxOutcome::Ended(_) => ApBeaconTxOutcome::NotDelivered,
                };
                // No group traffic is buffered: a DTIM releases nothing.
                let _release: Option<_> = context.complete(beacon);
            }
        }
        self.advance_stop();
        Ok(())
    }

    /// Return a terminal's buffer home, renewed for a new MPDU. A buffer
    /// whose memory the port still holds is not renewed: it goes home as
    /// it is, the port's break recorded, and the run ends on it.
    fn home_buffer(&mut self, mut buffer: Buffer<'id, P, C>) -> Result<(), Stray<'id, P, C>> {
        if buffer.renew_msdu().is_err() {
            self.broken.get_or_insert(ApContract::BufferClass);
        }
        self.buffers
            .push(buffer)
            // Excluded by `admit`: a buffer's place was free.
            .map_err(|buffer| (ApContract::BufferClass, Kept::Buffer(buffer)))
    }

    /// Return a buffer home, renewed for a new MPDU: one whose memory the
    /// port still holds, or without a place, stops its operation.
    fn give_buffer(&mut self, mut buffer: Buffer<'id, P, C>) -> Result<(), Stray<'id, P, C>> {
        if buffer.renew_msdu().is_err() {
            return Err((ApContract::BufferClass, Kept::Buffer(buffer)));
        }
        self.buffers
            .push(buffer)
            .map_err(|buffer| (ApContract::BufferClass, Kept::Buffer(buffer)))
    }

    /// The peer's teardown frames went: its key comes off and the engine
    /// forgets it.
    fn finish_teardown(&mut self, peer: [u8; 6]) -> Result<(), Stray<'id, P, C>> {
        self.forget_peer(peer)?;
        // A peer that left by itself is gone already.
        if self.engine.forget(peer).is_err() {
            self.refuse();
        }
        Ok(())
    }

    /// `peer` is gone: its key comes off, and nothing of its handshake
    /// waits.
    fn forget_peer(&mut self, peer: [u8; 6]) -> Result<(), Stray<'id, P, C>> {
        if let Some(at) = self.keys.iter().position(|(owner, _)| *owner == peer) {
            let (_, installed) = self.keys.swap_remove(at);
            if let Err(key) = self.removals.push_back(installed.into_key()) {
                return Err((ApContract::ForeignKey, Kept::PairwiseKey(key)));
            }
        }
        self.installs.retain(|waiting| *waiting != peer);
        self.held_message3.retain(|held| held.peer != peer);
        Ok(())
    }

    /// A frame of `purpose` did not go: what its terminal would have done
    /// without delivery.
    fn unsent(&mut self, purpose: Purpose, now: Instant) -> Result<(), Stray<'id, P, C>> {
        match purpose {
            Purpose::Eapol(context) => {
                if self.engine.eapol_sent(context, false, now).is_err() {
                    self.refuse();
                }
            }
            Purpose::Teardown(teardown, kind) => {
                // It goes when a ticket and a buffer are back.
                if self.teardowns.push_front((teardown, kind)).is_err() {
                    self.finish_teardown(teardown.peer)?;
                }
            }
            Purpose::Beacon(context) => {
                let _release: Option<_> = context.complete(ApBeaconTxOutcome::Unsent);
            }
            Purpose::Response(_) | Purpose::Data => {}
        }
        Ok(())
    }
}

impl<'id, 's, 'n, P, I, C, S, X> ApRunner<'id, 's, 'n, P, I, C, S, X>
where
    P: AccessPointPort<'id>,
    I: AccessPointCommands<'id, Types = Ty<'id, P>, Caps = Caps<'id, P>>,
    C: TxBufferClass,
    Ty<'id, P>: TxBufferPool<'id, C>,
    S: SppScope,
    X: ApNonces,
    <Keys<'id, P> as KeyTypes<'id>>::GroupSlotIdentity: PartialEq,
    <Keys<'id, P> as KeyTypes<'id>>::Identity: PartialEq,
    <Ty<'id, P> as PortTypes<'id>>::Context: Clone,
{
    /// A runner over `port` and the access point's `resources` of its
    /// installation, serving the BSS of `engine` on `channel` with the
    /// group key `group_key`. Each attempt carries a copy of `context`.
    #[expect(
        clippy::too_many_arguments,
        reason = "each owner of the run moves in once, by value"
    )]
    pub fn new(
        port: P,
        commands: I,
        resources: ApResources<'id, P, C>,
        engine: ApEngine<'s, S>,
        policy: ApPolicy,
        channel: SupportedChannel<Caps<'id, P>>,
        group_key: [u8; 16],
        network: ApNetworkEnd<'n>,
        context: <Ty<'id, P> as PortTypes<'id>>::Context,
        nonces: X,
    ) -> Self {
        let ApResources {
            tickets,
            access,
            buffers,
            configure,
            association,
            pairwise,
            group,
            group_permission,
            beacon,
        } = resources;
        let association = <Keys<'id, P> as KeyTypes<'id>>::begin_association(
            association,
            oer_protocols_ieee80211_core_roles_ap::engine::ApSpp::NotNegotiated,
        );
        let mut nonces = nonces;
        let nonce = nonces.nonce();
        let now = Instant::EPOCH;
        Self {
            port,
            commands,
            homes: Homes {
                engine,
                policy,
                channel,
                network,
                context,
                nonces,
                tickets,
                access,
                buffers,
                purposes: [const { None }; 32],
                configure: Some(configure),
                association: Some(association),
                pairwise,
                installs: heapless::Deque::new(),
                held_message3: heapless::Vec::new(),
                removals: heapless::Deque::new(),
                nonce,
                group_slot: Some(group),
                group_permission: Some(group_permission),
                group_key,
                group_pending: None,
                group: None,
                installing: heapless::Vec::new(),
                keys: heapless::Vec::new(),
                teardowns: heapless::Deque::new(),
                beacon: Some(beacon),
                phase: Phase::Configuring,
                status: ApStatus::default(),
                scratch: [0; SCRATCH],
                stop: false,
                held: None,
                returned: None,
                kept: None,
                stopped_on: None,
                unrecycled: None,
                unkept: None,
                unaccepted: None,
                broken: None,
                begins: heapless::Deque::new(),
                cut: heapless::Deque::new(),
                cut_units: 0,
                now,
                faulted: false,
                taken: 0,
            },
        }
    }

    /// Run the access point until its stop: start, serve, stop. The runner
    /// keeps the port and every owner across the end, in place: the end
    /// says why. An unconfirmed stop goes on with [`ApRunner::finish_stop`];
    /// a run that ended before its stop stops the radio there too.
    pub async fn run<T: Timer>(
        &mut self,
        shared: &ApShared,
        clock: &T,
    ) -> ApEnd<PortFault<'id, P>> {
        // A port that cannot beacon is refused before its radio starts.
        if let Err(unsupported) = serves_beacons::<Caps<'id, P>>() {
            return ApEnd::Unsupported(unsupported);
        }
        // A unit the port refused stays until its owner takes it: no run.
        if let Some(refused) = self.homes.unrecycled.as_ref() {
            return ApEnd::Unrecycled(refused.fault);
        }
        // Nor while an item a port call could not keep is here.
        if self.homes.unkept.is_some() {
            return ApEnd::Unkept;
        }
        // Nor while owners an operation stopped on are out.
        if let Some((contract, _)) = self.homes.stopped_on.as_ref().or(self.homes.kept.as_ref()) {
            return ApEnd::Contract(*contract);
        }
        // The radio starts on the BSS's channel.
        let channel = self.homes.channel;
        let started = poll_fn(|cx| {
            let now = clock.now();
            self.port.poll_start(cx, channel, now)
        })
        .await;
        if let Err(refused) = started {
            return ApEnd::StartRefused(refused);
        }
        let now = clock.now();
        if let Err(halt) = self.run_commands(now).and_then(|()| self.settle(now)) {
            return self.halted(halt, shared);
        }
        let mut facts = Facts::default();
        // Turns in a row that changed nothing.
        let mut idle: u8 = 0;
        loop {
            let now = clock.now();
            let before = self.homes.mark();
            let last = facts;
            if let Err(halt) = self.turn(now, core::mem::take(&mut facts)) {
                return self.halted(halt, shared);
            }
            shared.publish(self.homes.status);
            if self.homes.phase == Phase::StoppingRadio {
                break;
            }
            if !self.homes.stop && shared.stop.get() {
                self.homes.stop = true;
                if let Err(stray) = self.homes.begin_stop() {
                    return self.halted(stray.into(), shared);
                }
                continue;
            }
            // A turn that moved something may have left work for an earlier
            // stage of the next one (a terminal it took last queues a frame):
            // the runner polls its sources, which registers its waker with
            // the port, yields to the executor once and turns again.
            let moved = before != self.homes.mark();
            idle = if moved { 0 } else { idle.saturating_add(1) };
            // A port may owe the next call more than this one took (a cut, a
            // repost): an idle turn yields and the sources decide, up to
            // IDLE_REPEATS turns in a row. Then the runner has settled: a
            // deadline it found due wakes it no more, and the very facts it
            // just served stay held for the next turn, never dropped, until a
            // wake of any source.
            let settled = idle >= IDLE_REPEATS;
            let again = moved || !settled;
            let deadline = [self.port.deadline(), self.homes.engine_deadline()]
                .into_iter()
                .flatten()
                .filter(|at| !settled || *at > now)
                .min();
            let sends = self.homes.phase == Phase::Running
                && !self.homes.tickets.is_empty()
                && !self.homes.buffers.is_empty();
            let stopping = self.homes.stop;
            let port = &mut self.port;
            let network = &mut self.homes.network;
            let mut held = Facts::default();
            // Only the wait's first poll, which registers for the port's
            // next change, holds the facts just served: any later poll comes
            // of a wake, and its facts are an event.
            let mut first = settled;
            let polled = poll_fn(|cx| {
                let taken = port.poll_facts(cx);
                let registering = core::mem::replace(&mut first, false);
                if taken.is_empty() {
                    return Poll::Pending;
                }
                if registering && taken == last {
                    held = union(held, taken);
                    return Poll::Pending;
                }
                Poll::Ready(taken)
            });
            let timer = async {
                match deadline {
                    Some(at) => clock.wait_until(at).await,
                    None => core::future::pending().await,
                }
            };
            // The network's frames wake the runner only while it can send
            // one; a ticket or buffer comes back with a fact.
            let frames = poll_fn(|cx| {
                if !sends {
                    return Poll::Pending;
                }
                if network.waiting() {
                    return Poll::Ready(());
                }
                let mut ready = core::pin::pin!(network.transmit.ready());
                ready.as_mut().poll(cx).map(drop)
            });
            // An accepted stop is no source: the stop's own commands and
            // facts move it on.
            let stop = poll_fn(|cx| {
                if stopping {
                    Poll::Pending
                } else {
                    shared.poll_stop(cx)
                }
            });
            // An unsettled runner turns again after one yield.
            let yielded = async {
                if again {
                    embassy_futures::yield_now().await;
                } else {
                    core::future::pending::<()>().await;
                }
            };
            let woke = select(select4(polled, timer, frames, stop), yielded).await;
            facts = match woke {
                Either::First(Either4::First(taken)) => union(held, taken),
                Either::First(_) | Either::Second(()) => held,
            };
        }
        self.homes.status.running = false;
        shared.publish(self.homes.status);
        let watched = self.watch_stop(clock, |_| true).await;
        if let Some(refused) = self.homes.unrecycled.as_ref() {
            // The stop's watch found a unit the port refused; the stop is
            // watched on by [`ApRunner::finish_stop`] if not confirmed.
            return ApEnd::Unrecycled(refused.fault);
        }
        if self.homes.unkept.is_some() {
            // The stop's watch found an item the port could not keep; no
            // service follows it.
            return ApEnd::Unkept;
        }
        match watched {
            Ok(()) => ApEnd::Stopped,
            Err(unconfirmed) => ApEnd::StopUnconfirmed(unconfirmed),
        }
    }

    /// Stop the radio of an ended run, or go on watching a stop the port
    /// has not confirmed, until the port confirms it: the port's facts and
    /// its deadline drive the stop's own service, whose terminals bring
    /// their owners home. Each observation of the port that differs from
    /// the one before goes to `unconfirmed`; the stop is never asked for
    /// again, only watched. A port fault is taken back first when the port
    /// can ([`DataPort::recover_for_stop`]). Nothing is sent and no command
    /// goes meanwhile. It returns only once the port confirmed the stop.
    ///
    /// # Errors
    ///
    /// The port refused a unit's recycle, before or during the stop: the
    /// count that refused it; the unit is in the runner
    /// ([`ApRunner::take_unrecycled`]) and no unit went back after it.
    pub async fn finish_stop<T: Timer>(
        &mut self,
        clock: &T,
        mut unconfirmed: impl FnMut(StopUnconfirmed),
    ) -> Result<(), ReceiveAccounting> {
        if self.homes.faulted {
            self.homes.faulted = !self.port.recover_for_stop();
        }
        self.homes.stop = true;
        self.homes.phase = Phase::StoppingRadio;
        self.homes.status.running = false;
        self.homes.network.set_link(false);
        let _confirmed = self
            .watch_stop(clock, |observed| {
                unconfirmed(observed);
                false
            })
            .await;
        match self.homes.unrecycled.as_ref() {
            Some(refused) => Err(refused.fault),
            None => Ok(()),
        }
    }

    /// Watch the radio's stop: serve the port at its facts and its due
    /// deadline, bring the terminals' owners home, recycle what a cut hands
    /// over, and poll the stop. An unconfirmed answer goes to `ends`, which
    /// says whether it ends the watch; an answer equal to the last one is
    /// not reported again. A deadline the watch served at does not wake it
    /// again, nor do the very facts it just served.
    async fn watch_stop<T: Timer>(
        &mut self,
        clock: &T,
        mut ends: impl FnMut(StopUnconfirmed) -> bool,
    ) -> Result<(), StopUnconfirmed> {
        let mut facts = Facts::default();
        let mut served: Option<Instant> = None;
        let mut reported: Option<StopUnconfirmed> = None;
        loop {
            let now = clock.now();
            let due = self
                .port
                .deadline()
                .is_some_and(|at| at <= now && served.is_none_or(|served| at > served));
            let last = facts;
            if facts.tx.is_some() || facts.tbtt || due {
                served = Some(now);
                self.homes.now = now;
                // A fault of this service is the stop's to name: its answer
                // below says it. No service follows an item the port could
                // not keep: the place stays the first one's.
                if self.homes.unkept.is_none() {
                    let (_served, unkept) = crate::answered(self.port.service_tx(
                        facts.tx.unwrap_or(TxFacts::DUE),
                        facts.tbtt,
                        now,
                        &mut self.homes,
                    ));
                    self.homes.unkept = unkept;
                }
                // The stop reads nothing a cut hands over: each unit goes
                // back to its ring at once, until the port refuses one;
                // after it, every unit stays in its place.
                while self.homes.unrecycled.is_none()
                    && let Some((frame, _)) = self.homes.cut.pop_front()
                {
                    if let Err(refused) = self.port.recycle(frame) {
                        self.homes.unrecycled = Some(refused);
                    }
                }
            }
            facts = Facts::default();
            let deadline = self
                .port
                .deadline()
                .filter(|at| *at > now || served.is_none_or(|served| *at > served));
            let port = &mut self.port;
            let mut first = true;
            let polled = poll_fn(|cx| {
                match port.poll_stop(cx, clock.now()) {
                    Poll::Ready(Ok(())) => return Poll::Ready(Woke::Stopped(Ok(()))),
                    Poll::Ready(Err(observed)) if reported != Some(observed) => {
                        reported = Some(observed);
                        return Poll::Ready(Woke::Stopped(Err(observed)));
                    }
                    Poll::Ready(Err(_)) | Poll::Pending => {}
                }
                // The stop's own poll may have made work due at once: the
                // next service serves it.
                let polled_now = clock.now();
                if port
                    .deadline()
                    .is_some_and(|at| at <= polled_now && served.is_none_or(|served| at > served))
                {
                    return Poll::Ready(Woke::Other);
                }
                let taken = port.poll_facts(cx);
                let registering = core::mem::replace(&mut first, false);
                if taken.is_empty() || (registering && taken == last) {
                    facts = union(facts, taken);
                    return Poll::Pending;
                }
                Poll::Ready(Woke::Facts(taken))
            });
            let timer = async {
                match deadline {
                    Some(at) => clock.wait_until(at).await,
                    None => core::future::pending().await,
                }
            };
            let woke = match select(polled, timer).await {
                Either::First(woke) => woke,
                Either::Second(()) => Woke::Other,
            };
            match woke {
                Woke::Stopped(Ok(())) => {
                    self.take_unconsumed();
                    return Ok(());
                }
                Woke::Stopped(Err(observed)) => {
                    if ends(observed) {
                        return Err(observed);
                    }
                }
                Woke::Facts(taken) => facts = union(facts, taken),
                Woke::Other => {}
            }
        }
    }

    /// After the radio's confirmed stop: what the port kept for the homes'
    /// refusals comes home now, once; one whose home is still taken stays
    /// with the port. A unit goes back to its ring, until the port refuses
    /// one.
    fn take_unconsumed(&mut self) {
        let _kept: Result<Unconsumed, UnconsumedRefused> =
            self.port.take_unconsumed(&mut self.homes);
        while self.homes.unrecycled.is_none()
            && let Some((frame, _)) = self.homes.cut.pop_front()
        {
            if let Err(refused) = self.port.recycle(frame) {
                self.homes.unrecycled = Some(refused);
            }
        }
    }

    /// The end of a run a step halted: a refusal returns the runner with
    /// every owner, the access point no longer serving.
    fn halted(
        &mut self,
        halt: Halt<'id, P, C, PortFault<'id, P>>,
        shared: &ApShared,
    ) -> ApEnd<PortFault<'id, P>> {
        self.homes.status.running = false;
        self.homes.network.set_link(false);
        shared.publish(self.homes.status);
        match halt {
            Halt::Faulted(fault) => {
                self.homes.faulted = true;
                ApEnd::Faulted(fault)
            }
            Halt::Contract(contract) => ApEnd::Contract(contract),
            Halt::Kept(contract, kept) => {
                // A run halts once, and none starts while owners a step
                // stopped on are out: the place is empty.
                self.homes.stopped_on = Some((contract, kept));
                ApEnd::Contract(contract)
            }
            Halt::Refused(refusal) => ApEnd::Refused(refusal),
            Halt::Unrecycled(fault) => ApEnd::Unrecycled(fault),
            Halt::Unkept => ApEnd::Unkept,
        }
    }

    /// The item a port call could neither give the runner nor keep, once
    /// ([`ApEnd::Unkept`]).
    pub fn take_unkept(
        &mut self,
    ) -> Option<oer_protocols_ieee80211_contracts_port::Unkept<'id, Ty<'id, P>>> {
        self.homes.unkept.take()
    }

    /// The attempt a submission returned unaccepted beside its unkept
    /// item, once.
    pub fn take_unaccepted(&mut self) -> Option<<P as DataPort<'id>>::Admitted> {
        self.homes.unaccepted.take()
    }

    /// The command the port gave back unrun (refused, or beside its
    /// fault), once.
    pub fn take_returned(&mut self) -> Option<Command<'id, P>> {
        self.homes.returned.take()
    }

    /// The owners of an operation that stopped on one without a home of
    /// the runner's, whole: a step's first, then a terminal's; each once.
    pub fn take_kept(&mut self) -> Option<Kept<'id, P, C>> {
        self.homes
            .stopped_on
            .take()
            .or_else(|| self.homes.kept.take())
            .map(|(_, kept)| kept)
    }

    /// The unit whose recycle the port refused, with the count that
    /// refused it, once ([`ApEnd::Unrecycled`]).
    pub fn take_unrecycled(
        &mut self,
    ) -> Option<RecycleRefused<<Ty<'id, P> as PortTypes<'id>>::RxBuffer>> {
        self.homes.unrecycled.take()
    }

    /// Return `frame` to the port. A refusal keeps the unit in the runner
    /// and ends the run: the caller returns at once, and no step returns
    /// a unit after it, so the one place is empty here.
    fn recycle(
        &mut self,
        frame: <Ty<'id, P> as PortTypes<'id>>::RxBuffer,
    ) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        self.port.recycle(frame).map_err(|refused| {
            let fault = refused.fault;
            self.homes.unrecycled = Some(refused);
            Halt::Unrecycled(fault)
        })
    }

    /// The port, for the composition's release once the stop is confirmed.
    pub fn port_mut(&mut self) -> &mut P {
        &mut self.port
    }

    /// The access point's status now.
    pub fn status(&self) -> ApStatus {
        self.homes.status
    }

    /// One turn: the port's transmit service and terminals, the received
    /// units, the engine's due work, the network's frames and the
    /// commands.
    fn turn(
        &mut self,
        now: Instant,
        facts: Facts,
    ) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        self.homes.now = now;
        let due = self.port.deadline().is_some_and(|at| at <= now);
        if facts.tx.is_some() || facts.tbtt || due {
            // No turn runs while an unkept item is here: the place is empty.
            let (served, unkept) = crate::answered(self.port.service_tx(
                facts.tx.unwrap_or(TxFacts::DUE),
                facts.tbtt,
                now,
                &mut self.homes,
            ));
            if unkept.is_some() {
                // The port keeps its own fault; the run ends on the item.
                self.homes.unkept = unkept;
                return Err(Halt::Unkept);
            }
            if let Input::Accepted(Err(fault)) = served {
                return Err(Halt::Faulted(fault));
            }
        }
        self.settle(now)?;
        self.receive(now)?;
        self.settle(now)?;
        if self.homes.phase == Phase::Running {
            self.due(now)?;
            self.settle(now)?;
            self.transmit_network(now)?;
            self.settle(now)?;
        }
        self.send_teardowns(now)?;
        self.settle(now)?;
        self.run_commands(now)?;
        self.settle(now)?;
        self.homes.advance_stop();
        let status = self.homes.engine.service().status();
        self.homes.status.associated = status.associated;
        self.homes.status.authorized = status.authorized;
        Ok(())
    }

    /// Issue the commands the homes' owners make, in order: the one the
    /// port held first, then each the phase and the keys call for, until
    /// the port holds one.
    fn run_commands(&mut self, now: Instant) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        loop {
            self.homes.now = now;
            let command = match self.homes.held.take() {
                Some(command) => command,
                None => match self.homes.next_command(&mut self.commands)? {
                    Some(command) => command,
                    None => return Ok(()),
                },
            };
            let (progress, unkept) = crate::answered(self.port.access_point_command(
                command,
                now,
                &mut self.homes,
            ));
            let progress = match progress {
                Input::Accepted(progress) => progress,
                Input::NotAccepted(command) => {
                    // Found before the port accepted it: the command stays
                    // with the runner, never issued again.
                    self.homes.returned = Some(command);
                    self.homes.unkept = unkept;
                    return Err(Halt::Unkept);
                }
            };
            if unkept.is_some() {
                // The command's own answer goes to its home; the run ends
                // on the item, and nothing behind it runs.
                self.homes.unkept = unkept;
                match progress {
                    Ok(AccessPointProgress::Done) => {
                        self.homes.taken = self.homes.taken.wrapping_add(1);
                    }
                    Err(CommandFailed::Taken(_)) => {}
                    Ok(AccessPointProgress::Held(command)) => self.homes.held = Some(command),
                    Ok(AccessPointProgress::Refused(command, _))
                    | Err(CommandFailed::Returned { command, .. }) => {
                        self.homes.returned = Some(command);
                    }
                }
                return Err(Halt::Unkept);
            }
            match progress {
                Ok(AccessPointProgress::Done) => {
                    self.homes.taken = self.homes.taken.wrapping_add(1);
                    self.settle(now)?;
                }
                Ok(AccessPointProgress::Held(command)) => {
                    // Its place was emptied by the issue above.
                    self.homes.held = Some(command);
                    return Ok(());
                }
                Ok(AccessPointProgress::Refused(command, refusal)) => {
                    // The command and its owners stay with the runner,
                    // which returns them; nothing behind it runs.
                    self.homes.returned = Some(command);
                    return Err(Halt::Refused(refusal));
                }
                Err(CommandFailed::Returned { command, fault }) => {
                    // The port did not take it: it stays with the runner,
                    // never issued again.
                    self.homes.returned = Some(command);
                    return Err(Halt::Faulted(fault));
                }
                Err(CommandFailed::Taken(fault)) => return Err(Halt::Faulted(fault)),
            }
        }
    }

    /// After a port call: read and recycle what a cut handed over, send
    /// what the terminals asked for, and end the run on a contract break a
    /// terminal showed.
    fn settle(&mut self, now: Instant) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        while let Some((frame, meta)) = self.homes.cut.pop_front() {
            let result = self.received(
                oer_protocols_ieee80211_contracts_port::RxBuffer::bytes(&frame),
                integrity(&meta),
                now,
            );
            self.recycle(frame)?;
            result?;
        }
        while let Some(peer) = self.homes.begins.pop_front() {
            // A peer that left meanwhile has no handshake to begin.
            if let Ok(transmit) = self.homes.engine.begin_wpa2(peer) {
                let (frame, context) = transmit.into_parts();
                self.send_eapol(&frame, context, now)?;
            }
        }
        while let Some(at) = self.homes.held_message3.iter().position(|held| held.ready) {
            let held = self.homes.held_message3.swap_remove(at);
            let (frame, context) = held.message3.into_parts();
            self.send_eapol(&frame, context, now)?;
        }
        match self.homes.broken.take() {
            Some(contract) => Err(Halt::Contract(contract)),
            None => Ok(()),
        }
    }

    /// Pull the received units of this turn and act on each.
    fn receive(&mut self, now: Instant) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        for _ in 0..RECEIVE_BUDGET {
            let pulled = match self.port.receive(RECEIVE_ROOM, now) {
                Ok(pulled) => pulled,
                Err(fault) => return Err(Halt::Faulted(fault)),
            };
            let RxPull::Unit { frame, meta } = pulled else {
                break;
            };
            self.homes.taken = self.homes.taken.wrapping_add(1);
            let result = self.received(
                oer_protocols_ieee80211_contracts_port::RxBuffer::bytes(&frame),
                integrity(&meta),
                now,
            );
            self.recycle(frame)?;
            result?;
        }
        Ok(())
    }

    /// Act on one received frame.
    fn received(
        &mut self,
        bytes: &[u8],
        integrity: Integrity,
        now: Instant,
    ) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        let homes = &mut self.homes;
        let received =
            homes
                .engine
                .receive(bytes, integrity, now, homes.nonce, 1, &mut homes.scratch);
        // A frame of the air the engine refuses changes nothing.
        let Ok(received) = received else {
            return Ok(());
        };
        match received {
            ApReceived::Nothing => Ok(()),
            ApReceived::Respond { frame, after } => {
                if matches!(after, ApAfter::BeginWpa2 { .. }) {
                    // The association took the ANonce.
                    self.homes.nonce = self.homes.nonces.nonce();
                }
                let operation = match frame.kind {
                    ApFrameKind::Management { .. }
                        if matches!(after, ApAfter::BeginWpa2 { .. }) =>
                    {
                        TxOperation::Connection(ConnectionFrame::Association)
                    }
                    _ => TxOperation::Data,
                };
                self.send_scratch(frame, Purpose::Response(after), operation, now)
            }
            ApReceived::PeerLeft { peer } => {
                self.homes.forget_peer(peer)?;
                Ok(())
            }
            ApReceived::Eapol { peer, body } => {
                let homes = &mut self.homes;
                let Some(group) = homes.group.as_ref() else {
                    return Ok(());
                };
                match homes.engine.eapol(peer, body, group) {
                    Ok(ApEapol::Nothing) | Err(_) => Ok(()),
                    Ok(ApEapol::Transmit(transmit)) => {
                        let (frame, context) = transmit.into_parts();
                        self.send_eapol(&frame, context, now)
                    }
                    Ok(ApEapol::Install { peer, message3 }) => {
                        // A new PTK of a peer whose key is in or going in
                        // is no handshake the runner serves: it fails.
                        let keyed = homes.installs.iter().any(|waiting| *waiting == peer)
                            || homes.installing.iter().any(|(owner, _)| *owner == peer)
                            || homes.keys.iter().any(|(owner, _)| *owner == peer);
                        let held = !keyed
                            && homes.installs.push_back(peer).is_ok()
                            && homes
                                .held_message3
                                .push(HeldMessage3 {
                                    peer,
                                    message3,
                                    ready: false,
                                })
                                .is_ok();
                        if !held {
                            homes.installs.retain(|waiting| *waiting != peer);
                            if let Ok(teardown) = homes.engine.fail_handshake(peer) {
                                homes.teardown(teardown)?;
                            }
                        }
                        Ok(())
                    }
                    Ok(ApEapol::Authorize { peer }) => {
                        // Message 3 went only once the key was in.
                        let keyed = homes.keys.iter().any(|(owner, _)| *owner == peer);
                        if (!keyed || homes.engine.authorize(peer, now).is_err())
                            && let Ok(teardown) = homes.engine.fail_handshake(peer)
                        {
                            homes.teardown(teardown)?;
                        }
                        Ok(())
                    }
                    Ok(ApEapol::Deauthenticate { peer }) => {
                        if let Ok(teardown) = homes.engine.fail_handshake(peer) {
                            homes.teardown(teardown)?;
                        }
                        Ok(())
                    }
                }
            }
            ApReceived::Msdu(msdu) => {
                // A full network queue drops the MSDU: the stack's own
                // retries cover it.
                let homes = &mut self.homes;
                if homes.engine.delivers(&msdu) && !homes.network.deliver(msdu.parts()) {
                    homes.status.rx_dropped = homes.status.rx_dropped.saturating_add(1);
                }
                Ok(())
            }
        }
    }

    /// The engine's due work: the beacon, handshake retries and inactive
    /// peers.
    fn due(&mut self, now: Instant) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        if self.homes.beacon.is_some() {
            self.send_beacon(now)?;
        }
        let homes = &mut self.homes;
        if let Some(group) = homes.group.as_ref() {
            match homes.engine.due_retry(now, group) {
                Ok(Some(ApDue::Transmit(transmit))) => {
                    let (frame, context) = transmit.into_parts();
                    self.send_eapol(&frame, context, now)?;
                }
                Ok(Some(ApDue::Teardown(teardown))) => homes.teardown(teardown)?,
                Ok(None) | Err(_) => {}
            }
        }
        while let Some(teardown) = self.homes.engine.due_inactive(now) {
            self.homes.teardown(teardown)?;
        }
        Ok(())
    }

    /// Carry the network's frames into the BSS while tickets and buffers
    /// last.
    fn transmit_network(&mut self, now: Instant) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        for _ in 0..TRANSMIT_BUDGET {
            let homes = &mut self.homes;
            if homes.tickets.is_empty() || homes.buffers.is_empty() || !homes.network.waiting() {
                break;
            }
            let Some(frame) = homes.network.take() else {
                break;
            };
            let Some(mut buffer) = homes.buffers.pop() else {
                break;
            };
            let parts = frame.parts();
            let written = match buffer.bytes_mut() {
                Ok(bytes) => homes.engine.encapsulate(
                    parts.destination,
                    parts.source,
                    parts.ether_type,
                    parts.payload,
                    bytes,
                ),
                // The port still holds its memory: the step stops on it.
                Err(_) => return Err(Halt::Kept(ApContract::BufferClass, Kept::Buffer(buffer))),
            };
            match written {
                Ok(Some(written)) => {
                    self.submit(buffer, written, Purpose::Data, TxOperation::Data, None, now)?;
                }
                // No authorized peer of that destination, or a frame the
                // BSS cannot carry: it does not go.
                Ok(None) | Err(_) => homes.give_buffer(buffer)?,
            }
        }
        Ok(())
    }

    /// Send the queued teardown frames while tickets and buffers last.
    fn send_teardowns(&mut self, now: Instant) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        while !self.homes.tickets.is_empty() && !self.homes.buffers.is_empty() {
            let homes = &mut self.homes;
            let Some((teardown, kind)) = homes.teardowns.pop_front() else {
                break;
            };
            match homes
                .engine
                .write_teardown(teardown.peer, kind, &mut homes.scratch)
            {
                Ok(frame) => self.send_scratch(
                    frame,
                    Purpose::Teardown(teardown, kind),
                    TxOperation::Data,
                    now,
                )?,
                Err(_) => homes.finish_teardown(teardown.peer)?,
            }
        }
        Ok(())
    }

    /// Send one EAPOL-Key frame of a handshake.
    fn send_eapol(
        &mut self,
        frame: &oer_protocols_ieee80211_core_security_rsn::frames::RsnTxFrame<
            { oer_protocols_ieee80211_core_roles_ap::engine::AP_EAPOL_CAPACITY },
        >,
        context: ApEapolTxContext,
        now: Instant,
    ) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        let homes = &mut self.homes;
        match homes.engine.write_eapol(frame, &mut homes.scratch) {
            Ok(written) => self.send_scratch(
                written,
                Purpose::Eapol(context),
                TxOperation::Connection(ConnectionFrame::Eapol),
                now,
            ),
            Err(_) => {
                // The frame did not fit: its window returns unsent.
                if homes.engine.eapol_sent(context, false, now).is_err() {
                    homes.refuse();
                }
                Ok(())
            }
        }
    }

    /// Send the frame the scratch holds: copied into a port buffer, or
    /// not sent when no ticket or buffer is free.
    fn send_scratch(
        &mut self,
        frame: ApFrame,
        purpose: Purpose,
        operation: TxOperation,
        now: Instant,
    ) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        let homes = &mut self.homes;
        let Some(mut buffer) = homes.buffers.pop() else {
            homes.unsent(purpose, now)?;
            return Ok(());
        };
        let copied = match (buffer.bytes_mut(), homes.scratch.get(..frame.len)) {
            (Ok(bytes), Some(source)) => match bytes.get_mut(..frame.len) {
                Some(destination) => {
                    for (octet, byte) in destination.iter_mut().zip(source) {
                        *octet = *byte;
                    }
                    true
                }
                None => false,
            },
            _ => false,
        };
        if !copied {
            homes.give_buffer(buffer)?;
            homes.unsent(purpose, now)?;
            return Ok(());
        }
        self.submit(buffer, frame, purpose, operation, None, now)
    }

    /// Write and submit the beacon due now.
    fn send_beacon(&mut self, now: Instant) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        let homes = &mut self.homes;
        if homes.engine.next_beacon().is_none_or(|due| now < due) || homes.tickets.is_empty() {
            return Ok(());
        }
        let Some(mut buffer) = homes.buffers.pop() else {
            return Ok(());
        };
        let written = match buffer.bytes_mut() {
            Ok(bytes) => homes.engine.beacon(now, bytes),
            // The port still holds its memory: the step stops on it.
            Err(_) => return Err(Halt::Kept(ApContract::BufferClass, Kept::Buffer(buffer))),
        };
        match written {
            Ok(Some((frame, deadline, context))) => self.submit(
                buffer,
                frame,
                Purpose::Beacon(context),
                TxOperation::Beacon,
                Some(deadline),
                now,
            ),
            Ok(None) => {
                homes.give_buffer(buffer)?;
                Ok(())
            }
            Err(error) => {
                homes.give_buffer(buffer)?;
                Err(Halt::Contract(ApContract::Engine(error)))
            }
        }
    }

    /// Submit the frame `buffer` holds, as `frame` names it: a beacon with
    /// `deadline`, any other frame without one. Whatever does not go comes
    /// home; a submission the port refuses ends withdrawn, its terminal
    /// bringing the owners home during the call.
    fn submit(
        &mut self,
        buffer: Buffer<'id, P, C>,
        frame: ApFrame,
        purpose: Purpose,
        operation: TxOperation,
        deadline: Option<Instant>,
        now: Instant,
    ) -> Result<(), Halt<'id, P, C, PortFault<'id, P>>> {
        self.homes.now = now;
        let Some(ticket) = self.homes.tickets.pop() else {
            self.homes.give_buffer(buffer)?;
            self.homes.unsent(purpose, now)?;
            return Ok(());
        };
        let index = ticket.index();
        let admitted = match self.admit(ticket, buffer, frame, operation, deadline) {
            Ok(admitted) => admitted,
            Err((ticket, source)) => {
                let homes = &mut self.homes;
                if let Err(ticket) = homes.tickets.push(ticket) {
                    // Its place was emptied by the pop above.
                    return Err(Halt::Kept(
                        ApContract::UnknownTicket(index),
                        Kept::Submission(ticket, source),
                    ));
                }
                match source {
                    Source::Fresh(buffer) => homes.give_buffer(buffer)?,
                    // A fresh source comes back fresh: another owner is the
                    // port's break, and the step stops on it.
                    Source::Sent(sent) => {
                        return Err(Halt::Kept(ApContract::BufferClass, Kept::Sent(sent)));
                    }
                }
                homes.unsent(purpose, now)?;
                return Ok(());
            }
        };
        match self.homes.purposes.get_mut(usize::from(index)) {
            Some(place) => *place = Some(purpose),
            None => {
                // Its terminal names no purpose and ends the run.
                self.homes
                    .broken
                    .get_or_insert(ApContract::UnknownTicket(index));
            }
        }
        // A refusal's terminal is in the homes. No step runs while an
        // unkept item is here: the place is empty.
        let (submitted, unkept) = crate::answered(self.port.submit(admitted, now, &mut self.homes));
        if unkept.is_some() {
            self.homes.unkept = unkept;
            if let Input::NotAccepted(attempt) = submitted {
                self.homes.unaccepted = Some(attempt);
            }
            return Err(Halt::Unkept);
        }
        // A terminal of the call that showed a break ends the step here:
        // nothing more is submitted behind it.
        match self.homes.broken.take() {
            Some(contract) => Err(Halt::Contract(contract)),
            None => Ok(()),
        }
    }

    /// Seal the frame `buffer` holds as the port's admitted attempt, or
    /// give back the ticket and the buffer.
    #[allow(
        clippy::type_complexity,
        reason = "the refusal returns each owner by its own type"
    )]
    fn admit(
        &mut self,
        ticket: Ticket<'id, P>,
        mut buffer: Buffer<'id, P, C>,
        frame: ApFrame,
        operation: TxOperation,
        deadline: Option<Instant>,
    ) -> Result<<P as DataPort<'id>>::Admitted, (Ticket<'id, P>, Source<'id, P, C>)> {
        let homes = &mut self.homes;
        let policy = homes.policy;
        let (rate, transmissions, category) = match frame.kind {
            ApFrameKind::Pairwise { rate, .. } => (
                rate,
                policy.data_transmissions,
                WmmAccessCategory::BestEffort,
            ),
            ApFrameKind::Management { .. } => (
                policy.management_rate,
                policy.management_transmissions,
                match operation {
                    TxOperation::Connection(ConnectionFrame::Eapol) => {
                        WmmAccessCategory::BestEffort
                    }
                    _ => WmmAccessCategory::Voice,
                },
            ),
            ApFrameKind::Group | ApFrameKind::Beacon => (
                policy.management_rate,
                NonZeroU8::MIN,
                WmmAccessCategory::Voice,
            ),
        };
        let class_frame = match (buffer.bytes_mut(), self.port.rate(rate)) {
            (Ok(bytes), Some(rate)) => port_frame::<Caps<'id, P>, C>(
                bytes,
                frame,
                RetryPlan::new(RetryStage {
                    rate,
                    transmissions,
                    protection: Protection::None,
                }),
            ),
            _ => None,
        };
        let Some(class_frame) = class_frame else {
            return Err((ticket, Source::Fresh(buffer)));
        };
        let protection = match frame.kind {
            ApFrameKind::Pairwise { peer, .. } => {
                let numbered = homes
                    .keys
                    .iter()
                    .find(|(owner, _)| *owner == peer)
                    .and_then(|(_, key)| key.number());
                match numbered {
                    Some(numbered) => TxProtection::Ccmp(NumberedKey {
                        key: AttemptKey::Pairwise(numbered.key),
                        packet_number: numbered.packet_number,
                    }),
                    None => return Err((ticket, Source::Fresh(buffer))),
                }
            }
            ApFrameKind::Group => match homes.group.as_ref().and_then(InstalledCcmpTxKey::number) {
                Some(numbered) => TxProtection::Ccmp(NumberedKey {
                    key: AttemptKey::Group(&numbered.key.key),
                    packet_number: numbered.packet_number,
                }),
                None => return Err((ticket, Source::Fresh(buffer))),
            },
            ApFrameKind::Management { .. } | ApFrameKind::Beacon => TxProtection::Open,
        };
        // What the attempt needs beside its MPDU is taken first: a refusal
        // after the preparation would hold a prepared MPDU it cannot return.
        let Some(power) = SupportedPower::new(TxPower::Calibrated) else {
            return Err((ticket, Source::Fresh(buffer)));
        };
        let beacon = match (frame.kind, deadline) {
            (ApFrameKind::Beacon, Some(deadline)) => {
                let Some(stamp) = BeaconStamp::new(BeaconTimestamp::Caller) else {
                    return Err((ticket, Source::Fresh(buffer)));
                };
                let Some(permission) = homes.beacon.take() else {
                    return Err((ticket, Source::Fresh(buffer)));
                };
                Some((permission, stamp, deadline))
            }
            _ => None,
        };
        let source = TxSource::Fresh {
            buffer,
            frame: class_frame,
            body: None,
            protection,
        };
        let checked = match source.check() {
            Ok(checked) => checked,
            Err(refusal) => {
                if let Some((permission, _, _)) = beacon {
                    homes.beacon = Some(permission);
                }
                return Err((ticket, returned(refusal.operation)));
            }
        };
        let prepared =
            match <Ty<'id, P> as TxBufferPool<'id, C>>::prepare::<AccessPoint, Caps<'id, P>>(
                checked,
            ) {
                Ok(prepared) => prepared,
                Err(refusal) => {
                    if let Some((permission, _, _)) = beacon {
                        homes.beacon = Some(permission);
                    }
                    return Err((ticket, returned(refusal.operation)));
                }
            };
        match beacon {
            Some((permission, stamp, deadline)) => Ok(self.port.admit_beacon(
                BeaconAttempt {
                    context: homes.context.clone(),
                    ticket,
                    source: prepared,
                    power,
                    permission,
                    stamp,
                    deadline,
                }
                .admit(),
            )),
            None => Ok(self.port.admit_access_point(
                Attempt {
                    context: homes.context.clone(),
                    ticket,
                    source: prepared,
                    access_category: category,
                    protection: Protection::None,
                    power,
                    coex_operation: operation,
                }
                .admit(),
            )),
        }
    }
}

#[cfg(test)]
mod tests;

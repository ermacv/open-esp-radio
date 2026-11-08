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
//! the deadline says nothing of its accuracy on the air.
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
//! # Ends
//!
//! A port fault ends the run with every owner kept ([`ApEnd::Faulted`]). A
//! command the port refuses ends the run with the runner, the refused
//! command and every other owner returned ([`ApEnd::Refused`]). A frame of
//! the air the engine refuses changes nothing; a frame the runner has no
//! ticket or buffer for does not go, which its peer's own retry covers.

pub mod network;

use core::future::poll_fn;
use core::num::NonZeroU8;
use core::task::{Context, Poll};

use embassy_futures::select::{Either, Either4, select, select4};
use oer_portable_time::{Instant, Timer};
use oer_protocols_ieee80211_contracts_port::{
    AccessPoint, AccessPointCommand, AccessPointCommands, AccessPointPort, AccessPointProgress,
    AccessPointReceive, AccessPointRefusal, Attempt, AttemptKey, BeaconAttempt, BeaconPermission,
    BeaconStamp, BeaconTimestamp, ConfigurePermission, Configured, ConnectionFrame, DataPort,
    Facts, GroupKeyId, InstalledGroup, InstalledPairwise, KeyTypes, LowerMacTerminal, NumberedKey,
    PhyRate, PortSink, PortTypes, Protection, ReceiveFilter, Removed, RemovedKey, RetryPlan,
    RetryStage, RxCryptoStatus, RxEvidence, RxMetadata, RxPull, SingleEnd, StopUnconfirmed,
    SupportedChannel, SupportedFilter, SupportedPower, SupportedTsfControl, TbttEvent,
    TerminalAccess, TsfControl, TxBufferClass, TxBufferMemory, TxBufferPool, TxCompleted, TxFacts,
    TxMpdu, TxOperation, TxOutcome, TxPower, TxProtection, TxSource,
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

/// The access-point commands that wait for their turn at most.
const COMMANDS: usize = 8;

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

/// Why the runner ended.
#[derive(Debug)]
pub enum ApEnd<R, F> {
    /// The access point stopped and the radio with it; every owner is back.
    Stopped(R),
    /// The radio did not start.
    StartRefused,
    /// The radio's stop is not confirmed: the port keeps what it holds.
    StopUnconfirmed(StopUnconfirmed),
    /// The port reported its named fault; the runner keeps every owner.
    Faulted(F),
    /// A terminal named an owner the runner did not send, or the engine
    /// refused one of the runner's own steps; the runner keeps every owner.
    Contract(ApContract),
    /// The port refused one of the access point's commands for its
    /// refusal: the runner stopped serving and returns with the port and
    /// every owner, the refused command among them
    /// ([`ApRunner::take_refused`]).
    Refused(R, AccessPointRefusal),
}

/// Why a step of the runner ended the run.
enum Halt<F> {
    Faulted(F),
    Contract(ApContract),
    /// The port refused a command, which the runner holds
    /// ([`ApRunner::take_refused`]).
    Refused(AccessPointRefusal),
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

/// Records every terminal of one port call in order.
struct Sink<'id, T: PortTypes<'id>> {
    terminals: heapless::Deque<LowerMacTerminal<'id, T>, 24>,
    overflow: bool,
    /// The units a receive cut handed over, read and recycled before the
    /// terminals of the same call.
    cut: heapless::Deque<(T::RxBuffer, T::RxMeta), CUT_UNITS>,
    /// The units a receive cut handed over: a cut that moves is progress.
    cut_units: u32,
}

/// The units of a receive cut one port call hands over at most.
const CUT_UNITS: usize = 16;

impl<'id, T: PortTypes<'id>> PortSink<'id, T> for Sink<'id, T> {
    fn terminal(&mut self, terminal: LowerMacTerminal<'id, T>) {
        if let Err(terminal) = self.terminals.push_back(terminal) {
            // The deque holds every terminal the runner's owners can owe;
            // one beyond it is kept from the drop with the overflow.
            let _kept = core::mem::ManuallyDrop::new(terminal);
            self.overflow = true;
        }
    }
    fn tbtt(&mut self, _: TbttEvent) {}
    fn received(&mut self, frame: T::RxBuffer, meta: T::RxMeta) {
        self.cut_units = self.cut_units.wrapping_add(1);
        // A receive cut's unit precedes the mutation's terminal: the runner
        // reads and recycles it before the call's terminals.
        if let Err(unit) = self.cut.push_back((frame, meta)) {
            let _kept = core::mem::ManuallyDrop::new(unit);
            self.overflow = true;
        }
    }
}

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
    held_message3: heapless::Vec<([u8; 6], ApEapolTransmit<AP_EAPOL_CAPACITY>), AP_KEYS>,
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
    commands_waiting: heapless::Deque<Command<'id, P>, COMMANDS>,
    phase: Phase,
    status: ApStatus,
    scratch: [u8; SCRATCH],
    sink: Sink<'id, Ty<'id, P>>,
    stop: bool,
    /// The command the port refused.
    refused: Option<Command<'id, P>>,
    /// Units, terminals and commands taken from the port, counted for the
    /// progress of a turn.
    taken: u32,
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
        Self {
            port,
            commands,
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
            commands_waiting: heapless::Deque::new(),
            phase: Phase::Configuring,
            status: ApStatus::default(),
            scratch: [0; SCRATCH],
            sink: Sink {
                terminals: heapless::Deque::new(),
                overflow: false,
                cut: heapless::Deque::new(),
                cut_units: 0,
            },
            stop: false,
            refused: None,
            taken: 0,
        }
    }

    /// Run the access point until its stop: start, serve, stop, and return
    /// the port and the runner's owners.
    pub async fn run<T: Timer>(
        mut self,
        shared: &ApShared,
        clock: &T,
    ) -> ApEnd<Self, PortFault<'id, P>> {
        // The radio starts on the BSS's channel.
        let started = poll_fn(|cx| {
            let now = clock.now();
            self.port.poll_start(cx, self.channel, now)
        })
        .await;
        if started.is_err() {
            return ApEnd::StartRefused;
        }
        if let Err(halt) = self.begin(clock.now()) {
            return self.halted(halt, shared);
        }
        let mut facts = Facts::default();
        // Turns in a row that changed nothing.
        let mut idle: u8 = 0;
        loop {
            let now = clock.now();
            let before = self.mark();
            let last = facts;
            if let Err(halt) = self.turn(now, core::mem::take(&mut facts)) {
                return self.halted(halt, shared);
            }
            shared.publish(self.status);
            if self.phase == Phase::StoppingRadio {
                break;
            }
            if !self.stop && shared.stop.get() {
                self.stop = true;
                if let Err(halt) = self.begin_stop(now) {
                    return self.halted(halt, shared);
                }
                continue;
            }
            // A turn that moved something may have left work for an earlier
            // stage of the next one (a terminal it took last queues a frame):
            // the runner polls its sources, which registers its waker with
            // the port, yields to the executor once and turns again.
            let moved = before != self.mark();
            idle = if moved { 0 } else { idle.saturating_add(1) };
            // A port may owe the next call more than this one took (a cut, a
            // repost): an idle turn yields and the sources decide, up to
            // IDLE_REPEATS turns in a row. Then the runner has settled: a
            // deadline it found due wakes it no more, and the very facts it
            // just served stay held for the next turn, never dropped, until a
            // wake of any source.
            let settled = idle >= IDLE_REPEATS;
            let again = moved || !settled;
            let deadline = [self.port.deadline(), self.engine_deadline()]
                .into_iter()
                .flatten()
                .filter(|at| !settled || *at > now)
                .min();
            let sends = self.phase == Phase::Running
                && !self.tickets.is_empty()
                && !self.buffers.is_empty();
            let stopping = self.stop;
            let port = &mut self.port;
            let network = &mut self.network;
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
        let stopped = poll_fn(|cx| {
            let now = clock.now();
            self.port.poll_stop(cx, now)
        })
        .await;
        self.status.running = false;
        shared.publish(self.status);
        match stopped {
            Ok(()) => ApEnd::Stopped(self),
            Err(unconfirmed) => ApEnd::StopUnconfirmed(unconfirmed),
        }
    }

    /// The end of a run a step halted: a refusal returns the runner with
    /// every owner, the access point no longer serving.
    fn halted(
        mut self,
        halt: Halt<PortFault<'id, P>>,
        shared: &ApShared,
    ) -> ApEnd<Self, PortFault<'id, P>> {
        match halt {
            Halt::Faulted(fault) => ApEnd::Faulted(fault),
            Halt::Contract(contract) => ApEnd::Contract(contract),
            Halt::Refused(refusal) => {
                self.status.running = false;
                self.network.set_link(false);
                shared.publish(self.status);
                ApEnd::Refused(self, refusal)
            }
        }
    }

    /// The command the port refused, once.
    pub fn take_refused(&mut self) -> Option<Command<'id, P>> {
        self.refused.take()
    }

    /// What a turn can change: equal marks before and after a turn mean
    /// it made no progress.
    fn mark(&self) -> Mark {
        Mark {
            phase: self.phase,
            status: self.status,
            owners: [
                self.tickets.len(),
                self.buffers.len(),
                self.commands_waiting.len(),
                self.teardowns.len(),
                self.installs.len(),
                self.installing.len(),
                self.keys.len(),
                self.removals.len(),
            ],
            network: self.network.waiting(),
            taken: self.taken.wrapping_add(self.sink.cut_units),
        }
    }

    /// The port, for the composition's release once the run ended.
    pub fn port_mut(&mut self) -> &mut P {
        &mut self.port
    }

    /// The engine's next work, while the access point serves.
    fn engine_deadline(&self) -> Option<Instant> {
        match self.phase {
            Phase::Running => self.engine.deadline(),
            _ => None,
        }
    }

    /// Start: configure the access point's receive rules.
    fn begin(&mut self, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        let permission = self
            .configure
            .take()
            .ok_or(Halt::Contract(ApContract::CommandRefused))?;
        let filter = SupportedFilter::new(ReceiveFilter::BSS_MEMBER)
            .ok_or(Halt::Contract(ApContract::CommandRefused))?;
        self.queue(AccessPointCommand::Configure {
            permission,
            receive: AccessPointReceive::Bss { filter },
            tsf: None,
        });
        self.run_commands(now)
    }

    /// Stop: tear every peer down, then close the interface.
    fn begin_stop(&mut self, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        self.phase = Phase::TearingDown;
        self.status.running = false;
        self.network.set_link(false);
        while let Some(teardown) = self.engine.stopping_peer() {
            self.teardown(teardown);
        }
        self.advance_stop(now)
    }

    /// Queue a teardown of `peer`.
    fn teardown(&mut self, teardown: ApTeardown) {
        let kind = if teardown.associated {
            ApPeerDisconnectKind::Disassociation
        } else {
            ApPeerDisconnectKind::Deauthentication
        };
        // A full queue keeps the earlier teardowns; this peer's key and
        // state end as the earlier ones do, without its frames.
        if self.teardowns.push_back((teardown, kind)).is_err() {
            self.finish_teardown(teardown.peer);
        }
    }

    /// One turn: the port's transmit service and terminals, the received
    /// units, the engine's due work, the network's frames and the waiting
    /// commands.
    fn turn(&mut self, now: Instant, facts: Facts) -> Result<(), Halt<PortFault<'id, P>>> {
        let due = self.port.deadline().is_some_and(|at| at <= now);
        if facts.tx.is_some() || facts.tbtt || due {
            let served = self.port.service_tx(
                facts.tx.unwrap_or(TxFacts::DUE),
                facts.tbtt,
                now,
                &mut self.sink,
            );
            if let Err(fault) = served {
                return Err(Halt::Faulted(fault));
            }
        }
        self.terminals(now)?;
        self.receive(now)?;
        self.terminals(now)?;
        if self.phase == Phase::Running {
            self.due(now)?;
            self.terminals(now)?;
            self.transmit_network(now)?;
            self.terminals(now)?;
        }
        self.send_teardowns(now)?;
        self.terminals(now)?;
        self.key_commands()?;
        self.run_commands(now)?;
        self.terminals(now)?;
        if self.stop {
            self.advance_stop(now)?;
        }
        let status = self.engine.service().status();
        self.status.associated = status.associated;
        self.status.authorized = status.authorized;
        Ok(())
    }

    /// Queue an access-point command behind the waiting ones.
    fn queue(&mut self, command: Command<'id, P>) {
        if let Err(command) = self.commands_waiting.push_back(command) {
            // The queue holds every command the runner's owners make at
            // once; one beyond it is kept from the drop.
            let _kept = core::mem::ManuallyDrop::new(command);
        }
    }

    /// Run the waiting commands in order: a held one waits for the next
    /// turn, and the ones behind it with it.
    fn run_commands(&mut self, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        while let Some(command) = self.commands_waiting.pop_front() {
            let progress = self.port.access_point_command(command, now, &mut self.sink);
            match progress {
                Ok(AccessPointProgress::Done) => {
                    self.taken = self.taken.wrapping_add(1);
                    self.terminals(now)?;
                }
                Ok(AccessPointProgress::Held(command)) => {
                    if let Err(command) = self.commands_waiting.push_front(command) {
                        let _kept = core::mem::ManuallyDrop::new(command);
                    }
                    return Ok(());
                }
                Ok(AccessPointProgress::Refused(command, refusal)) => {
                    // The command and its owners stay with the runner,
                    // which returns them; nothing behind it runs.
                    self.refused = Some(command);
                    return Err(Halt::Refused(refusal));
                }
                Err(fault) => return Err(Halt::Faulted(fault)),
            }
        }
        Ok(())
    }

    /// Issue the pairwise installations and removals the association
    /// allows: installations while it is home, one removal at a time, which
    /// holds it until its terminal.
    fn key_commands(&mut self) -> Result<(), Halt<PortFault<'id, P>>> {
        while let (Some(association), Some(peer)) =
            (self.association.as_ref(), self.installs.front().copied())
        {
            let _waiting: Option<[u8; 6]> = self.installs.pop_front();
            let Ok((key, peer_address)) = self.engine.pairwise_key(peer) else {
                // The peer left meanwhile.
                continue;
            };
            let Some(slot) = self.pairwise.pop() else {
                // No slot is vacant: the peer cannot be authorized.
                if let Ok(teardown) = self.engine.fail_handshake(peer) {
                    self.teardown(teardown);
                }
                continue;
            };
            let (command, identity) =
                self.commands
                    .install_access_point_pairwise(association, slot, peer_address, key);
            if self
                .installing
                .push((
                    peer,
                    PendingCcmpTxKey::issued(identity, self.policy.packet_numbers),
                ))
                .is_err()
            {
                let _kept = core::mem::ManuallyDrop::new(command);
                return Err(Halt::Contract(ApContract::ForeignKey));
            }
            self.queue(command);
        }
        if self.association.is_some()
            && let Some(key) = self.removals.pop_front()
            && let Some(association) = self.association.take()
        {
            self.queue(AccessPointCommand::RemovePairwise { association, key });
        }
        Ok(())
    }

    /// Take every terminal the port calls left in the sink, and the ones
    /// handling them left.
    fn terminals(&mut self, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        if self.sink.overflow {
            return Err(Halt::Contract(ApContract::UnknownTicket(u8::MAX)));
        }
        while let Some((frame, meta)) = self.sink.cut.pop_front() {
            let result = self.received(
                oer_protocols_ieee80211_contracts_port::RxBuffer::bytes(&frame),
                integrity(&meta),
                now,
            );
            self.port.recycle(frame);
            result?;
        }
        while let Some(terminal) = self.sink.terminals.pop_front() {
            self.taken = self.taken.wrapping_add(1);
            self.terminal(terminal, now)?;
        }
        Ok(())
    }

    /// One terminal.
    fn terminal(
        &mut self,
        terminal: LowerMacTerminal<'id, Ty<'id, P>>,
        now: Instant,
    ) -> Result<(), Halt<PortFault<'id, P>>> {
        match terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(completed)) => {
                self.completed(completed, now)
            }
            LowerMacTerminal::TxCompleted(TxCompleted::Beacon {
                completed,
                permission,
            }) => {
                self.beacon = Some(permission);
                self.completed(completed, now)
            }
            LowerMacTerminal::Configured(Configured::AccessPoint(permission)) => {
                self.configure = Some(permission);
                self.configured(now)
            }
            LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint {
                key,
                permission,
            }) => {
                self.group_permission = Some(permission);
                let Some(pending) = self.group_pending.take() else {
                    let _kept = core::mem::ManuallyDrop::new(key);
                    return Err(Halt::Contract(ApContract::ForeignKey));
                };
                let Some(key_id) = GroupKeyId::new(GROUP_KEY_ID) else {
                    let _kept = core::mem::ManuallyDrop::new(key);
                    return Err(Halt::Contract(ApContract::ForeignKey));
                };
                let installation = ApGroupKeyInstallation {
                    key_id,
                    generation: self.policy.group_generation,
                };
                let installed = match pending
                    .installed_group::<Keys<'id, P>, _>(key, |key| ApGroupKey { key, installation })
                {
                    Ok(installed) => installed,
                    Err(foreign) => {
                        let _kept = core::mem::ManuallyDrop::new(foreign);
                        return Err(Halt::Contract(ApContract::ForeignKey));
                    }
                };
                self.group = Some(installed);
                // The TSF restarts last.
                self.phase = Phase::StartingTsf;
                let Some(filter) = SupportedFilter::new(ReceiveFilter::BSS_MEMBER) else {
                    return Err(Halt::Contract(ApContract::CommandRefused));
                };
                self.configure_interface(
                    AccessPointReceive::Bss { filter },
                    Some(TsfControl::Restart),
                )
            }
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(key)) => {
                self.pairwise_installed(key, now)
            }
            LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Pairwise {
                slot,
                association,
            })) => {
                self.association = Some(association);
                if let Err(slot) = self.pairwise.push(slot) {
                    let _kept = core::mem::ManuallyDrop::new(slot);
                }
                Ok(())
            }
            LowerMacTerminal::KeyRemoved(Removed::AccessPoint(RemovedKey::Group {
                slot,
                permission,
            })) => {
                self.group_slot = Some(slot);
                self.group_permission = Some(permission);
                if self.phase == Phase::RemovingGroup {
                    self.phase = Phase::StoppingTsf;
                    self.configure_interface(AccessPointReceive::Off, Some(TsfControl::Stop))?;
                }
                Ok(())
            }
            // The station's terminals never come to the access point's
            // runner: no station command goes.
            other => {
                let _kept = core::mem::ManuallyDrop::new(other);
                Err(Halt::Contract(ApContract::UnknownTicket(u8::MAX)))
            }
        }
    }

    /// A peer's pairwise key is installed: its controlled port opens.
    fn pairwise_installed(
        &mut self,
        key: <Keys<'id, P> as KeyTypes<'id>>::PairwiseKey,
        now: Instant,
    ) -> Result<(), Halt<PortFault<'id, P>>> {
        let mut key = key;
        let mut matched = None;
        let mut index = 0;
        while index < self.installing.len() {
            let (peer, pending) = self.installing.swap_remove(index);
            match pending.installed::<Keys<'id, P>>(key) {
                Ok(installed) => {
                    matched = Some((peer, installed));
                    break;
                }
                Err(foreign) => {
                    key = foreign.key;
                    if let Err(back) = self.installing.push((peer, foreign.pending)) {
                        let _kept = core::mem::ManuallyDrop::new(back);
                    }
                    // The swapped-in entry now sits at `index`; the one put
                    // back went to the end.
                    index = index.saturating_add(1);
                }
            }
        }
        let Some((peer, installed)) = matched else {
            return Err(Halt::Contract(ApContract::ForeignKey));
        };
        let message3 = self
            .held_message3
            .iter()
            .position(|(owner, _)| *owner == peer)
            .map(|at| self.held_message3.swap_remove(at).1);
        if self.engine.service().peer_status(peer).is_none()
            || self.engine.pairwise_installed(peer).is_err()
        {
            // The peer left meanwhile: its key comes off, and its Message 3
            // does not go.
            if let Err(key) = self.removals.push_back(installed.into_key()) {
                let _kept = core::mem::ManuallyDrop::new(key);
            }
            return Ok(());
        }
        if let Err(installed) = self.keys.push((peer, installed)) {
            let _kept = core::mem::ManuallyDrop::new(installed);
            return Err(Halt::Contract(ApContract::ForeignKey));
        }
        // The key reads the station's Message 4 now, protected or not; the
        // controlled port stays closed until Message 4 is verified.
        if let Some(message3) = message3 {
            let (frame, context) = message3.into_parts();
            self.send_eapol(&frame, context, now)?;
        }
        Ok(())
    }

    /// The access point's configuration took effect: the next step of the
    /// start or the stop.
    fn configured(&mut self, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        match self.phase {
            Phase::Configuring => {
                // The group key goes in before the TSF starts.
                self.phase = Phase::InstallingGroup;
                let (Some(slot), Some(permission), Some(association)) = (
                    self.group_slot.take(),
                    self.group_permission.take(),
                    self.association.as_ref(),
                ) else {
                    return Err(Halt::Contract(ApContract::CommandRefused));
                };
                let Some(key_id) = GroupKeyId::new(GROUP_KEY_ID) else {
                    return Err(Halt::Contract(ApContract::CommandRefused));
                };
                self.group_pending = Some(PendingCcmpTxKey::issued(
                    <Keys<'id, P> as KeyTypes<'id>>::vacant_group_slot_identity(&slot),
                    self.policy.packet_numbers,
                ));
                let command = self.commands.install_access_point_group(
                    association,
                    permission,
                    slot,
                    key_id,
                    oer_protocols_ieee80211_contracts_security::Ccmp128Key::new(&self.group_key),
                );
                self.queue(command);
                Ok(())
            }
            Phase::StartingTsf => {
                self.phase = Phase::Running;
                self.engine.tsf_started(now);
                self.status.running = true;
                self.network.set_link(true);
                Ok(())
            }
            Phase::ClosingReceive => {
                // Its keys come off: every pairwise key went with its peer.
                self.phase = Phase::RemovingGroup;
                match (self.group.take(), self.group_permission.take()) {
                    (Some(group), Some(permission)) => {
                        self.queue(AccessPointCommand::RemoveGroup {
                            permission,
                            key: group.into_key().key,
                        });
                        Ok(())
                    }
                    (group, permission) => {
                        // No group key went in: the TSF stops at once.
                        drop(group);
                        self.group_permission = permission;
                        self.phase = Phase::StoppingTsf;
                        self.configure_interface(AccessPointReceive::Off, Some(TsfControl::Stop))
                    }
                }
            }
            Phase::StoppingTsf => {
                self.engine.tsf_stopped();
                self.phase = Phase::StoppingRadio;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Queue a configuration of the access point's interface.
    fn configure_interface(
        &mut self,
        receive: AccessPointReceive<Caps<'id, P>>,
        tsf: Option<TsfControl>,
    ) -> Result<(), Halt<PortFault<'id, P>>> {
        let Some(permission) = self.configure.take() else {
            return Err(Halt::Contract(ApContract::CommandRefused));
        };
        let tsf = match tsf {
            Some(control) => match SupportedTsfControl::new(control) {
                Some(control) => Some(control),
                None => {
                    self.configure = Some(permission);
                    return Err(Halt::Contract(ApContract::CommandRefused));
                }
            },
            None => None,
        };
        self.queue(AccessPointCommand::Configure {
            permission,
            receive,
            tsf,
        });
        Ok(())
    }

    /// Move the stop on: once every peer is gone and its key off, close the
    /// receive policy.
    fn advance_stop(&mut self, _now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        if self.phase != Phase::TearingDown {
            return Ok(());
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
            return Ok(());
        }
        self.phase = Phase::ClosingReceive;
        self.configure_interface(AccessPointReceive::Off, None)
    }

    /// A single MPDU's terminal: its ticket and buffer back, then what its
    /// purpose asks.
    fn completed(
        &mut self,
        completed: oer_protocols_ieee80211_contracts_port::SingleCompleted<'id, Ty<'id, P>>,
        now: Instant,
    ) -> Result<(), Halt<PortFault<'id, P>>> {
        let parts = completed.into_parts(&mut self.access);
        let index = parts.ticket.index();
        let outcome = parts.outcome;
        if let Err(ticket) = self.tickets.push(parts.ticket) {
            let _kept = core::mem::ManuallyDrop::new(ticket);
        }
        let (buffer, _) = parts.payload.into_parts();
        let buffer = match <Ty<'id, P> as TxBufferPool<'id, C>>::class(buffer) {
            Ok(buffer) => buffer,
            Err(buffer) => match <Ty<'id, P> as TxBufferPool<'id, C>>::sent(buffer) {
                Ok(sent) => <Ty<'id, P> as TxBufferPool<'id, C>>::renew(sent).0,
                Err(other) => {
                    let _kept = core::mem::ManuallyDrop::new(other);
                    return Err(Halt::Contract(ApContract::BufferClass));
                }
            },
        };
        self.give_buffer(buffer)?;
        let delivered = matches!(outcome, TxOutcome::Ended(SingleEnd::Delivered { .. }));
        let Some(purpose) = self
            .purposes
            .get_mut(usize::from(index))
            .and_then(Option::take)
        else {
            return Err(Halt::Contract(ApContract::UnknownTicket(index)));
        };
        match purpose {
            Purpose::Response(ApAfter::BeginWpa2 { peer }) if delivered && !self.stop => {
                // A peer that left meanwhile has no handshake to begin.
                if let Ok(transmit) = self.engine.begin_wpa2(peer) {
                    let (frame, context) = transmit.into_parts();
                    self.send_eapol(&frame, context, now)?;
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
                    self.finish_teardown(teardown.peer);
                }
            }
            Purpose::Teardown(teardown, ApPeerDisconnectKind::Deauthentication) => {
                self.finish_teardown(teardown.peer);
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
        Ok(())
    }

    /// Return a buffer to the pool, renewed for a new MPDU.
    fn give_buffer(
        &mut self,
        mut buffer: Buffer<'id, P, C>,
    ) -> Result<(), Halt<PortFault<'id, P>>> {
        if buffer.renew_msdu().is_err() {
            let _kept = core::mem::ManuallyDrop::new(buffer);
            return Err(Halt::Contract(ApContract::BufferClass));
        }
        if let Err(buffer) = self.buffers.push(buffer) {
            let _kept = core::mem::ManuallyDrop::new(buffer);
        }
        Ok(())
    }

    /// The peer's teardown frames went: its key comes off and the engine
    /// forgets it.
    fn finish_teardown(&mut self, peer: [u8; 6]) {
        self.remove_key(peer);
        self.installs.retain(|waiting| *waiting != peer);
        self.held_message3.retain(|(waiting, _)| *waiting != peer);
        // A peer that left by itself is gone already.
        if self.engine.forget(peer).is_err() {
            self.refuse();
        }
    }

    /// Count a step the engine refused.
    fn refuse(&mut self) {
        self.status.refused = self.status.refused.saturating_add(1);
    }

    /// Queue the removal of `peer`'s pairwise key, if it holds one.
    fn remove_key(&mut self, peer: [u8; 6]) {
        if let Some(at) = self.keys.iter().position(|(owner, _)| *owner == peer) {
            let (_, installed) = self.keys.swap_remove(at);
            if let Err(key) = self.removals.push_back(installed.into_key()) {
                let _kept = core::mem::ManuallyDrop::new(key);
            }
        }
    }

    /// Pull the received units of this turn and act on each.
    fn receive(&mut self, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        for _ in 0..RECEIVE_BUDGET {
            let pulled = match self.port.receive(RECEIVE_ROOM, now) {
                Ok(pulled) => pulled,
                Err(fault) => return Err(Halt::Faulted(fault)),
            };
            let RxPull::Unit { frame, meta } = pulled else {
                break;
            };
            self.taken = self.taken.wrapping_add(1);
            let result = self.received(
                oer_protocols_ieee80211_contracts_port::RxBuffer::bytes(&frame),
                integrity(&meta),
                now,
            );
            self.port.recycle(frame);
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
    ) -> Result<(), Halt<PortFault<'id, P>>> {
        let received = self
            .engine
            .receive(bytes, integrity, now, self.nonce, 1, &mut self.scratch);
        // A frame of the air the engine refuses changes nothing.
        let Ok(received) = received else {
            return Ok(());
        };
        match received {
            ApReceived::Nothing => Ok(()),
            ApReceived::Respond { frame, after } => {
                if matches!(after, ApAfter::BeginWpa2 { .. }) {
                    // The association took the ANonce.
                    self.nonce = self.nonces.nonce();
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
                self.remove_key(peer);
                self.installs.retain(|waiting| *waiting != peer);
                self.held_message3.retain(|(waiting, _)| *waiting != peer);
                Ok(())
            }
            ApReceived::Eapol { peer, body } => {
                let Some(group) = self.group.as_ref() else {
                    return Ok(());
                };
                match self.engine.eapol(peer, body, group) {
                    Ok(ApEapol::Nothing) | Err(_) => Ok(()),
                    Ok(ApEapol::Transmit(transmit)) => {
                        let (frame, context) = transmit.into_parts();
                        self.send_eapol(&frame, context, now)
                    }
                    Ok(ApEapol::Install { peer, message3 }) => {
                        // A new PTK of a peer whose key is in or going in
                        // is no handshake the runner serves: it fails.
                        let keyed = self.installs.iter().any(|waiting| *waiting == peer)
                            || self.installing.iter().any(|(owner, _)| *owner == peer)
                            || self.keys.iter().any(|(owner, _)| *owner == peer);
                        let held = !keyed
                            && self.installs.push_back(peer).is_ok()
                            && self.held_message3.push((peer, message3)).is_ok();
                        if !held {
                            self.installs.retain(|waiting| *waiting != peer);
                            if let Ok(teardown) = self.engine.fail_handshake(peer) {
                                self.teardown(teardown);
                            }
                        }
                        Ok(())
                    }
                    Ok(ApEapol::Authorize { peer }) => {
                        // Message 3 went only once the key was in.
                        let keyed = self.keys.iter().any(|(owner, _)| *owner == peer);
                        if (!keyed || self.engine.authorize(peer, now).is_err())
                            && let Ok(teardown) = self.engine.fail_handshake(peer)
                        {
                            self.teardown(teardown);
                        }
                        Ok(())
                    }
                    Ok(ApEapol::Deauthenticate { peer }) => {
                        if let Ok(teardown) = self.engine.fail_handshake(peer) {
                            self.teardown(teardown);
                        }
                        Ok(())
                    }
                }
            }
            ApReceived::Msdu(msdu) => {
                // A full network queue drops the MSDU: the stack's own
                // retries cover it.
                if self.engine.delivers(&msdu) && !self.network.deliver(msdu.parts()) {
                    self.status.rx_dropped = self.status.rx_dropped.saturating_add(1);
                }
                Ok(())
            }
        }
    }

    /// The engine's due work: the beacon, handshake retries and inactive
    /// peers.
    fn due(&mut self, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        if self.beacon.is_some() {
            self.send_beacon(now)?;
        }
        if let Some(group) = self.group.as_ref() {
            match self.engine.due_retry(now, group) {
                Ok(Some(ApDue::Transmit(transmit))) => {
                    let (frame, context) = transmit.into_parts();
                    self.send_eapol(&frame, context, now)?;
                }
                Ok(Some(ApDue::Teardown(teardown))) => self.teardown(teardown),
                Ok(None) | Err(_) => {}
            }
        }
        while let Some(teardown) = self.engine.due_inactive(now) {
            self.teardown(teardown);
        }
        Ok(())
    }

    /// Carry the network's frames into the BSS while tickets and buffers
    /// last.
    fn transmit_network(&mut self, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        for _ in 0..TRANSMIT_BUDGET {
            if self.tickets.is_empty() || self.buffers.is_empty() || !self.network.waiting() {
                break;
            }
            let Some(frame) = self.network.take() else {
                break;
            };
            let Some(mut buffer) = self.buffers.pop() else {
                break;
            };
            let parts = frame.parts();
            let written = match buffer.bytes_mut() {
                Ok(bytes) => self.engine.encapsulate(
                    parts.destination,
                    parts.source,
                    parts.ether_type,
                    parts.payload,
                    bytes,
                ),
                Err(_) => {
                    let _kept = core::mem::ManuallyDrop::new(buffer);
                    return Err(Halt::Contract(ApContract::BufferClass));
                }
            };
            match written {
                Ok(Some(written)) => {
                    self.submit(buffer, written, Purpose::Data, TxOperation::Data, None, now)?
                }
                // No authorized peer of that destination, or a frame the
                // BSS cannot carry: it does not go.
                Ok(None) | Err(_) => self.give_buffer(buffer)?,
            }
        }
        Ok(())
    }

    /// Send the queued teardown frames while tickets and buffers last.
    fn send_teardowns(&mut self, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        while !self.tickets.is_empty() && !self.buffers.is_empty() {
            let Some((teardown, kind)) = self.teardowns.pop_front() else {
                break;
            };
            match self
                .engine
                .write_teardown(teardown.peer, kind, &mut self.scratch)
            {
                Ok(frame) => self.send_scratch(
                    frame,
                    Purpose::Teardown(teardown, kind),
                    TxOperation::Data,
                    now,
                )?,
                Err(_) => self.finish_teardown(teardown.peer),
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
    ) -> Result<(), Halt<PortFault<'id, P>>> {
        match self.engine.write_eapol(frame, &mut self.scratch) {
            Ok(written) => self.send_scratch(
                written,
                Purpose::Eapol(context),
                TxOperation::Connection(ConnectionFrame::Eapol),
                now,
            ),
            Err(_) => {
                // The frame did not fit: its window returns unsent.
                if self.engine.eapol_sent(context, false, now).is_err() {
                    self.refuse();
                }
                Ok(())
            }
        }
    }

    /// Send the frame the scratch holds: copied into a port buffer, or
    /// dropped when no ticket or buffer is free.
    fn send_scratch(
        &mut self,
        frame: ApFrame,
        purpose: Purpose,
        operation: TxOperation,
        now: Instant,
    ) -> Result<(), Halt<PortFault<'id, P>>> {
        let Some(mut buffer) = self.buffers.pop() else {
            return self.unsent(purpose, now);
        };
        let copied = match (buffer.bytes_mut(), self.scratch.get(..frame.len)) {
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
            self.give_buffer(buffer)?;
            return self.unsent(purpose, now);
        }
        self.submit(buffer, frame, purpose, operation, None, now)
    }

    /// A frame of `purpose` did not go: what its terminal would have done
    /// without delivery.
    fn unsent(&mut self, purpose: Purpose, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        match purpose {
            Purpose::Eapol(context) => {
                if self.engine.eapol_sent(context, false, now).is_err() {
                    self.refuse();
                }
            }
            Purpose::Teardown(teardown, kind) => {
                // It goes when a ticket and a buffer are back.
                if self.teardowns.push_front((teardown, kind)).is_err() {
                    self.finish_teardown(teardown.peer);
                }
            }
            Purpose::Beacon(context) => {
                let _release: Option<_> = context.complete(ApBeaconTxOutcome::Unsent);
            }
            Purpose::Response(_) | Purpose::Data => {}
        }
        Ok(())
    }

    /// Write and submit the beacon due now.
    fn send_beacon(&mut self, now: Instant) -> Result<(), Halt<PortFault<'id, P>>> {
        if self.engine.next_beacon().is_none_or(|due| now < due) || self.tickets.is_empty() {
            return Ok(());
        }
        let Some(mut buffer) = self.buffers.pop() else {
            return Ok(());
        };
        let written = match buffer.bytes_mut() {
            Ok(bytes) => self.engine.beacon(now, bytes),
            Err(_) => {
                let _kept = core::mem::ManuallyDrop::new(buffer);
                return Err(Halt::Contract(ApContract::BufferClass));
            }
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
            Ok(None) => self.give_buffer(buffer),
            Err(error) => {
                self.give_buffer(buffer)?;
                Err(Halt::Contract(ApContract::Engine(error)))
            }
        }
    }

    /// Submit the frame `buffer` holds, as `frame` names it: a beacon with
    /// `deadline`, any other frame without one.
    fn submit(
        &mut self,
        buffer: Buffer<'id, P, C>,
        frame: ApFrame,
        purpose: Purpose,
        operation: TxOperation,
        deadline: Option<Instant>,
        now: Instant,
    ) -> Result<(), Halt<PortFault<'id, P>>> {
        let Some(ticket) = self.tickets.pop() else {
            self.give_buffer(buffer)?;
            return self.unsent(purpose, now);
        };
        let index = ticket.index();
        let admitted = match self.admit(ticket, buffer, frame, operation, deadline) {
            Ok(admitted) => admitted,
            Err((ticket, buffer)) => {
                if let Err(ticket) = self.tickets.push(ticket) {
                    let _kept = core::mem::ManuallyDrop::new(ticket);
                }
                if let Some(buffer) = buffer {
                    self.give_buffer(buffer)?;
                }
                return self.unsent(purpose, now);
            }
        };
        let Some(place) = self.purposes.get_mut(usize::from(index)) else {
            let _kept = core::mem::ManuallyDrop::new(admitted);
            return Err(Halt::Contract(ApContract::UnknownTicket(index)));
        };
        *place = Some(purpose);
        // A refused submission ends at once, withdrawn: its terminal in the
        // sink returns the owners and ends the purpose when the turn takes
        // the sink's terminals. Handling a terminal never takes them
        // itself, so a terminal that submits a frame does not recurse.
        let _refused = self.port.submit(admitted, now, &mut self.sink);
        Ok(())
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
    ) -> Result<<P as DataPort<'id>>::Admitted, (Ticket<'id, P>, Option<Buffer<'id, P, C>>)> {
        let policy = self.policy;
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
            return Err((ticket, Some(buffer)));
        };
        let protection = match frame.kind {
            ApFrameKind::Pairwise { peer, .. } => {
                let numbered = self
                    .keys
                    .iter()
                    .find(|(owner, _)| *owner == peer)
                    .and_then(|(_, key)| key.number());
                match numbered {
                    Some(numbered) => TxProtection::Ccmp(NumberedKey {
                        key: AttemptKey::Pairwise(numbered.key),
                        packet_number: numbered.packet_number,
                    }),
                    None => return Err((ticket, Some(buffer))),
                }
            }
            ApFrameKind::Group => match self.group.as_ref().and_then(InstalledCcmpTxKey::number) {
                Some(numbered) => TxProtection::Ccmp(NumberedKey {
                    key: AttemptKey::Group(&numbered.key.key),
                    packet_number: numbered.packet_number,
                }),
                None => return Err((ticket, Some(buffer))),
            },
            ApFrameKind::Management { .. } | ApFrameKind::Beacon => TxProtection::Open,
        };
        let source = TxSource::Fresh {
            buffer,
            frame: class_frame,
            body: None,
            protection,
        };
        let checked = match source.check() {
            Ok(checked) => checked,
            Err(refused) => {
                let buffer = match refused.operation {
                    TxSource::Fresh { buffer, .. } => Some(buffer),
                    TxSource::Retransmission { mpdu, .. } => {
                        let _kept = core::mem::ManuallyDrop::new(mpdu);
                        None
                    }
                };
                return Err((ticket, buffer));
            }
        };
        let prepared =
            match <Ty<'id, P> as TxBufferPool<'id, C>>::prepare::<AccessPoint, Caps<'id, P>>(
                checked,
            ) {
                Ok(prepared) => prepared,
                Err(refused) => {
                    let buffer = match refused.operation {
                        TxSource::Fresh { buffer, .. } => Some(buffer),
                        TxSource::Retransmission { mpdu, .. } => {
                            let _kept = core::mem::ManuallyDrop::new(mpdu);
                            None
                        }
                    };
                    return Err((ticket, buffer));
                }
            };
        let Some(power) = SupportedPower::new(TxPower::Calibrated) else {
            let _kept = core::mem::ManuallyDrop::new(prepared);
            return Err((ticket, None));
        };
        match (frame.kind, deadline) {
            (ApFrameKind::Beacon, Some(deadline)) => {
                let (Some(permission), Some(stamp)) = (
                    self.beacon.take(),
                    BeaconStamp::new(BeaconTimestamp::Caller),
                ) else {
                    let _kept = core::mem::ManuallyDrop::new(prepared);
                    return Err((ticket, None));
                };
                Ok(self.port.admit_beacon(
                    BeaconAttempt {
                        context: self.context.clone(),
                        ticket,
                        source: prepared,
                        power,
                        permission,
                        stamp,
                        deadline,
                    }
                    .admit(),
                ))
            }
            _ => Ok(self.port.admit_access_point(
                Attempt {
                    context: self.context.clone(),
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

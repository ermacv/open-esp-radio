//! The station's thin service on a lower-MAC port: the one loop that waits
//! and carries out what the sans-IO machines decide.
//!
//! It polls every input source at each waiting point: the port's next event, the machines' next
//! deadline and the application (a command, or an MSDU while the data
//! machine takes one). After each input it takes every action of the
//! arbiter ([`Arbiter`]), the station ([`Station`]) and its data machine
//! ([`StaData`]) until none owes one; an answer of the arbiter goes back to
//! the station in the same step. A retune leaves the phase: the port is
//! disabled, tuned and enabled, and the arbiter hears how it went.
//! Each step consumes at most one input per source and polls all sources even
//! when RX is ready. Batches stop after eight steps and yield, so a continuous
//! RX stream cannot suppress application commands, data TX or driver progress.
//! Station requests retain the application's correlation through either a
//! synchronous `CommandRefused` or their reserved `CommandCompleted` output;
//! link notices describe lifecycle independently of application requests.
//! `event` returns a typed contract error with the rejected owners. After the
//! first violation, disable retains later terminals sealed in `TerminalCustody`;
//! none enters Station, Data, Arbiter or rate selection. Its fixed capacity is
//! checked against the backend's terminal capacity at service construction.
//! A full bank returns the next terminal to the port, which retains it and all
//! remaining owners for poisoned teardown. `run` consumes its service and returns `ServiceCompleted`:
//! the port's terminal state and separate `RetiredService` resource banks.
//! On Stop, failure or poisoned disable, Station/Data finalize without issuing
//! Configure, key or transmit effects. Accepted requests return once before the
//! lifecycle notice. Unconsumed requests remain in their application queue.
//! `FailedInstallation` retains the Disabled stop proof and permits normal
//! uninstall only; recovery constructs a new service in a new installation.
//! Its failure does not itself require hardware reset. Failed and Poisoned
//! results retain `ServiceFault`: the enable refusal, backend/submission cause,
//! or precise contract violation. The poisoned owner also retains the disable
//! cause. Stop finishes the arbiter with `ArbiterEnd::Stopped`, preserving that
//! cause in every cancelled lease and retaining its restart bank.
//! `ServiceCompleted::into_stopped` grants `StoppedService` only after ordinary
//! Stop. Its consuming `restart` reuses the same pools, epoch source, lease
//! identity frontier, gate and data admission permissions. Startup reclaims
//! old keys and receive settings before taking queued application commands.
//! Deferred EAPOL frames return separately with that bank: they belong to the
//! old association. Failed or poisoned runs have no restart continuation.
//! The loop owns the working arbiter separately from Station/Data so a failed
//! return can consume it. Finalization accepts only `RetiredArbiter`; no active
//! arbiter or placeholder state remains in the retired service.
//! A command failure closes admission but still drains earlier terminals.
//! `EventEnd::Drained` is checked against the identities of every admitted
//! ticket and control permission; a missing return is a contract violation.
//! `EventEnd::Quarantined` goes directly to fail-stop, without another disable
//! or event wait. The common lifecycle retains the backend's owners even when
//! the `PortEnd` quarantine receipt is dropped. These transitions follow the
//! portable stack's lifecycle contract.
//!
//! The tokens the machines hold are the port's: tickets and buffers of two
//! classes, the small one for the station's frames and the large one for
//! data. A completion carries `TxContext`, which routes its ticket and
//! resources to the original machine and link. Buffer classes are used
//! only to recover their memory types; `StationTxBuffer` exhaustively
//! represents both classes and both publication states at this boundary.
//! Machines receive `ReturnedMpdu::Fresh` or `Sent` and decide when to renew.
//! Cross-AC completion order has no bearing on ownership. Each machine
//! returns stale-link resources without advancing its current protocol,
//! and the service excludes those terminals from current-link rate control.
//! A data operation's closed context supplies its frame, access category and
//! checked `DataProtection`. Submission refuses a changed link or protection
//! before preparation and returns the original context, ticket and buffer.
//! Data admission is an owned permit per AC, including old-link work until
//! its terminal, and preserves the station's Voice waiting bound.
//! Preparation returns the original typed source on refusal; context and
//! ticket have not left this service. Submission can refuse only poisoning.
//! The application hears `ServiceOutput::TxRefused`; Station gets `Unsent`
//! and Data gets `Refused`. Neither path fabricates a completion or updates
//! rate statistics, and the service continues. A `Sent` return retains its
//! external body until its machine retires the MPDU; service never renews
//! a completion merely to route it. `StationPort::Types` fixes `TxBody` to
//! `Infallible`, so a returned external body is uninhabited at this boundary.
//! GTK actions move the unique mutation permission into the port. Their
//! private pending token stays here until the matching FIFO slot terminal;
//! admission never fabricates installation success. RX continues through the
//! old replay epoch while the command is pending. Configure retains its one pending token until the port
//! reports it applied; Station cannot issue another before that return.
//! Arbiter answers return their affine places/leases in the same step.
//! Commands and MSDUs use affine SPSC queues with atomic wakeups. Each
//! readiness future holds its endpoint exclusively and consumes no value;
//! a port fault leaves a simultaneously ready command in its queue. A received
//! MSDU the network's full queue cannot take is dropped, as an Ethernet
//! receiver drops one.
//! `WakingSpscQueue::split` exclusively borrows its storage. Both endpoints retain
//! that borrow, so no second pair can coexist. After the pair returns, queued
//! values remain owned by the queue and reach the next receiver in order.
//! Network frames are the data machine's checked [`EthernetFrame`]s; a
//! received payload too long for one is refused, never clipped.

mod commands;
pub mod contract;
mod events;
pub use events::{RxDrop, RxMsdusRefused, ServiceEvent, StationEvents};
mod control;
mod reconnect;
use commands::CommandCredits;
pub use commands::{CommandRefused, CommandTerminal, Request};
pub use control::{
    Control, ControlCreateRefused, ControlError, ControlRefused, Device, Events, RunnerControl,
    State, Status, StatusReader,
};
pub use reconnect::{ReconnectPolicy, ZeroReconnectInterval};

use core::marker::PhantomData;
use oer_events::{EventSink, report};
use oer_ieee80211_core_roles_sta::machine::TxRefusal;

use core::future::{Future, poll_fn};
use core::task::Poll;
use embassy_futures::yield_now;
use oer_crypto::KeyUnwrap;
use oer_ieee80211_contracts_port::{
    ClassFrame, Ieee80211Capabilities, InstalledGroup, InterfacePhase, KeyTypes, LowerMacBackend,
    LowerMacEvent, LowerMacPhase, LowerMacTerminal, PhaseSingleOutcome, PhaseTypes, PoolMpdu,
    PoolSource, Removed, RemovedKey, RetransmissionPlan, ReturnedMpdu, RxBuffer, StationPhase,
    SupportedChannel, TxBufferClass, TxBufferPool, TxCompleted, TxMpdu, TxProtection, TxSource,
};
use oer_ieee80211_core_arbiter::{
    Arbiter, ArbiterAction, ArbiterEnd, ArbiterNotice, NoReservation, RetiredArbiter, RetuneRefused,
};
use oer_ieee80211_core_roles_sta::port::{PortDataTokens, PortStaTokens, StationPairwiseKey};
use oer_ieee80211_core_roles_sta::{
    data,
    data::{
        DataInput, DataProtection, DataReturn, DataTxContext, EthernetFrame, FrameTooLong, Msdu,
        StaData, TxMsdu,
    },
    machine::{
        KEY_DATA_CAPACITY, KeyUnwrapId, Peer, RxBlockAckReply, StaAction, StaCallbacks, StaInput,
        StaKeyUnwrapRequest, StaNotice, StaTokens, StaTransmit, StaTxContext, Station,
    },
};
use oer_ieee80211_core_upper_mac::block_ack::RxDisposition;
use oer_ieee80211_core_upper_mac::rate_selection::{FrameRates, RateSelection};
use oer_memory::{WakingSpscReceiver, WakingSpscSender};
use oer_radio_port::Slot;
use oer_radio_port::{
    Disabled, Enabled, Ended, EventEnd, FailStopRequested, Failed, Owner, PoisonedOwner,
    PortBackend,
};
use oer_time::{Instant, Timer};

/// The composition's mapping from its concrete backend to station resources.
/// This is a mapping rather than a backend supertrait because orphan rules and
/// BodyLoan's unreplaceable backend identity prevent an image-owned backend impl.
pub trait StationPort: Sized {
    /// The concrete lower-MAC backend; its lifecycle owns every hardware resource.
    type Backend: LowerMacBackend<Self::SlotId, Caps = Self::Caps>;
    /// Capability family shared by the backend and portable machines.
    type Caps: Ieee80211Capabilities;
    /// Nominal identity of this backend's registered slot.
    type SlotId: oer_radio_port::SlotIdentity;
    /// Public phase/resource family under each separate body loan.
    type Types<'id>: PhaseTypes<
            'id,
            StationConfigureContext = oer_ieee80211_core_roles_sta::machine::StaConfigurationContext<
                'id,
                <Self::Types<'id> as PhaseTypes<'id>>::EpochPath,
            >,
            Fault = <Self::Backend as PortBackend<Self::SlotId>>::Fault,
            TxBody = core::convert::Infallible,
            Context = TxContext<
                'id,
                Self::Caps,
                Self::Large,
                <Self::Types<'id> as PhaseTypes<'id>>::EpochPath,
            >,
            EpochPath: oer_radio_port::PoolPath<Identity = Self::SlotId>,
            TicketPath: oer_radio_port::PoolPath<Identity = Self::SlotId>,
        > + TxBufferPool<'id, Self::Small>
        + TxBufferPool<'id, Self::Large>
    where
        Self: 'id,
        Self::Backend: 'id,
        Self::SlotId: 'id;
    /// Fixed terminal custody from the backend's capacity layout.
    type TerminalStorage<T>: TerminalStorage<T>;
    /// Buffer class for station management traffic.
    type Small: TxBufferClass;
    /// Buffer class for connected data traffic.
    type Large: TxBufferClass;

    /// Lend only the public phase, retaining the body's loan and slot identity.
    fn lend_phase<'a, 'id, 'r>(
        phase: &'r mut <Self::Backend as PortBackend<Self::SlotId>>::Phase<'a, 'id>,
    ) -> &'r mut impl LowerMacPhase<'a, 'id, Caps = Self::Caps, Types = Self::Types<'id>>
    where
        Self: 'id,
        Self::Backend: 'id,
        Self::SlotId: 'id,
        'id: 'a;
    /// Preserve the complete public terminal family at the disable boundary.
    fn terminal<'id>(
        terminal: <Self::Backend as PortBackend<Self::SlotId>>::Terminal<'id>,
    ) -> LowerMacTerminal<'id, Self::Types<'id>>
    where
        Self: 'id,
        Self::Backend: 'id,
        Self::SlotId: 'id;
    /// Return the same rejected public terminal to the backend's custody.
    fn port_terminal<'id>(
        terminal: LowerMacTerminal<'id, Self::Types<'id>>,
    ) -> <Self::Backend as PortBackend<Self::SlotId>>::Terminal<'id>
    where
        Self: 'id,
        Self::Backend: 'id,
        Self::SlotId: 'id;

    /// Tune disabled settings under their existing body loan.
    fn tune<'a, 'id>(
        setup: &mut <Self::Backend as PortBackend<Self::SlotId>>::Setup<'a, 'id>,
        channel: SupportedChannel<Self::Caps>,
    ) where
        Self: 'id,
        Self::Backend: 'id,
        Self::SlotId: 'id,
        'id: 'a;
    /// Preserve the complete public event family.
    fn event<'id>(
        event: LowerMacEvent<'id, Self::Types<'id>>,
    ) -> LowerMacEvent<'id, Self::Types<'id>>
    where
        Self: 'id,
        Self::Backend: 'id,
        Self::SlotId: 'id;
    /// Observe the start or end of a stretch of the service's work.
    fn span(_span: ServiceSpan, _end: bool) {}
    /// Recover either complete resource class, without an unknown-class branch.
    fn tx_buffer<'id>(
        buffer: <Self::Types<'id> as PhaseTypes<'id>>::AnyTxBuffer,
    ) -> StationTxBuffer<'id, Self>
    where
        Self: 'id,
        Self::Backend: 'id,
        Self::SlotId: 'id;
}

/// The only TX memory a station service's port returns. This describes
/// resources, not operation identity: only `TxContext` selects the operation.
pub enum StationTxBuffer<'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id> {
    /// The station's fresh allocation or its published MPDU.
    Small(PoolMpdu<'id, B::Types<'id>, B::Small>),
    /// Data's fresh allocation or its published MPDU.
    Large(PoolMpdu<'id, B::Types<'id>, B::Large>),
}

/// The composition's context for this service. Each variant owns the
/// original machine operation; the backend treats the entire value as opaque.
#[derive(Debug)]
pub enum TxContext<
    'id,
    K,
    C,
    Path: oer_radio_port::PoolPath = oer_radio_port::Root<oer_radio_port::LocalId<'id>>,
> {
    /// A station protocol operation, including its connection epoch and purpose.
    Station(StaTxContext<'id, Path>),
    /// An MSDU operation, including its original link, TID and queue order.
    Data(DataTxContext<'id, K, C, oer_ieee80211_core_roles_sta::data::AcPermit, Path>),
}

/// Owned operation context selected before the service's body loan.
/// Epoch fields remain opaque until their public projection restores that loan.
pub type TxOwnedContext<K, C, Path> = oer_radio_port::ContextChoice<
    oer_ieee80211_core_roles_sta::epoch::EpochContext<
        Path,
        oer_ieee80211_core_roles_sta::machine::StaTxPurpose,
    >,
    oer_ieee80211_core_roles_sta::epoch::EpochContext<
        Path,
        oer_ieee80211_core_roles_sta::data::DataTxInfo<
            K,
            C,
            oer_ieee80211_core_roles_sta::data::AcPermit,
        >,
    >,
>;

impl<'p, K: 'p, C: 'p, Path: oer_radio_port::PoolPath + 'p>
    oer_radio_port::LoanedContext<'p, Path::Identity> for TxContext<'p, K, C, Path>
where
    Path::Identity: 'p,
{
    type Owned = TxOwnedContext<K, C, Path>;
    fn enclose(self) -> oer_radio_port::PortContext<'p, Path::Identity, Self::Owned> {
        match self {
            Self::Station(context) => context.enclose().first(),
            Self::Data(context) => context.enclose().second(),
        }
    }
    fn restore(context: oer_radio_port::PortContext<'p, Path::Identity, Self::Owned>) -> Self {
        match context.into_choice() {
            oer_radio_port::ContextChoice::First(context) => {
                Self::Station(StaTxContext::restore(context))
            }
            oer_radio_port::ContextChoice::Second(context) => {
                Self::Data(DataTxContext::restore(context))
            }
        }
    }
}

/// A stretch of the service's own work, reported to its port at both ends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ServiceSpan {
    /// The machines' owed actions.
    Step,
    /// Released and deferred received frames.
    Pending,
    /// One poll of every input source.
    Sources,
    /// A received frame's event.
    Received,
    /// A frame's delivery to the station machine.
    Deliver,
    /// One MSDU's hand-off to the network queue.
    Queue,
    /// A transmission's terminal.
    Terminal,
    /// The network stack's MSDU to the data machine.
    Send,
    /// A received buffer's return.
    Recycle,
    /// EXPERIMENT: an MSDU the network queue refused.
    Refused,
}

/// Each step polls every source and consumes at most one input from each.
/// Eight steps amortize wakeup cost while forcing a cooperative yield before
/// a continuously ready source can monopolize the executor.
const MAX_INPUT_STEPS_PER_BATCH: u8 = 8;

/// A station frame's transmission on the port `B`.
type StationTransmit<'id, B, const GROUPS: usize> = StaTransmit<
    'id,
    PortStaTokens<'id, <B as StationPort>::Types<'id>, <B as StationPort>::Small, GROUPS>,
    <B as StationPort>::Caps,
>;

/// A data MPDU's operation on the port `B`.
type DataContext<'id, B> = DataTxContext<
    'id,
    <B as StationPort>::Caps,
    <B as StationPort>::Large,
    oer_ieee80211_core_roles_sta::data::AcPermit,
    <<B as StationPort>::Types<'id> as PhaseTypes<'id>>::EpochPath,
>;

/// The metadata the port `B` gives a received frame.
pub type PortRxMeta<'id, B> = <<B as StationPort>::Types<'id> as PhaseTypes<'id>>::RxMeta;
/// The ACK evidence of the port `B`.
pub type PortAck<'id, B> = <<B as StationPort>::Types<'id> as PhaseTypes<'id>>::AckEvidence;

/// The station of the service on the port `B`.
pub type PortStation<'id, B, C, const GROUPS: usize = 2> = Station<
    'id,
    PortStaTokens<'id, <B as StationPort>::Types<'id>, <B as StationPort>::Small, GROUPS>,
    <B as StationPort>::Caps,
    C,
>;

/// The terminal bank of the service's station on the port `B`.
pub type PortRetiredStation<'id, B, C, const GROUPS: usize = 2> =
    oer_ieee80211_core_roles_sta::machine::RetiredStation<
        'id,
        PortStaTokens<'id, <B as StationPort>::Types<'id>, <B as StationPort>::Small, GROUPS>,
        <B as StationPort>::Caps,
        C,
    >;

/// A stopped service's restart: the running service, or the bank it could
/// not restart.
type Restart<'p, 'id, B, C, S, const GROUPS: usize, O> = Result<
    RestartedService<'p, 'id, B, C, S, GROUPS, O>,
    RestartFailed<'p, 'id, B, C, S, GROUPS, O>,
>;

/// An uninstalled port's end and the service's retired bank.
type Uninstalled<'id, B, C, S, const GROUPS: usize, O, Out> = (
    Ended<<B as StationPort>::SlotId, <B as StationPort>::Backend, Out>,
    RetiredService<'id, B, C, S, GROUPS, O>,
);

/// The data machine of the service on the port `B`.
pub type PortData<'id, B> = StaData<
    'id,
    PortDataTokens<'id, <B as StationPort>::Types<'id>, <B as StationPort>::Large>,
    <B as StationPort>::Caps,
    8,
>;

/// The arbiter of the service on the port `B`.
pub type PortArbiter<'id, B> = Arbiter<
    'id,
    <B as StationPort>::Caps,
    oer_ieee80211_core_arbiter::NoReservation,
    <<B as StationPort>::Types<'id> as PhaseTypes<'id>>::GatePath,
>;

// Preserve the terminal-routing frame when static dispatch removes the
// vtable call boundary; the target remains a direct generic call.
#[inline(never)]
fn retain_after_event_end<'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id>(
    custody: &mut TerminalCustody<'id, B>,
    terminal: <B::Backend as PortBackend<B::SlotId>>::Terminal<'id>,
) -> core::ops::ControlFlow<<B::Backend as PortBackend<B::SlotId>>::Terminal<'id>> {
    let terminal = B::terminal(terminal);
    let result = match &mut *custody {
        TerminalCustody::Clear => {
            *custody =
                ContractFault::sealed(BackendContractViolation::TerminalAfterEventEnd, terminal)
                    .into();
            core::ops::ControlFlow::Continue(())
        }
        TerminalCustody::Quarantined(bank) => match bank.retain(terminal) {
            Ok(()) => core::ops::ControlFlow::Continue(()),
            Err(terminal) => core::ops::ControlFlow::Break(terminal),
        },
    };
    result.map_break(B::port_terminal)
}

mod admissions;
mod rx_block_ack;
use admissions::Admissions;
pub use admissions::{ControlTerminal, PendingTerminal};
use rx_block_ack::PhaseBlockAcks;

pub use rx_block_ack::{PortRxBlockAck, RxBlockAckStorage, RxBlockAckView};

mod custody;
mod key_unwrap;
use custody::{ContractFault, StepFailure};
pub use custody::{
    QuarantinedTerminals, RejectedTerminal, TerminalCustody, TerminalStorage, UnroutedTx,
};
pub use key_unwrap::KeyUnwrapSlot;

/// What the application asks of the service.
#[derive(Debug, Eq, PartialEq)]
pub enum ServiceCommand<K> {
    /// A command of the station.
    Station(Request),
    /// A fact of the access point beside the station.
    AccessPoint(contract::AccessPointFact<K>),
    /// The access point beside the station is wanted.
    WantAccessPoint,
    /// Stop: the service disables the port and returns its owner.
    Stop,
}

/// What the service tells the application.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "a refusal returns the original owned packet without allocation"
)]
pub enum ServiceOutput<K> {
    CommandStarted(Request),
    /// The original network operation refused before data admission.
    MsduRefused {
        packet: contract::TransmitPacket,
        reason: contract::MsduRefusal,
    },
    /// An operation refused before admission. Its resources return and the
    /// service continues; this is not a TX completion or radio statistic.
    TxRefused {
        /// The machine that originated the operation.
        operation: TxOperation,
        /// The preparation failure.
        reason: contract::TransmitRefusal,
    },
    /// The station's notice.
    Notice(contract::Notice),
    /// A request refused synchronously, with its original correlation.
    CommandRefused(CommandRefused),
    /// A raw producer submitted a second request without the first terminal.
    CommandInFlight(Request),
    /// The unique terminal of one admitted station request.
    CommandCompleted(CommandTerminal),
    /// The arbiter's command to the access point beside the station.
    AccessPoint(contract::AccessPointCommand<K>),
}

/// The origin of a preparation refusal reported to the application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxOperation {
    /// A station protocol frame.
    Station,
    /// A connected-link MSDU.
    Data,
}

/// How a transmission returned: its port terminal's outcome `O`, or the
/// service's refusal before submission.
#[derive(Clone, Copy)]
pub enum ReturnedTx<O> {
    Completed(O),
    Refused(TxRefusal),
}

/// A returned transmission of the service on the port `B`.
pub type PortReturnedTx<'id, B> =
    ReturnedTx<PhaseSingleOutcome<'id, <B as StationPort>::Types<'id>>>;

/// A backend returned owners that cannot belong to the submitted operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendContractViolation {
    UnexpectedAggregate,
    UnexpectedTerminal(PendingTerminal),
    TerminalAfterEventEnd,
    UnexpectedAccessPointTerminal,
    UnexpectedTsfSample,
    GroupTerminalWithoutRequest,
    GroupSlotMismatch,
    GroupTerminalKindMismatch,
    TxContextBufferMismatch,
    StationKeyBank(contract::KeyContractViolation),
    /// The station asked for a second key-data unwrap while one was waiting
    /// to start; it owns one ticket.
    StationKeyUnwrap,
}

/// The original failure that ended this service. A poisoned disabled owner
/// additionally retains the backend's disable failure through `cause()`.
pub enum ServiceFault<B: StationPort> {
    PolicyDeadlineOverflow,
    /// A checked management request exceeded its retained response domain.
    RxBlockAckCapacity,
    /// A validated receive BA reply could not fit the control frame encoder.
    RxBlockAckEncode,

    EnableRefused(<B::Backend as PortBackend<B::SlotId>>::Refusal),
    Backend(<B::Backend as PortBackend<B::SlotId>>::Fault),
    Submission(<B::Backend as PortBackend<B::SlotId>>::Fault),
    Contract(BackendContractViolation),
    DrainedWithOutstanding {
        cause: <B::Backend as PortBackend<B::SlotId>>::Fault,
        missing: PendingTerminal,
    },
}

/// A concrete output recipient. Events carry their original owners by value.
pub trait ServiceOutputSink<K> {
    fn emit(&mut self, output: ServiceOutput<K>);
}

impl<K, F: FnMut(ServiceOutput<K>)> ServiceOutputSink<K> for F {
    fn emit(&mut self, output: ServiceOutput<K>) {
        self(output);
    }
}

/// The service's ends to the application and the network stack.
///
/// Commands require an exclusive SPSC receiver rather than a shareable channel:
///
/// ```compile_fail,E0308
/// use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Channel};
/// use oer_ieee80211_runtime::station::{Control, State, ServiceCommand, ServiceIo, ServiceOutput};
/// use oer_ieee80211_core_roles_sta::data::{EthernetFrame, TxMsdu};
/// use oer_memory::WakingSpscQueue;
/// let commands = Channel::<NoopRawMutex, ServiceCommand<()>, 1>::new();
/// let mut tx = WakingSpscQueue::<TxMsdu, 1>::new();
/// let mut rx = WakingSpscQueue::<EthernetFrame, 1>::new();
/// let (_, transmit) = tx.split();
/// let (receive, _) = rx.split();
/// let mut output = |_: ServiceOutput<()>| {};
/// let mut control_queue = WakingSpscQueue::<ServiceCommand<()>, 1>::new();
/// let (application, _) = control_queue.split();
/// let state = State::new();
/// let (_control, control) = Control::new(application, &state).unwrap();
/// let io = ServiceIo {
///     commands: commands.dyn_receiver(), transmit, receive, output: &mut output, control,
/// };
/// ```
///
/// The composition moves one receiver into the service and retains its sole sender:
///
/// ```
/// use oer_ieee80211_runtime::station::{Control, State, ServiceCommand, ServiceIo, ServiceOutput};
/// use oer_ieee80211_core_roles_sta::data::{EthernetFrame, TxMsdu};
/// use oer_memory::WakingSpscQueue;
/// let mut commands = WakingSpscQueue::<ServiceCommand<()>, 1>::new();
/// let mut tx = WakingSpscQueue::<TxMsdu, 1>::new();
/// let mut rx = WakingSpscQueue::<EthernetFrame, 1>::new();
/// let (application, commands) = commands.split();
/// let (_, transmit) = tx.split();
/// let (receive, _) = rx.split();
/// let mut output = |_: ServiceOutput<()>| {};
/// let state = State::new();
/// let (mut application, control) = Control::new(application, &state).unwrap();
/// let io = ServiceIo { commands, transmit, receive, output: &mut output, control };
/// assert!(application.try_command(ServiceCommand::Stop).is_ok());
/// assert!(matches!(io.commands.try_receive(), Some(ServiceCommand::Stop)));
/// ```
pub struct ServiceIo<
    'c,
    K,
    O: ServiceOutputSink<K>,
    const TX_DEPTH: usize,
    const RX_DEPTH: usize,
    const COMMAND_DEPTH: usize,
> {
    /// The application's exclusive command receiver. Readiness does not
    /// remove a command; a simultaneous port fault leaves it in this queue.
    pub commands: WakingSpscReceiver<'c, ServiceCommand<K>, COMMAND_DEPTH>,
    /// The MSDUs the network stack sends.
    pub transmit: WakingSpscReceiver<'c, TxMsdu, TX_DEPTH>,
    /// The MSDUs the station received for the network stack.
    pub receive: WakingSpscSender<'c, EthernetFrame, RX_DEPTH>,
    /// What the service tells the application.
    pub output: &'c mut O,
    /// Affine cooperative stop and completion connection to Control.
    pub control: RunnerControl<'c>,
}

/// How the service ended.
pub enum ServiceEnd<'p, 'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id> {
    /// Stopped on the application's command: the port is disabled.
    Stopped(Owner<'p, 'id, B::SlotId, B::Backend, Disabled>),
    /// The port failed (it refused to come back to a channel, or a phase
    /// reported it poisoned and its disable still ended): it is disabled,
    /// and the arbiter stopped the station.
    Failed {
        owner: FailedInstallation<'p, 'id, B>,
        fault: ServiceFault<B>,
        custody: TerminalCustody<'id, B>,
    },
    /// Stream quarantine or a false drained claim ends admission without
    /// another disable. The request is completed by the lifecycle supervisor.
    Quarantined {
        request: FailStopRequested<B::SlotId, B::Backend>,
        fault: ServiceFault<B>,
        custody: TerminalCustody<'id, B>,
    },
    /// The original failure and the poisoned backend's disable outcome.
    Poisoned {
        owner: PoisonedOwner<'p, 'id, B::SlotId, B::Backend>,
        fault: ServiceFault<B>,
        custody: TerminalCustody<'id, B>,
    },
}

/// The loop returns only lifecycle authority; the large custody bank stays
/// in its caller's placed storage until final output construction.
enum ServiceStop<'p, 'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id> {
    Stopped(Owner<'p, 'id, B::SlotId, B::Backend, Disabled>),
    Failed {
        owner: FailedInstallation<'p, 'id, B>,
        fault: ServiceFault<B>,
    },
    Quarantined {
        request: FailStopRequested<B::SlotId, B::Backend>,
        fault: ServiceFault<B>,
    },
    Poisoned {
        owner: PoisonedOwner<'p, 'id, B::SlotId, B::Backend>,
        fault: ServiceFault<B>,
    },
}

impl<'p, 'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id> ServiceStop<'p, 'id, B> {
    fn into_end(self, custody: TerminalCustody<'id, B>) -> ServiceEnd<'p, 'id, B> {
        match self {
            Self::Stopped(owner) => ServiceEnd::Stopped(owner),
            Self::Failed { owner, fault } => ServiceEnd::Failed {
                owner,
                fault,
                custody,
            },
            Self::Quarantined { request, fault } => ServiceEnd::Quarantined {
                request,
                fault,
                custody,
            },
            Self::Poisoned { owner, fault } => ServiceEnd::Poisoned {
                owner,
                fault,
                custody,
            },
        }
    }

    #[cfg(feature = "placement")]
    #[inline(never)]
    fn init_end<'out>(
        self,
        output: &'out mut core::mem::MaybeUninit<ServiceEnd<'p, 'id, B>>,
        custody: oer_memory::Placed<'_, TerminalCustody<'id, B>>,
    ) -> &'out mut ServiceEnd<'p, 'id, B> {
        let end = match self {
            Self::Stopped(owner) => output.write(ServiceEnd::Stopped(owner)),
            Self::Failed { owner, fault } => output.write(ServiceEnd::Failed {
                owner,
                fault,
                custody: TerminalCustody::Clear,
            }),
            Self::Quarantined { request, fault } => output.write(ServiceEnd::Quarantined {
                request,
                fault,
                custody: TerminalCustody::Clear,
            }),
            Self::Poisoned { owner, fault } => output.write(ServiceEnd::Poisoned {
                owner,
                fault,
                custody: TerminalCustody::Clear,
            }),
        };
        match end {
            ServiceEnd::Stopped(_) => drop(custody),
            ServiceEnd::Failed {
                custody: destination,
                ..
            }
            | ServiceEnd::Quarantined {
                custody: destination,
                ..
            }
            | ServiceEnd::Poisoned {
                custody: destination,
                ..
            } => custody.replace_into(destination),
        }
        end
    }
}

/// A service failure with a stopped backend. Its only continuation is normal
/// uninstall; starting another service requires a new installation. This is
/// not a poisoned owner and does not require an otherwise unnecessary reset.
pub struct FailedInstallation<'p, 'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id> {
    owner: Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
}

impl<'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id> FailedInstallation<'_, 'id, B> {
    /// Retire the old installation normally, preserving Disabled's stop proof.
    pub fn uninstall<Out>(
        self,
        control: <B::Backend as PortBackend<B::SlotId>>::Uninstall,
        output: Out,
    ) -> Ended<B::SlotId, B::Backend, Out> {
        Ended::uninstall(self.owner, control, output)
    }
}

/// A completed run and all resource banks it owned. Neither machine in the
/// returned banks has a run/admission interface.
pub struct ServiceCompleted<
    'p,
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S,
    const GROUPS: usize = 2,
    O: StationEvents = (),
> {
    pub end: ServiceEnd<'p, 'id, B>,
    pub retired: RetiredService<'id, B, C, S, GROUPS, O>,
}

/// Only a normal Stop grants this continuation on the existing installation.
pub struct StoppedService<
    'p,
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S,
    const GROUPS: usize = 2,
    O: StationEvents = (),
> {
    owner: Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
    retired: RetiredService<'id, B, C, S, GROUPS, O>,
}

/// The same port owner and resources, ready for a new service run. Deferred
/// EAPOL owners belong to the old association and return separately.
pub struct RestartedService<
    'p,
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S,
    const GROUPS: usize = 2,
    O: StationEvents = (),
> {
    pub owner: Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
    pub service: Runner<'id, B, C, S, GROUPS, O>,
    pub deferred_eapol: oer_ieee80211_core_roles_sta::machine::DeferredEapol,
    pub notice: Option<oer_ieee80211_core_roles_sta::machine::StaNotice>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestartFailure {
    PendingTerminal,
    MissingGateTerminal,
    MissingStationPlaces,
    FailedArbiter,
}

/// An incomplete stop cannot mint replacement resources or enable a new run.
/// Normal uninstall remains possible, with every original owner retained.
pub struct RestartFailed<
    'p,
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S,
    const GROUPS: usize = 2,
    O: StationEvents = (),
> {
    pub reason: RestartFailure,
    pub owner: FailedInstallation<'p, 'id, B>,
    pub retired: RetiredService<'id, B, C, S, GROUPS, O>,
}

impl<
    'p,
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S,
    const GROUPS: usize,
    O: StationEvents,
> ServiceCompleted<'p, 'id, B, C, S, GROUPS, O>
{
    /// Extract the reusable continuation only from the ordinary Stop outcome.
    #[expect(
        clippy::result_large_err,
        reason = "a failed completion returns all resource banks without allocation"
    )]
    pub fn into_stopped(self) -> Result<StoppedService<'p, 'id, B, C, S, GROUPS, O>, Self> {
        match self.end {
            ServiceEnd::Stopped(owner) => Ok(StoppedService {
                owner,
                retired: self.retired,
            }),
            end => Err(Self {
                end,
                retired: self.retired,
            }),
        }
    }
}

impl<
    'p,
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S: RateSelection<PortRxMeta<'id, B>, PortAck<'id, B>>,
    const GROUPS: usize,
    O: StationEvents,
> StoppedService<'p, 'id, B, C, S, GROUPS, O>
{
    /// Resume on the selected channel without minting pools, epochs, leases,
    /// gate authority or admission permissions. Startup reclaims old keys and
    /// receive configuration before taking queued application commands.
    #[expect(
        clippy::result_large_err,
        reason = "a broken stop returns the complete affine bank without allocation"
    )]
    pub fn restart(
        self,
        channel: oer_ieee80211_contracts_port::SupportedChannel<B::Caps>,
    ) -> Restart<'p, 'id, B, C, S, GROUPS, O> {
        let Self {
            mut owner,
            mut retired,
        } = self;
        if retired._pending.admissions.first().is_some()
            || retired._pending.retune.is_some()
            || retired._pending.group.is_some()
        {
            return Err(RestartFailed {
                reason: RestartFailure::PendingTerminal,
                owner: FailedInstallation { owner },
                retired,
            });
        }
        let (arbiter, places) = match retired._arbiter {
            RetiredArbiter::Stopped(stopped) => match stopped.restart(channel, retired._places) {
                Ok(restarted) => restarted.into_parts(),
                Err(missing) => {
                    retired._arbiter = RetiredArbiter::Stopped(missing.bank);
                    retired._places = missing.places;
                    return Err(RestartFailed {
                        reason: match missing.missing {
                            oer_ieee80211_core_arbiter::MissingArbiterReturn::GateTerminal => {
                                RestartFailure::MissingGateTerminal
                            }
                            oer_ieee80211_core_arbiter::MissingArbiterReturn::StationPlaces => {
                                RestartFailure::MissingStationPlaces
                            }
                        },
                        owner: FailedInstallation { owner },
                        retired,
                    });
                }
            },
            failed => {
                retired._arbiter = failed;
                return Err(RestartFailed {
                    reason: RestartFailure::FailedArbiter,
                    owner: FailedInstallation { owner },
                    retired,
                });
            }
        };
        let (station, deferred_eapol, notice) = retired.station.restart(places).into_parts();
        let RetiredService {
            data,
            events,
            rates,
            _pending: pending,
            _terminal_access: terminal_access,
            ..
        } = retired;
        {
            let mut setup = owner.setup();
            B::tune(&mut setup, channel);
        }
        Ok(RestartedService {
            owner,
            service: Runner {
                arbiter,
                state: ServiceState {
                    station,
                    data: data.restart(),
                    events,
                    rates,
                    pending,
                    terminal_access,
                },
            },
            deferred_eapol,
            notice,
        })
    }

    /// Retire the disabled installation and retain its resource bank.
    pub fn uninstall<Out>(
        self,
        control: <B::Backend as PortBackend<B::SlotId>>::Uninstall,
        output: Out,
    ) -> Uninstalled<'id, B, C, S, GROUPS, O, Out> {
        (Ended::uninstall(self.owner, control, output), self.retired)
    }
}

/// The completed service's owners and diagnostics. Only `StoppedService`
/// combines this bank with the matching reusable disabled port owner.
pub struct RetiredService<
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S,
    const GROUPS: usize = 2,
    O: StationEvents = (),
> {
    pub station: PortRetiredStation<'id, B, C, GROUPS>,
    pub data: oer_ieee80211_core_roles_sta::data::RetiredData<
        'id,
        PortDataTokens<'id, B::Types<'id>, B::Large>,
        B::Caps,
        8,
    >,
    pub rates: S,
    /// The composition's event sink, which a restart carries on.
    events: O,
    _places: oer_ieee80211_core_arbiter::StationPlaceReturns<
        'id,
        <B::Types<'id> as PhaseTypes<'id>>::GatePath,
    >,
    _arbiter:
        RetiredArbiter<'id, B::Caps, NoReservation, <B::Types<'id> as PhaseTypes<'id>>::GatePath>,
    _pending: PendingActions<'id, B>,
    _terminal_access: oer_ieee80211_contracts_port::TerminalAccess<
        'id,
        <B::Types<'id> as PhaseTypes<'id>>::TicketKind,
        <B::Types<'id> as PhaseTypes<'id>>::TicketPath,
    >,
}

/// A constructed station and the original permissions issued with it.
pub type ConstructedStation<'id, B, C, const GROUPS: usize> = (
    PortStation<'id, B, C, GROUPS>,
    oer_ieee80211_core_roles_sta::machine::StaCommandCredits<
        'id,
        <<B as StationPort>::Types<'id> as PhaseTypes<'id>>::StationCommandPath,
    >,
);

/// The station's service: its machines and the rate selection of its frames.
/// Disable returns only command terminals; the backend recycles unsolicited RX.
pub struct Runner<
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S,
    const GROUPS: usize = 2,
    O: StationEvents = (),
> {
    arbiter: PortArbiter<'id, B>,
    state: ServiceState<'id, B, C, S, GROUPS, O>,
}

struct ServiceState<
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S,
    const GROUPS: usize = 2,
    O: StationEvents = (),
> {
    /// The station.
    station: PortStation<'id, B, C, GROUPS>,
    /// Its data machine.
    data: PortData<'id, B>,
    /// The rates of the station's frames and data.
    rates: S,
    /// The composition's sink, lent to each call of the station, its data
    /// machine and its receive agreements, and the service's own events.
    events: O,
    terminal_access: oer_ieee80211_contracts_port::TerminalAccess<
        'id,
        <B::Types<'id> as PhaseTypes<'id>>::TicketKind,
        <B::Types<'id> as PhaseTypes<'id>>::TicketPath,
    >,
    pending: PendingActions<'id, B>,
}

struct PendingActions<'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id> {
    receive: RxBlockAckView<'id, B::Types<'id>>,
    commands: CommandCredits<'id, <B::Types<'id> as PhaseTypes<'id>>::StationCommandPath>,
    admissions: Admissions,
    /// A retune whose disable/enable failed remains owned until retirement.
    retune: Option<
        oer_ieee80211_core_arbiter::PendingRetune<
            'id,
            B::Caps,
            <B::Types<'id> as PhaseTypes<'id>>::GatePath,
        >,
    >,
    group: Option<PendingGroup<'id, B>>,
    /// The station's unwrap request, until the key-unwrap slot takes it.
    unwrap: Option<(KeyUnwrapId, StaKeyUnwrapRequest)>,
}

struct PendingGroup<'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id> {
    slot: <<B::Types<'id> as PhaseTypes<'id>>::StationKeys as KeyTypes<'id>>::GroupSlotIdentity,
    key_id: Option<oer_ieee80211_contracts_port::GroupKeyId>,
    action: PendingGroupAction<'id, <B::Types<'id> as PhaseTypes<'id>>::EpochPath>,
}

enum PendingGroupAction<'id, Path: oer_radio_port::PoolPath> {
    Install(oer_ieee80211_core_roles_sta::machine::PendingGroupKey<'id, Path>),
    Replace(oer_ieee80211_core_roles_sta::machine::PendingGroupReplacement<'id, Path>),
    Select(oer_ieee80211_core_roles_sta::machine::PendingGroupSelection<'id, Path>),
    Remove(oer_ieee80211_core_roles_sta::machine::PendingGroupRemoval<'id, Path>),
}

/// What a step left the loop to do.
enum Next<'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id> {
    /// Wait for the next input.
    Wait,
    /// Move the port to the channel.
    Retune(
        oer_ieee80211_core_arbiter::PendingRetune<
            'id,
            B::Caps,
            <B::Types<'id> as PhaseTypes<'id>>::GatePath,
        >,
    ),
    /// The port is poisoned.
    Failed(StepFailure<'id, B>),
}

impl<
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S: RateSelection<PortRxMeta<'id, B>, PortAck<'id, B>>,
    const GROUPS: usize,
    O: StationEvents,
> Runner<'id, B, C, S, GROUPS, O>
{
    /// The service of the machines the composition built, its frames at
    /// the rates `rates` selects.
    pub fn new(
        station: ConstructedStation<'id, B, C, GROUPS>,
        data: PortData<'id, B>,
        arbiter: PortArbiter<'id, B>,
        rates: S,
        terminal_access: oer_ieee80211_contracts_port::TerminalAccess<
            'id,
            <B::Types<'id> as PhaseTypes<'id>>::TicketKind,
            <B::Types<'id> as PhaseTypes<'id>>::TicketPath,
        >,
        receive: RxBlockAckView<'id, B::Types<'id>>,
        events: O,
    ) -> Self {
        // The port's values, checked as the type is built.
        const {
            let _power = DataContext::<'id, B>::POWER;
            if B::TerminalStorage::<LowerMacTerminal<'id, B::Types<'id>>>::CAPACITY
                < <B::Types<'id> as PhaseTypes<'id>>::TERMINAL_CAPACITY
            {
                panic!("the service retains the backend's complete terminal capacity");
            }
        };
        let (station, credits) = station;
        Self {
            arbiter,
            state: ServiceState {
                station,
                data,
                rates,
                terminal_access,
                events,
                pending: PendingActions {
                    receive,
                    commands: CommandCredits::new(credits),
                    admissions: Admissions::default(),
                    retune: None,
                    group: None,
                    unwrap: None,
                },
            },
        }
    }

    /// Construct both machines directly in the caller's backing. The returned
    /// loan owns the service; running it consumes that ownership as usual.
    #[allow(clippy::too_many_arguments)]
    #[cfg(feature = "placement")]
    #[inline(never)]
    pub fn init_in_place<'s>(
        storage: &'s mut core::mem::MaybeUninit<Self>,
        station: impl for<'a> FnOnce(
            &'a mut core::mem::MaybeUninit<PortStation<'id, B, C, GROUPS>>,
        ) -> (
            &'a mut PortStation<'id, B, C, GROUPS>,
            oer_ieee80211_core_roles_sta::machine::StaCommandCredits<
                'id,
                <B::Types<'id> as PhaseTypes<'id>>::StationCommandPath,
            >,
        ),
        data: impl for<'a> FnOnce(
            &'a mut core::mem::MaybeUninit<PortData<'id, B>>,
        ) -> &'a mut PortData<'id, B>,
        arbiter: PortArbiter<'id, B>,
        rates: S,
        terminal_access: oer_ieee80211_contracts_port::TerminalAccess<
            'id,
            <B::Types<'id> as PhaseTypes<'id>>::TicketKind,
            <B::Types<'id> as PhaseTypes<'id>>::TicketPath,
        >,
        receive: RxBlockAckView<'id, B::Types<'id>>,
        events: O,
    ) -> oer_memory::Placed<'s, Self> {
        const {
            let _power = DataContext::<'id, B>::POWER;
            if B::TerminalStorage::<LowerMacTerminal<'id, B::Types<'id>>>::CAPACITY
                < <B::Types<'id> as PhaseTypes<'id>>::TERMINAL_CAPACITY
            {
                panic!("the service retains the backend's complete terminal capacity");
            }
        };
        let credits = core::cell::Cell::new(CommandCredits::empty());
        oer_memory::Placed::init(storage, |place| {
            oer_memory::init_in_place!(place, Self {
                arbiter: arbiter,
                state => |place: &mut core::mem::MaybeUninit<ServiceState<'id, B, C, S, GROUPS, O>>| oer_memory::init_in_place!(place, ServiceState {
                    station => |place| { let (station, granted) = station(place); credits.set(CommandCredits::new(granted)); station },
                    data => data,
                    rates: rates,
                    terminal_access: terminal_access,
                    events: events,
                    pending: PendingActions { receive, commands: credits.into_inner(), admissions: Admissions::default(), retune: None, group: None, unwrap: None },
                }),
            })
        })
    }

    /// Consume a placed run right and retire into caller-owned storage. Both
    /// success and fault paths retain the same banks as the owned run API.
    #[cfg(feature = "placement")]
    pub async fn run_placed<
        's,
        'out,
        'p,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        const COMMAND_DEPTH: usize,
    >(
        service: oer_memory::Placed<'s, Self>,
        output: &'out mut core::mem::MaybeUninit<ServiceCompleted<'p, 'id, B, C, S, GROUPS, O>>,
        owner: Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
        timer: &impl Timer,
        unwrap: &mut KeyUnwrapSlot<impl KeyUnwrap<KEY_DATA_CAPACITY, Operation: Unpin>>,
    ) -> oer_memory::Placed<'out, ServiceCompleted<'p, 'id, B, C, S, GROUPS, O>> {
        let (arbiter, mut state) = oer_memory::unpack_placed!(service, Self { arbiter, state });
        let mut custody_storage = core::mem::MaybeUninit::uninit();
        let mut custody = oer_memory::Placed::init(&mut custody_storage, |place| {
            place.write(TerminalCustody::Clear)
        });
        let (end, arbiter) = state
            .run_loop(arbiter.read(), owner, io, timer, unwrap, &mut custody)
            .await;
        state.drain_key_unwrap(timer, unwrap).await;
        let reason = match &end {
            ServiceStop::Stopped(_) => {
                oer_ieee80211_core_roles_sta::machine::StaDisconnectReason::Requested
            }
            _ => oer_ieee80211_core_roles_sta::machine::StaDisconnectReason::Arbiter(
                oer_ieee80211_core_arbiter::LeaveReason::PortFailed,
            ),
        };
        let (station, data, mut rates, mut events, terminal_access, pending) = oer_memory::unpack_placed!(
            state,
            ServiceState {
                station,
                data,
                rates,
                events,
                terminal_access,
                pending
            }
        );
        let completed = oer_memory::Placed::init(output, |place| {
            oer_memory::init_in_place!(place, ServiceCompleted {
                end => |place| end.init_end(place, custody),
                retired => |place: &mut core::mem::MaybeUninit<RetiredService<'id, B, C, S, GROUPS, O>>| {
                    // Destructure the complete finish result before constructing
                    // the retired bank with its station and returned places.
                    let mut station_storage = core::mem::MaybeUninit::uninit();
                    let (station, places) = Station::finish_placed(station, &mut station_storage, reason);
                    let mut retired = oer_memory::Placed::init(place, |place| oer_memory::init_in_place!(place, RetiredService {
                        station => |place| station.move_to(place).into_mut(),
                        data => |place| StaData::finish_placed(data, place, &mut *events).into_mut(),
                        rates: { rates.disconnected(); rates.read() },
                        events: events.read(),
                        _places: places,
                        _arbiter: arbiter,
                        _pending: pending.read(),
                        _terminal_access: terminal_access.read(),
                    }));
                    while let Some(notice) = retired._arbiter.poll_notice() {
                        match notice {
                            ArbiterNotice::Station(notice) => retired._places.returned(notice),
                            ArbiterNotice::AccessPoint(command) => io.output.emit(ServiceOutput::AccessPoint(command.into())),
                        }
                    }
                    while let Some(output) = retired.station.poll_output() {
                        match output {
                            oer_ieee80211_core_roles_sta::machine::StaFinalOutput::CommandCompleted(terminal) =>
                                io.output.emit(ServiceOutput::CommandCompleted(retired._pending.commands.completed(terminal))),
                            oer_ieee80211_core_roles_sta::machine::StaFinalOutput::Notify(notice) =>
                                io.output.emit(ServiceOutput::Notice(notice.into())),
                        }
                    }
                    retired.into_mut()
                },
            })
        });
        io.control.complete();
        completed
    }

    /// Consume the service and run until Stop or port failure. Completion
    /// returns terminal resource banks, never another runnable service.
    pub async fn run<
        'p,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        const COMMAND_DEPTH: usize,
    >(
        self,
        owner: Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
        timer: &impl Timer,
        unwrap: &mut KeyUnwrapSlot<impl KeyUnwrap<KEY_DATA_CAPACITY, Operation: Unpin>>,
    ) -> ServiceCompleted<'p, 'id, B, C, S, GROUPS, O> {
        let Self { arbiter, mut state } = self;
        let mut custody = TerminalCustody::Clear;
        let (end, mut arbiter) = state
            .run_loop(arbiter, owner, io, timer, unwrap, &mut custody)
            .await;
        state.drain_key_unwrap(timer, unwrap).await;
        let end = end.into_end(custody);
        let reason = match &end {
            ServiceEnd::Stopped(_) => {
                oer_ieee80211_core_roles_sta::machine::StaDisconnectReason::Requested
            }
            ServiceEnd::Failed { .. }
            | ServiceEnd::Poisoned { .. }
            | ServiceEnd::Quarantined { .. } => {
                oer_ieee80211_core_roles_sta::machine::StaDisconnectReason::Arbiter(
                    oer_ieee80211_core_arbiter::LeaveReason::PortFailed,
                )
            }
        };
        let ServiceState {
            station,
            data,
            mut rates,
            mut events,
            mut pending,
            terminal_access,
        } = state;
        let (mut station, mut places) = station.finish(reason);
        let data = data.finish(&mut events);
        rates.disconnected();
        while let Some(notice) = arbiter.poll_notice() {
            match notice {
                ArbiterNotice::Station(notice) => places.returned(notice),
                ArbiterNotice::AccessPoint(command) => {
                    io.output.emit(ServiceOutput::AccessPoint(command.into()))
                }
            }
        }
        while let Some(output) = station.poll_output() {
            match output {
                oer_ieee80211_core_roles_sta::machine::StaFinalOutput::CommandCompleted(
                    terminal,
                ) => io.output.emit(ServiceOutput::CommandCompleted(
                    pending.commands.completed(terminal),
                )),
                oer_ieee80211_core_roles_sta::machine::StaFinalOutput::Notify(notice) => {
                    io.output.emit(ServiceOutput::Notice(notice.into()))
                }
            }
        }
        io.control.complete();
        ServiceCompleted {
            end,
            retired: RetiredService {
                station,
                data,
                rates,
                events,
                _arbiter: arbiter,
                _places: places,
                _pending: pending,
                _terminal_access: terminal_access,
            },
        }
    }
}

impl<
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S: RateSelection<PortRxMeta<'id, B>, PortAck<'id, B>>,
    const GROUPS: usize,
    O: StationEvents,
> ServiceState<'id, B, C, S, GROUPS, O>
{
    async fn run_loop<
        'p,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        const COMMAND_DEPTH: usize,
    >(
        &mut self,
        arbiter: PortArbiter<'id, B>,
        owner: Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
        timer: &impl Timer,
        unwrap: &mut KeyUnwrapSlot<impl KeyUnwrap<KEY_DATA_CAPACITY, Operation: Unpin>>,
        custody: &mut TerminalCustody<'id, B>,
    ) -> (
        ServiceStop<'p, 'id, B>,
        RetiredArbiter<'id, B::Caps, NoReservation, <B::Types<'id> as PhaseTypes<'id>>::GatePath>,
    ) {
        // An operation left by a dropped run belongs to a station that no
        // longer exists: its engine finishes it before this station's first.
        let _abandoned = unwrap.finish().await;
        let (mut arbiter, mut owner) = match self.enable(arbiter, owner, None, io, timer).await {
            Ok(running) => running,
            Err(ended) => return ended,
        };
        loop {
            let next = {
                let mut lent = owner.phase();
                let phase = B::lend_phase(&mut lent);
                self.serve(&mut arbiter, phase, io, timer, unwrap).await
            };
            match next {
                Served::Stop => {
                    let owner = self.disable(&mut arbiter, owner, timer, custody).await;
                    let end = stop_disabled(owner, custody);
                    let arbiter_end = match &end {
                        ServiceStop::Stopped(_) => ArbiterEnd::Stopped,
                        ServiceStop::Failed { .. }
                        | ServiceStop::Poisoned { .. }
                        | ServiceStop::Quarantined { .. } => ArbiterEnd::PortFailed,
                    };
                    return (end, arbiter.finish(arbiter_end));
                }
                Served::Failed(failure) => {
                    let fault = failure.retain_in(custody);
                    let owner = self.disable(&mut arbiter, owner, timer, custody).await;
                    let end = match owner {
                        Ok(owner) => ServiceStop::Failed {
                            owner: FailedInstallation { owner },
                            fault,
                        },
                        Err(owner) => ServiceStop::Poisoned { owner, fault },
                    };
                    return (end, arbiter.finish(ArbiterEnd::PortFailed));
                }
                Served::StreamEnd { end, fault } => {
                    let cause = match end {
                        EventEnd::Drained { cause } | EventEnd::Quarantined { cause } => cause,
                    };
                    if matches!(end, EventEnd::Quarantined { .. }) {
                        return (
                            ServiceStop::Quarantined {
                                request: owner.fail_stop(),
                                fault,
                            },
                            arbiter.finish(ArbiterEnd::PortFailed),
                        );
                    }
                    if let Some(missing) = self.pending.admissions.first() {
                        return (
                            ServiceStop::Quarantined {
                                request: owner.fail_stop(),
                                fault: ServiceFault::DrainedWithOutstanding { cause, missing },
                            },
                            arbiter.finish(ArbiterEnd::PortFailed),
                        );
                    }
                    // Drained promises there can be no further terminal.
                    // A violating disable return stays sealed, never reaching a machine.
                    let mut sink = |terminal| retain_after_event_end::<B>(custody, terminal);
                    let end = match owner.disable(&mut sink).await {
                        Ok(owner) => ServiceStop::Failed {
                            owner: FailedInstallation { owner },
                            fault,
                        },
                        Err(owner) => ServiceStop::Poisoned { owner, fault },
                    };
                    return (end, arbiter.finish(ArbiterEnd::PortFailed));
                }
                Served::Retune(pending) => {
                    let disabled = self.disable(&mut arbiter, owner, timer, custody).await;
                    let disabled = match stop_disabled(disabled, custody) {
                        ServiceStop::Stopped(owner) => owner,
                        end => {
                            self.pending.retune = Some(pending);
                            return (end, arbiter.finish(ArbiterEnd::PortFailed));
                        }
                    };
                    (arbiter, owner) = match self
                        .enable(arbiter, disabled, Some(pending), io, timer)
                        .await
                    {
                        Ok(running) => running,
                        Err(ended) => return ended,
                    };
                }
            }
        }
    }

    /// Enable the port, tuned to `channel` first when it moves; a refused
    /// move goes back to the arbiter, which names the next.
    async fn enable<
        'p,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        const COMMAND_DEPTH: usize,
    >(
        &mut self,
        mut arbiter: PortArbiter<'id, B>,
        mut owner: Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
        mut pending: Option<
            oer_ieee80211_core_arbiter::PendingRetune<
                'id,
                B::Caps,
                <B::Types<'id> as PhaseTypes<'id>>::GatePath,
            >,
        >,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
        timer: &impl Timer,
    ) -> Result<
        (
            PortArbiter<'id, B>,
            Owner<'p, 'id, B::SlotId, B::Backend, Enabled>,
        ),
        (
            ServiceStop<'p, 'id, B>,
            RetiredArbiter<
                'id,
                B::Caps,
                NoReservation,
                <B::Types<'id> as PhaseTypes<'id>>::GatePath,
            >,
        ),
    > {
        loop {
            if let Some(pending) = pending.as_ref() {
                let mut setup = owner.setup();
                B::tune(&mut setup, pending.channel());
            }
            match owner.enable().await {
                Ok(enabled) => {
                    if let Some(pending) = pending {
                        arbiter.retuned(pending);
                    }
                    return Ok((arbiter, enabled));
                }
                Err(Failed::Refused(disabled, reason)) => {
                    owner = disabled;
                    let Some(requested) = pending.take() else {
                        // Initial enable has no retune action to answer.
                        return Err((
                            ServiceStop::Failed {
                                owner: FailedInstallation { owner },
                                fault: ServiceFault::EnableRefused(reason),
                            },
                            arbiter.finish(ArbiterEnd::PortFailed),
                        ));
                    };
                    match arbiter.retune_refused(requested) {
                        RetuneRefused::Return {
                            arbiter: returning,
                            pending: next,
                        } => {
                            arbiter = returning;
                            pending = Some(next);
                        }
                        RetuneRefused::Failed(failed) => {
                            return Err((
                                ServiceStop::Failed {
                                    owner: FailedInstallation { owner },
                                    fault: ServiceFault::EnableRefused(reason),
                                },
                                RetiredArbiter::PortFailed(failed),
                            ));
                        }
                    }
                    let now = timer.now();
                    // A disabled port receives only notices. The return retune
                    // came directly with its affine token, without polling
                    // actions that could consume a gate.
                    self.deliver_arbiter_notices(&mut arbiter, now, io);
                }
                Err(Failed::Poisoned(poisoned)) => {
                    self.pending.retune = pending.take();
                    return Err((
                        ServiceStop::Poisoned {
                            fault: ServiceFault::Backend(poisoned.cause()),
                            owner: poisoned,
                        },
                        arbiter.finish(ArbiterEnd::PortFailed),
                    ));
                }
            }
        }
    }

    // Static dispatch can share the terminal/fault temporaries with the
    // caller instead of retaining a separate routing frame below it.
    #[inline(always)]
    fn disable_terminal(
        &mut self,
        arbiter: &mut PortArbiter<'id, B>,
        timer: &impl Timer,
        custody: &mut TerminalCustody<'id, B>,
        terminal: <B::Backend as PortBackend<B::SlotId>>::Terminal<'id>,
    ) -> core::ops::ControlFlow<<B::Backend as PortBackend<B::SlotId>>::Terminal<'id>> {
        let terminal = B::terminal(terminal);
        let result = match custody {
            TerminalCustody::Clear => {
                if let Err(fault) = self.terminal(arbiter, timer.now(), terminal) {
                    *custody = fault.into();
                }
                core::ops::ControlFlow::Continue(())
            }
            TerminalCustody::Quarantined(bank) => match bank.retain(terminal) {
                Ok(()) => core::ops::ControlFlow::Continue(()),
                Err(terminal) => core::ops::ControlFlow::Break(terminal),
            },
        };
        result.map_break(B::port_terminal)
    }

    /// Disable routes terminals only up to the first contract violation.
    /// The first rejected owner and every later sealed terminal are retained.
    /// A full bank returns the next owner to the port for poisoned teardown.
    async fn disable<'p>(
        &mut self,
        arbiter: &mut PortArbiter<'id, B>,
        owner: Owner<'p, 'id, B::SlotId, B::Backend, Enabled>,
        timer: &impl Timer,
        custody: &mut TerminalCustody<'id, B>,
    ) -> Result<
        Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
        PoisonedOwner<'p, 'id, B::SlotId, B::Backend>,
    > {
        let mut sink = |terminal| self.disable_terminal(arbiter, timer, custody, terminal);
        owner.disable(&mut sink).await
    }

    /// Serve the running phase until the port must move, stop or failed.
    async fn serve<
        'p,
        P,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        const COMMAND_DEPTH: usize,
    >(
        &mut self,
        arbiter: &mut PortArbiter<'id, B>,
        phase: &mut P,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
        timer: &impl Timer,
        unwrap: &mut KeyUnwrapSlot<impl KeyUnwrap<KEY_DATA_CAPACITY, Operation: Unpin>>,
    ) -> Served<'id, B>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        // Each step consumes at most one input per source. Every source is
        // polled even when RX is ready; eight steps bound a batch before a
        // cooperative yield lets the driver and application progress.
        let mut batch = 0_u8;
        loop {
            B::span(ServiceSpan::Step, false);
            let stepped = self.step(arbiter, timer.now(), phase, io);
            B::span(ServiceSpan::Step, true);
            match stepped {
                Next::Wait => {}
                Next::Retune(pending) => return Served::Retune(pending),
                Next::Failed(StepFailure::Port(
                    fault @ (ServiceFault::Backend(_) | ServiceFault::Submission(_)),
                )) => {
                    return self.drain_port_error(arbiter, phase, timer, fault).await;
                }
                Next::Failed(fault) => return Served::Failed(fault),
            }
            // Releasing RX owners is a separate input source. Its receive
            // transition must not nest inside the control-action step frame.
            B::span(ServiceSpan::Pending, false);
            let pending = self.receive_pending(arbiter, timer.now(), phase, io);
            B::span(ServiceSpan::Pending, true);
            match pending {
                Ok(true) => continue,
                Ok(false) => {}
                Err(fault) => return Served::Failed(fault),
            }
            let accepting_commands = !self.station.is_reclaiming();
            let machine_deadline = [
                arbiter.poll_deadline(),
                self.station.poll_deadline(),
                self.pending.receive.deadline(),
            ]
            .into_iter()
            .flatten()
            .min();
            let deadline = [
                machine_deadline,
                if accepting_commands {
                    io.control.policy_deadline()
                } else {
                    None
                },
            ]
            .into_iter()
            .flatten()
            .min();
            // The station's unwrap request goes to the engine; its response
            // is one more input source.
            if let Some((id, request)) = self.pending.unwrap.take()
                && let Err(waiting) = unwrap.start(id, request)
            {
                self.pending.unwrap = Some(waiting);
            }
            let sending = self.data.accepts() && self.station.link().is_some();
            let (event, command_ready, tick, transmit, unwrapped) = {
                let event = phase.next_event();
                let command = async {
                    match embassy_futures::select::select(
                        io.control.wait_for_stop(),
                        io.commands.ready(),
                    )
                    .await
                    {
                        embassy_futures::select::Either::First(()) => true,
                        embassy_futures::select::Either::Second(()) => false,
                    }
                };
                let alarm = async {
                    match deadline {
                        Some(deadline) => timer.wait_until(deadline).await,
                        None => core::future::pending::<()>().await,
                    }
                };
                let transmit_ready = io.transmit.ready();
                let mut transmit_ready = core::pin::pin!(transmit_ready);
                let mut event = core::pin::pin!(event);
                let mut command = core::pin::pin!(command);
                let mut alarm = core::pin::pin!(alarm);
                poll_fn(|cx| {
                    B::span(ServiceSpan::Sources, false);
                    let event = match event.as_mut().poll(cx) {
                        Poll::Ready(event) => Some(event),
                        Poll::Pending => None,
                    };
                    let command_ready = if accepting_commands {
                        match command.as_mut().poll(cx) {
                            Poll::Ready(stop) => Some(stop),
                            Poll::Pending => None,
                        }
                    } else {
                        None
                    };
                    io.control.policy_register(cx);
                    let alarm_ready = alarm.as_mut().poll(cx).is_ready();
                    let policy_ready = accepting_commands && io.control.policy_ready(timer.now());
                    let tick = alarm_ready
                        && machine_deadline.is_some_and(|deadline| timer.now() >= deadline);
                    let transmit = sending && transmit_ready.as_mut().poll(cx).is_ready();
                    let unwrapped = match unwrap.poll_done(cx) {
                        Poll::Ready(done) => Some(done),
                        Poll::Pending => None,
                    };
                    if event.is_some()
                        || command_ready.is_some()
                        || alarm_ready
                        || policy_ready
                        || transmit
                        || unwrapped.is_some()
                    {
                        B::span(ServiceSpan::Sources, true);
                        Poll::Ready((event, command_ready, tick, transmit, unwrapped))
                    } else {
                        B::span(ServiceSpan::Sources, true);
                        Poll::Pending
                    }
                })
                .await
            };
            let now = timer.now();
            // The response returns the station's ticket before any other
            // input can end the phase.
            if let Some((id, result)) = unwrapped {
                self.station.input(
                    now,
                    StaInput::KeyDataUnwrapped { id, result },
                    &mut self.events,
                );
            }
            match event {
                Some(Ok(Ok(event))) => match self.event(arbiter, now, B::event(event), phase, io) {
                    Ok(Some(buffer)) => {
                        B::span(ServiceSpan::Recycle, false);
                        phase.recycle(buffer);
                        B::span(ServiceSpan::Recycle, true);
                    }
                    Ok(None) => {}
                    Err(fault) => return Served::Failed(fault.into()),
                },
                Some(Ok(Err(_))) => report(&mut self.events, || ServiceEvent::PortEventsLost),
                None => {}
                Some(Err(end)) => {
                    let cause = match end {
                        EventEnd::Drained { cause } | EventEnd::Quarantined { cause } => cause,
                    };
                    return Served::StreamEnd {
                        end,
                        fault: ServiceFault::Backend(cause),
                    };
                }
            }
            if tick {
                self.station.input(now, StaInput::Tick, &mut self.events);
            }
            // A ready command stays in its sole-consumer queue until the
            // event was accepted. An event fault above cannot discard it.
            let command = match command_ready {
                Some(true) => Some(ServiceCommand::Stop),
                Some(false) => io.commands.try_receive(),
                None => None,
            };
            let command = match command {
                Some(command) => Some(command),
                None if accepting_commands => match io.control.policy_command(now) {
                    Ok(command) => command,
                    Err(_) => return Served::Failed(ServiceFault::PolicyDeadlineOverflow.into()),
                },
                None => None,
            };
            if let Some(command) = command {
                match command {
                    ServiceCommand::Station(command) => {
                        match self.pending.commands.request(command) {
                            Some(request) => {
                                io.output.emit(ServiceOutput::CommandStarted(command));
                                if let Err(refused) = self.station.command(now, request) {
                                    let refused = self.pending.commands.refused(refused);
                                    io.output.emit(ServiceOutput::CommandRefused(refused));
                                }
                            }
                            None => io.output.emit(ServiceOutput::CommandInFlight(command)),
                        }
                    }
                    ServiceCommand::AccessPoint(fact) => arbiter.access_point(now, fact.into()),
                    ServiceCommand::WantAccessPoint => arbiter.want_access_point(),
                    ServiceCommand::Stop => {
                        self.pending
                            .receive
                            .clear(&mut PhaseBlockAcks::new(phase), &mut self.events);
                        return Served::Stop;
                    }
                }
            }
            if transmit && self.data.accepts() && self.station.link().is_some() {
                B::span(ServiceSpan::Send, false);
                self.send(now, io);
                B::span(ServiceSpan::Send, true);
            }
            batch = batch.saturating_add(1);
            if batch == MAX_INPUT_STEPS_PER_BATCH {
                batch = 0;
                yield_now().await;
            }
        }
    }

    /// Finish the station's unwrap before it retires, so its ticket comes
    /// back: a request not yet started still runs in the engine. The station
    /// takes the response without an action being served.
    async fn drain_key_unwrap(
        &mut self,
        timer: &impl Timer,
        unwrap: &mut KeyUnwrapSlot<impl KeyUnwrap<KEY_DATA_CAPACITY, Operation: Unpin>>,
    ) {
        loop {
            if let Some((id, request)) = self.pending.unwrap.take()
                && let Err(waiting) = unwrap.start(id, request)
            {
                self.pending.unwrap = Some(waiting);
            }
            match unwrap.finish().await {
                Some((id, result)) => self.station.input(
                    timer.now(),
                    StaInput::KeyDataUnwrapped { id, result },
                    &mut self.events,
                ),
                None if self.pending.unwrap.is_none() => return,
                None => {}
            }
        }
    }

    /// A command poison closes admission, but earlier terminals still own
    /// their places. Consume them until the backend names its final end.
    async fn drain_port_error<'p, P>(
        &mut self,
        arbiter: &mut PortArbiter<'id, B>,
        phase: &mut P,
        timer: &impl Timer,
        fault: ServiceFault<B>,
    ) -> Served<'id, B>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        loop {
            for _ in 0..MAX_INPUT_STEPS_PER_BATCH {
                match phase.next_event().await.map(|event| event.map(B::event)) {
                    Ok(Ok(LowerMacEvent::Terminal(terminal))) => {
                        if let Err(violation) = self.terminal(arbiter, timer.now(), terminal) {
                            return Served::Failed(violation.into());
                        }
                    }
                    Ok(Ok(LowerMacEvent::Received { frame, .. })) => {
                        phase.recycle(frame);
                        report(&mut self.events, || {
                            ServiceEvent::RxFrameDropped(RxDrop::Draining)
                        });
                    }
                    Ok(Ok(LowerMacEvent::Tbtt(_))) => {}
                    Ok(Err(_)) => report(&mut self.events, || ServiceEvent::PortEventsLost),
                    Err(end) => {
                        return Served::StreamEnd { end, fault };
                    }
                }
            }
            yield_now().await;
        }
    }

    /// Hand the network stack's next MSDU to the data machine on the
    /// station's link.
    /// Its admission is `now`: its age starts when the service takes it.
    // Keep source-specific temporaries out of the service poll frame.
    #[inline(never)]
    fn send<const TX_DEPTH: usize, const RX_DEPTH: usize, const COMMAND_DEPTH: usize>(
        &mut self,
        now: Instant,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
    ) {
        let Some(mut link) = self.station.link() else {
            return;
        };
        let Some(packet) = io.transmit.try_receive() else {
            return;
        };
        let parts = packet.frame.parts();
        let msdu = Msdu {
            destination: parts.destination,
            ether_type: parts.ether_type,
            priority: packet.priority,
            payload: parts.payload,
            admitted: now,
        };
        // Admission was polled immediately above under this same exclusive
        // service borrow. A too-long MSDU is refused without changing bytes.
        match self.data.input(
            DataInput {
                msdu,
                link: &mut link,
            },
            &mut self.events,
        ) {
            Ok(()) => {}
            Err(reason) => {
                // The available-pool observation is dynamic and the public
                // data input can refuse it. Return the original owned packet,
                // rather than manufacturing a terminal or losing its bytes.
                io.output.emit(ServiceOutput::MsduRefused {
                    packet: contract::TransmitPacket(packet),
                    reason: reason.into(),
                });
            }
        }
    }

    /// Carry out every action the machines owe at `now`.
    // Keep source-specific temporaries out of the service poll frame.
    #[inline(never)]
    fn step<'p, P, const TX_DEPTH: usize, const RX_DEPTH: usize, const COMMAND_DEPTH: usize>(
        &mut self,
        arbiter: &mut PortArbiter<'id, B>,
        now: Instant,
        phase: &mut P,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
    ) -> Next<'id, B>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        loop {
            let mut progress = false;
            if self.pending.receive.failed() {
                return Next::Failed(ServiceFault::RxBlockAckCapacity.into());
            }
            if !self.station.is_connected() && self.pending.receive.association().is_some() {
                self.pending
                    .receive
                    .clear(&mut PhaseBlockAcks::new(phase), &mut self.events);
            }
            while let Some(action) = arbiter.poll_action(now) {
                progress = true;
                match action {
                    ArbiterAction::Station(notice) => {
                        self.station
                            .input(now, StaInput::Arbiter(notice), &mut self.events);
                    }
                    ArbiterAction::AccessPoint(command) => {
                        io.output.emit(ServiceOutput::AccessPoint(command.into()));
                    }
                    ArbiterAction::CloseGate { gate, reserve: () } => {
                        self.pending
                            .admissions
                            .insert(PendingTerminal::Control(ControlTerminal::GateClosed));
                        phase.close_gate(gate);
                    }
                    ArbiterAction::Retune(pending) => return Next::Retune(pending),
                    ArbiterAction::OpenGate(closed) => {
                        let open = phase.open_gate(closed);
                        arbiter.gate_opened(open);
                    }
                }
            }
            while let Some(action) = self.station.poll_action(now, &mut self.events) {
                progress = true;
                let submitted = station_action(
                    now,
                    phase,
                    io,
                    arbiter,
                    &self.rates,
                    &mut self.events,
                    &mut self.pending,
                    action,
                )
                .map_err(StepFailure::from)
                .and_then(|follow| self.follow(now, follow, io).map_err(StepFailure::from));
                if let Err(fault) = submitted {
                    return Next::Failed(fault);
                }
            }
            self.pending
                .receive
                .tick(now, &mut PhaseBlockAcks::new(phase), &mut self.events);
            if let Some(owed) = self.pending.receive.next_reply()
                && let Some(reply) =
                    RxBlockAckReply::from_body(owed.association, owed.bssid, owed.body)
            {
                let place = owed.place;
                match self.station.try_block_ack_reply(reply, &mut self.events) {
                    Ok(action) => {
                        self.pending.receive.sent(place);
                        progress = true;
                        let submitted = station_action(
                            now,
                            phase,
                            io,
                            arbiter,
                            &self.rates,
                            &mut self.events,
                            &mut self.pending,
                            action,
                        )
                        .map_err(StepFailure::from)
                        .and_then(|follow| self.follow(now, follow, io).map_err(StepFailure::from));
                        if let Err(fault) = submitted {
                            return Next::Failed(fault);
                        }
                    }
                    Err((_, oer_ieee80211_core_roles_sta::machine::BlockAckReplyRefusal::Busy)) => {
                    }
                    Err((
                        _,
                        oer_ieee80211_core_roles_sta::machine::BlockAckReplyRefusal::Encode,
                    )) => {
                        return Next::Failed(ServiceFault::RxBlockAckEncode.into());
                    }
                    Err((_, _)) => {
                        self.pending.receive.sent(place);
                        progress = true;
                    }
                }
            }
            // Data goes only while the link is up: otherwise its MPDUs wait
            // in the machine, none lost. At most the backend's queue depth
            // of data attempts of a category waits at the port, so a
            // station frame, submitted at once, waits for at most that
            // depth and the one on the air; the limit is the service's, for
            // every data source it runs.
            loop {
                // This scope owns the short LinkView borrow. Only an owned
                // follow-up leaves it; handling it may then borrow all of self.
                let submitted = {
                    let Some(link) = self.station.link() else {
                        break;
                    };
                    let Some(transmit) = self.data.poll_action() else {
                        break;
                    };
                    progress = true;
                    let data::DataTransmitParts {
                        context,
                        ticket,
                        buffer,
                    } = transmit.into_parts();
                    let access_category = context.access_category();
                    let plan = context.protection(&link).and_then(|protection| {
                        self.rates
                            .plan(FrameRates::Data, |rate| phase.rate(rate))
                            .map(|plan| (protection, plan))
                            .ok_or(TxRefusal::NoRate)
                    });
                    let prepared = match (buffer, plan) {
                        (ReturnedMpdu::Fresh(buffer), Ok((protection, plan))) => {
                            let protection = match protection {
                                DataProtection::Open => Some(TxProtection::Open),
                                DataProtection::Ccmp(key) => key.number().map(TxProtection::Ccmp),
                            };
                            match protection {
                                Some(protection) => {
                                    prepare_source::<B, B::Large>(TxSource::Fresh {
                                        buffer,
                                        frame: ClassFrame::individual(*context.frame(), plan),
                                        body: None,
                                        protection,
                                    })
                                }
                                None => Err((
                                    TxRefusal::PacketNumberExhausted,
                                    <B::Types<'id> as TxBufferPool<'id, B::Large>>::erase(buffer),
                                )),
                            }
                        }
                        (ReturnedMpdu::Sent(mpdu), Ok((_protection, plan))) => {
                            prepare_source::<B, B::Large>(TxSource::Retransmission {
                                mpdu,
                                plan: RetransmissionPlan::Individual(plan),
                            })
                        }
                        (buffer, Err(reason)) => {
                            let buffer = match buffer {
                                ReturnedMpdu::Fresh(buffer) => {
                                    <B::Types<'id> as TxBufferPool<'id, B::Large>>::erase(buffer)
                                }
                                ReturnedMpdu::Sent(mpdu) => {
                                    <B::Types<'id> as TxBufferPool<'id, B::Large>>::erase_sent(mpdu)
                                }
                            };
                            Err((reason, buffer))
                        }
                    };
                    match prepared {
                        Err((reason, buffer)) => Ok(Follow::Refused {
                            context: TxContext::Data(context),
                            ticket,
                            buffer,
                            reason,
                        }),
                        Ok(source) => {
                            self.pending
                                .admissions
                                .insert(PendingTerminal::Transmit(ticket.index()));
                            let submitted = phase.station().submit(
                                (oer_ieee80211_contracts_port::Attempt {
                                    context: TxContext::Data(context),
                                    ticket,
                                    source,
                                    access_category,
                                    protection: DataContext::<'id, B>::PROTECTION,
                                    power: DataContext::<'id, B>::POWER,
                                    coex_operation: DataContext::<'id, B>::COEX_OPERATION,
                                })
                                .admit(),
                            );
                            match submitted {
                                Ok(()) => Ok(Follow::Nothing),
                                Err(poisoned) => Err(ServiceFault::Submission(poisoned.cause)),
                            }
                        }
                    }
                };
                match submitted {
                    Ok(follow) => {
                        if let Err(reason) = self.follow(now, follow, io) {
                            return Next::Failed(reason.into());
                        }
                    }
                    Err(fault) => return Next::Failed(fault.into()),
                }
            }
            if !progress {
                return Next::Wait;
            }
        }
    }

    /// What follows a station's action, once it no longer borrows the
    /// station.
    // Keep source-specific temporaries out of the service poll frame.
    #[inline(never)]
    fn follow<const TX_DEPTH: usize, const RX_DEPTH: usize, const COMMAND_DEPTH: usize>(
        &mut self,
        now: Instant,
        follow: Follow<'id, B, GROUPS>,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
    ) -> Result<(), ContractFault<'id, B>> {
        match follow {
            Follow::Nothing => {}
            Follow::Input(input) => self.station.input(now, input, &mut self.events),
            // The rates start from what the association made known.
            Follow::Associated(peer) => {
                self.rates
                    .associated(&peer.associated.rate_peer(), &peer.response);
            }
            Follow::Notice(notice) => {
                if let StaNotice::Connected { .. } = notice
                    && let Some(link) = self.station.link()
                {
                    io.control.state.epoch(link.epoch.sequence());
                }
                io.output.emit(ServiceOutput::Notice(notice.into()));
                if let StaNotice::Disconnected(_) = notice {
                    self.rates.disconnected();
                    self.data.leave(&mut self.events);
                }
            }
            Follow::Refused {
                context,
                ticket,
                buffer,
                reason,
            } => {
                let operation = match &context {
                    TxContext::Station(_) => TxOperation::Station,
                    TxContext::Data(_) => TxOperation::Data,
                };
                io.output.emit(ServiceOutput::TxRefused {
                    operation,
                    reason: reason.into(),
                });
                self.returned(now, context, ticket, buffer, ReturnedTx::Refused(reason))?;
            }
        }
        Ok(())
    }

    fn deliver_arbiter_notices<
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        const COMMAND_DEPTH: usize,
    >(
        &mut self,
        arbiter: &mut PortArbiter<'id, B>,
        now: Instant,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
    ) {
        while let Some(notice) = arbiter.poll_notice(now) {
            match notice {
                ArbiterNotice::Station(notice) => {
                    self.station
                        .input(now, StaInput::Arbiter(notice), &mut self.events)
                }
                ArbiterNotice::AccessPoint(command) => {
                    io.output.emit(ServiceOutput::AccessPoint(command.into()))
                }
            }
        }
    }

    #[inline(never)]
    fn receive_pending<
        'p,
        P,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        const COMMAND_DEPTH: usize,
    >(
        &mut self,
        arbiter: &mut PortArbiter<'id, B>,
        now: Instant,
        phase: &mut P,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
    ) -> Result<bool, StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        if let Some((frame, meta)) = self.pending.receive.pop_ready() {
            self.deliver_received(now, &frame, meta, io);
            phase.recycle(frame);
            return Ok(true);
        }
        if let Some((frame, meta)) = self.pending.receive.pop_deferred() {
            if let Some(frame) = self.event(
                arbiter,
                now,
                LowerMacEvent::Received { frame, meta },
                phase,
                io,
            )? {
                phase.recycle(frame);
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Take one event of the port; a received frame's buffer comes back
    /// for its recycle.
    // Keep source-specific temporaries out of the service poll frame.
    #[inline(never)]
    fn event<'p, P, const TX_DEPTH: usize, const RX_DEPTH: usize, const COMMAND_DEPTH: usize>(
        &mut self,
        arbiter: &mut PortArbiter<'id, B>,
        now: Instant,
        event: LowerMacEvent<'id, B::Types<'id>>,
        phase: &mut P,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
    ) -> Result<Option<<B::Types<'id> as PhaseTypes<'id>>::RxBuffer>, StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        match event {
            LowerMacEvent::Received { frame, meta } => {
                B::span(ServiceSpan::Received, false);
                let received = self.received(now, frame, meta, phase, io);
                B::span(ServiceSpan::Received, true);
                received
            }
            LowerMacEvent::Tbtt(_) => Ok(None),
            LowerMacEvent::Terminal(terminal) => {
                B::span(ServiceSpan::Terminal, false);
                let terminal = self.terminal(arbiter, now, terminal);
                B::span(ServiceSpan::Terminal, true);
                terminal?;
                Ok(None)
            }
        }
    }

    #[inline(never)]
    fn received<'p, P, const TX_DEPTH: usize, const RX_DEPTH: usize, const COMMAND_DEPTH: usize>(
        &mut self,
        now: Instant,
        frame: <B::Types<'id> as PhaseTypes<'id>>::RxBuffer,
        meta: <B::Types<'id> as PhaseTypes<'id>>::RxMeta,
        phase: &mut P,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
    ) -> Result<Option<<B::Types<'id> as PhaseTypes<'id>>::RxBuffer>, StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        {
            {
                let local = self.station.link().map(|link| link.address.get());
                let (frame, meta) =
                    match self
                        .pending
                        .receive
                        .receive(now, local, frame, meta, &mut self.events)
                    {
                        RxDisposition::Deliver(received) => received,
                        RxDisposition::Retained => return Ok(None),
                        RxDisposition::Recycle(frame) => return Ok(Some(frame)),
                        RxDisposition::Capacity(frame) => {
                            phase.recycle(frame);
                            return Err(ServiceFault::RxBlockAckCapacity.into());
                        }
                    };
                self.deliver_received(now, &frame, meta, io);
                Ok(Some(frame))
            }
        }
    }

    #[inline(never)]
    fn deliver_received<
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        const COMMAND_DEPTH: usize,
    >(
        &mut self,
        now: Instant,
        frame: &<B::Types<'id> as PhaseTypes<'id>>::RxBuffer,
        meta: <B::Types<'id> as PhaseTypes<'id>>::RxMeta,
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
    ) {
        B::span(ServiceSpan::Deliver, false);
        let mut refused = RxMsdusRefused::default();
        let result = self.station.receive(
            now,
            frame.bytes(),
            meta,
            |parts| {
                B::span(ServiceSpan::Queue, false);
                let refusal = match io
                    .receive
                    .try_send_with(|place| EthernetFrame::init_from_parts(place, parts).is_ok())
                {
                    Some(true) => None,
                    Some(false) => Some(MsduRefusal::TooLong),
                    None => Some(MsduRefusal::QueueFull),
                };
                B::span(ServiceSpan::Queue, true);
                if refusal.is_some() {
                    B::span(ServiceSpan::Refused, false);
                    B::span(ServiceSpan::Refused, true);
                }
                // Delivery runs for every sink; only an enabled one counts.
                if <O as EventSink<ServiceEvent>>::ENABLED {
                    count_refusal(&mut refused, refusal);
                }
            },
            &mut self.events,
        );
        B::span(ServiceSpan::Deliver, true);
        // The sink is the station's during delivery: the frame's refusals
        // follow its events as one aggregate.
        if refused != RxMsdusRefused::default() {
            report(&mut self.events, || ServiceEvent::RxMsdusRefused(refused));
        }
        if let Err(reason) = result {
            report(&mut self.events, || {
                ServiceEvent::RxFrameDropped(RxDrop::Station(reason))
            });
        }
    }

    /// Hand one received MSDU to the network queue; a refusal returns its
    /// reason.
    #[inline(never)]
    fn queue_received<const TX_DEPTH: usize, const RX_DEPTH: usize, const COMMAND_DEPTH: usize>(
        io: &mut ServiceIo<
            '_,
            B::Caps,
            impl ServiceOutputSink<B::Caps>,
            TX_DEPTH,
            RX_DEPTH,
            COMMAND_DEPTH,
        >,
        frame: Result<EthernetFrame, FrameTooLong>,
    ) -> Option<MsduRefusal> {
        match frame {
            Ok(frame) => io
                .receive
                .try_send(frame)
                .is_err()
                .then_some(MsduRefusal::QueueFull),
            Err(_) => Some(MsduRefusal::TooLong),
        }
    }

    fn terminal(
        &mut self,
        arbiter: &mut PortArbiter<'id, B>,
        now: Instant,
        terminal: LowerMacTerminal<'id, B::Types<'id>>,
    ) -> Result<(), ContractFault<'id, B>> {
        let control = match &terminal {
            LowerMacTerminal::GateClosed(_) => Some(ControlTerminal::GateClosed),
            LowerMacTerminal::Configured(oer_ieee80211_contracts_port::Configured::Station(_)) => {
                Some(ControlTerminal::Configure)
            }
            LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Pairwise { .. })) => {
                Some(ControlTerminal::PairwiseRemoval)
            }
            _ => None,
        };
        if let Some(control) = control {
            let operation = PendingTerminal::Control(control);
            if !self.pending.admissions.remove(operation) {
                return Err(ContractFault::sealed(
                    BackendContractViolation::UnexpectedTerminal(operation),
                    terminal,
                ));
            }
        }
        match terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal)) => {
                let oer_ieee80211_contracts_port::TerminalParts {
                    context,
                    ticket,
                    payload,
                    outcome,
                } = terminal.into_parts(&mut self.terminal_access);
                let (buffer, body) = payload.into_parts();
                if !self
                    .pending
                    .admissions
                    .remove(PendingTerminal::Transmit(ticket.index()))
                {
                    return Err(ContractFault {
                        reason: BackendContractViolation::UnexpectedTerminal(
                            PendingTerminal::Transmit(ticket.index()),
                        ),
                        terminal: RejectedTerminal::Transmit(UnroutedTx {
                            context,
                            ticket,
                            buffer: B::tx_buffer(buffer),
                            outcome: ReturnedTx::Completed(outcome),
                        }),
                    });
                }
                if let Some(body) = body {
                    match body {}
                }
                self.returned(now, context, ticket, buffer, ReturnedTx::Completed(outcome))?;
            }
            terminal @ LowerMacTerminal::TxCompleted(TxCompleted::Aggregate(_)) => {
                return Err(ContractFault::sealed(
                    BackendContractViolation::UnexpectedAggregate,
                    terminal,
                ));
            }
            LowerMacTerminal::GateClosed(closed) => arbiter.gate_closed(closed),
            LowerMacTerminal::Configured(oer_ieee80211_contracts_port::Configured::Station(
                permission,
            )) => {
                self.station
                    .input(now, StaInput::Configured(permission), &mut self.events);
            }
            LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Pairwise {
                slot,
                association,
            })) => {
                self.station.input(
                    now,
                    StaInput::PairwiseKeyRemoved { slot, association },
                    &mut self.events,
                );
            }
            terminal @ (LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group {
                ..
            }))
            | LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { .. })
            | LowerMacTerminal::GroupKeyReplaced { .. }
            | LowerMacTerminal::GroupKeySelected { .. }) => self.group_terminal(now, terminal)?,
            terminal @ (LowerMacTerminal::Configured(
                oer_ieee80211_contracts_port::Configured::AccessPoint(_),
            )
            | LowerMacTerminal::KeyRemoved(Removed::AccessPoint(_))
            | LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint {
                ..
            })) => {
                return Err(ContractFault::sealed(
                    BackendContractViolation::UnexpectedAccessPointTerminal,
                    terminal,
                ));
            }
            terminal @ LowerMacTerminal::TsfSampled { .. } => {
                return Err(ContractFault::sealed(
                    BackendContractViolation::UnexpectedTsfSample,
                    terminal,
                ));
            }
        }
        Ok(())
    }

    fn group_terminal(
        &mut self,
        now: Instant,
        terminal: LowerMacTerminal<'id, B::Types<'id>>,
    ) -> Result<(), ContractFault<'id, B>> {
        let Some(pending) = self.pending.group.take() else {
            return Err(ContractFault::sealed(
                BackendContractViolation::GroupTerminalWithoutRequest,
                terminal,
            ));
        };
        let identity = match &terminal {
            LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group { slot, .. })) => {
                <B::Types<'id> as PhaseTypes<'id>>::StationKeys::vacant_group_slot_identity(slot)
            }
            LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { key, .. })
            | LowerMacTerminal::GroupKeyReplaced { key, .. }
            | LowerMacTerminal::GroupKeySelected { key, .. } => {
                <B::Types<'id> as PhaseTypes<'id>>::StationKeys::group_slot_identity(key)
            }
            _ => {
                self.pending.group = Some(pending);
                return Err(ContractFault::sealed(
                    BackendContractViolation::GroupTerminalKindMismatch,
                    terminal,
                ));
            }
        };
        if pending.slot != identity {
            self.pending.group = Some(pending);
            return Err(ContractFault::sealed(
                BackendContractViolation::GroupSlotMismatch,
                terminal,
            ));
        }
        let input = match (pending.action, terminal) {
            (
                PendingGroupAction::Remove(pending),
                LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group {
                    slot,
                    permission,
                })),
            ) => StaInput::GroupKeyRemoved {
                pending,
                slot,
                permission,
            },
            (
                PendingGroupAction::Install(installed),
                LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { key, permission }),
            ) => {
                if pending.key_id.is_some() {
                    report(&mut self.events, || ServiceEvent::GroupKeyInstalled {
                        key_id: pending.key_id.map_or(0, |id| id.get()),
                        replaced: false,
                    });
                }
                StaInput::GroupKeyInstalled {
                    pending: installed,
                    key,
                    permission,
                }
            }
            (
                PendingGroupAction::Replace(replaced),
                LowerMacTerminal::GroupKeyReplaced { key, permission },
            ) => {
                if pending.key_id.is_some() {
                    report(&mut self.events, || ServiceEvent::GroupKeyInstalled {
                        key_id: pending.key_id.map_or(0, |id| id.get()),
                        replaced: true,
                    });
                }
                StaInput::GroupKeyReplaced {
                    pending: replaced,
                    key,
                    permission,
                }
            }
            (
                PendingGroupAction::Select(pending),
                LowerMacTerminal::GroupKeySelected { key, permission },
            ) => StaInput::GroupKeySelected {
                pending,
                key,
                permission,
            },
            (action, terminal) => {
                self.pending.group = Some(PendingGroup {
                    slot: pending.slot,
                    key_id: pending.key_id,
                    action,
                });
                return Err(ContractFault::sealed(
                    BackendContractViolation::GroupTerminalKindMismatch,
                    terminal,
                ));
            }
        };
        self.pending
            .admissions
            .remove(PendingTerminal::Control(ControlTerminal::GroupChange));
        self.station.input(now, input, &mut self.events);
        Ok(())
    }

    /// The context identifies the operation. Class is checked exactly once
    /// against that identity; a mismatch violates the backend contract and
    /// takes the same fail-stop path as Poisoned. A shared AC queue erases
    /// classes, so this identity obligation cannot be proved by this API.
    fn returned(
        &mut self,
        now: Instant,
        context: TxContext<'id, B::Caps, B::Large, <B::Types<'id> as PhaseTypes<'id>>::EpochPath>,
        ticket: Slot<
            'id,
            <B::Types<'id> as PhaseTypes<'id>>::TicketKind,
            <B::Types<'id> as PhaseTypes<'id>>::TicketPath,
        >,
        buffer: <B::Types<'id> as PhaseTypes<'id>>::AnyTxBuffer,
        end: PortReturnedTx<'id, B>,
    ) -> Result<(), ContractFault<'id, B>> {
        match (context, B::tx_buffer(buffer)) {
            (TxContext::Station(context), StationTxBuffer::Small(buffer)) => {
                if let ReturnedTx::Completed(completion) = &end {
                    let current = self.station.is_current(&context);
                    self.rates
                        .learn(now, context.frame_rates(), current, completion);
                }
                let input = match end {
                    ReturnedTx::Completed(completion) => StaInput::TxDone {
                        context,
                        ticket,
                        buffer,
                        completion,
                    },
                    ReturnedTx::Refused(reason) => StaInput::Unsent {
                        context,
                        ticket,
                        buffer,
                        reason,
                    },
                };
                self.station.input(now, input, &mut self.events);
                Ok(())
            }
            (TxContext::Data(context), StationTxBuffer::Large(buffer)) => {
                if let ReturnedTx::Completed(completion) = &end {
                    let current = self.data.is_current(&context);
                    self.rates.learn(now, FrameRates::Data, current, completion);
                }
                let input = match end {
                    ReturnedTx::Completed(completion) => DataReturn::TxDone {
                        context,
                        ticket,
                        buffer,
                        completion,
                    },
                    ReturnedTx::Refused(reason) => DataReturn::Refused {
                        context,
                        ticket,
                        buffer,
                        reason,
                    },
                };
                self.data.returned(input, &mut self.events);
                Ok(())
            }
            (context @ TxContext::Station(_), buffer @ StationTxBuffer::Large(_))
            | (context @ TxContext::Data(_), buffer @ StationTxBuffer::Small(_)) => {
                Err(ContractFault {
                    reason: BackendContractViolation::TxContextBufferMismatch,
                    terminal: RejectedTerminal::Transmit(UnroutedTx {
                        context,
                        ticket,
                        buffer,
                        outcome: end,
                    }),
                })
            }
        }
    }
}

/// What follows a station's action.
#[expect(
    clippy::large_enum_variant,
    reason = "a station input owns its key-unwrap response by value; the service has no allocator"
)]
enum Follow<'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id, const GROUPS: usize> {
    Nothing,
    /// An arbiter's answer or a key, back to the station.
    Input(StaInput<'id, PortStaTokens<'id, B::Types<'id>, B::Small, GROUPS>, B::Caps>),
    /// The station associated: the rates start from its peer.
    Associated(Peer<PortRxMeta<'id, B>>),
    /// A frame of the station did not go.
    Refused {
        context: TxContext<'id, B::Caps, B::Large, <B::Types<'id> as PhaseTypes<'id>>::EpochPath>,
        ticket: <PortStaTokens<'id, B::Types<'id>, B::Small, GROUPS> as StaTokens>::Ticket,
        buffer: <B::Types<'id> as PhaseTypes<'id>>::AnyTxBuffer,
        reason: TxRefusal,
    },
    /// The station left: the rates leave its peer.
    Notice(StaNotice),
}

type StationPreparation<'k, 'p, 'id, B> = Result<
    <<B as StationPort>::Types<'id> as PhaseTypes<'id>>::Prepared<
        'k,
        'p,
        oer_ieee80211_contracts_port::Station,
        <B as StationPort>::Caps,
    >,
    (
        TxRefusal,
        <<B as StationPort>::Types<'id> as PhaseTypes<'id>>::AnyTxBuffer,
    ),
>;

/// Prepare within the class pool; a refusal retains the original owners.
fn prepare_source<
    'k,
    'p,
    'id: 'p + 'k,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: TxBufferClass,
>(
    source: PoolSource<
        'k,
        'p,
        'id,
        B::Types<'id>,
        C,
        B::Caps,
        oer_ieee80211_contracts_port::Station,
    >,
) -> StationPreparation<'k, 'p, 'id, B>
where
    B::Types<'id>: TxBufferPool<'id, C>,
{
    let prepared = match source.check() {
        Ok(source) => <B::Types<'id> as TxBufferPool<'id, C>>::prepare(source)
            .map_err(|refused| (TxRefusal::Preparation(refused.reason), refused.operation)),
        Err(refused) => Err((TxRefusal::Layout(refused.reason), refused.operation)),
    };
    prepared.map_err(|(reason, operation)| {
        let buffer = match operation {
            TxSource::Fresh { buffer, body, .. } => {
                if let Some(body) = body {
                    match body {}
                }
                <B::Types<'id> as TxBufferPool<'id, C>>::erase(buffer)
            }
            TxSource::Retransmission { mpdu, .. } => {
                <B::Types<'id> as TxBufferPool<'id, C>>::erase_sent(mpdu)
            }
        };
        (reason, buffer)
    })
}

/// Submit a station frame with its phase rate and optional pairwise key.
/// Preparation refusal returns the operation to the station without admission.
fn submit_station<'p, 'id: 'p, B, P, S, const GROUPS: usize>(
    phase: &mut P,
    rates: &S,
    admissions: &mut Admissions,
    key: Option<&StationPairwiseKey<'id, <B::Types<'id> as PhaseTypes<'id>>::StationKeys>>,
    transmit: StaTransmit<'id, PortStaTokens<'id, B::Types<'id>, B::Small, GROUPS>, B::Caps>,
) -> Result<Follow<'id, B, GROUPS>, ServiceFault<B>>
where
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    S: RateSelection<PortRxMeta<'id, B>, PortAck<'id, B>>,
{
    let StaTransmit {
        context,
        ticket,
        buffer,
        frame,
        class,
        plan,
        access_category,
    } = transmit;
    let frame = match frame
        .rated(plan, rates, |rate| phase.rate(rate))
        .ok_or(TxRefusal::NoRate)
    {
        Ok(frame) => frame,
        Err(reason) => {
            return Ok(Follow::Refused {
                context: TxContext::Station(context),
                ticket,
                buffer: <B::Types<'id> as TxBufferPool<'id, B::Small>>::erase(buffer),
                reason,
            });
        }
    };
    let protection = match key {
        None => TxProtection::Open,
        Some(key) => match key.number() {
            Some(numbered) => TxProtection::Ccmp(numbered),
            None => {
                return Ok(Follow::Refused {
                    context: TxContext::Station(context),
                    ticket,
                    buffer: <B::Types<'id> as TxBufferPool<'id, B::Small>>::erase(buffer),
                    reason: TxRefusal::PacketNumberExhausted,
                });
            }
        },
    };
    let source = match prepare_source::<B, B::Small>(TxSource::Fresh {
        buffer,
        frame,
        body: None,
        protection,
    }) {
        Ok(source) => source,
        Err((reason, buffer)) => {
            return Ok(Follow::Refused {
                context: TxContext::Station(context),
                ticket,
                buffer,
                reason,
            });
        }
    };
    let mut station = phase.station();
    admissions.insert(PendingTerminal::Transmit(ticket.index()));
    match station.submit(
        (oer_ieee80211_contracts_port::Attempt {
            context: TxContext::Station(context),
            ticket,
            source,
            access_category,
            protection: StationTransmit::<'id, B, GROUPS>::PROTECTION,
            power: StationTransmit::<'id, B, GROUPS>::POWER,
            coex_operation: class.coex_operation(),
        })
        .admit(),
    ) {
        Ok(()) => Ok(Follow::Nothing),
        Err(poisoned) => Err(ServiceFault::Submission(poisoned.cause)),
    }
}

/// Carry out one action of the station; an arbiter's answer goes back to
/// it at once.
#[allow(
    clippy::too_many_arguments,
    reason = "the existing service action inputs also lend their event sink"
)]
fn station_action<
    'p,
    'id: 'p,
    B,
    P,
    S,
    const TX_DEPTH: usize,
    const RX_DEPTH: usize,
    const COMMAND_DEPTH: usize,
    const GROUPS: usize,
>(
    now: Instant,
    phase: &mut P,
    io: &mut ServiceIo<
        '_,
        B::Caps,
        impl ServiceOutputSink<B::Caps>,
        TX_DEPTH,
        RX_DEPTH,
        COMMAND_DEPTH,
    >,
    arbiter: &mut PortArbiter<'id, B>,
    rates: &S,
    events: &mut impl EventSink<oer_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent>,
    pending_actions: &mut PendingActions<'id, B>,
    action: StaAction<'_, 'id, PortStaTokens<'id, B::Types<'id>, B::Small, GROUPS>, B::Caps>,
) -> Result<Follow<'id, B, GROUPS>, ServiceFault<B>>
where
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    S: RateSelection<PortRxMeta<'id, B>, PortAck<'id, B>>,
{
    let input = match action {
        StaAction::BlockAck(request) => {
            pending_actions
                .receive
                .offer(
                    now,
                    request.epoch(),
                    request.bssid(),
                    request.action(),
                    request.supported(),
                    &mut PhaseBlockAcks::new(phase),
                    events,
                )
                .map_err(|_| ServiceFault::RxBlockAckCapacity)?;
            return Ok(Follow::Nothing);
        }
        StaAction::RequestChannel(request, place) => {
            StaInput::Answer(arbiter.request(now, request, place))
        }
        StaAction::RequestAbsence {
            request,
            home,
            place,
        } => StaInput::AbsenceAnswer(arbiter.request_absence(now, request, home, place)),
        StaAction::Ready(lease) => {
            arbiter.ready(lease);
            return Ok(Follow::Nothing);
        }
        StaAction::ExchangeLease(old) => StaInput::Exchanged(arbiter.exchange(old)),
        StaAction::ReleaseChannel(lease) => {
            arbiter.release(lease);
            return Ok(Follow::Nothing);
        }
        StaAction::Upstream(fact) => {
            arbiter.upstream(now, fact);
            return Ok(Follow::Nothing);
        }
        StaAction::Configure(receive, pending) => {
            pending_actions
                .admissions
                .insert(PendingTerminal::Control(ControlTerminal::Configure));
            phase.station().configure(pending, receive);
            return Ok(Follow::Nothing);
        }
        StaAction::SetEdca(edca) => {
            // The station's frames contend with whatever the backend
            // reconciled; nothing in the station follows from that set.
            let _applied = phase.station().set_edca(edca);
            return Ok(Follow::Nothing);
        }
        StaAction::InstallPairwiseKey {
            association,
            pending,
            slot,
            peer,
            key,
        } => StaInput::PairwiseKeyInstalled {
            pending,
            key: StationPairwiseKey::install(
                &mut phase.station(),
                association,
                slot,
                peer,
                key,
                rates.packet_number_step(),
            ),
        },
        StaAction::InstallGroupKey {
            association,
            pending,
            permission,
            slot,
            key_id,
            key,
        } => {
            let identity =
                <B::Types<'id> as PhaseTypes<'id>>::StationKeys::vacant_group_slot_identity(&slot);
            pending_actions.group = Some(PendingGroup {
                slot: identity,
                key_id: Some(key_id),
                action: PendingGroupAction::Install(pending),
            });
            pending_actions
                .admissions
                .insert(PendingTerminal::Control(ControlTerminal::GroupChange));
            phase
                .station()
                .install_group_key(association, permission, slot, key_id, key);
            return Ok(Follow::Nothing);
        }
        StaAction::RemovePairwiseKey(key) => {
            pending_actions
                .admissions
                .insert(PendingTerminal::Control(ControlTerminal::PairwiseRemoval));
            let (association, key) = key.into_parts();
            phase.station().remove_pairwise_key(association, key);
            return Ok(Follow::Nothing);
        }
        StaAction::ReplaceGroupKey {
            pending,
            permission,
            installed,
            key_id,
            key,
        } => {
            let slot =
                <B::Types<'id> as PhaseTypes<'id>>::StationKeys::group_slot_identity(&installed);
            pending_actions.group = Some(PendingGroup {
                slot,
                action: PendingGroupAction::Replace(pending),
                key_id: Some(key_id),
            });
            pending_actions
                .admissions
                .insert(PendingTerminal::Control(ControlTerminal::GroupChange));
            phase
                .station()
                .replace_group_key(permission, installed, key_id, key);
            return Ok(Follow::Nothing);
        }
        StaAction::SelectGroupKey {
            pending,
            permission,
            installed,
        } => {
            let slot =
                <B::Types<'id> as PhaseTypes<'id>>::StationKeys::group_slot_identity(&installed);
            pending_actions.group = Some(PendingGroup {
                slot,
                action: PendingGroupAction::Select(pending),
                key_id: None,
            });
            pending_actions
                .admissions
                .insert(PendingTerminal::Control(ControlTerminal::GroupChange));
            phase.station().select_group_key(permission, installed);
            return Ok(Follow::Nothing);
        }
        StaAction::RemoveGroupKey {
            pending,
            key,
            permission,
        } => {
            let slot = <B::Types<'id> as PhaseTypes<'id>>::StationKeys::group_slot_identity(&key);
            pending_actions.group = Some(PendingGroup {
                slot,
                action: PendingGroupAction::Remove(pending),
                key_id: None,
            });
            pending_actions
                .admissions
                .insert(PendingTerminal::Control(ControlTerminal::GroupChange));
            phase.station().remove_group_key(permission, key);
            return Ok(Follow::Nothing);
        }
        StaAction::Transmit(transmit) => {
            return submit_station::<B, P, S, GROUPS>(
                phase,
                rates,
                &mut pending_actions.admissions,
                None,
                transmit,
            );
        }
        StaAction::TransmitProtected { transmit, key } => {
            return submit_station::<B, P, S, GROUPS>(
                phase,
                rates,
                &mut pending_actions.admissions,
                Some(key),
                transmit,
            );
        }
        StaAction::ContractViolation(reason) => {
            return Err(ServiceFault::Contract(
                BackendContractViolation::StationKeyBank(reason.into()),
            ));
        }
        StaAction::Notify(notice) => return Ok(Follow::Notice(notice)),
        StaAction::CommandCompleted(terminal) => {
            io.output.emit(ServiceOutput::CommandCompleted(
                pending_actions.commands.completed(terminal),
            ));
            return Ok(Follow::Nothing);
        }
        StaAction::Associated(peer) => {
            return Ok(Follow::Associated(peer));
        }
        StaAction::UnwrapKeyData { id, request } => {
            if pending_actions.unwrap.is_some() {
                return Err(ServiceFault::Contract(
                    BackendContractViolation::StationKeyUnwrap,
                ));
            }
            pending_actions.unwrap = Some((id, request));
            return Ok(Follow::Nothing);
        }
    };
    Ok(Follow::Input(input))
}

/// What serving a phase ended with.
enum Served<'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id> {
    Stop,
    Retune(
        oer_ieee80211_core_arbiter::PendingRetune<
            'id,
            B::Caps,
            <B::Types<'id> as PhaseTypes<'id>>::GatePath,
        >,
    ),
    Failed(StepFailure<'id, B>),
    StreamEnd {
        end: EventEnd<<B::Backend as PortBackend<B::SlotId>>::Fault>,
        fault: ServiceFault<B>,
    },
}

#[cfg(test)]
mod tests;

/// Preserve the first contract violation even when the disable also fails.
#[inline(never)]
fn stop_disabled<'p, 'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id>(
    owner: Result<
        Owner<'p, 'id, B::SlotId, B::Backend, Disabled>,
        PoisonedOwner<'p, 'id, B::SlotId, B::Backend>,
    >,
    custody: &TerminalCustody<'id, B>,
) -> ServiceStop<'p, 'id, B> {
    match (owner, custody) {
        (Ok(owner), TerminalCustody::Clear) => ServiceStop::Stopped(owner),
        (Ok(owner), TerminalCustody::Quarantined(bank)) => ServiceStop::Failed {
            owner: FailedInstallation { owner },
            fault: ServiceFault::Contract(bank.violation()),
        },
        (Err(owner), TerminalCustody::Clear) => ServiceStop::Poisoned {
            fault: ServiceFault::Backend(owner.cause()),
            owner,
        },
        (Err(owner), TerminalCustody::Quarantined(bank)) => ServiceStop::Poisoned {
            owner,
            fault: ServiceFault::Contract(bank.violation()),
        },
    }
}

/// Why the service did not deliver one received MSDU.
#[derive(Clone, Copy)]
enum MsduRefusal {
    TooLong,
    QueueFull,
}

fn count_refusal(refused: &mut RxMsdusRefused, refusal: Option<MsduRefusal>) {
    match refusal {
        Some(MsduRefusal::TooLong) => refused.too_long = refused.too_long.saturating_add(1),
        Some(MsduRefusal::QueueFull) => refused.queue_full = refused.queue_full.saturating_add(1),
        None => {}
    }
}

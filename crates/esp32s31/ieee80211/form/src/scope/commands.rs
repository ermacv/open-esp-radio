//! The two stage-specific command stores of `Held<'id>`.
//!
//! Actual lower-MAC permissions, accepted operations and S31 `KeyUse` rights
//! move by value. Configure is a FIFO command with its exact receive value;
//! it is never a coalescing setting. These borrowed endpoints do not register
//! the held context with DriverScope or extend its lifetime.
pub mod bridge;
use atomic_waker::AtomicWaker;
use core::{
    future::poll_fn,
    task::{Context, Poll},
};
use oer_esp32s31_ieee80211::lower_mac::{CorePools, CoreResources, TxAdmission};
use oer_ieee80211_lower_mac::{
    AccessPoint, Channel, ConfigurePermission, Configured, GateClosed, GateIssuer, GateOpen,
    GatePool, Ieee80211Capabilities, InFlight, InterfaceCommandPool, InterfacePermissions,
    LowerMacTerminal, PhaseTypes, SampledTsfPermission, Station, StationReceive, SupportedFilter,
    TerminalAccess, TransmitPool, TsfPermission, TsfSample,
};
use oer_ieee80211_mac::ccmp::Ccmp128Key;
use oer_memory::{
    AffineIndexedSpscQueue, AffineIndexedSpscReceiver, AffineIndexedSpscSender, UniqueQueueIndex,
};
use oer_radio_port::{Child, PoolBuilder, PoolPath, PoolSet, Root, Slot, Slots};

pub type CorePath = Child<Root, 0>;
pub type StationPath = Child<Root, 1>;
pub type AccessPointPath = Child<Root, 2>;
pub type GatePath = Child<Root, 6>;
pub type TicketPath = Child<Root, 11>;
pub enum InstallKeyKind {}
pub enum RemoveKeyKind {}
pub enum GroupMutationKind {}
pub enum RetuneKind {}
pub enum EnableKind {}
pub enum DisableKind {}
pub enum BridgeKind {}
pub enum TicketKind {}
const GROUP_PERMISSIONS: usize = 2;
const GATE_COMMANDS: usize = 2;
const LIFECYCLE_COMMAND: usize = 1;
const RETUNE_COMMAND: usize = 1;
const CONFIGURE_PER_INTERFACE: usize = 1;
const TSF_PER_INTERFACE: usize = 1;

// Lower-MAC exports a total, while the form maps the two concrete permission
// types separately. A changed total must stop compilation, not silently move
// a cell beyond its allocation. Singleton commands likewise own one place.
const _: () = {
    assert!(
        InterfaceCommandPool::<Station>::COMMAND_CAPACITY
            == CONFIGURE_PER_INTERFACE + TSF_PER_INTERFACE
    );
    assert!(
        InterfaceCommandPool::<AccessPoint>::COMMAND_CAPACITY
            == CONFIGURE_PER_INTERFACE + TSF_PER_INTERFACE
    );
    assert!(CONFIGURE_PER_INTERFACE == 1 && TSF_PER_INTERFACE == 1);
    assert!(GATE_COMMANDS == 2);
    assert!(LIFECYCLE_COMMAND == 1 && RETUNE_COMMAND == 1);
};

/// The same parameters issue the tickets/slots and determine both FIFO sizes.
pub struct CommandPools<const TICKETS: usize, const PAIRWISE: usize>;
impl<const T: usize, const K: usize> CommandPools<T, K> {
    pub const PHASE_COMMAND_CAPACITY: usize = T
        + 2 * K
        + GROUP_PERMISSIONS
        + InterfaceCommandPool::<Station>::COMMAND_CAPACITY
        + InterfaceCommandPool::<AccessPoint>::COMMAND_CAPACITY
        + GATE_COMMANDS;
    /// The phase-contract formula plus the separate consuming Disable place.
    pub const PHASE_STORAGE: usize = Self::PHASE_COMMAND_CAPACITY + LIFECYCLE_COMMAND;
    /// One configure per interface, one retune and the final Enable.
    pub const SETUP_STORAGE: usize =
        2 * CONFIGURE_PER_INTERFACE + RETUNE_COMMAND + LIFECYCLE_COMMAND;
}
type Layout<const T: usize, const K: usize> = (
    CorePools<T>,
    InterfaceCommandPool<Station>,
    InterfaceCommandPool<AccessPoint>,
    Slots<InstallKeyKind, K>,
    Slots<RemoveKeyKind, K>,
    Slots<GroupMutationKind, GROUP_PERMISSIONS>,
    GatePool,
    Slots<RetuneKind, RETUNE_COMMAND>,
    Slots<EnableKind, LIFECYCLE_COMMAND>,
    Slots<DisableKind, LIFECYCLE_COMMAND>,
    Slots<BridgeKind, 1>,
    TransmitPool<TicketKind, T>,
);
impl<const T: usize, const K: usize> PoolSet for CommandPools<T, K> {
    type Pools<'id, P: PoolPath> = Resources<'id, P, T, K>;
    fn build<'id, P: PoolPath>(builder: PoolBuilder<'id, P>) -> Self::Pools<'id, P> {
        let (
            core,
            station,
            access_point,
            installs,
            removes,
            groups,
            (gate, issuer),
            [retune],
            [enable],
            [disable],
            [bridge],
            (tickets, terminals),
        ) = builder.into_pools::<Layout<T, K>>();
        Resources {
            core,
            station,
            access_point,
            installs,
            removes,
            groups,
            gate,
            issuer,
            retune,
            enable,
            disable,
            bridge: CommandAuthority { _slot: bridge },
            tickets,
            terminals,
        }
    }
}
/// The queue's pool sizes remain in this affine construction authority's
/// type. A small store cannot accept the authority minted for larger pools.
#[must_use]
pub struct CommandAuthority<'id, P: PoolPath, const T: usize, const K: usize> {
    _slot: Slot<'id, BridgeKind, Child<P, 10>>,
}

/// One complete mint result; no second bridge can be built from its authority.
pub struct Resources<'id, P: PoolPath, const T: usize, const K: usize> {
    pub core: CoreResources<'id, Child<P, 0>, T>,
    pub station: InterfacePermissions<'id, Station, Child<P, 1>>,
    pub access_point: InterfacePermissions<'id, AccessPoint, Child<P, 2>>,
    pub installs: [Slot<'id, InstallKeyKind, Child<P, 3>>; K],
    pub removes: [Slot<'id, RemoveKeyKind, Child<P, 4>>; K],
    pub groups: [Slot<'id, GroupMutationKind, Child<P, 5>>; GROUP_PERMISSIONS],
    pub gate: GateOpen<'id, Child<P, 6>>,
    pub issuer: GateIssuer<'id, Child<P, 6>>,
    pub retune: Slot<'id, RetuneKind, Child<P, 7>>,
    pub enable: Slot<'id, EnableKind, Child<P, 8>>,
    pub disable: Slot<'id, DisableKind, Child<P, 9>>,
    pub bridge: CommandAuthority<'id, P, T, K>,
    pub tickets: [Slot<'id, TicketKind, Child<P, 11>>; T],
    pub terminals: TerminalAccess<'id, TicketKind, Child<P, 11>>,
}

#[must_use]
pub enum Configuration<'id, Caps: Ieee80211Capabilities, SC, AC> {
    Station {
        permission: ConfigurePermission<'id, Station, StationPath, SC>,
        receive: StationReceive<Caps>,
    },
    AccessPoint {
        permission: ConfigurePermission<'id, AccessPoint, AccessPointPath, AC>,
        receive: SupportedFilter<Caps, AccessPoint>,
    },
}
impl<'id, Caps: Ieee80211Capabilities, SC: 'id, AC: 'id> Configuration<'id, Caps, SC, AC> {
    /// Called after the receive-configuration hardware barrier completes.
    /// Both the affine admission and opaque context are returned unchanged.
    pub fn configured<P>(self) -> LowerMacTerminal<'id, P>
    where
        P: PhaseTypes<
                'id,
                StationCommandPath = StationPath,
                AccessPointCommandPath = AccessPointPath,
                StationConfigureContext = SC,
                AccessPointConfigureContext = AC,
            >,
    {
        LowerMacTerminal::Configured(match self {
            Self::Station { permission, .. } => Configured::Station(permission),
            Self::AccessPoint { permission, .. } => Configured::AccessPoint(permission),
        })
    }
}
#[must_use]
pub enum Sampling<'id> {
    Station(TsfPermission<'id, Station, StationPath>),
    AccessPoint(TsfPermission<'id, AccessPoint, AccessPointPath>),
}
impl<'id> Sampling<'id> {
    pub fn sampled<P>(self, sample: TsfSample) -> LowerMacTerminal<'id, P>
    where
        P: PhaseTypes<
                'id,
                StationCommandPath = StationPath,
                AccessPointCommandPath = AccessPointPath,
            >,
    {
        let permission = match self {
            Self::Station(permission) => SampledTsfPermission::Station(permission),
            Self::AccessPoint(permission) => SampledTsfPermission::AccessPoint(permission),
        };
        LowerMacTerminal::TsfSampled { permission, sample }
    }
}

/// The accepted operation is never unpacked or reconstructed by the FIFO.
/// Protected admission is the existing S31 KeyUse owner inside TxAdmission.
/// M owns the whole MPDU, including a header and any number of retained bodies.
/// This envelope requires neither one contiguous slice nor one DMA descriptor;
/// retirement, retry and quarantine retain or return the complete M.
#[must_use]
pub struct Submission<'id, C, M> {
    pub operation: InFlight<C, Slot<'id, TicketKind, TicketPath>, M>,
    pub admission: TxAdmission<'id, CorePath>,
}
#[must_use]
pub struct Enable<'id> {
    _place: Slot<'id, EnableKind, Child<Root, 8>>,
}
#[must_use]
pub struct Disable<'id> {
    _place: Slot<'id, DisableKind, Child<Root, 9>>,
}
#[must_use]
pub struct Disabled<'id> {
    enable: Enable<'id>,
    disable: Disable<'id>,
}

#[must_use]
pub enum SetupCommand<'id, Caps: Ieee80211Capabilities, SC, AC> {
    Configure(Configuration<'id, Caps, SC, AC>),
    Retune {
        permission: Slot<'id, RetuneKind, Child<Root, 7>>,
        channel: Channel,
    },
    Enable(Enable<'id>),
}
#[must_use]
pub enum PhaseCommand<'id, Caps: Ieee80211Capabilities, SC, AC, C, M> {
    Configure(Configuration<'id, Caps, SC, AC>),
    SampleTsf(Sampling<'id>),
    Submit(Submission<'id, C, M>),
    /// Own the common key until projection; a direct bridge reply returns
    /// this permission with erased material, including after withdrawal.
    InstallPairwise {
        permission: Slot<'id, InstallKeyKind, Child<Root, 3>>,
        key: Ccmp128Key,
    },
    RemovePairwise(Slot<'id, RemoveKeyKind, Child<Root, 4>>),
    /// The same material ownership and erasure rules as InstallPairwise.
    MutateGroup {
        permission: Slot<'id, GroupMutationKind, Child<Root, 5>>,
        key: Ccmp128Key,
    },
    CloseGate(ClosingGate<'id>),
    /// Private publication authority is the consuming open_gate call. The
    /// command is already in FIFO before that call returns GateOpen.
    OpenGate,
    Disable(Disable<'id>),
}

/// Quarantine erases key material in place; every permission and owner stays.
#[allow(unsafe_code, reason = "command quarantine only zeroes key bytes")]
// SAFETY: only key bytes are zeroed in place; every permission, MPDU owner
// and context stays where it is, and nothing is awaited.
unsafe impl<'id, Caps: Ieee80211Capabilities, SC, AC, C, M> oer_memory::Quarantine
    for PhaseCommand<'id, Caps, SC, AC, C, M>
{
    fn quarantine(&mut self) {
        match self {
            Self::InstallPairwise { key, .. } | Self::MutateGroup { key, .. } => {
                zeroize::Zeroize::zeroize(key);
            }
            Self::Configure(_)
            | Self::SampleTsf(_)
            | Self::Submit(_)
            | Self::RemovePairwise(_)
            | Self::CloseGate(_)
            | Self::OpenGate
            | Self::Disable(_) => {}
        }
    }
}
/// Setup commands carry no key material; quarantine keeps them as they are.
#[allow(unsafe_code, reason = "setup quarantine does nothing")]
// SAFETY: the method does nothing, so it releases and moves nothing.
unsafe impl<'id, Caps: Ieee80211Capabilities, SC, AC> oer_memory::Quarantine
    for SetupCommand<'id, Caps, SC, AC>
{
    fn quarantine(&mut self) {}
}

/// Storage owned by Held, with two exclusive SPSC endpoint pairs. There is
/// no general-purpose send operation: every producer method consumes one of
/// the bounded admissions from this installation's sole mint.
type IndexedStore<T, const N: usize> = AffineIndexedSpscQueue<Entry<T, N>, N>;

pub struct CommandStorage<
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const SETUP: usize,
    const PHASE: usize,
> {
    setup: IndexedStore<SetupCommand<'id, Caps, SC, AC>, SETUP>,
    phase: IndexedStore<PhaseCommand<'id, Caps, SC, AC, C, M>, PHASE>,
    setup_wake: AtomicWaker,
    phase_wake: AtomicWaker,
    _authority: CommandAuthority<'id, Root, T, K>,
}
/// The unique pair of endpoints lent by one command store.
pub type SetupEndpoints<
    'q,
    'id,
    Caps,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> = (
    SetupSender<'q, 'id, Caps, SC, AC, C, M, T, K, S, P>,
    SetupReceiver<'q, 'id, Caps, SC, AC, C, M, S, P>,
);
impl<
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> CommandStorage<'id, Caps, SC, AC, C, M, T, K, S, P>
{
    pub fn new(authority: CommandAuthority<'id, Root, T, K>) -> Self {
        const {
            assert!(
                S == CommandPools::<T, K>::SETUP_STORAGE,
                "setup storage must match its minted permissions"
            );
            assert!(
                P == CommandPools::<T, K>::PHASE_STORAGE,
                "phase storage must include every permission and Disable"
            );
            assert!(
                SetupPlaces::END == S,
                "every setup cell is in the allocation"
            );
            assert!(
                PhasePlaces::<T, K>::END == P,
                "every phase cell is in the allocation"
            );
        }
        Self {
            setup: AffineIndexedSpscQueue::new(),
            phase: AffineIndexedSpscQueue::new(),
            setup_wake: AtomicWaker::new(),
            phase_wake: AtomicWaker::new(),
            _authority: authority,
        }
    }
    pub fn split(
        &mut self,
        issuer: GateIssuer<'id, GatePath>,
        enable: Slot<'id, EnableKind, Child<Root, 8>>,
        disable: Slot<'id, DisableKind, Child<Root, 9>>,
    ) -> SetupEndpoints<'_, 'id, Caps, SC, AC, C, M, T, K, S, P> {
        let (setup_tx, setup_rx) = self.setup.split();
        let (phase_tx, phase_rx) = self.phase.split();
        (
            SetupSender {
                channels: SendChannels {
                    setup: Sender::Queued(QueuedSender {
                        queue: setup_tx,
                        wake: &self.setup_wake,
                    }),
                    phase: Sender::Queued(QueuedSender {
                        queue: phase_tx,
                        wake: &self.phase_wake,
                    }),
                    issuer,
                },
                enable: Enable { _place: enable },
                disable: Disable { _place: disable },
            },
            SetupReceiver {
                channels: ReceiveChannels {
                    setup: Receiver {
                        queue: setup_rx,
                        wake: &self.setup_wake,
                    },
                    phase: Receiver {
                        queue: phase_rx,
                        wake: &self.phase_wake,
                    },
                },
            },
        )
    }
}
struct SendChannels<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> {
    setup: Sender<'q, SetupCommand<'id, Caps, SC, AC>, S>,
    phase: Sender<'q, PhaseCommand<'id, Caps, SC, AC, C, M>, P>,
    issuer: GateIssuer<'id, GatePath>,
}
struct ReceiveChannels<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    setup: Receiver<'q, SetupCommand<'id, Caps, SC, AC>, S>,
    phase: Receiver<'q, PhaseCommand<'id, Caps, SC, AC, C, M>, P>,
}
#[must_use]
pub struct SetupSender<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> {
    channels: SendChannels<'q, 'id, Caps, SC, AC, C, M, T, K, S, P>,
    enable: Enable<'id>,
    disable: Disable<'id>,
}
#[must_use]
pub struct PhaseSender<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> {
    channels: SendChannels<'q, 'id, Caps, SC, AC, C, M, T, K, S, P>,
    disable: Disable<'id>,
}
/// The sole producer waiting for its owned Disable reply; no command method.
#[must_use]
pub struct DisablingSender<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> {
    channels: SendChannels<'q, 'id, Caps, SC, AC, C, M, T, K, S, P>,
}
#[must_use]
pub struct SetupReceiver<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    channels: ReceiveChannels<'q, 'id, Caps, SC, AC, C, M, S, P>,
}
#[must_use]
pub struct PhaseReceiver<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    channels: ReceiveChannels<'q, 'id, Caps, SC, AC, C, M, S, P>,
    enable: Enable<'id>,
}
impl<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> SetupSender<'q, 'id, Caps, SC, AC, C, M, T, K, S, P>
{
    pub fn configure(&mut self, command: Configuration<'id, Caps, SC, AC>) {
        self.channels.send_setup(SetupCommand::Configure(command));
    }
    pub fn retune(&mut self, permission: Slot<'id, RetuneKind, Child<Root, 7>>, channel: Channel) {
        self.channels.send_setup(SetupCommand::Retune {
            permission,
            channel,
        });
    }
    /// Publishes the final setup command before transferring the producer.
    pub fn enable(mut self) -> PhaseSender<'q, 'id, Caps, SC, AC, C, M, T, K, S, P> {
        self.channels.send_setup(SetupCommand::Enable(self.enable));
        PhaseSender {
            channels: self.channels,
            disable: self.disable,
        }
    }
}
impl<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> PhaseSender<'q, 'id, Caps, SC, AC, C, M, T, K, S, P>
{
    pub fn configure(&mut self, command: Configuration<'id, Caps, SC, AC>) {
        self.channels.send_phase(PhaseCommand::Configure(command));
    }
    pub fn sample_tsf(&mut self, permission: Sampling<'id>) {
        self.channels
            .send_phase(PhaseCommand::SampleTsf(permission));
    }
    pub fn submit(
        &mut self,
        operation: InFlight<C, Slot<'id, TicketKind, TicketPath>, M>,
        admission: TxAdmission<'id, CorePath>,
    ) {
        self.channels.send_phase(PhaseCommand::Submit(Submission {
            operation,
            admission,
        }));
    }
    pub fn install_pairwise(
        &mut self,
        permission: Slot<'id, InstallKeyKind, Child<Root, 3>>,
        key: Ccmp128Key,
    ) {
        self.channels
            .send_phase(PhaseCommand::InstallPairwise { permission, key });
    }
    pub fn remove_pairwise(&mut self, permission: Slot<'id, RemoveKeyKind, Child<Root, 4>>) {
        self.channels
            .send_phase(PhaseCommand::RemovePairwise(permission));
    }
    pub fn mutate_group(
        &mut self,
        permission: Slot<'id, GroupMutationKind, Child<Root, 5>>,
        key: Ccmp128Key,
    ) {
        self.channels
            .send_phase(PhaseCommand::MutateGroup { permission, key });
    }
    pub fn close_gate(&mut self, gate: GateOpen<'id, GatePath>) {
        self.channels
            .send_phase(PhaseCommand::CloseGate(ClosingGate { gate }));
    }
    /// Complete the driver's barrier reply at the original gate owner.
    pub fn closed(&mut self, barrier: GateBarrier<'id>) -> GateClosed<'id, GatePath> {
        self.channels.issuer.close(barrier.gate)
    }
    /// No public command constructor or later publication step exists.
    pub fn open_gate(&mut self, gate: GateClosed<'id, GatePath>) -> GateOpen<'id, GatePath> {
        self.channels.send_phase(PhaseCommand::OpenGate);
        self.channels.issuer.open(gate)
    }
    pub fn disable(mut self) -> DisablingSender<'q, 'id, Caps, SC, AC, C, M, T, K, S, P> {
        self.channels
            .send_phase(PhaseCommand::Disable(self.disable));
        DisablingSender {
            channels: self.channels,
        }
    }
}
impl<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> DisablingSender<'q, 'id, Caps, SC, AC, C, M, T, K, S, P>
{
    pub fn finished(
        self,
        reply: Disabled<'id>,
    ) -> SetupSender<'q, 'id, Caps, SC, AC, C, M, T, K, S, P> {
        SetupSender {
            channels: self.channels,
            enable: reply.enable,
            disable: reply.disable,
        }
    }
}
impl<'q, 'id, Caps: Ieee80211Capabilities, SC, AC, C, M, const S: usize, const P: usize>
    SetupReceiver<'q, 'id, Caps, SC, AC, C, M, S, P>
{
    pub async fn next(&mut self) -> SetupCommand<'id, Caps, SC, AC> {
        poll_fn(|cx| self.channels.setup.poll(cx)).await
    }
    /// S31 enable is synchronous; no intermediate hardware Enabling state.
    pub fn enabled(self, enable: Enable<'id>) -> PhaseReceiver<'q, 'id, Caps, SC, AC, C, M, S, P> {
        PhaseReceiver {
            channels: self.channels,
            enable,
        }
    }
}
#[must_use]
pub struct ClosingGate<'id> {
    gate: GateOpen<'id, GatePath>,
}
#[must_use]
pub struct GateBarrier<'id> {
    gate: GateOpen<'id, GatePath>,
}
impl<'q, 'id, Caps: Ieee80211Capabilities, SC, AC, C, M, const S: usize, const P: usize>
    PhaseReceiver<'q, 'id, Caps, SC, AC, C, M, S, P>
{
    pub async fn next(&mut self) -> PhaseCommand<'id, Caps, SC, AC, C, M> {
        poll_fn(|cx| self.channels.phase.poll(cx)).await
    }
    /// Called by the driver only after its close barrier finishes.
    pub fn gate_barrier(&mut self, closing: ClosingGate<'id>) -> GateBarrier<'id> {
        GateBarrier { gate: closing.gate }
    }
    /// Called after withdrawing terminal owners and recycling RX locally.
    pub fn disabled(
        self,
        disable: Disable<'id>,
    ) -> (
        SetupReceiver<'q, 'id, Caps, SC, AC, C, M, S, P>,
        Disabled<'id>,
    ) {
        (
            SetupReceiver {
                channels: self.channels,
            },
            Disabled {
                enable: self.enable,
                disable,
            },
        )
    }
}

// Only these private envelopes implement the memory credit contract. Public
// commands alone cannot be submitted through a generic queue API (notably the
// unit OpenGate variant). Construction happens only in the two typed sends.
struct Entry<T, const N: usize> {
    index: usize,
    command: T,
}
#[allow(
    unsafe_code,
    reason = "the sole Mint and typed sends own every indexed permission"
)]
// SAFETY: send_setup/send_phase create command Entries; the direct bridge
// preserves that same index in its private unit Entry. CommandAuthority binds
// their capacities and brand to the one CommandPools mint. Slot indices occupy
// disjoint ranges; configure/TSF have one place per concrete interface type.
// A value consumes its permission until receive has freed its cell and the
// terminal returns it. Enable/Disable cross phases by value. OpenGate's next
// use requires a Close reply; FIFO receipt of that Close follows receipt of
// the previous Open. The private GateIssuer cannot bypass that barrier.
// Thus equal indices cannot coexist in the same queue. Returned owners / gate
// barriers also order reuse across cores. Const range ends are checked against
// N at CommandStorage / bridge Held construction; singleton widths and both interface totals
// are checked at module compilation, so a contract change cannot escape the
// index proof. There is no public constructor,
// credit lookup or generic insertion. Direct publication keeps the brand
// in Held and routes pointer-free names only through its sealed paired lanes;
// callers cannot interchange names or stores from different installations.
unsafe impl<T, const N: usize> UniqueQueueIndex<N> for Entry<T, N> {
    fn queue_index(&self) -> usize {
        self.index
    }
}

// Each base follows the whole preceding minted range. The final end is
// checked against the independently stated capacity in CommandStorage::new.
// These types have no instances or runtime geometry fields.
struct SetupPlaces;
impl SetupPlaces {
    const STATION_CONFIGURE: usize = 0;
    const ACCESS_POINT_CONFIGURE: usize = Self::STATION_CONFIGURE + CONFIGURE_PER_INTERFACE;
    const RETUNE: usize = Self::ACCESS_POINT_CONFIGURE + CONFIGURE_PER_INTERFACE;
    const ENABLE: usize = Self::RETUNE + RETUNE_COMMAND;
    const END: usize = Self::ENABLE + LIFECYCLE_COMMAND;

    fn index<Caps: Ieee80211Capabilities, SC, AC>(
        command: &SetupCommand<'_, Caps, SC, AC>,
    ) -> usize {
        match command {
            SetupCommand::Configure(Configuration::Station { .. }) => Self::STATION_CONFIGURE,
            SetupCommand::Configure(Configuration::AccessPoint { .. }) => {
                Self::ACCESS_POINT_CONFIGURE
            }
            SetupCommand::Retune { .. } => Self::RETUNE,
            SetupCommand::Enable(_) => Self::ENABLE,
        }
    }
}
struct PhasePlaces<const T: usize, const K: usize>;
impl<const T: usize, const K: usize> PhasePlaces<T, K> {
    const SUBMIT: usize = 0;
    const INSTALL: usize = Self::SUBMIT + T;
    const REMOVE: usize = Self::INSTALL + K;
    const GROUP: usize = Self::REMOVE + K;
    const STATION_CONFIGURE: usize = Self::GROUP + GROUP_PERMISSIONS;
    const STATION_TSF: usize = Self::STATION_CONFIGURE + CONFIGURE_PER_INTERFACE;
    const ACCESS_POINT_CONFIGURE: usize =
        Self::STATION_CONFIGURE + InterfaceCommandPool::<Station>::COMMAND_CAPACITY;
    const ACCESS_POINT_TSF: usize = Self::ACCESS_POINT_CONFIGURE + CONFIGURE_PER_INTERFACE;
    const CLOSE_GATE: usize =
        Self::ACCESS_POINT_CONFIGURE + InterfaceCommandPool::<AccessPoint>::COMMAND_CAPACITY;
    const OPEN_GATE: usize = Self::CLOSE_GATE + 1;
    const DISABLE: usize = Self::CLOSE_GATE + GATE_COMMANDS;
    const END: usize = Self::DISABLE + LIFECYCLE_COMMAND;

    fn index<Caps: Ieee80211Capabilities, SC, AC, C, M>(
        command: &PhaseCommand<'_, Caps, SC, AC, C, M>,
    ) -> usize {
        match command {
            PhaseCommand::Submit(submission) => {
                Self::SUBMIT + usize::from(submission.operation.ticket().index())
            }
            PhaseCommand::InstallPairwise { permission, .. } => {
                Self::INSTALL + usize::from(permission.index())
            }
            PhaseCommand::RemovePairwise(permission) => {
                Self::REMOVE + usize::from(permission.index())
            }
            PhaseCommand::MutateGroup { permission, .. } => {
                Self::GROUP + usize::from(permission.index())
            }
            PhaseCommand::Configure(Configuration::Station { .. }) => Self::STATION_CONFIGURE,
            PhaseCommand::Configure(Configuration::AccessPoint { .. }) => {
                Self::ACCESS_POINT_CONFIGURE
            }
            PhaseCommand::SampleTsf(Sampling::Station(_)) => Self::STATION_TSF,
            PhaseCommand::SampleTsf(Sampling::AccessPoint(_)) => Self::ACCESS_POINT_TSF,
            PhaseCommand::CloseGate(_) => Self::CLOSE_GATE,
            PhaseCommand::OpenGate => Self::OPEN_GATE,
            PhaseCommand::Disable(_) => Self::DISABLE,
        }
    }
}
impl<
    'q,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const T: usize,
    const K: usize,
    const S: usize,
    const P: usize,
> SendChannels<'q, 'id, Caps, SC, AC, C, M, T, K, S, P>
{
    fn send_setup(&mut self, command: SetupCommand<'id, Caps, SC, AC>) {
        let index = SetupPlaces::index(&command);
        self.setup.send(Entry { index, command });
    }
    fn send_phase(&mut self, command: PhaseCommand<'id, Caps, SC, AC, C, M>) {
        let index = PhasePlaces::<T, K>::index(&command);
        self.phase.send(Entry { index, command });
    }
}
struct QueuedSender<'q, T, const N: usize> {
    queue: AffineIndexedSpscSender<'q, Entry<T, N>, N>,
    wake: &'q AtomicWaker,
}
enum Sender<'q, T: oer_memory::Quarantine, const N: usize> {
    Queued(QueuedSender<'q, T, N>),
    Direct(bridge::DirectSender<'q, T, N>),
}
impl<T: oer_memory::Quarantine, const N: usize> Sender<'_, T, N> {
    fn send(&mut self, entry: Entry<T, N>) {
        match self {
            Self::Queued(sender) => {
                sender.queue.send(entry);
                sender.wake.wake();
            }
            Self::Direct(sender) => sender.send(entry),
        }
    }
}
struct Receiver<'q, T, const N: usize> {
    queue: AffineIndexedSpscReceiver<'q, Entry<T, N>, N>,
    wake: &'q AtomicWaker,
}
impl<T, const N: usize> Receiver<'_, T, N> {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<T> {
        self.wake.register(cx.waker());
        match self.queue.try_receive() {
            Ok(entry) => Poll::Ready(entry.command),
            Err(_) => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests;

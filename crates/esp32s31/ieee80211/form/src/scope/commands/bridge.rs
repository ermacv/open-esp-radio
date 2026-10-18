//! Direct command-publication prototype, without a hardware PortBackend.
//!
//! Whole branded owners stay in `Held`. Only execution records and affine
//! place names enter permanent storage. The model executor never dereferences
//! a DMA address or programs MMIO: its completion means the model's finite
//! operation finished. A real DMA terminal requires the driver's completion
//! proof, not merely acceptance by a hardware queue.
//!
//! Cancellation of a reply wait leaves every owner in Held. When the driver's
//! stop proved DMA quiescent (on any of the form's own stop paths; the core's
//! fail-stop and whole-run cancel still quarantine, ), `Held::finish`
//! takes Held (ending every producer endpoint), this binding's executor
//! (lent by `StoppedOwners::parts` from the stopped `Bridged` hardware) and
//! that `DriverStopped` proof: it withdraws every pending record with its key
//! erased, takes every replied record back and returns every remaining owner.
//! Without the proof Held stays quarantined: dropping it leaks every owner,
//! never dropping one. After every stop whose drain completed, the form
//! itself withdraws the pending records of the executor inside `Bridged` and
//! zeroes their keys before any owner can go into quarantine. A key command
//! published after that stop keeps its record copy until reset.
//! Forgetting Held cannot free the permanent records; names have no pointer
//! back to it. Composition wakers may schedule, never synchronously detach.
//!
//! Key commands and execution records use the same zeroizing `Ccmp128Key`.
//! Projection borrows material to initialize the record owner and erases the
//! Held owner before publication. Apply borrows the permanent owner; apply
//! and withdrawal both erase it in place before publishing their typed reply.

use super::*;
use crate::scope::{DriverStopped, OwnsDma, ReturnedOwners, StoppedOwners, sealed};
use core::marker::PhantomData;
use oer_ieee80211_lower_mac::{ExecutionPayload, ReceiveFilter};
use oer_memory::{
    CustodyDomain, Drained, IndexedCustody, Quarantine, RetainedName, StableDmaRegion,
};
use zeroize::Zeroize;

/// Fixture descriptor budget: header and two independently retained bodies.
/// This is a prototype budget, not a one-buffer MPDU ownership restriction.
/// The hardware descriptor-chain layout and CCMP support still require HIL.
pub const MODEL_FRAGMENTS: usize = 3;

/// Address metadata, not a CPU reference or permission to start hardware DMA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaSpan {
    address: usize,
    length: usize,
}
impl DmaSpan {
    pub fn from_region(region: StableDmaRegion<'_>) -> Self {
        let bytes = region.into_mut_slice();
        Self {
            address: bytes.as_mut_ptr() as usize,
            length: bytes.len(),
        }
    }
    pub const fn address(self) -> usize {
        self.address
    }
    pub const fn len(self) -> usize {
        self.length
    }
    pub const fn is_empty(self) -> bool {
        self.length == 0
    }
}

/// Data-only projection of a model MPDU; all leases remain in the InFlight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxImage {
    pub fragments: [DmaSpan; MODEL_FRAGMENTS],
    pub sequence: u16,
    pub packet_number: u64,
}
/// Project only the narrow `ExecutionPayload::Execution` view, never an
/// `&mut Self` owner. The associated function avoids a higher-ranked GAT
/// bound that would accidentally require a borrowed payload to be 'static.
/// No borrow, owner or Context may be returned. The view prepares retained
/// stable buffers for a reader before publishing their metadata.
pub trait ProjectExecution: ExecutionPayload {
    fn project(view: Self::Execution<'_>) -> TxImage;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReceiveImage {
    Off,
    OtherBssManagement,
    Bss {
        bssid: [u8; 6],
        filter: ReceiveFilter,
    },
    AccessPoint(ReceiveFilter),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Configure(ReceiveImage),
    Retune(Channel),
    Enable,
    SampleStationTsf,
    SampleAccessPointTsf,
    Transmit(TxImage),
    InstallPairwise(usize),
    RemovePairwise(usize),
    MutateGroup(usize),
    CloseGate,
    OpenGate,
    Disable,
}
/// Model observation (for example a sampled TSF). Carries no owner or address.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Observation(pub u64);
/// A model operation either ran to completion or was withdrawn without being
/// applied. Withdrawal never manufactures a TSF sample or hardware success.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    Applied(Observation),
    Withdrawn,
}

/// Finite model step. A real S31 backend is deliberately not implemented here.
/// Apply pending settings/BA after command Acquire and before this command.
/// A key is borrowed only while programming the slot; it must not be retained.
pub trait ModelHardware {
    fn apply(&mut self, operation: Operation, key: &[u8; 16]) -> Observation;
}
struct Record {
    operation: Operation,
    key: Ccmp128Key,
    completion: Completion,
}
impl Record {
    fn plain(operation: Operation) -> Self {
        Self {
            operation,
            key: Ccmp128Key::new(&[0; 16]),
            completion: Completion::Withdrawn,
        }
    }
    fn secret(operation: Operation, key: &mut Ccmp128Key) -> Self {
        let record = Self {
            operation,
            key: Ccmp128Key::new(key.as_bytes()),
            completion: Completion::Withdrawn,
        };
        // The original command retains its permission, not a second secret.
        key.zeroize();
        record
    }
    fn execute(&mut self, hardware: &mut impl ModelHardware) {
        let observation = hardware.apply(self.operation, self.key.as_bytes());
        // Erase the actual permanent cell, before its name enters the reply
        // queue. Moving the key to a temporary and erasing that is insufficient.
        self.key.zeroize();
        self.completion = Completion::Applied(observation);
    }
    fn withdraw(&mut self) {
        self.key.zeroize();
        self.completion = Completion::Withdrawn;
    }
}
/// The permanent record store is static and never destroyed, so this runs
/// only if a binding ever drops one. Pending record keys are erased by
/// `Executor::erase_pending_keys` (on `Held::finish`, and by the form after
/// every stop whose drain completed) and by the executor's drop; execution
/// erases them before replying.
#[allow(unsafe_code, reason = "record quarantine only zeroes its key copy")]
// SAFETY: it zeroes the key bytes in place; operation and completion stay,
// and nothing is released, moved or awaited.
unsafe impl Quarantine for Record {
    fn quarantine(&mut self) {
        self.key.zeroize();
    }
}

// These names cannot be constructed, obtained, or routed by a bridge caller.
// The sole binding consumes the Mint's CommandAuthority and &'static mut
// ExecutionStorage; split lends only that binding's endpoints. Even two
// installations with identical types cannot interchange a reply or a store.
struct Owners;
struct Records;
type OwnerName<const N: usize> = RetainedName<Entry<(), N>, N>;
type RecordName<const N: usize> = RetainedName<OwnerName<N>, N>;
#[allow(
    unsafe_code,
    reason = "sealed endpoints bind Mint names to one Held store"
)]
// SAFETY: Held::bind is the only constructor and consumes one CommandAuthority.
// DirectSender receives only entries made by the existing typed admissions;
// all range and uniqueness proofs are shared with Entry. Sender and Replies
// borrow the same private store. The exact name returns along its private
// reply endpoint before taking the owner. Neither names nor stores escape,
// including when there are multiple instances of this zero-sized domain type.
// Names contain only an index, with no lifetime, pointer or owner destructor.
unsafe impl<const N: usize> CustodyDomain<N> for Owners {
    type Name = Entry<(), N>;
    type Stopped = Stopped;
}
#[allow(
    unsafe_code,
    reason = "permanent records are paired with private reply endpoints"
)]
// SAFETY: an exclusive static lane is split once by Held::bind. Only that
// lane's sender inserts a retained owner name, only its executor receives
// the resulting record name, and only its paired Replies takes the record.
// Both names preserve Entry's unique index and response-ordered reuse. No
// public API can mix lanes or installations, recreate names, or recover the
// static storage. Names cannot access the owner store, even after forget.
unsafe impl<const N: usize> CustodyDomain<N> for Records {
    type Name = OwnerName<N>;
    type Stopped = Stopped;
}

/// The custody stores' stop proof, issued only inside [`Held::finish`].
pub struct Stopped {
    _private: (),
}
/// Held refused an executor of another binding; Held returns unchanged.
#[must_use = "a foreign executor returns Held unchanged"]
pub struct ForeignExecutor<H> {
    pub held: H,
}

type OwnersOf<T, const N: usize> = IndexedCustody<T, Owners, N>;
type RecordsOf<const N: usize> = IndexedCustody<Record, Records, N>;
struct Lane<const N: usize> {
    records: RecordsOf<N>,
    requests: AffineIndexedSpscQueue<RecordName<N>, N>,
    replies: AffineIndexedSpscQueue<RecordName<N>, N>,
    request_wake: AtomicWaker,
    reply_wake: AtomicWaker,
}
impl<const N: usize> Lane<N> {
    const fn new() -> Self {
        Self {
            records: IndexedCustody::new(Records),
            requests: AffineIndexedSpscQueue::new(),
            replies: AffineIndexedSpscQueue::new(),
            request_wake: AtomicWaker::new(),
            reply_wake: AtomicWaker::new(),
        }
    }
    fn split(&'static mut self) -> (CallerLane<N>, ExecutorLane<N>) {
        let (send, receive) = self.requests.split();
        let (reply, replied) = self.replies.split();
        (
            CallerLane {
                records: &self.records,
                send,
                replied,
                request_wake: &self.request_wake,
                reply_wake: &self.reply_wake,
            },
            ExecutorLane {
                records: &self.records,
                receive,
                reply,
                reply_wake: &self.reply_wake,
                request_wake: &self.request_wake,
            },
        )
    }
}
/// Composition-owned permanent SRAM, disjoint from the driver's exclusive
/// region. `bind` consumes its static lease exactly once; this prototype has
/// no reinstall or backing-recovery operation. Dropping a future or a boxed
/// Held never frees these records. No allocator is needed by the binding.
pub struct ExecutionStorage<const S: usize, const P: usize> {
    setup: Lane<S>,
    phase: Lane<P>,
}
impl<const S: usize, const P: usize> ExecutionStorage<S, P> {
    pub const fn new() -> Self {
        Self {
            setup: Lane::new(),
            phase: Lane::new(),
        }
    }
}
impl<const S: usize, const P: usize> Default for ExecutionStorage<S, P> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> CallerLane<N> {
    /// Take back every replied record, at most N, dropping each one.
    fn take_replied_records(&mut self) -> usize {
        let mut taken = 0;
        for _ in 0..N {
            let Ok(name) = self.replied.try_receive() else {
                break;
            };
            let (_owner, record) = self.records.take(name);
            drop(record);
            taken += 1;
        }
        taken
    }
}
struct CallerLane<const N: usize> {
    records: &'static RecordsOf<N>,
    send: AffineIndexedSpscSender<'static, RecordName<N>, N>,
    replied: AffineIndexedSpscReceiver<'static, RecordName<N>, N>,
    request_wake: &'static AtomicWaker,
    reply_wake: &'static AtomicWaker,
}
struct ExecutorLane<const N: usize> {
    records: &'static RecordsOf<N>,
    receive: AffineIndexedSpscReceiver<'static, RecordName<N>, N>,
    reply: AffineIndexedSpscSender<'static, RecordName<N>, N>,
    reply_wake: &'static AtomicWaker,
    request_wake: &'static AtomicWaker,
}
impl<const N: usize> ExecutorLane<N> {
    fn withdraw_pending(&mut self) {
        // At most N names, so this finite operation cannot spin even while a
        // producer still publishes; it runs with exclusive executor access.
        for _ in 0..N {
            let Ok(mut name) = self.receive.try_receive() else {
                break;
            };
            self.records.with_mut(&mut name, Record::withdraw);
            self.reply.send(name);
        }
        self.reply_wake.wake();
    }
    fn step(&mut self, hardware: &mut impl ModelHardware) -> Option<Operation> {
        let mut name = self.receive.try_receive().ok()?;
        let operation = self.records.with_mut(&mut name, |record| {
            record.execute(hardware);
            record.operation
        });
        self.reply.send(name);
        self.reply_wake.wake();
        Some(operation)
    }
}
#[derive(Clone, Copy)]
enum Stage {
    Setup,
    Phase,
}
/// The separate executor owns only permanent cells and names, never Held.
/// Each step completes at most one model operation. It has no awaited local
/// owner; dropping a reply wait leaves finish/abandon all accepted commands.
///
/// `D` is the type of the DMA owner its records reach ( ,
/// ), fixed at [`Bridged::bind`]. Outside the form it steps only
/// through [`Bridged::step`], on the DMA owner of the hardware beside it.
pub struct Executor<D, const S: usize, const P: usize> {
    setup: ExecutorLane<S>,
    phase: ExecutorLane<P>,
    stage: Stage,
    _dma: PhantomData<fn(&mut D)>,
}
impl<D, const S: usize, const P: usize> Executor<D, S, P> {
    /// Withdraw every pending record and erase its key copy, keeping the
    /// executor. Call it only when no step can be inside this executor: the
    /// form does, after a completed drain and stop. After a drain timeout no
    /// stop runs; those copies stay in SRAM until reset.
    pub(crate) fn erase_pending_keys(&mut self) {
        self.setup.withdraw_pending();
        self.phase.withdraw_pending();
    }
    /// Register before checking work, so direct publication cannot lose a
    /// wake between the empty observation and parking the separate executor.
    pub(crate) fn poll_step(&mut self, cx: &mut Context<'_>, dma: &mut D) -> Poll<Operation>
    where
        D: ModelHardware,
    {
        self.setup.request_wake.register(cx.waker());
        self.phase.request_wake.register(cx.waker());
        match self.step(dma) {
            Some(operation) => Poll::Ready(operation),
            None => Poll::Pending,
        }
    }
    /// Run at most one operation on the DMA owner `dma`. Outside the form it
    /// is reachable only through [`Bridged::step`].
    pub(crate) fn step(&mut self, dma: &mut D) -> Option<Operation>
    where
        D: ModelHardware,
    {
        let operation = match self.stage {
            Stage::Setup => self.setup.step(dma),
            Stage::Phase => self.phase.step(dma),
        }?;
        match operation {
            Operation::Enable => self.stage = Stage::Phase,
            Operation::Disable => self.stage = Stage::Setup,
            _ => {}
        }
        Some(operation)
    }
}
/// Dropping the executor withdraws every pending record and erases its key
/// copy; an executed record erased its key before its reply. Quarantine
/// never runs this destructor: after every stop whose drain completed the form
/// erases the pending keys of the executor inside [`Bridged`] itself.
impl<D, const S: usize, const P: usize> Drop for Executor<D, S, P> {
    fn drop(&mut self) {
        self.setup.withdraw_pending();
        self.phase.withdraw_pending();
    }
}

/// Driver hardware together with its bridge executor: the only way outside
/// the form to bind a Held and to step its executor ( ).
///
/// The executor is bound for `HW`'s DMA owner type and steps only on that
/// owner, borrowed from `HW` beside it. The form finds the executor here to
/// erase pending keys after every stop whose drain completed, and
/// [`StoppedOwners::parts`] lends it beside a stop proof, never `HW`.
pub struct Bridged<HW: OwnsDma, const S: usize, const P: usize> {
    hardware: HW,
    executor: Executor<HW::Dma, S, P>,
}
impl<HW: OwnsDma, const S: usize, const P: usize> Bridged<HW, S, P> {
    /// Bind the command stores for `hardware`'s DMA owner type, returning
    /// the caller-side Held and the composite to install as the driver's
    /// hardware.
    #[allow(
        clippy::type_complexity,
        reason = "Held names every owner type it retains"
    )]
    pub fn bind<'id, Caps: Ieee80211Capabilities, SC, AC, C, M, const T: usize, const K: usize>(
        hardware: HW,
        authority: CommandAuthority<'id, Root, T, K>,
        storage: &'static mut ExecutionStorage<S, P>,
    ) -> (Held<'id, Caps, SC, AC, C, M, T, K, S, P, HW::Dma>, Self) {
        let (held, executor) = Held::bind(authority, storage);
        (held, Self { hardware, executor })
    }
    /// The driver's hardware, for its own MMIO; the executor stays here.
    pub fn hardware(&mut self) -> &mut HW {
        &mut self.hardware
    }
    /// The executor, for [`Held::finish`]; it cannot step without `HW`.
    pub fn executor(&mut self) -> &mut Executor<HW::Dma, S, P> {
        &mut self.executor
    }
    /// Run at most one bridge operation on this hardware's DMA owner.
    pub fn step(&mut self) -> Option<Operation>
    where
        HW::Dma: ModelHardware,
    {
        self.executor.step(sealed::OwnsDma::dma(&mut self.hardware))
    }
    /// [`Self::step`], registering the waker first.
    pub fn poll_step(&mut self, cx: &mut Context<'_>) -> Poll<Operation>
    where
        HW::Dma: ModelHardware,
    {
        self.executor
            .poll_step(cx, sealed::OwnsDma::dma(&mut self.hardware))
    }
}
impl<HW: OwnsDma, const S: usize, const P: usize> sealed::OwnsDma for Bridged<HW, S, P> {
    type Dma = HW::Dma;
    fn dma(&mut self) -> &mut HW::Dma {
        sealed::OwnsDma::dma(&mut self.hardware)
    }
    fn erase_pending_keys(&mut self) {
        self.executor.erase_pending_keys();
        sealed::OwnsDma::erase_pending_keys(&mut self.hardware);
    }
}
impl<HW: OwnsDma, const S: usize, const P: usize, I, PL, E, SC>
    StoppedOwners<ReturnedOwners<Bridged<HW, S, P>, I, PL, E, SC>>
{
    /// Lend the composite's executor and the stop proof together, for
    /// finishing its Held. Only the executor is lent, never the hardware or
    /// its DMA owner, so no step can restart DMA while the proof lives
    /// ( ).
    pub fn parts(&mut self) -> (&mut Executor<HW::Dma, S, P>, DriverStopped<'_, HW::Dma>) {
        (
            &mut self.owners.hardware.executor,
            DriverStopped {
                _scope: PhantomData,
                _dma: PhantomData,
            },
        )
    }
}

/// Caller-owned branded waiting places. No Context: Send or Context: 'static
/// bound is imposed. [`Bridged::bind`] returns it beside the composite that
/// owns the executor.
pub struct Held<
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
    D,
> {
    setup: OwnersOf<SetupCommand<'id, Caps, SC, AC>, S>,
    phase: OwnersOf<PhaseCommand<'id, Caps, SC, AC, C, M>, P>,
    setup_lane: CallerLane<S>,
    phase_lane: CallerLane<P>,
    _authority: CommandAuthority<'id, Root, T, K>,
    /// The DMA owner type its records reach, fixed at bind.
    _dma: PhantomData<fn(&mut D)>,
}
/// Every command owner a finished Held still retained, in place order.
pub struct HeldOwners<
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    pub setup: Drained<SetupCommand<'id, Caps, SC, AC>, S>,
    pub phase: Drained<PhaseCommand<'id, Caps, SC, AC, C, M>, P>,
    /// Permanent records finish took back out of the static storage after
    /// the executor's withdrawal: every replied record nobody had received,
    /// applied or withdrawn. No key copy of any of them remains there.
    pub records_taken_back: usize,
}
/// Borrowed producer and reply consumer for one direct-publication binding.
pub type Endpoints<
    'a,
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
    SetupSender<'a, 'id, Caps, SC, AC, C, M, T, K, S, P>,
    Replies<'a, 'id, Caps, SC, AC, C, M, S, P>,
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
    D,
> Held<'id, Caps, SC, AC, C, M, T, K, S, P, D>
{
    /// Bind the command stores for the DMA owner type `D`. Outside the form
    /// it is reachable only through [`Bridged::bind`].
    pub(crate) fn bind(
        authority: CommandAuthority<'id, Root, T, K>,
        storage: &'static mut ExecutionStorage<S, P>,
    ) -> (Self, Executor<D, S, P>) {
        const {
            assert!(S == CommandPools::<T, K>::SETUP_STORAGE && S == SetupPlaces::END);
            assert!(P == CommandPools::<T, K>::PHASE_STORAGE && P == PhasePlaces::<T, K>::END);
        }
        let (setup_lane, setup) = storage.setup.split();
        let (phase_lane, phase) = storage.phase.split();
        (
            Self {
                setup: IndexedCustody::new(Owners),
                phase: IndexedCustody::new(Owners),
                setup_lane,
                phase_lane,
                _authority: authority,
                _dma: PhantomData,
            },
            Executor {
                setup,
                phase,
                stage: Stage::Setup,
                _dma: PhantomData,
            },
        )
    }
    /// The ordinary end, after the driver's stop proved DMA quiescent on any
    /// of the form's own stop paths (the core's fail-stop and whole-run cancel
    /// still quarantine, ). Consuming Held ends every producer endpoint, so no command
    /// can be published afterwards; its executor, which may live inside the
    /// stopped hardware ([`crate::scope::StoppedOwners::parts`]), then withdraws
    /// every pending record with its key erased, finish takes every replied
    /// record (applied or withdrawn) back out of the static storage, and every
    /// command owner Held still retains returns to its pool. Without that proof Held is
    /// quarantined: dropping it keeps every owner.
    ///
    /// # Errors
    ///
    /// Held unchanged, when the executor belongs to another binding (its
    /// permanent record stores differ).
    #[allow(clippy::result_large_err, reason = "a refusal returns Held whole")]
    pub fn finish(
        self,
        executor: &mut Executor<D, S, P>,
        _dma: &crate::scope::DriverStopped<'_, D>,
    ) -> Result<HeldOwners<'id, Caps, SC, AC, C, M, S, P>, ForeignExecutor<Self>> {
        if !(core::ptr::eq(self.setup_lane.records, executor.setup.records)
            && core::ptr::eq(self.phase_lane.records, executor.phase.records))
        {
            return Err(ForeignExecutor { held: self });
        }
        // Held is consumed, so no endpoint can publish again: this withdraws
        // every record that was ever sent, erasing its key.
        executor.erase_pending_keys();
        let mut held = self;
        // Take every replied record (applied or withdrawn) out of the static
        // storage, so no record or key copy outlives this installation.
        let records_taken_back =
            held.setup_lane.take_replied_records() + held.phase_lane.take_replied_records();
        let stopped = Stopped { _private: () };
        Ok(HeldOwners {
            setup: held.setup.finish(&stopped),
            phase: held.phase.finish(&stopped),
            records_taken_back,
        })
    }
    pub fn split(
        &mut self,
        issuer: GateIssuer<'id, GatePath>,
        enable: Slot<'id, EnableKind, Child<Root, 8>>,
        disable: Slot<'id, DisableKind, Child<Root, 9>>,
    ) -> Endpoints<'_, 'id, Caps, SC, AC, C, M, T, K, S, P>
    where
        M: ProjectExecution,
    {
        let (setup, setup_rx) = direct(&self.setup, &mut self.setup_lane, project_setup);
        let (phase, phase_rx) = direct(&self.phase, &mut self.phase_lane, project_phase);
        (
            SetupSender {
                channels: SendChannels {
                    setup: Sender::Direct(setup),
                    phase: Sender::Direct(phase),
                    issuer,
                },
                enable: Enable { _place: enable },
                disable: Disable { _place: disable },
            },
            Replies {
                setup: setup_rx,
                phase: phase_rx,
            },
        )
    }
}
fn direct<'a, T: Quarantine, const N: usize>(
    owners: &'a OwnersOf<T, N>,
    lane: &'a mut CallerLane<N>,
    project: fn(&mut T) -> Record,
) -> (DirectSender<'a, T, N>, DirectReceiver<'a, T, N>) {
    (
        DirectSender {
            owners,
            records: lane.records,
            send: &mut lane.send,
            wake: lane.request_wake,
            project,
        },
        DirectReceiver {
            owners,
            records: lane.records,
            receive: &mut lane.replied,
            wake: lane.reply_wake,
        },
    )
}
pub(super) struct DirectSender<'a, T: Quarantine, const N: usize> {
    owners: &'a OwnersOf<T, N>,
    records: &'static RecordsOf<N>,
    send: &'a mut AffineIndexedSpscSender<'static, RecordName<N>, N>,
    wake: &'static AtomicWaker,
    project: fn(&mut T) -> Record,
}
impl<T: Quarantine, const N: usize> DirectSender<'_, T, N> {
    pub(super) fn send(&mut self, entry: Entry<T, N>) {
        let mut name = self.owners.insert(
            Entry {
                index: entry.index,
                command: (),
            },
            entry.command,
        );
        let record = self.owners.with_mut(&mut name, self.project);
        let name = self.records.insert(name, record);
        self.send.send(name);
        self.wake.wake();
    }
}
struct DirectReceiver<'a, T: Quarantine, const N: usize> {
    owners: &'a OwnersOf<T, N>,
    records: &'static RecordsOf<N>,
    receive: &'a mut AffineIndexedSpscReceiver<'static, RecordName<N>, N>,
    wake: &'static AtomicWaker,
}
/// The exact original owner, distinguished from an applied command by type.
pub enum Reply<T> {
    Applied(Applied<T>),
    Withdrawn(T),
}
/// Receipt created only by the model step's completed reply path. The fields
/// and constructor are private: recovering an owner consumes this receipt.
pub struct Applied<T> {
    command: T,
    observation: Observation,
}
impl<T> Applied<T> {
    pub fn command(&self) -> &T {
        &self.command
    }
    pub const fn observation(&self) -> Observation {
        self.observation
    }
    pub fn into_parts(self) -> (T, Observation) {
        (self.command, self.observation)
    }
}
impl<T> Reply<T> {
    pub fn completion(&self) -> Completion {
        match self {
            Self::Applied(reply) => Completion::Applied(reply.observation),
            Self::Withdrawn(_) => Completion::Withdrawn,
        }
    }
    pub fn command(&self) -> &T {
        match self {
            Self::Applied(reply) => &reply.command,
            Self::Withdrawn(command) => command,
        }
    }
}
type PhaseApplied<'id, Caps, SC, AC, C, M> = Applied<PhaseCommand<'id, Caps, SC, AC, C, M>>;
pub type DisableReply<'id, Caps, SC, AC, C, M> =
    Result<Disabled<'id>, (Enable<'id>, PhaseApplied<'id, Caps, SC, AC, C, M>)>;
impl<T: Quarantine, const N: usize> DirectReceiver<'_, T, N> {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Reply<T>> {
        self.wake.register(cx.waker());
        let Ok(name) = self.receive.try_receive() else {
            return Poll::Pending;
        };
        let (name, record) = self.records.take(name);
        let (_, command) = self.owners.take(name);
        Poll::Ready(match record.completion {
            Completion::Applied(observation) => Reply::Applied(Applied {
                command,
                observation,
            }),
            Completion::Withdrawn => Reply::Withdrawn(command),
        })
    }
}
pub struct Replies<
    'a,
    'id,
    Caps: Ieee80211Capabilities,
    SC,
    AC,
    C,
    M,
    const S: usize,
    const P: usize,
> {
    setup: DirectReceiver<'a, SetupCommand<'id, Caps, SC, AC>, S>,
    phase: DirectReceiver<'a, PhaseCommand<'id, Caps, SC, AC, C, M>, P>,
}
impl<'id, Caps: Ieee80211Capabilities, SC, AC, C, M, const S: usize, const P: usize>
    Replies<'_, 'id, Caps, SC, AC, C, M, S, P>
{
    pub async fn setup(&mut self) -> Reply<SetupCommand<'id, Caps, SC, AC>> {
        poll_fn(|cx| self.setup.poll(cx)).await
    }
    pub async fn phase(&mut self) -> Reply<PhaseCommand<'id, Caps, SC, AC, C, M>> {
        poll_fn(|cx| self.phase.poll(cx)).await
    }
    /// Only an applied CloseGate can issue the barrier. A different command
    /// is returned unchanged; a withdrawn command cannot enter this method.
    pub fn gate_barrier(
        &self,
        reply: PhaseApplied<'id, Caps, SC, AC, C, M>,
    ) -> Result<GateBarrier<'id>, PhaseApplied<'id, Caps, SC, AC, C, M>> {
        match reply.command {
            PhaseCommand::CloseGate(closing) => Ok(GateBarrier { gate: closing.gate }),
            _ => Err(reply),
        }
    }
    /// A withdrawn Disable cannot recover a SetupSender. A mismatched applied
    /// command returns both owners, including the retained Enable place.
    pub fn disabled(
        &self,
        enable: Enable<'id>,
        reply: PhaseApplied<'id, Caps, SC, AC, C, M>,
    ) -> DisableReply<'id, Caps, SC, AC, C, M> {
        match reply.command {
            PhaseCommand::Disable(disable) => Ok(Disabled { enable, disable }),
            _ => Err((enable, reply)),
        }
    }
}
fn configuration<Caps: Ieee80211Capabilities, SC, AC>(
    config: &Configuration<'_, Caps, SC, AC>,
) -> Operation {
    Operation::Configure(match config {
        Configuration::Station { receive, .. } => match receive {
            StationReceive::Off => ReceiveImage::Off,
            StationReceive::OtherBssManagement => ReceiveImage::OtherBssManagement,
            StationReceive::Bss { bssid, filter } => ReceiveImage::Bss {
                bssid: bssid.get(),
                filter: filter.get(),
            },
        },
        Configuration::AccessPoint { receive, .. } => ReceiveImage::AccessPoint(receive.get()),
    })
}
fn project_setup<Caps: Ieee80211Capabilities, SC, AC>(
    command: &mut SetupCommand<'_, Caps, SC, AC>,
) -> Record {
    Record::plain(match command {
        SetupCommand::Configure(config) => configuration(config),
        SetupCommand::Retune { channel, .. } => Operation::Retune(*channel),
        SetupCommand::Enable(_) => Operation::Enable,
    })
}
fn project_phase<Caps: Ieee80211Capabilities, SC, AC, C, M: ProjectExecution>(
    command: &mut PhaseCommand<'_, Caps, SC, AC, C, M>,
) -> Record {
    Record::plain(match command {
        PhaseCommand::Configure(config) => configuration(config),
        PhaseCommand::SampleTsf(Sampling::Station(_)) => Operation::SampleStationTsf,
        PhaseCommand::SampleTsf(Sampling::AccessPoint(_)) => Operation::SampleAccessPointTsf,
        PhaseCommand::Submit(submission) => {
            Operation::Transmit(M::project(submission.operation.execute()))
        }
        PhaseCommand::InstallPairwise { permission, key } => {
            return Record::secret(
                Operation::InstallPairwise(usize::from(permission.index())),
                key,
            );
        }
        PhaseCommand::RemovePairwise(permission) => {
            Operation::RemovePairwise(usize::from(permission.index()))
        }
        PhaseCommand::MutateGroup { permission, key } => {
            return Record::secret(Operation::MutateGroup(usize::from(permission.index())), key);
        }
        PhaseCommand::CloseGate(_) => Operation::CloseGate,
        PhaseCommand::OpenGate => Operation::OpenGate,
        PhaseCommand::Disable(_) => Operation::Disable,
    })
}

#[cfg(test)]
pub(crate) mod tests;

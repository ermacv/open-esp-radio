#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
    )
)]

//! An in-memory lower-MAC backend on the phase shape
//! (`oer_ieee80211_port::phase`), for host tests of the shape and of
//! the code that drives it.
//!
//! [`ModelBackend::new`] takes the install's channel and interface
//! addresses and returns the backend, which a test hands to
//! [`run_port`](oer_radio_port::run_port), and the [`ModelAir`] it
//! plays on: the air answers each transmission, delivers received frames
//! and records what the hardware saw. Receive Block Ack agreements are
//! per-slot desired state: updates coalesce before the driver applies them,
//! without occupying the command queue. Configure and TSF sampling consume
//! their affine permissions in the FIFO; their terminals return those same
//! permissions, including through disable's drain.
//!
//! The backend has the phase's shape: the phase
//! is the body's handle over queues to the model's driver
//! (`PortBackend::drive`), which alone owns the model's hardware (its
//! queues of prepared attempts, its key table, its receive rules). The
//! handle writes commands, in one queue and in order, and setting cells;
//! the driver takes the cells before its next command, carries the
//! commands out, retries the head of each queue by its plan and posts the
//! events. The queues are cells of the one host thread a test runs on: the
//! stand-ins of a backend's lock-free single-producer single-consumer
//! queues.
//!
//! Each interface sends A-MPDUs ([`AmpduPhase`]) of up to
//! [`MODEL_AMPDU_SUBFRAMES`] subframes: under a key the new subframes are
//! numbered at their first transmission and a retransmitted one keeps its
//! number and gets the Retry bit; the air answers each aggregate with a
//! Block Ack (every subframe by default). `ModelAir::answer` queues one
//! `AirAnswer` per publication: ACK, BA bitmap, missing response, collision or
//! hardware timeout. The checked frame determines whether it expects no
//! response, an ACK or a Block Ack (including a single BAR). A mismatched
//! response poisons before publication and
//! quarantines the rejected operation until teardown. There is no separate
//! BA queue whose injection could escape to a later aggregate.
//! The builder rejects duplicate sequence numbers and subframes outside the
//! 64-sequence BA window rooted at
//! its first subframe, using sequence arithmetic modulo 4096. Refusal returns
//! both the unchanged builder and the rejected subframe before publication.
//!
//! Each concrete class pool uses the portable `MpduLayout` proof before
//! writing headers or copying an external body. Its allocation is privately
//! minted at the class capacity; `ClassFrame` proves the MPDU fits it.
//! Small and large classes share one pool implementation. Their private
//! `ModelClass` dispatch only erases or recovers the appropriate owner
//! variant, returning every other class unchanged on refusal.
//! `TxProtection` alone determines Protected and the MAC/CCMP prefix. Sent returns own that layout, body and the
//! identity of the original key installation. Retry supplies only a new
//! attempt's rates, never an independently reconstructed MPDU or body.
//! Key identity is an allocation identity in this host model; a reused slot
//! cannot match an earlier installation. A refused attempt returns all its
//! resources synchronously and allocates no PN or completion event.
//! Prepared operations retain a use of their key installation through their
//! terminal or cancellation. `KeyRemoved` waits for these owners; a returned
//! Sent retains only identity and therefore permits removal and slot reuse.
//! PTK and GTK material live in the model's installation table, separately from
//! cloneable identities. Removal clears its material after the last prepared
//! use ends. Uninstall and abandon deactivate the table, discard captured
//! ingress and clear all secrets even if a test retains an identity. Normal
//! disable/reenable preserves installed keys and their counters.
//! The unique `ModelAssociation` owns the protocol's decision. Modeled hardware
//! holds one CCMP record per interface; keys select it with their interface
//! identity, without sharing ownership of the association. The driver applies
//! its SPP mode before publishing material and records that order. A mode can
//! change only after the complete returned bank permits a new association.
//! `AirFrame::frame` contains actual CCMP ciphertext and MIC; `plaintext` is
//! the DMA image before encryption. `ModelAir::receive_ccmp` authenticates
//! encrypted input under the installed association before publishing verified
//! RX metadata. Full-bank association end is a pure ownership transition.
//! Pending GTK installation owns only a vacant slot, permission, KeyID and
//! material. The driver creates its identity and installed owner after the
//! hardware step, then publishes the success terminal at the RX boundary.
//! The key-table mutation and finite ingress capture do not wake the body.
//! Delivery wakes only after the old ingress is closed: reentrant capture
//! under the new key stays queued behind the terminal, while the frozen old
//! DMA/ISR/software facts are delivered before it.
//! A prepared retry submitted after removal is withdrawn in its AC's order,
//! without publication. Cancellation wakes the driver to finish the barrier.
//! An aggregate preflights every MPDU and the entire fresh-PN range before
//! changing any subframe. A publication refusal retains the complete operation;
//! submission after poisoning likewise transfers its owners to teardown custody.
//!
//! `ModelContext` selects an opaque context for each install lifetime, so
//! a station context can retain its `LinkEpoch` brand through every queue
//! and terminal. The model grants one epoch-source slot in its resources.
//!
//! Capacity is the tokens': every terminal returns a ticket, so no terminal
//! is ever lost; received frames beyond [`MODEL_RX_EVENTS`] queued are
//! dropped and the stream says [`EventsLost`] once.
//! Queued events keep FIFO order; after that finite queue, a pending loss
//! indication precedes the next synthetic continuous-RX frame.
//! The host fixture's VecDeque cannot express the token/capacity equation in
//! its type. A broken equation retains the offending owner, poisons the
//! backend and ends the installation without delivering later events. A
//! received owner dropped without recycle likewise poisons this fixture.

//! Publication and completion are separate owners. Tests may hold a result
//! while posting gate/key/disable barriers; only that publication can finish.
//! The model's 1 ms publication bound is synthetic model time. `set_time`
//! feeds the test's virtual clock. Hardware backends
//! must declare their own PPDU plus response/timeout bound.
//!
//! Unit tests and library consumers use the same implementation,
//! including continuous RX and immediate recycling of its bounded slots. Only the unit-test module is under `cfg(test)`.
//! Runtime's `tests/model_feature.rs` exercises these paths through the public port
//! from a separate test crate, so the model itself is compiled without that cfg.

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    rc::{Rc, Weak},
    vec::Vec,
};
use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    marker::PhantomData,
    num::NonZeroU16,
    task::{Poll, Waker},
};
use oer_ieee80211_port::{EdcaAcParameters, EdcaContention, EdcaParameterSet};
use oer_ieee80211_port::{
    association,
    association::{
        Association, AssociationPool, AssociationStart, BlockAckSlot, BlockAckSlots, GroupKeySlot,
        KeyBankParts, PairwiseKeySlot, VacantAssociation, VacantKeyBank,
    },
};
use oer_ieee80211_wire::ccmp::Ccmp128Key;
use oer_ieee80211_wire::ccmp::GroupKeyId;

use oer_ieee80211_wire::phy::{HeMcs, HtMcs, PhyRate};
use oer_radio_port::{
    Abandon, Call, ClockError, Detach, Detached, DriverExit, DriverLoan, EnableResult, EventEnd,
    InstallParts, Installed, PoisonState, PortBackend, PortPhase, Quarantine, Retire, Retired,
};
use oer_radio_port::{EventsLost, Mint, Poisoned, PortResult, Slot};
use oer_time::RadioInstant;

use oer_ieee80211_port::{
    AccessPoint, AccessPointPhase, BandSet, BssColor, Channel, CheckedAggregate, CheckedPoolSource,
    CheckedSubframe, CheckedTxSource, ConfigurePermission, Configured, ConnectionEpochKind,
    ExecutionPayload, FreshSource, FreshSubframe, GateClosed, GateIssuer, GateOpen,
    HardwareServices, Ieee80211Capabilities, Ieee80211Radio, InFlight, IndeterminateCause,
    InterfaceCommandPool, InterfacePermissions, InterfacePhase, KeyTypes, LinkMetric,
    LowerMacBackend, LowerMacCapabilities, LowerMacEvent, LowerMacPhase, LowerMacSetup,
    LowerMacTerminal, MpduKind, MpduLayout, PhaseTypes, PhyFormatSet, PreparationError,
    PreparationRefused, PreparedTx, RateSupport, ReceiveFilter, Removed, RemovedKey,
    RetransmissionPlan, RoleKeys, RxBuffer, RxEvidence, RxMeta, RxMetadata, SignalMetadata,
    Station, StationPhase, StationReceive, SupportedBlockAck, SupportedChannel, SupportedEdca,
    SupportedFilter, TxBody, TxBufferClass, TxBufferMemory, TxBufferPool, TxCompleted, TxFrame,
    TxMpdu, TxOutcome, TxProtection, TxSource, VifRole, WidthSet, WithdrawCause,
};
use oer_ieee80211_port::{
    AckSnr, AggregateEnd, AggregateOutcome, Air, BlockAckSnr, SingleEnd, SingleOutcome,
    SingleResponse, SnrSample,
};
use oer_ieee80211_port::{
    AggregateRefused, AggregateState, AmpduCapabilities, AmpduPhase, AmpduRate, BlockAckReport,
    ClassSubframe, EmptyAggregate, InterfaceProtection, NonEmptyAggregate, PhaseAggregate,
    SubframeSource, Tid,
};
use oer_ieee80211_wire::address::IndividualAddress;
use oer_ieee80211_wire::qos::WmmAccessCategory;
use oer_ieee80211_wire::sequence::SequenceNumber;

/// The longest MPDU the model's large buffers hold.
pub const MODEL_MAX_MPDU: usize = 2304;

/// What the model serves.
pub const MODEL_CAPABILITIES: LowerMacCapabilities = LowerMacCapabilities {
    bands: BandSet::GHZ2_4.union(BandSet::GHZ5),
    widths: WidthSet::MHZ20.union(WidthSet::MHZ40),
    rates: RateSupport {
        dsss_cck: true,
        ofdm: true,
        ht_max_mcs: HtMcs::new(7),
        he_max_mcs: HeMcs::new(9),
        he_max_bandwidth_mhz: 20,
        he_dcm: true,
        he_ldpc: true,
        spatial_streams: 1,
    },
    services: HardwareServices::FCS
        .union(HardwareServices::IMMEDIATE_ACK)
        .union(HardwareServices::CIPHER_TRANSFORM)
        .union(HardwareServices::RX_BLOCK_ACK_MATCHING),
    vifs: 2,
    tx_queues: 4,
    max_mpdu_length: MODEL_MAX_MPDU as u16,
    tx_power_ceiling_min_dbm: Some(0),
    individual_no_ack: PhyFormatSet::NON_HT,
    station_receive_filters: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
    access_point_receive_filters: ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::PROBE_REQUESTS),
    rx_block_ack_max_tid: 7,
    rx_block_ack_max_window: 64,
    edca_max_aifsn: 15,
    edca_max_cw_exponent: 10,
    edca_max_txop_limit_units_32_us: u16::MAX,
};

/// The model's capabilities marker.
pub struct ModelCaps;

impl Ieee80211Capabilities for ModelCaps {
    type SppScope = oer_ieee80211_wire::security::spp::Interface;
    const CAPS: LowerMacCapabilities = MODEL_CAPABILITIES;
}

/// The model's capabilities without its 5 GHz band, for host tests of
/// station rate selection constrained to a single band.
pub struct ModelCaps24;

impl Ieee80211Capabilities for ModelCaps24 {
    type SppScope = oer_ieee80211_wire::security::spp::Interface;
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        ..MODEL_CAPABILITIES
    };
}

/// The model's transmit tickets.
pub const MODEL_TICKETS: usize = 8;
/// The model's small transmit buffers.
pub const MODEL_SMALL_BUFFERS: usize = 4;
/// The model's large transmit buffers.
pub const MODEL_LARGE_BUFFERS: usize = 4;
/// The received frames the event queue holds beside the terminals.
pub const MODEL_RX_EVENTS: usize = 4;
/// The key slots of each kind of each role.
pub const MODEL_KEY_SLOTS: usize = 2;
/// The receive Block Ack slots both roles share.
pub const MODEL_BLOCK_ACK_SLOTS: usize = 2;
/// The pool of receive Block Ack slots: the one source of their capacity.
pub type ModelBlockAckPool = BlockAckSlots<ModelBlockAcks, MODEL_BLOCK_ACK_SLOTS>;
/// One receive Block Ack slot of the model.
pub type ModelBlockAckSlot<'id> =
    BlockAckSlot<'id, ModelBlockAcks, MODEL_BLOCK_ACK_SLOTS, ModelPool<7>>;
/// The key pool of one role's association domain.
pub type ModelAssociationPool<R> = AssociationPool<R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS>;

/// FIFO commands: each ticket, install/remove for every pairwise slot,
/// one GTK mutation per interface permission, and close/open of the gate.
/// Configure and TSF each reserve their own FIFO cell and terminal.
/// Receive agreements remain coalesced settings.
pub const MODEL_COMMANDS: usize = MODEL_TICKETS
    + 2 * (2 * MODEL_KEY_SLOTS)
    + 2
    + 2
    + InterfaceCommandPool::<Station>::COMMAND_CAPACITY
    + InterfaceCommandPool::<AccessPoint>::COMMAND_CAPACITY;
/// Reserved terminals: tickets, pairwise removals, the two GTK permissions,
/// gate closing, two configuration confirmations and the bounded RX owners.
pub const MODEL_TERMINALS: usize = MODEL_TICKETS
    + 2 * MODEL_KEY_SLOTS
    + 2
    + 1
    + InterfaceCommandPool::<Station>::TERMINAL_CAPACITY
    + InterfaceCommandPool::<AccessPoint>::TERMINAL_CAPACITY;
/// The event queue additionally reserves the bounded RX owners.
pub const MODEL_EVENTS: usize = MODEL_TERMINALS + MODEL_RX_EVENTS;

/// A queue of the contract's capacity: an overflow is a broken promise and
/// poisons the install while retaining every owner.
struct Bounded<T> {
    items: RefCell<VecDeque<T>>,
    capacity: usize,
    name: &'static str,
}

struct QueueCapacityViolation {
    queue: &'static str,
    capacity: usize,
}
impl core::fmt::Debug for QueueCapacityViolation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("QueueCapacityViolation")
            .field("queue", &self.queue)
            .field("capacity", &self.capacity)
            .finish()
    }
}
impl<T> Bounded<T> {
    fn new(capacity: usize, name: &'static str) -> Self {
        Self {
            items: RefCell::new(VecDeque::with_capacity(capacity)),
            capacity,
            name,
        }
    }

    fn push(&self, item: T) -> Result<(), QueueCapacityViolation> {
        let mut items = self.items.borrow_mut();
        let failure = (items.len() >= self.capacity).then_some(QueueCapacityViolation {
            queue: self.name,
            capacity: self.capacity,
        });
        // On a backend accounting violation retain every owner in this same
        // queue for fail-stop. No terminal or command is dropped or delivered
        // after the caller latches poisoning. Healthy queues stay bounded by
        // their tickets/slots; Rust cannot connect a VecDeque length to them.
        items.push_back(item);
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn pop(&self) -> Option<T> {
        self.items.borrow_mut().pop_front()
    }

    fn is_empty(&self) -> bool {
        self.items.borrow().is_empty()
    }
}

/// The kind of the model's transmit tickets.
pub enum ModelTickets {}

/// The small buffer class: management frames.
#[derive(Debug)]
pub enum ModelSmall {}

impl TxBufferClass for ModelSmall {
    const CAPACITY: usize = 256;
}

/// The large buffer class: any MPDU.
#[derive(Debug)]
pub enum ModelLarge {}

impl TxBufferClass for ModelLarge {
    const CAPACITY: usize = MODEL_MAX_MPDU;
}

/// Paths of the model's fixed installation layout. They add no runtime state.
pub type ModelPool<const I: u8> = oer_radio_port::Child<oer_radio_port::Root, I>;

/// The distinct key pools belonging to each modeled interface role.
pub trait ModelRole: oer_ieee80211_port::InterfaceRole {
    /// Component containing the role's pairwise slots.
    type AssociationPath: oer_radio_port::PoolPath;
    /// Component containing the unique GTK mutation permission.
    type GroupPermissionPath: oer_radio_port::PoolPath;
}
impl ModelRole for Station {
    type AssociationPath = ModelPool<5>;
    type GroupPermissionPath = ModelPool<8>;
}
impl ModelRole for AccessPoint {
    type AssociationPath = ModelPool<6>;
    type GroupPermissionPath = oer_radio_port::Child<ModelPool<9>, 0>;
}

/// The kind of the one GTK mutation permission per interface.
pub struct ModelGroupPermission<R>(PhantomData<fn() -> R>);
/// The kind of the receive Block Ack slots.
pub enum ModelBlockAcks {}

/// A transmit buffer of the class `C`: its token and its memory, which
/// moves with it; only the install mints one.
pub struct ModelBuffer<'id, C: ModelClass> {
    slot: Slot<'id, C, C::Path>,
    memory: ModelMemory,
}

enum ModelMemory {
    Small(Box<[u8; ModelSmall::CAPACITY]>),
    Large(Box<[u8; ModelLarge::CAPACITY]>),
}
impl core::ops::Deref for ModelMemory {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Small(bytes) => bytes.as_slice(),
            Self::Large(bytes) => bytes.as_slice(),
        }
    }
}
impl core::ops::DerefMut for ModelMemory {
    fn deref_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Small(bytes) => bytes.as_mut_slice(),
            Self::Large(bytes) => bytes.as_mut_slice(),
        }
    }
}
impl AsRef<[u8]> for ModelMemory {
    fn as_ref(&self) -> &[u8] {
        self
    }
}
/// Retry is bit 11 of Frame Control, bit 3 of its second wire octet.
const RETRY_FLAG: u8 = 1 << 3;

impl ModelMemory {
    fn sequence(&self) -> SequenceNumber {
        let control = match self {
            Self::Small(bytes) => {
                let [
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    a,
                    b,
                    ..,
                ] = bytes.as_ref();
                [*a, *b]
            }
            Self::Large(bytes) => {
                let [
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    _,
                    a,
                    b,
                    ..,
                ] = bytes.as_ref();
                [*a, *b]
            }
        };
        SequenceNumber::from_sequence_control(u16::from_le_bytes(control))
    }
    fn retry(&mut self) {
        match self {
            Self::Small(bytes) => {
                let [_, flags, ..] = bytes.as_mut();
                *flags |= RETRY_FLAG;
            }
            Self::Large(bytes) => {
                let [_, flags, ..] = bytes.as_mut();
                *flags |= RETRY_FLAG;
            }
        }
    }
}

impl<C: ModelClass> TxBufferMemory for ModelBuffer<'_, C> {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.memory
    }
}

impl<C: ModelClass> core::fmt::Debug for ModelBuffer<'_, C> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("ModelBuffer")
            .field(&self.slot.index())
            .finish()
    }
}

/// A returned published MPDU (open or protected). Its checked layout, installed-key identity,
/// and external body remain inseparable from the immutable buffer.
#[derive(Debug)]
pub struct ModelSent<'id, C: ModelClass, D = ModelBody> {
    role: VifRole,
    buffer: ModelBuffer<'id, C>,
    layout: MpduLayout,
    response: ResponsePolicy,
    key: Option<KeyIdentity>,
    body: Option<D>,
    packet_number: Option<u64>,
}

impl<'id, C: ModelClass, D> ModelSent<'id, C, D> {
    /// The original PN for a protected MPDU, otherwise `None`; a caller supplies no unchecked header offset.
    pub const fn packet_number(&self) -> Option<u64> {
        self.packet_number
    }
    /// The retained external body, available read-only until renewal.
    pub fn body(&self) -> Option<&D> {
        self.body.as_ref()
    }
}

/// A transmit buffer of either class, fresh or published.
#[derive(Debug)]
pub enum ModelAnyBuffer<'id, D = ModelBody> {
    /// Fresh small memory.
    Small(ModelBuffer<'id, ModelSmall>),
    /// Fresh large memory.
    Large(ModelBuffer<'id, ModelLarge>),
    /// The original small published MPDU, including its body.
    SmallSent(ModelSent<'id, ModelSmall, D>),
    /// The original large published MPDU, including its body.
    LargeSent(ModelSent<'id, ModelLarge, D>),
}

impl<D> ModelAnyBuffer<'_, D> {
    fn sequence(&self) -> SequenceNumber {
        match self {
            Self::Small(buffer) => buffer.memory.sequence(),
            Self::Large(buffer) => buffer.memory.sequence(),
            Self::SmallSent(mpdu) => mpdu.buffer.memory.sequence(),
            Self::LargeSent(mpdu) => mpdu.buffer.memory.sequence(),
        }
    }
    fn retry(&mut self) {
        match self {
            Self::Small(buffer) => buffer.memory.retry(),
            Self::Large(buffer) => buffer.memory.retry(),
            Self::SmallSent(mpdu) => mpdu.buffer.memory.retry(),
            Self::LargeSent(mpdu) => mpdu.buffer.memory.retry(),
        }
    }

    fn memory(&self) -> &[u8] {
        match self {
            Self::Small(buffer) => &buffer.memory,
            Self::Large(buffer) => &buffer.memory,
            Self::SmallSent(mpdu) => &mpdu.buffer.memory,
            Self::LargeSent(mpdu) => &mpdu.buffer.memory,
        }
    }
    fn packet_number(&self) -> Option<u64> {
        match self {
            Self::SmallSent(mpdu) => mpdu.packet_number,
            Self::LargeSent(mpdu) => mpdu.packet_number,
            Self::Small(_) | Self::Large(_) => None,
        }
    }
}

/// Identity of an installation of a key, not its reusable slot index.
/// Rc is the host model's allocation identity; only the driver allocates PN.
/// Cloning an identity never retains secret material past removal or teardown.
#[derive(Clone, Debug)]
pub struct KeyIdentity(Rc<KeyInstall>);

#[derive(Debug)]
struct KeyInstall {
    index: u8,
    key_id: u8,
    active: Cell<bool>,
    group_role: Option<VifRole>,
    role: VifRole,
    peer: Option<IndividualAddress>,
    rx_active: Cell<bool>,
    preparations: Rc<()>,
    air: Weak<ModelAir>,
}

#[derive(Debug)]
struct KeyInstallation {
    identity: Weak<KeyInstall>,
    key: Ccmp128Key,
}

impl PartialEq for KeyIdentity {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for KeyIdentity {}

impl KeyIdentity {
    fn new(
        index: u8,
        key_id: u8,
        role: VifRole,
        peer: IndividualAddress,
        air: &Rc<ModelAir>,
    ) -> Self {
        Self(Rc::new(KeyInstall {
            index,
            key_id,
            active: Cell::new(true),
            group_role: None,
            role,
            peer: Some(peer),
            rx_active: Cell::new(false),
            preparations: Rc::new(()),
            air: Rc::downgrade(air),
        }))
    }
    fn group(
        index: u8,
        key_id: GroupKeyId,
        role: VifRole,
        key: Ccmp128Key,
        air: &Rc<ModelAir>,
    ) -> Self {
        let identity = Self(Rc::new(KeyInstall {
            index,
            key_id: key_id.get(),
            active: Cell::new(true),
            group_role: Some(role),
            role,
            peer: None,
            rx_active: Cell::new(false),
            preparations: Rc::new(()),
            air: Rc::downgrade(air),
        }));
        air.install_material(&identity, key);
        identity
    }
}

/// A preparation owns a use of this installed key until cancellation or its
/// terminal. Sent MPDUs retain identity without retaining a preparation use.
/// The model uses Rc ownership counts, not a parallel operation lookup table.
struct PreparedKey {
    identity: KeyIdentity,
    _lease: Rc<()>,
}
impl PreparedKey {
    fn new(identity: KeyIdentity) -> Self {
        let lease = identity.0.preparations.clone();
        Self {
            identity,
            _lease: lease,
        }
    }
}
impl Drop for PreparedKey {
    fn drop(&mut self) {
        // A pending removal may now finish, including cancellation of an
        // externally held preparation, which enqueues no command of its own.
        if let Some(air) = self.identity.0.air.upgrade() {
            air.wake_driver();
        }
    }
}

/// One closed MPDU, retaining its memory, external body and publication state.
pub struct ModelMpdu<'id, D = ModelBody> {
    buffer: ModelAnyBuffer<'id, D>,
    layout: MpduLayout,
    response: ResponsePolicy,
    // Fresh memory keeps its body here, including during publication. Only
    // consuming the terminal moves both into the returned Sent owner.
    body: Option<D>,
    published: Option<PublishedMpdu>,
    packet_number: Option<oer_ieee80211_wire::ccmp::CcmpPacketNumber>,
}

struct PublishedMpdu {
    role: VifRole,
    key: Option<KeyIdentity>,
    packet_number: Option<u64>,
}

/// Execution access cannot replace or take the MPDU owner.
pub struct ModelMpduExecution<'a, 'id, D = ModelBody> {
    mpdu: &'a mut ModelMpdu<'id, D>,
}
impl<'id, D> ExecutionPayload for ModelMpdu<'id, D> {
    type Execution<'a>
        = ModelMpduExecution<'a, 'id, D>
    where
        Self: 'a;
    fn execution(&mut self) -> Self::Execution<'_> {
        ModelMpduExecution { mpdu: self }
    }
}
impl<D> ModelMpduExecution<'_, '_, D> {
    fn publish(&mut self, role: VifRole, key: Option<&KeyIdentity>) -> Result<(), ModelFault> {
        let mpdu = &mut *self.mpdu;
        mpdu.preflight(key.is_some())?;
        if mpdu.published.is_some() {
            return Ok(());
        }
        let memory = match &mut mpdu.buffer {
            ModelAnyBuffer::Small(buffer) => buffer.bytes_mut(),
            ModelAnyBuffer::Large(buffer) => buffer.bytes_mut(),
            ModelAnyBuffer::SmallSent(_) | ModelAnyBuffer::LargeSent(_) => return Ok(()),
        };
        let packet_number = match key {
            Some(key) => {
                let tail = memory
                    .split_at_mut_checked(mpdu.layout.header_len())
                    .ok_or(ModelFault)?
                    .1;
                let (header, _) = tail.split_first_chunk_mut::<8>().ok_or(ModelFault)?;
                let pn = mpdu.packet_number.ok_or(ModelFault)?.value();
                *header = ccmp_header(pn, key.0.key_id);
                Some(pn)
            }
            None => None,
        };
        mpdu.published = Some(PublishedMpdu {
            role,
            key: key.cloned(),
            packet_number,
        });
        Ok(())
    }
    fn retry(&mut self) {
        self.mpdu.buffer.retry();
    }
}
impl<'id, D> TxMpdu for ModelMpdu<'id, D> {
    type Buffer = ModelAnyBuffer<'id, D>;
    type Body = D;
    fn into_parts(self) -> (Self::Buffer, Option<Self::Body>) {
        let Self {
            buffer,
            layout,
            response,
            body,
            published,
            packet_number: _,
        } = self;
        match (buffer, published) {
            (
                ModelAnyBuffer::Small(buffer),
                Some(PublishedMpdu {
                    role,
                    key,
                    packet_number,
                }),
            ) => (
                ModelAnyBuffer::SmallSent(ModelSent {
                    role,
                    buffer,
                    layout,
                    response,
                    key,
                    body,
                    packet_number,
                }),
                None,
            ),
            (
                ModelAnyBuffer::Large(buffer),
                Some(PublishedMpdu {
                    role,
                    key,
                    packet_number,
                }),
            ) => (
                ModelAnyBuffer::LargeSent(ModelSent {
                    role,
                    buffer,
                    layout,
                    response,
                    key,
                    body,
                    packet_number,
                }),
                None,
            ),
            (buffer, _) => (buffer, body),
        }
    }
}
impl<'id, D> ModelMpdu<'id, D> {
    /// Validate geometry before publication; fresh PNs were allocated by the caller.
    fn preflight(&self, protected: bool) -> Result<(), ModelFault> {
        let memory = self.buffer.memory();
        memory
            .split_at_checked(self.layout.mpdu_len())
            .ok_or(ModelFault)?;
        let fresh = self.published.is_none()
            && matches!(
                self.buffer,
                ModelAnyBuffer::Small(_) | ModelAnyBuffer::Large(_)
            );
        if protected && fresh {
            memory
                .split_at_checked(self.layout.header_len())
                .ok_or(ModelFault)?
                .1
                .first_chunk::<8>()
                .ok_or(ModelFault)?;
        }
        Ok(())
    }

    fn from_sent(
        mut buffer: ModelAnyBuffer<'id, D>,
        layout: MpduLayout,
        response: ResponsePolicy,
    ) -> Self {
        buffer.retry();
        Self {
            buffer,
            layout,
            response,
            body: None,
            published: None,
            packet_number: None,
        }
    }
    fn on_air(
        &self,
        role: VifRole,
        keys: &[KeyIdentity],
        key: Option<&KeyIdentity>,
        rate: PhyRate,
    ) -> Result<AirFrame, ModelFault> {
        let bytes = self
            .buffer
            .memory()
            .split_at_checked(self.layout.mpdu_len())
            .ok_or(ModelFault)?
            .0;
        let mut wire: Vec<u8> = bytes.into();
        if let Some(identity) = key {
            let air = identity.0.air.upgrade().ok_or(ModelFault)?;
            let installations = air.key_installations.borrow();
            let material = installations
                .iter()
                .find(|entry| entry.identity.ptr_eq(&Rc::downgrade(&identity.0)))
                .ok_or(ModelFault)?;
            wire.extend_from_slice(&[0; 8]);
            air.associations
                .borrow()
                .get(identity.0.role)
                .seal(material.key.as_bytes(), &mut wire)
                .map_err(|_| ModelFault)?;
        }
        Ok(AirFrame {
            role,
            plaintext: bytes.into(),
            frame: wire,
            rate,
            key: key.map(|key| (key.0.index, keys.contains(key))),
            packet_number: self
                .published
                .as_ref()
                .and_then(|state| state.packet_number)
                .or_else(|| self.buffer.packet_number()),
        })
    }
}

/// The CCMP header of the packet number `number` under the key `key_id`.
fn ccmp_header(number: u64, key_id: u8) -> [u8; 8] {
    let pn = number.to_le_bytes();
    [
        pn[0],
        pn[1],
        0,
        0x20 | (key_id << 6),
        pn[2],
        pn[3],
        pn[4],
        pn[5],
    ]
}

/// Composition-selected contexts, including the install brand when needed.
/// The model neither interprets nor clones the associated value. Its lifetime
/// is the install, not the phase loan. A machine created inside the generic
/// body cannot be borrowed for that lifetime:
///
/// ```compile_fail,E0597
/// use oer_ieee80211_port::PhaseTypes;
/// use oer_radio_port::{Disabled, Ended, Owner, PortBody};
/// use oer_ieee80211_model::{
///     ModelBackend, ModelCaps, ModelContext, ModelResources, ModelTypes,
/// };
/// struct Machine;
/// struct Contexts;
/// impl ModelContext for Contexts { type Body = core::convert::Infallible; type Context<'id> = &'id Machine; type StationConfigureContext<'id> = (); type AccessPointConfigureContext<'id> = (); }
/// type Backend = ModelBackend<ModelCaps, Contexts>;
/// struct Body;
/// impl PortBody<Backend> for Body {
///     type Output = ();
///     async fn run<'id>(self, owner: Owner<'_, 'id, Backend, Disabled>,
///         _: ModelResources<'id>) -> Ended<'id, Backend, ()> {
///         let machine = Machine;
///         let context: <ModelTypes<Contexts> as PhaseTypes<'id>>::Context = &machine;
///         core::hint::black_box(context);
///         Ended::uninstall(owner, (), ())
///     }
/// }
/// ```
///
/// A context can instead own an install-branded value. This example uses
/// a local epoch brand; runtime consumers can choose their core's epoch type.
/// The `Rc` also demonstrates that the portable contract imposes no `Send`:
///
/// ```
/// use std::rc::Rc;
/// use oer_ieee80211_port::PhaseTypes;
/// use oer_radio_port::{Disabled, Ended, Owner, PortBody};
/// use core::marker::PhantomData;
/// struct LinkEpoch<'id>(PhantomData<fn(&'id ()) -> &'id ()>);
/// use oer_ieee80211_model::{
///     ModelBackend, ModelCaps, ModelContext, ModelResources, ModelTypes,
/// };
/// struct Context<'id> { epoch: Option<LinkEpoch<'id>>, owner: Rc<()> }
/// struct Contexts;
/// impl ModelContext for Contexts { type Body = core::convert::Infallible; type Context<'id> = Context<'id>; type StationConfigureContext<'id> = (); type AccessPointConfigureContext<'id> = (); }
/// type Backend = ModelBackend<ModelCaps, Contexts>;
/// struct Body;
/// impl PortBody<Backend> for Body {
///     type Output = ();
///     async fn run<'id>(self, owner: Owner<'_, 'id, Backend, Disabled>,
///         _: ModelResources<'id>) -> Ended<'id, Backend, ()> {
///         let context: <ModelTypes<Contexts> as PhaseTypes<'id>>::Context =
///             Context { epoch: None, owner: Rc::new(()) };
///         core::hint::black_box(context);
///         Ended::uninstall(owner, (), ())
///     }
/// }
/// ```
pub trait ModelContext {
    /// External body owners selected by the composition; `Infallible` excludes them.
    type Body: TxBody + core::fmt::Debug;
    /// One operation's opaque context in the install `'id`.
    type Context<'id>: 'id;
    /// Opaque context returned with the configuration permission.
    type StationConfigureContext<'id>: 'id;
    /// Opaque context returned with an AP configuration permission.
    type AccessPointConfigureContext<'id>: 'id;
}

impl ModelContext for () {
    type Body = ModelBody;
    type Context<'id> = ();
    type StationConfigureContext<'id> = ();
    type AccessPointConfigureContext<'id> = ();
}

/// The types the model's events carry.
pub struct ModelTypes<X: ModelContext = ()>(PhantomData<fn() -> X>);

impl<'id, X: ModelContext> PhaseTypes<'id> for ModelTypes<X> {
    const COMMAND_CAPACITY: usize = MODEL_COMMANDS;
    type StationCommandPath = oer_radio_port::Child<ModelPool<9>, 1>;
    type AccessPointCommandPath = oer_radio_port::Child<ModelPool<9>, 2>;
    type StationConfigureContext = X::StationConfigureContext<'id>;
    type AccessPointConfigureContext = X::AccessPointConfigureContext<'id>;
    const TERMINAL_CAPACITY: usize = MODEL_TERMINALS;
    type EpochPath = ModelPool<4>;
    type GatePath = ModelPool<0>;
    type TicketPath = ModelPool<1>;
    type BlockAckPath = ModelPool<7>;
    type Fault = ModelFault;
    type Context = X::Context<'id>;
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities> = ModelPrepared<'k, 'p, 'id, R, K, X>;
    type KeyIdentity = KeyIdentity;
    type Aggregate<'k, 'p, R, K, S>
        = ModelAggregate<'k, 'p, 'id, R, K, X, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k;
    type TxBody = X::Body;
    type TicketKind = ModelTickets;
    type AckEvidence = ModelAck;
    type BlockAckEvidence = ModelBlockAckEvidence;
    type AirDenied = ModelAirDenied;
    type AnyTxBuffer = ModelAnyBuffer<'id, X::Body>;
    type TxMpdu = ModelMpdu<'id, X::Body>;
    type TxAggregate = ModelSubframes<'id, X::Body>;
    type RxBuffer = ModelRxBuffer;
    type RxMeta = ModelRxMeta;
    type StationKeys = ModelKeys<'id, Station>;
    type AccessPointKeys = ModelKeys<'id, AccessPoint>;
    type BlockAckKind = ModelBlockAcks;
    type BlockAckPool = ModelBlockAckPool;
    type BlockAck = ModelBlockAck<'id>;
}

/// The unique protocol owner selecting an interface's immutable association.
/// Installed keys retain only their interface identity; the modeled hardware
/// holds exactly one CCMP record at that interface's index.
pub struct ModelAssociation<'id, R: ModelRole> {
    owner: Association<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>,
}

/// The keys of the role `R` and their slots.
pub struct ModelKeys<'id, R>(PhantomData<fn(&'id ()) -> R>);

impl<'id, R: ModelRole + 'id> KeyTypes<'id> for ModelKeys<'id, R> {
    type Pool = ModelAssociationPool<R>;
    type AssociationStart =
        AssociationStart<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>;
    type Association = ModelAssociation<'id, R>;
    type VacantKeyBank =
        VacantKeyBank<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>;
    type VacantAssociation =
        VacantAssociation<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>;
    fn begin_association(
        start: Self::AssociationStart,
        spp: oer_ieee80211_wire::security::Spp,
    ) -> Self::Association {
        ModelAssociation {
            owner: start.begin(spp),
        }
    }
    fn collect_key_bank<const N: usize>(
        parts: KeyBankParts<Self::PairwiseSlot, Self::GroupSlot, N>,
    ) -> Result<Self::VacantKeyBank, KeyBankParts<Self::PairwiseSlot, Self::GroupSlot, N>> {
        VacantKeyBank::collect(parts)
    }
    fn end_association(
        association: Self::Association,
        vacant: Self::VacantKeyBank,
    ) -> Self::VacantAssociation {
        association::end_association(association.owner, vacant)
    }
    fn split_vacant<const N: usize>(
        vacant: Self::VacantAssociation,
    ) -> (
        Self::AssociationStart,
        Self::PairwiseSlot,
        KeyBankParts<Self::PairwiseSlot, Self::GroupSlot, N>,
    ) {
        let (start, bank) = vacant.into_parts();
        let ([first, spare], group) = bank.into_parts();
        (
            start,
            first,
            KeyBankParts {
                pairwise: heapless::Vec::from_array([spare]),
                group: heapless::Vec::from_array(group),
            },
        )
    }

    type PairwiseSlot =
        PairwiseKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>;
    type GroupSlot = GroupKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>;
    type PairwiseKey = ModelPairwiseKey<'id, R>;
    type GroupKey = ModelGroupKey<'id, R>;
    type GroupPermission = Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath>;
    type Identity = KeyIdentity;
    type GroupSlotIdentity = u8;
    fn pairwise_identity(key: &Self::PairwiseKey) -> KeyIdentity {
        key.identity.clone()
    }
    fn group_identity(key: &Self::GroupKey) -> KeyIdentity {
        key.identity.clone()
    }
    fn group_slot_identity(key: &Self::GroupKey) -> u8 {
        key.slot.index()
    }
    fn vacant_group_slot_identity(slot: &Self::GroupSlot) -> u8 {
        slot.index()
    }
}

impl<'id, X: ModelContext, C: ModelClass> TxBufferPool<'id, C> for ModelTypes<X> {
    type Buffer = ModelBuffer<'id, C>;
    type Sent = ModelSent<'id, C, X::Body>;

    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: CheckedPoolSource<'k, 'p, 'id, Self, C, K, R>,
    ) -> oer_ieee80211_port::PoolPreparation<'k, 'p, 'id, Self, C, K, R>
    where
        'id: 'k,
    {
        prepare_model(source)
    }

    fn push_aggregate<'k, 'p, R, K, S>(
        operation: oer_ieee80211_port::CheckedPoolAggregate<'k, 'p, 'id, Self, C, R, K, S>,
    ) -> oer_ieee80211_port::AggregatePushResult<'k, 'p, 'id, Self, C, R, K, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k + 'p,
    {
        push_model(operation)
    }

    fn erase(buffer: Self::Buffer) -> ModelAnyBuffer<'id, X::Body> {
        C::erase(buffer)
    }

    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent(buffer: Self::Sent) -> ModelAnyBuffer<'id, X::Body> {
        C::erase_sent(buffer)
    }

    fn class(
        buffer: ModelAnyBuffer<'id, X::Body>,
    ) -> Result<Self::Buffer, ModelAnyBuffer<'id, X::Body>> {
        C::class(buffer)
    }

    fn sent(
        buffer: ModelAnyBuffer<'id, X::Body>,
    ) -> Result<Self::Sent, ModelAnyBuffer<'id, X::Body>> {
        C::sent(buffer)
    }

    fn renew(mpdu: Self::Sent) -> (Self::Buffer, Option<X::Body>) {
        (mpdu.buffer, mpdu.body)
    }
}

/// What the model's radio observed of a received frame: the portable
/// fields and, always, its signal and the noise floor, so the model is a
/// port whose metadata measures the signal ([`SignalMetadata`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelRxMeta {
    common: RxMeta,
    rssi_dbm: i8,
    noise_floor_dbm: i8,
}

impl ModelRxMeta {
    /// A frame received on `channel` at `rssi_dbm` over `noise_floor_dbm`.
    pub const fn new(channel: Channel, rssi_dbm: i8, noise_floor_dbm: i8) -> Self {
        let mut common = RxMeta::unavailable(channel);
        common.rssi_dbm = RxEvidence::HardwareObserved(rssi_dbm);
        common.noise_floor_dbm = RxEvidence::HardwareObserved(noise_floor_dbm);
        Self {
            common,
            rssi_dbm,
            noise_floor_dbm,
        }
    }

    /// The same frame, with what the hardware observed of its decryption.
    pub const fn with_crypto(
        mut self,
        crypto: RxEvidence<oer_ieee80211_port::RxCryptoStatus>,
    ) -> Self {
        self.common.crypto = crypto;
        self
    }
}

impl RxMetadata for ModelRxMeta {
    fn common(&self) -> &RxMeta {
        &self.common
    }
}

impl SignalMetadata for ModelRxMeta {
    fn link_metric(&self) -> LinkMetric {
        LinkMetric::new(self.rssi_dbm, self.noise_floor_dbm)
    }
}

/// The memory of one received MPDU, which goes back through
/// [`LowerMacPhase::recycle`]: dropping it instead poisons the backend.
#[derive(Debug)]
pub struct ModelRxBuffer {
    bytes: Vec<u8>,
    recycled: bool,
    air: Weak<ModelAir>,
    streamed: bool,
}

impl RxBuffer for ModelRxBuffer {
    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for ModelRxBuffer {
    fn drop(&mut self) {
        if !self.recycled
            && let Some(air) = self.air.upgrade()
        {
            // A dropped affine receive owner cannot be recycled or admitted
            // again. Signal the existing backend fail-stop, never a panic.
            air.poison();
        }
    }
}

/// The owner of an MPDU's body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelBody(pub Vec<u8>);

impl TxBody for ModelBody {
    fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// What poisons the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelFault;

/// The tokens the install gives the body once.
pub struct ModelResources<'id> {
    /// Station's unique association-mode selection permission.
    pub station_association:
        AssociationStart<'id, Station, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<5>>,
    /// AP's interface-wide association-mode selection permission.
    pub access_point_association:
        AssociationStart<'id, AccessPoint, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<6>>,
    /// Station command admission from its own installation component.
    pub station_commands:
        InterfacePermissions<'id, Station, oer_radio_port::Child<ModelPool<9>, 1>>,
    /// Access-point command admission from its own installation component.
    pub access_point_commands:
        InterfacePermissions<'id, AccessPoint, oer_radio_port::Child<ModelPool<9>, 2>>,
    /// The install's single epoch-source slot; Station constructs its source.
    pub connection_epoch: Slot<'id, ConnectionEpochKind, ModelPool<4>>,
    /// The transmit tickets.
    pub tickets: Vec<Slot<'id, ModelTickets, ModelPool<1>>>,
    /// Sole permission for the body to unpack TX terminals.
    pub terminal_access: oer_ieee80211_port::TerminalAccess<'id, ModelTickets, ModelPool<1>>,
    /// The small transmit buffers.
    pub small: Vec<ModelBuffer<'id, ModelSmall>>,
    /// The large transmit buffers.
    pub large: Vec<ModelBuffer<'id, ModelLarge>>,
    /// The station's pairwise key slots.
    pub station_pairwise:
        [PairwiseKeySlot<'id, Station, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<5>>;
            MODEL_KEY_SLOTS],
    /// The station's group key slots.
    pub station_group: [GroupKeySlot<'id, Station, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<5>>;
        MODEL_KEY_SLOTS],
    /// The access point's pairwise key slots.
    pub access_point_pairwise:
        [PairwiseKeySlot<'id, AccessPoint, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<6>>;
            MODEL_KEY_SLOTS],
    /// The access point's group key slots.
    pub access_point_group:
        [GroupKeySlot<'id, AccessPoint, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, ModelPool<6>>;
            MODEL_KEY_SLOTS],
    /// The station's sole GTK mutation permission.
    pub station_group_permission: Slot<'id, ModelGroupPermission<Station>, ModelPool<8>>,
    /// The access point's sole GTK mutation permission.
    pub access_point_group_permission:
        Slot<'id, ModelGroupPermission<AccessPoint>, oer_radio_port::Child<ModelPool<9>, 0>>,
    /// The receive Block Ack slots both roles share.
    pub block_acks: [ModelBlockAckSlot<'id>; MODEL_BLOCK_ACK_SLOTS],
    /// The sole authority to construct this installation's arbiter.
    pub arbiter_authority: oer_ieee80211_port::ArbiterAuthority<'id, ModelPool<0>>,
    /// The open transmit gate.
    pub gate: GateOpen<'id, ModelPool<0>>,
}

/// One transmission the air saw.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AirFrame {
    /// The interface that sent it.
    pub role: VifRole,
    /// The MPDU, header and body, without the FCS.
    pub frame: Vec<u8>,
    /// The DMA input before modeled hardware encryption, without a hardware MIC.
    pub plaintext: Vec<u8>,
    /// Its rate.
    pub rate: PhyRate,
    /// The hardware key it named, and whether that key was installed when
    /// it went.
    pub key: Option<(u8, bool)>,
    /// The packet number of its CCMP header, under a key.
    pub packet_number: Option<u64>,
}

/// One FIFO injection for the next PPDU. ACK and Block Ack must match its
/// shape; a mismatch poisons the model before publication, retaining owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirAnswer {
    /// An ACK, which the model measures at [`MODEL_ACK_SNR_DB`].
    Ack,
    /// A measured BlockAck. Bit n acknowledges subframe n of this one
    /// aggregate.
    BlockAck(u64),
    /// An aggregate's BlockAck whose completion carries no SNR measurement,
    /// as an S31 BlockAck accepted with a failed hardware status does.
    BlockAckUnmeasured(u64),
    NoResponse,
    Collision,
    /// Coexistence denied the air before the PPDU went out.
    AirDenied,
    HardwareTimeout,
}

/// The SNR the model measures for every ACK and measured BlockAck.
pub const MODEL_ACK_SNR_DB: i8 = 30;

/// What the model observed of a single MPDU's response: its SNR, always.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelAck {
    /// The response's SNR, in dB.
    pub snr_db: i8,
}

impl AckSnr for ModelAck {
    fn snr_db(&self) -> i8 {
        self.snr_db
    }
}

/// What the model observed of an aggregate's BlockAck.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelBlockAckEvidence {
    /// The BlockAck's SNR, in dB.
    Measured(i8),
    /// The completion carried no SNR measurement.
    Unmeasured,
}

impl BlockAckSnr for ModelBlockAckEvidence {
    fn snr_sample(&self) -> SnrSample {
        match *self {
            Self::Measured(snr_db) => SnrSample::Measured(snr_db),
            Self::Unmeasured => SnrSample::Unmeasured,
        }
    }
}

/// The model's evidence that coexistence denied an attempt the air.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelAirDenied;

/// A single MPDU's outcome in the model.
pub type ModelSingleOutcome = SingleOutcome<ModelAck, ModelAirDenied>;
/// An aggregate's outcome in the model.
pub type ModelAggregateOutcome = AggregateOutcome<ModelBlockAckEvidence, ModelAirDenied>;

/// One receive agreement applied by the model driver, after capability checking.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelReceiveAgreement {
    /// The receiving interface.
    pub role: VifRole,
    /// The originator.
    pub peer: IndividualAddress,
    /// The receive sequence space.
    pub tid: Tid,
    /// The window's start.
    pub start_sequence: SequenceNumber,
    /// The receive window in MPDUs.
    pub window: u16,
}

impl ModelReceiveAgreement {
    fn checked<K: Ieee80211Capabilities>(role: VifRole, agreement: SupportedBlockAck<K>) -> Self {
        let (peer, tid, start_sequence, window) = agreement.get();
        Self {
            role,
            peer,
            tid,
            start_sequence,
            window,
        }
    }
}

/// Hardware uncertainty injected at a concrete mutation boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupFaultStage {
    Before,
    After,
}

/// A receive fact deliberately delayed in the modeled ingress pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupRxStage {
    Dma,
    Interrupt,
    Software,
    Ready,
}

struct CapturedGroupRx {
    frame: Vec<u8>,
    meta: ModelRxMeta,
    installation: KeyIdentity,
    stage: GroupRxStage,
}

#[derive(Clone, Copy, Default)]
enum ModelStream {
    #[default]
    Running,
    Draining,
    Ended(EventEnd<ModelFault>),
}

/// One actual AC-queue draw from the VIF of its head frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelContention {
    pub role: VifRole,
    pub access_category: WmmAccessCategory,
    pub parameters: EdcaAcParameters,
    pub window_slots: u16,
    pub backoff_slots: u16,
}

struct ModelEdca {
    parameters: [EdcaParameterSet; 2],
    queues: [EdcaContention; 4],
    heads: [Option<VifRole>; 4],
    entropy: u32,
    draws: Vec<ModelContention>,
}
impl Default for ModelEdca {
    fn default() -> Self {
        Self {
            parameters: [EdcaParameterSet::STANDARD_DEFAULT; 2],
            queues: WmmAccessCategory::ALL.map(|ac| {
                EdcaContention::from_wmm(EdcaParameterSet::STANDARD_DEFAULT.access_category(ac).wmm)
            }),
            heads: [None; 4],
            entropy: 0,
            draws: Vec::new(),
        }
    }
}
impl ModelEdca {
    fn parameters(&self, role: VifRole) -> EdcaParameterSet {
        *match role {
            VifRole::Station => &self.parameters[0],
            VifRole::AccessPoint => &self.parameters[1],
        }
    }
    fn parameters_mut(&mut self, role: VifRole) -> &mut EdcaParameterSet {
        match role {
            VifRole::Station => &mut self.parameters[0],
            VifRole::AccessPoint => &mut self.parameters[1],
        }
    }
    fn draw(&mut self, role: VifRole, access_category: WmmAccessCategory) {
        let parameters = self.parameters(role).access_category(access_category);
        let queue = access_category.select_mut(&mut self.queues);
        queue.reconfigure(parameters.wmm.ecw_min, parameters.wmm.ecw_max);
        let head = access_category.select_mut(&mut self.heads);
        if *head != Some(role) {
            queue.reset();
            *head = Some(role);
        }
        self.draws.push(ModelContention {
            role,
            access_category,
            parameters,
            window_slots: queue.cw(),
            backoff_slots: queue.draw(&mut || self.entropy),
        });
    }
    fn failure(&mut self, ac: WmmAccessCategory) {
        ac.select_mut(&mut self.queues).record_failure();
    }
    fn reset(&mut self, ac: WmmAccessCategory) {
        ac.select_mut(&mut self.queues).reset();
        *ac.select_mut(&mut self.heads) = None;
    }
}

/// The air and the world of a model: shared by the backend's driver and
/// the test.
#[derive(Default)]
pub struct ModelAir {
    sent: RefCell<Vec<AirFrame>>,
    /// The answers of the next transmissions, in order; once none is
    /// queued, each is acknowledged.
    answers: RefCell<VecDeque<AirAnswer>>,
    rx: RefCell<VecDeque<(Vec<u8>, ModelRxMeta)>>,
    rx_stream: RefCell<Option<(Vec<u8>, ModelRxMeta)>>,
    refuse_enable: Cell<bool>,
    group_failure: Cell<Option<GroupFaultStage>>,
    group_held: Cell<bool>,
    // Secret material belongs to this installation table, never to the
    // cloneable identity carried by captured RX or returned Sent owners.
    key_installations: RefCell<Vec<KeyInstallation>>,
    associations: RefCell<InterfaceAssociations>,
    crypto_operations: RefCell<Vec<ModelCryptoOperation>>,
    group_rx: RefCell<VecDeque<CapturedGroupRx>>,
    group_writes: Cell<usize>,
    stream: Cell<ModelStream>,
    disables: Cell<u32>,
    driver: RefCell<Option<Waker>>,
    /// The driver's passes.
    passes: Cell<u32>,
    /// The commands the driver took, of every kind.
    commands: Cell<u32>,
    /// The settings the driver applied.
    settings: Cell<u32>,
    /// The EDCA set each interface's attempts contend with, by role.
    edca: RefCell<ModelEdca>,
    /// The queues the air keeps busy: their attempts wait unpublished.
    held: Cell<[bool; WmmAccessCategory::ALL.len()]>,
    /// Keep a published PPDU's result pending, independently of prepublication CCA.
    held_results: Cell<[bool; WmmAccessCategory::ALL.len()]>,
    now_micros: Cell<u64>,
    receive_agreements: RefCell<BTreeMap<u8, ModelReceiveAgreement>>,
}

/// Mode application precedes publication of each key's material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelCryptoOperation {
    Association {
        role: VifRole,
        spp: oer_ieee80211_wire::security::Spp,
    },
    Key {
        role: VifRole,
        slot: u8,
    },
}

/// A wire MPDU could not pass modeled CCMP reception.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelCcmpError {
    MalformedHeader,
    MissingKey,
    Authentication(oer_ieee80211_cipher::ccmp_mpdu::CcmpMpduError),
}

// ModelCaps declares Interface scope. These are hardware policy records,
// indexed by the same interface identity that keys already carry. The
// association owner is never cloned. A queued install takes its decision
// from that owner and the driver applies it before publishing material.
// Like key-table hardware, vacant interfaces retain their last mode; changing
// it requires a new owner obtained through the complete returned slot bank.
struct InterfaceAssociations {
    station: oer_ieee80211_cipher::ccmp_mpdu::CcmpAssociation,
    access_point: oer_ieee80211_cipher::ccmp_mpdu::CcmpAssociation,
}
impl Default for InterfaceAssociations {
    fn default() -> Self {
        use oer_ieee80211_cipher::ccmp_mpdu::CcmpAssociation;
        use oer_ieee80211_wire::security::Spp;
        Self {
            station: CcmpAssociation::new(Spp::NotNegotiated),
            access_point: CcmpAssociation::new(Spp::NotNegotiated),
        }
    }
}
impl InterfaceAssociations {
    fn get(&self, role: VifRole) -> &oer_ieee80211_cipher::ccmp_mpdu::CcmpAssociation {
        match role {
            VifRole::Station => &self.station,
            VifRole::AccessPoint => &self.access_point,
        }
    }
    fn apply(&mut self, role: VifRole, spp: oer_ieee80211_wire::security::Spp) {
        let record = match role {
            VifRole::Station => &mut self.station,
            VifRole::AccessPoint => &mut self.access_point,
        };
        *record = oer_ieee80211_cipher::ccmp_mpdu::CcmpAssociation::new(spp);
    }
}

impl ModelAir {
    fn install_material(&self, identity: &KeyIdentity, key: Ccmp128Key) {
        self.crypto_operations
            .borrow_mut()
            .push(ModelCryptoOperation::Key {
                role: identity.0.role,
                slot: identity.0.index,
            });
        self.key_installations.borrow_mut().push(KeyInstallation {
            identity: Rc::downgrade(&identity.0),
            key,
        });
    }

    fn apply_association(&self, role: VifRole, spp: oer_ieee80211_wire::security::Spp) {
        self.associations.borrow_mut().apply(role, spp);
        self.crypto_operations
            .borrow_mut()
            .push(ModelCryptoOperation::Association { role, spp });
    }

    /// Programming observations, in the same order as the driver's FIFO.
    pub fn crypto_operations(&self) -> Vec<ModelCryptoOperation> {
        self.crypto_operations.borrow().clone()
    }

    /// Receive actual encrypted data. Only successful MIC verification can
    /// publish decrypted metadata; a changed SPP A-MSDU bit never reaches RX.
    pub fn receive_ccmp(
        &self,
        frame: &[u8],
        meta: ModelRxMeta,
        role: VifRole,
    ) -> Result<(), ModelCcmpError> {
        use oer_ieee80211_wire::{ccmp::CcmpHeader, frame::DecodedHeader};
        let header = DecodedHeader::decode(frame).ok_or(ModelCcmpError::MalformedHeader)?;
        let ccmp = header
            .body()
            .first_chunk::<8>()
            .ok_or(ModelCcmpError::MalformedHeader)?;
        let key_id = CcmpHeader::parse(*ccmp)
            .map_err(|_| ModelCcmpError::MalformedHeader)?
            .key_id()
            .value();
        let group = oer_ieee80211_port::GroupAddress::new(header.receiver()).is_some();
        let installations = self.key_installations.borrow();
        let material = installations
            .iter()
            .find(|entry| {
                entry.identity.upgrade().is_some_and(|key| {
                    key.role == role
                        && key.key_id == key_id
                        && key.active.get()
                        && if group {
                            key.group_role.is_some() && key.rx_active.get()
                        } else {
                            key.peer
                                .is_some_and(|peer| peer.get() == header.transmitter())
                        }
                })
            })
            .ok_or(ModelCcmpError::MissingKey)?;
        let identity = material
            .identity
            .upgrade()
            .ok_or(ModelCcmpError::MissingKey)?;
        let mut plaintext = frame.to_vec();
        self.associations
            .borrow()
            .get(identity.role)
            .open(material.key.as_bytes(), &mut plaintext)
            .map_err(ModelCcmpError::Authentication)?;
        let (plaintext, _mic) = plaintext
            .split_last_chunk::<8>()
            .ok_or(ModelCcmpError::MalformedHeader)?;
        self.receive(
            plaintext,
            meta.with_crypto(RxEvidence::HardwareObserved(
                oer_ieee80211_port::RxCryptoStatus::DecryptedAndIntegrityVerified,
            )),
        );
        Ok(())
    }

    fn remove_key_material(&self, identity: &KeyIdentity) {
        self.key_installations
            .borrow_mut()
            .retain(|entry| !entry.identity.ptr_eq(&Rc::downgrade(&identity.0)));
    }

    fn end_installation(&self) {
        let mut installations = self.key_installations.borrow_mut();
        for entry in installations.iter() {
            if let Some(identity) = entry.identity.upgrade() {
                identity.active.set(false);
                identity.rx_active.set(false);
            }
        }
        self.group_rx.borrow_mut().clear();
        // Ccmp128Key clears its bytes even if an external identity still exists.
        installations.clear();
    }

    /// Inject uncertainty before or after a GTK hardware mutation. No success
    /// owner or ordinary permission leaves the poisoned installation.
    pub fn fail_next_group_change(&self, stage: GroupFaultStage) {
        self.group_failure.set(Some(stage));
        self.wake_driver();
    }

    /// Delay commands while RX continues. `set_time` supplies the deadline
    /// wake; the driver poisons at GROUP_COMMAND_BOUND. Normal disable finishes
    /// admitted commands instead of waiting for this test hold to be released.
    pub fn hold_group_commands(&self, held: bool) {
        self.group_held.set(held);
        self.wake_driver();
    }

    /// Actual material writes, including replacement and excluding selection.
    pub fn group_writes(&self) -> usize {
        self.group_writes.get()
    }

    /// Capture a group frame against the current modeled hardware key table.
    /// Delayed DMA/ISR/software stages retain the original installation inside
    /// the backend; no installation identity is added to public RX metadata.
    pub fn receive_group(
        &self,
        frame: &[u8],
        meta: ModelRxMeta,
        role: VifRole,
        key_id: GroupKeyId,
        material: &[u8; 16],
        stage: GroupRxStage,
    ) -> bool {
        let installation = self
            .key_installations
            .borrow()
            .iter()
            .filter_map(|entry| {
                (entry.key.as_bytes() == material)
                    .then(|| entry.identity.upgrade())
                    .flatten()
            })
            .find(|key| {
                key.active.get()
                    && key.rx_active.get()
                    && key.key_id == key_id.get()
                    && key.group_role == Some(role)
            });
        let Some(installation) = installation else {
            return false;
        };
        self.group_rx.borrow_mut().push_back(CapturedGroupRx {
            frame: frame.into(),
            meta,
            installation: KeyIdentity(installation),
            stage,
        });
        self.wake_driver();
        true
    }

    /// Every publication so far, including collisions and unknown outcomes.
    pub fn sent(&self) -> Vec<AirFrame> {
        self.sent.borrow().clone()
    }

    /// Supply exactly one result for the next publication. A Block Ack cannot
    /// be left behind for a later aggregate when a Single is ahead of it.
    pub fn answer(&self, answer: AirAnswer) {
        self.answers.borrow_mut().push_back(answer);
    }

    /// Applied receive agreements by hardware slot. Pending writes remain
    /// invisible until the driver takes the latest value of each slot.
    pub fn receive_agreements(&self) -> Vec<(u8, ModelReceiveAgreement)> {
        self.receive_agreements
            .borrow()
            .iter()
            .map(|(&slot, &agreement)| (slot, agreement))
            .collect()
    }

    /// Keep reception ready on every service poll, like an RX interrupt
    /// source on another executor. This exercises scheduling against a
    /// continuously ready source, rather than a finite queued burst.
    pub fn receive_continuously(&self, frame: &[u8], meta: ModelRxMeta) {
        self.rx_stream.borrow_mut().replace((frame.into(), meta));
        self.wake_driver();
    }

    /// Stop generating RX events. Already returned buffers still require
    /// recycling through the phase.
    pub fn stop_continuous_receive(&self) {
        self.rx_stream.borrow_mut().take();
    }

    /// Deliver `frame` to the radio.
    pub fn receive(&self, frame: &[u8], meta: ModelRxMeta) {
        self.rx.borrow_mut().push_back((frame.into(), meta));
        self.wake_driver();
    }

    /// Keep the queue of `access_category` busy, or free it: its attempts
    /// wait unpublished meanwhile, as behind a long transmission.
    pub fn hold(&self, access_category: oer_ieee80211_wire::qos::WmmAccessCategory, held: bool) {
        let mut queues = self.held.get();
        *access_category.select_mut(&mut queues) = held;
        self.held.set(queues);
        self.wake_driver();
    }

    /// Delay the current publication's result until released or its bound
    /// expires. A queued successor cannot pass this publication.
    pub fn hold_completion(
        &self,
        category: oer_ieee80211_wire::qos::WmmAccessCategory,
        held: bool,
    ) {
        let mut results = self.held_results.get();
        *category.select_mut(&mut results) = held;
        self.held_results.set(results);
        self.wake_driver();
    }
    /// Set model time from the test's monotonic virtual clock. A held result
    /// becomes a hardware timeout at the backend's publication bound.
    pub fn set_time(&self, now: oer_time::Instant) {
        self.now_micros.set(now.as_micros());
        self.wake_driver();
    }

    /// Refuse the next enable.
    pub fn refuse_enable(&self) {
        self.refuse_enable.set(true);
    }

    /// Poison the backend.
    pub fn poison(&self) {
        if !matches!(self.stream.get(), ModelStream::Ended(_)) {
            self.stream.set(ModelStream::Ended(EventEnd::Quarantined {
                cause: ModelFault,
            }));
        }
        self.wake_driver();
    }

    /// An ordinary fault closes admission and returns every admitted owner
    /// before the event stream reports Drained. This differs from `poison`,
    /// which models an accounting/publication uncertainty and retains owners.
    pub fn fail_after_terminals(&self) {
        if matches!(self.stream.get(), ModelStream::Running) {
            self.stream.set(ModelStream::Draining);
            self.wake_driver();
        }
    }

    /// Inject a false drain claim for a consumer's contract regression.
    pub fn falsely_claim_drained(&self) {
        // Fault injection retains real queued owners, but lies about their
        // return. The service must reject this claim and fail-stop.
        self.stream
            .set(ModelStream::Ended(EventEnd::Drained { cause: ModelFault }));
        self.wake_driver();
    }

    fn failed(&self) -> bool {
        matches!(self.stream.get(), ModelStream::Ended(_))
    }

    fn admission_closed(&self) -> bool {
        !matches!(self.stream.get(), ModelStream::Running)
    }

    /// Number of actual disable calls; fail-stop is a different transition.
    pub fn disable_calls(&self) -> u32 {
        self.disables.get()
    }

    /// The driver's passes so far.
    pub fn passes(&self) -> u32 {
        self.passes.get()
    }

    /// The commands the driver took so far.
    pub fn commands(&self) -> u32 {
        self.commands.get()
    }

    /// The settings the driver applied so far.
    pub fn settings(&self) -> u32 {
        self.settings.get()
    }

    /// The complete EDCA set currently installed for this interface.
    pub fn edca(&self, role: VifRole) -> EdcaParameterSet {
        self.edca.borrow().parameters(role)
    }

    /// Actual contention draws, one per PPDU publication attempt.
    pub fn contentions(&self) -> Vec<ModelContention> {
        self.edca.borrow().draws.clone()
    }

    /// Inject a deterministic entropy word for subsequent backoff draws.
    pub fn set_backoff_entropy(&self, entropy: u32) {
        self.edca.borrow_mut().entropy = entropy;
    }

    /// The frames the air holds for the driver yet.
    pub fn pending_rx(&self) -> usize {
        self.rx.borrow().len()
    }

    fn wake_driver(&self) {
        if let Some(waker) = self.driver.borrow_mut().take() {
            waker.wake();
        }
    }

    fn next_answer(&self, default: AirAnswer) -> AirAnswer {
        self.answers.borrow_mut().pop_front().unwrap_or(default)
    }

    /// The next answer to a single MPDU of `response`, checked against it.
    fn single_result(&self, response: ResponsePolicy) -> Result<SingleAnswer, AnswerShapeMismatch> {
        let answer = self.next_answer(match response {
            ResponsePolicy::NoResponseExpected => AirAnswer::NoResponse,
            ResponsePolicy::Ack => AirAnswer::Ack,
            ResponsePolicy::BlockAck { .. } => AirAnswer::BlockAck(u64::MAX),
        });
        match (response, answer) {
            (_, AirAnswer::Collision) => Ok(SingleAnswer::Collision),
            (_, AirAnswer::AirDenied) => Ok(SingleAnswer::AirDenied),
            (_, AirAnswer::HardwareTimeout) => Ok(SingleAnswer::HardwareTimeout),
            (ResponsePolicy::NoResponseExpected, AirAnswer::NoResponse) => {
                Ok(SingleAnswer::Unsolicited)
            }
            (ResponsePolicy::Ack | ResponsePolicy::BlockAck { .. }, AirAnswer::NoResponse) => {
                Ok(SingleAnswer::Unanswered)
            }
            (ResponsePolicy::Ack, AirAnswer::Ack) => Ok(SingleAnswer::Ack),
            (ResponsePolicy::BlockAck { start_sequence }, AirAnswer::BlockAck(bitmap)) => {
                Ok(SingleAnswer::BlockAck(BlockAckReport {
                    start_sequence,
                    bitmap,
                }))
            }
            (
                ResponsePolicy::NoResponseExpected,
                AirAnswer::Ack | AirAnswer::BlockAck(_) | AirAnswer::BlockAckUnmeasured(_),
            )
            | (ResponsePolicy::Ack, AirAnswer::BlockAck(_) | AirAnswer::BlockAckUnmeasured(_))
            // A BlockAckReq's BlockAck is a single MPDU's response, which
            // the model always measures.
            | (ResponsePolicy::BlockAck { .. }, AirAnswer::Ack | AirAnswer::BlockAckUnmeasured(_)) => {
                Err(AnswerShapeMismatch)
            }
        }
    }

    /// The next answer to an aggregate, checked against its BlockAck.
    fn aggregate_result(&self) -> Result<AggregateAnswer, AnswerShapeMismatch> {
        match self.next_answer(AirAnswer::BlockAck(u64::MAX)) {
            AirAnswer::BlockAck(bitmap) => Ok(AggregateAnswer::BlockAck {
                bitmap,
                evidence: ModelBlockAckEvidence::Measured(MODEL_ACK_SNR_DB),
            }),
            AirAnswer::BlockAckUnmeasured(bitmap) => Ok(AggregateAnswer::BlockAck {
                bitmap,
                evidence: ModelBlockAckEvidence::Unmeasured,
            }),
            AirAnswer::NoResponse => Ok(AggregateAnswer::Unanswered),
            AirAnswer::Collision => Ok(AggregateAnswer::Collision),
            AirAnswer::AirDenied => Ok(AggregateAnswer::AirDenied),
            AirAnswer::HardwareTimeout => Ok(AggregateAnswer::HardwareTimeout),
            AirAnswer::Ack => Err(AnswerShapeMismatch),
        }
    }
}

/// A single MPDU's answer, checked against the response it solicited.
#[derive(Clone, Copy, Debug)]
enum SingleAnswer {
    /// The frame solicited no response.
    Unsolicited,
    Ack,
    /// A BlockAckReq's BlockAck.
    BlockAck(BlockAckReport),
    /// The solicited response did not come.
    Unanswered,
    Collision,
    AirDenied,
    HardwareTimeout,
}

/// An aggregate's answer, checked against its BlockAck.
#[derive(Clone, Copy, Debug)]
enum AggregateAnswer {
    /// Bit n acknowledges subframe n.
    BlockAck {
        bitmap: u64,
        evidence: ModelBlockAckEvidence,
    },
    Unanswered,
    Collision,
    AirDenied,
    HardwareTimeout,
}

#[derive(Clone, Copy, Debug)]
enum ResponsePolicy {
    NoResponseExpected,
    Ack,
    BlockAck { start_sequence: SequenceNumber },
}

impl ResponsePolicy {
    fn of_frame<K>(frame: &TxFrame<'_, '_, K>) -> Self {
        match frame {
            TxFrame::Individual { .. } => Self::Ack,
            TxFrame::IndividualNoAck { .. } | TxFrame::Group { .. } => Self::NoResponseExpected,
            TxFrame::BlockAckRequest {
                starting_sequence, ..
            } => Self::BlockAck {
                start_sequence: *starting_sequence,
            },
        }
    }
}
#[derive(Debug)]
struct AnswerShapeMismatch;

/// A command the handle sends the driver, in the order it made it.
/// The host model boxes its larger single-operation owner so small commands
/// do not reserve that space; the box moves opaque context without cloning.
enum Command<'id, X: ModelContext> {
    Configure(Configured<'id, ModelTypes<X>>),
    Submit(Box<Prepared<'id, X>>),
    SubmitAmpdu(PreparedAggregate<'id, X>),
    InstallKey {
        spp: oer_ieee80211_wire::security::Spp,
        key: KeyIdentity,
        material: Ccmp128Key,
    },
    Group {
        admitted_at: u64,
        change: GroupChange<'id, X>,
    },
    RemoveKey {
        key: KeyIdentity,
        removed: Removed<'id, ModelTypes<X>>,
    },
    CloseGate(GateOpen<'id, ModelPool<0>>),
    OpenGate,
}

type StationGroupPermission<'id> = Slot<'id, ModelGroupPermission<Station>, ModelPool<8>>;

struct PendingGroupInstall<'id, R: ModelRole> {
    spp: oer_ieee80211_wire::security::Spp,
    slot: GroupKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>,
    permission: Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath>,
    key_id: GroupKeyId,
    key: Ccmp128Key,
}

enum GroupInstallationCommand<'id> {
    Station(PendingGroupInstall<'id, Station>),
    AccessPoint(PendingGroupInstall<'id, AccessPoint>),
}

trait GroupInstallationRole: ModelRole + Sized {
    fn installation(pending: PendingGroupInstall<'_, Self>) -> GroupInstallationCommand<'_>;
}
impl GroupInstallationRole for Station {
    fn installation(pending: PendingGroupInstall<'_, Self>) -> GroupInstallationCommand<'_> {
        GroupInstallationCommand::Station(pending)
    }
}
impl GroupInstallationRole for AccessPoint {
    fn installation(pending: PendingGroupInstall<'_, Self>) -> GroupInstallationCommand<'_> {
        GroupInstallationCommand::AccessPoint(pending)
    }
}

/// Whole command custody, including permission, material and physical owner.
enum GroupChange<'id, X: ModelContext> {
    Install(GroupInstallationCommand<'id>),
    Replace {
        permission: StationGroupPermission<'id>,
        installed: ModelGroupKey<'id, Station>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    },
    Select {
        permission: StationGroupPermission<'id>,
        installed: ModelGroupKey<'id, Station>,
    },
    Remove {
        key: KeyIdentity,
        removed: Removed<'id, ModelTypes<X>>,
    },
}

/// The work of a queue: one MPDU, or one A-MPDU.
enum Work<'id, X: ModelContext> {
    Single(Box<Prepared<'id, X>>),
    Aggregate(PreparedAggregate<'id, X>),
}

impl<X: ModelContext> Work<'_, X> {
    fn role(&self) -> VifRole {
        match self {
            Self::Single(prepared) => prepared.role,
            Self::Aggregate(prepared) => prepared.role,
        }
    }
    fn key(&self) -> Option<&KeyIdentity> {
        match self {
            Self::Single(prepared) => prepared.key.as_ref(),
            Self::Aggregate(prepared) => prepared.key.as_ref(),
        }
        .map(|key| &key.identity)
    }
}

/// One subframe of an aggregate, in its buffer.
struct Subframe<'id, D> {
    mpdu: ModelMpdu<'id, D>,
    /// Checked at admission against the first subframe's BA window.
    acknowledgement_bit: u64,
}

/// Complete aggregate storage; each subframe retains its own MPDU/body.
pub struct ModelSubframes<'id, D = ModelBody>(Vec<Subframe<'id, D>>);
impl<D> ModelSubframes<'_, D> {
    /// Number of complete subframes returned by a terminal or cancellation.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// An empty builder can be cancelled before it accepts any subframe.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
/// Owning iteration is available after terminal consumption or cancellation.
pub struct ModelSubframeIter<'id, D = ModelBody>(alloc::vec::IntoIter<Subframe<'id, D>>);
impl<D> ModelSubframeIter<'_, D> {
    /// Whether cancellation or terminal iteration has any MPDUs left.
    pub fn is_empty(&self) -> bool {
        self.0.as_slice().is_empty()
    }
}
impl<'id, D> Iterator for ModelSubframeIter<'id, D> {
    type Item = ModelMpdu<'id, D>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|subframe| subframe.mpdu)
    }
}
impl<'id, D> IntoIterator for ModelSubframes<'id, D> {
    type Item = ModelMpdu<'id, D>;
    type IntoIter = ModelSubframeIter<'id, D>;
    fn into_iter(self) -> Self::IntoIter {
        ModelSubframeIter(self.0.into_iter())
    }
}
/// Execution lends subframe storage without exposing replaceable owners.
pub struct ModelSubframesExecution<'a, 'id, D = ModelBody> {
    subframes: &'a mut [Subframe<'id, D>],
}
impl<'id, D> ExecutionPayload for ModelSubframes<'id, D> {
    type Execution<'a>
        = ModelSubframesExecution<'a, 'id, D>
    where
        Self: 'a;
    fn execution(&mut self) -> Self::Execution<'_> {
        ModelSubframesExecution {
            subframes: &mut self.0,
        }
    }
}
impl<D> ModelSubframesExecution<'_, '_, D> {
    fn publish(
        &mut self,
        role: VifRole,
        keys: &[KeyIdentity],
        key: Option<&KeyIdentity>,
        rate: PhyRate,
    ) -> Result<Vec<AirFrame>, ModelFault> {
        for subframe in &*self.subframes {
            subframe.mpdu.preflight(key.is_some())?;
        }
        let mut frames = Vec::with_capacity(self.subframes.len());
        for subframe in &mut *self.subframes {
            subframe.mpdu.execution().publish(role, key)?;
            frames.push(subframe.mpdu.on_air(role, keys, key, rate)?);
        }
        Ok(frames)
    }
}

type ModelInFlight<'id, X, M> =
    InFlight<<X as ModelContext>::Context<'id>, Slot<'id, ModelTickets, ModelPool<1>>, M>;

/// An A-MPDU attempt prepared for its queue.
struct PreparedAggregate<'id, X: ModelContext> {
    operation: ModelInFlight<'id, X, ModelSubframes<'id, X::Body>>,
    role: VifRole,
    start_sequence: SequenceNumber,
    queue: oer_ieee80211_wire::qos::WmmAccessCategory,
    rate: PhyRate,
    key: Option<PreparedKey>,
}

/// An attempt prepared for its queue.
struct Prepared<'id, X: ModelContext> {
    operation: ModelInFlight<'id, X, ModelMpdu<'id, X::Body>>,
    role: VifRole,
    queue: oer_ieee80211_wire::qos::WmmAccessCategory,
    plan: oer_ieee80211_port::RetryPlan<PhyRate>,
    key: Option<PreparedKey>,
}

/// The settings the handle writes and the driver takes before its next
/// command; the latest value of each kind wins.
#[derive(Default)]
struct Settings {
    other: bool,
    /// Minted slots bound the keys to MODEL_BLOCK_ACK_SLOTS. Repeated writes
    /// replace a slot's pending value instead of occupying command capacity.
    block_acks: BTreeMap<u8, Option<ModelReceiveAgreement>>,
}

/// The install's queues between the handle and the driver. RX capacity stays
/// charged until recycle, including buffers handed to the body by disable.
/// Disable preserves an unobserved EventsLost indication for the next enable;
/// draining events does not acknowledge that separate loss indication.
pub struct ModelHeld<'id, X: ModelContext = ()> {
    air: Rc<ModelAir>,
    poison_state: RefCell<PoisonState<ModelFault>>,
    driver_state: RefCell<Driver<'id, X>>,
    commands: Bounded<Command<'id, X>>,
    /// The received buffers the body gives back, at most one per buffer.
    recycled: Bounded<ModelRxBuffer>,
    settings: RefCell<Settings>,
    events: Bounded<LowerMacEvent<'id, ModelTypes<X>>>,
    quarantined: Bounded<Work<'id, X>>,
    failed_group: Bounded<GroupChange<'id, X>>,
    failed_group_terminal: Bounded<LowerMacEvent<'id, ModelTypes<X>>>,
    /// The receive buffers out with the body, queued or held.
    rx_out: Cell<usize>,
    /// Received frames dropped since the stream last said so.
    lost: Cell<bool>,
    body: RefCell<Option<Waker>>,
    /// The backend alone attests the hardware barrier with the one gate place.
    gate_issuer: RefCell<GateIssuer<'id, ModelPool<0>>>,
    /// Disable asks the driver to return all queued operations before the
    /// owner ends the phase. Unlike poison, this is an ordinary terminal.
    draining: Cell<bool>,
}

impl<'id, X: ModelContext> ModelHeld<'id, X> {
    fn command(&self, command: Command<'id, X>) {
        if self.commands.push(command).is_err() {
            self.poison();
        }
        self.air.wake_driver();
    }

    fn post(&self, event: LowerMacEvent<'id, ModelTypes<X>>) {
        if self.events.push(event).is_err() {
            self.poison();
        }
        self.wake_body();
    }

    fn take_event(&self) -> Option<LowerMacEvent<'id, ModelTypes<X>>> {
        self.events.pop()
    }

    fn wake_body(&self) {
        if let Some(waker) = self.body.borrow_mut().take() {
            waker.wake();
        }
    }

    fn poison(&self) {
        self.air.poison();
        self.wake_body();
    }

    /// Charge one received buffer to the bounded RX capacity, when a place
    /// is left. The bound is checked before the count moves; a count that
    /// could not move is a broken model invariant and poisons, as a return
    /// with none charged does.
    fn charge_rx(&self) -> bool {
        let out = self.rx_out.get();
        if out >= MODEL_RX_EVENTS {
            return false;
        }
        match out.checked_add(1) {
            Some(out) => {
                self.rx_out.set(out);
                true
            }
            None => {
                self.poison();
                false
            }
        }
    }

    /// Return one received buffer's capacity. Each buffer is charged once
    /// when delivered, so a return with none charged is a broken model
    /// invariant: it poisons instead of hiding the count.
    fn release_rx(&self) {
        match self.rx_out.get().checked_sub(1) {
            Some(out) => self.rx_out.set(out),
            None => self.poison(),
        }
    }
}

/// The install's local brand. Key identity belongs to each installed key;
/// there is no independent wrapping key-index source in the handle.
#[derive(Default)]
pub struct ModelLocal<'id> {
    brand: PhantomData<fn(&'id ()) -> &'id ()>,
}

/// The model's backend; only [`run_port`](oer_radio_port::run_port)
/// holds it.
pub struct ModelBackend<C = ModelCaps, X: ModelContext = ()> {
    air: Rc<ModelAir>,
    channel: Channel,
    addresses: [IndividualAddress; 2],
    caps: PhantomData<fn() -> (C, X)>,
}

impl<C: Ieee80211Capabilities, X: ModelContext> ModelBackend<C, X> {
    /// A backend tuned to `channel` with the station's and the access
    /// point's addresses, and the air it plays on.
    pub fn new(
        channel: Channel,
        addresses: [IndividualAddress; 2],
    ) -> (Installed<Self>, Rc<ModelAir>) {
        let air = Rc::new(ModelAir::default());
        (
            Installed::new(
                Self {
                    air: air.clone(),
                    channel,
                    addresses,
                    caps: PhantomData,
                },
                (),
            ),
            air,
        )
    }
}

/// The model's disabled-only settings.
pub struct ModelSetup<'a, C = ModelCaps> {
    channel: &'a mut Channel,
    caps: PhantomData<fn() -> C>,
}

impl<C: Ieee80211Capabilities> LowerMacSetup for ModelSetup<'_, C> {
    type Caps = C;

    fn set_channel(&mut self, channel: SupportedChannel<C>) {
        *self.channel = channel.channel();
    }
}

impl<C: Ieee80211Capabilities, X: ModelContext> PortBackend for ModelBackend<C, X> {
    type Event<'id> = LowerMacEvent<'id, ModelTypes<X>>;
    type Terminal<'id> = LowerMacTerminal<'id, ModelTypes<X>>;
    type Domain = Ieee80211Radio;
    type Fault = ModelFault;
    type Refusal = ();
    type Setup<'a, 'id>
        = ModelSetup<'a, C>
    where
        Self: 'a,
        'id: 'a;
    type Phase<'a, 'id>
        = ModelPhase<'a, 'id, C, X>
    where
        Self: 'a,
        'id: 'a;
    type Pools = (
        oer_ieee80211_port::ArbiterPool,
        oer_ieee80211_port::TransmitPool<ModelTickets, MODEL_TICKETS>,
        oer_radio_port::Slots<ModelSmall, MODEL_SMALL_BUFFERS>,
        oer_radio_port::Slots<ModelLarge, MODEL_LARGE_BUFFERS>,
        oer_radio_port::Slots<ConnectionEpochKind, 1>,
        ModelAssociationPool<Station>,
        ModelAssociationPool<AccessPoint>,
        ModelBlockAckPool,
        oer_radio_port::Slots<ModelGroupPermission<Station>, 1>,
        (
            oer_radio_port::Slots<ModelGroupPermission<AccessPoint>, 1>,
            InterfaceCommandPool<Station>,
            InterfaceCommandPool<AccessPoint>,
        ),
    );
    type Resources<'id> = ModelResources<'id>;
    type Held<'id> = ModelHeld<'id, X>;
    type Local<'id> = ModelLocal<'id>;
    type Driver = ();
    type State = ();
    type Uninstall = ();
    type Uninstalled = Rc<ModelAir>;

    fn enable<'a, 'id: 'a>(
        &'a mut self,
        _call: Call,
        _held: &'a Self::Held<'id>,
        _local: &'a mut Self::Local<'id>,
    ) -> impl Future<Output = EnableResult<(), ModelFault>> + 'a {
        let air = &self.air;
        core::future::ready(if air.admission_closed() {
            Err(Poisoned { cause: ModelFault })
        } else if air.refuse_enable.replace(false) {
            Ok(Err(()))
        } else {
            Ok(Ok(()))
        })
    }

    fn disable<'a, 'id>(
        &'a mut self,
        _call: Call,
        held: &'a ModelHeld<'id, X>,
        _: &'a mut ModelLocal<'id>,
        sink: &'a mut dyn FnMut(
            LowerMacTerminal<'id, ModelTypes<X>>,
        )
            -> core::ops::ControlFlow<LowerMacTerminal<'id, ModelTypes<X>>>,
    ) -> impl Future<Output = Result<(), Poisoned<ModelFault>>> + 'a
    where
        'id: 'a,
    {
        self.air
            .disables
            .set(self.air.disables.get().saturating_add(1));
        // The driver ends every admitted command. The terminal sink cannot
        // receive unsolicited RX; this backend recycles those owners below.
        held.draining.set(true);
        held.air.wake_driver();
        poll_fn(move |cx| {
            if held.air.failed() {
                return Poll::Ready(Err(Poisoned { cause: ModelFault }));
            }
            if held.draining.get() || !held.commands.is_empty() || !held.recycled.is_empty() {
                *held.body.borrow_mut() = Some(cx.waker().clone());
                held.air.wake_driver();
                return Poll::Pending;
            }
            while let Some(event) = held.take_event() {
                match event {
                    LowerMacEvent::Terminal(terminal) => {
                        if let core::ops::ControlFlow::Break(terminal) = sink(terminal) {
                            held.events
                                .items
                                .borrow_mut()
                                .push_front(LowerMacEvent::Terminal(terminal));
                            held.poison();
                            return Poll::Ready(Err(Poisoned { cause: ModelFault }));
                        }
                    }
                    LowerMacEvent::Received { mut frame, .. } => {
                        frame.recycled = true;
                        held.release_rx();
                        held.lost.set(true);
                    }
                    LowerMacEvent::Tbtt(_) => held.lost.set(true),
                }
            }
            if !held.air.group_rx.borrow().is_empty() {
                held.air.group_rx.borrow_mut().clear();
                held.lost.set(true);
            }
            // Only owners actually recycled here release RX capacity; buffers
            // previously lent to the body remain charged until their return.
            // The coalesced loss remains observable after re-enable.
            Poll::Ready(Ok(()))
        })
    }

    fn now(
        &self,
        _call: Call,
    ) -> impl Future<Output = PortResult<RadioInstant<Ieee80211Radio>, ClockError, ModelFault>> + '_
    {
        core::future::ready(Ok(Err(ClockError::Unavailable)))
    }

    fn resources<'id>(&mut self, mint: Mint<'id, Self::Pools>) -> InstallParts<'id, Self> {
        let (
            (gate, gate_issuer, arbiter_authority),
            (tickets, terminal_access),
            small,
            large,
            [epoch_slot],
            station_vacant,
            access_point_vacant,
            block_acks,
            [station_group_permission],
            ([access_point_group_permission], station_commands, access_point_commands),
        ) = mint.into_pools();
        let (station_association, station_bank) = station_vacant.into_parts();
        let (station_pairwise, station_group) = station_bank.into_parts();
        let (access_point_association, access_point_bank) = access_point_vacant.into_parts();
        let (access_point_pairwise, access_point_group) = access_point_bank.into_parts();
        let held = ModelHeld {
            air: self.air.clone(),
            poison_state: RefCell::default(),
            driver_state: RefCell::new(Driver {
                queues: core::array::from_fn(|_| TxQueue {
                    waiting: VecDeque::new(),
                    air: None,
                }),
                keys: Vec::new(),
                removing: Vec::new(),
                gate: DriverGate::Open,
            }),
            commands: Bounded::new(MODEL_COMMANDS, "command"),
            recycled: Bounded::new(MODEL_RX_EVENTS, "recycle"),
            settings: RefCell::default(),
            events: Bounded::new(MODEL_EVENTS, "event"),
            quarantined: Bounded::new(1, "answer shape rejection"),
            failed_group: Bounded::new(2, "failed GTK command"),
            failed_group_terminal: Bounded::new(2, "uncertain GTK terminal"),
            rx_out: Cell::new(0),
            lost: Cell::new(false),
            body: RefCell::default(),
            gate_issuer: RefCell::new(gate_issuer),
            draining: Cell::new(false),
        };
        let resources = ModelResources {
            station_association,
            access_point_association,
            station_commands,
            access_point_commands,
            connection_epoch: epoch_slot,
            tickets: tickets.into_iter().collect(),
            terminal_access,
            small: small
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot,
                    memory: ModelMemory::Small(Box::new([0; ModelSmall::CAPACITY])),
                })
                .collect(),
            large: large
                .into_iter()
                .map(|slot| ModelBuffer {
                    slot,
                    memory: ModelMemory::Large(Box::new([0; ModelLarge::CAPACITY])),
                })
                .collect(),
            station_pairwise,
            station_group,
            access_point_pairwise,
            access_point_group,
            block_acks,
            station_group_permission,
            access_point_group_permission,
            gate,
            arbiter_authority,
        };
        InstallParts {
            held,
            local: ModelLocal::default(),
            resources,
        }
    }

    fn setup<'a, 'id: 'a>(
        &'a mut self,
        _call: Call,
        _held: &'a Self::Held<'id>,
        _: &'a mut Self::Local<'id>,
    ) -> Self::Setup<'a, 'id> {
        ModelSetup {
            channel: &mut self.channel,
            caps: PhantomData,
        }
    }

    fn phase<'a, 'id>(
        &'a mut self,
        _call: Call,
        held: &'a ModelHeld<'id, X>,
        _: &'a mut ModelLocal<'id>,
    ) -> ModelPhase<'a, 'id, C, X>
    where
        'id: 'a,
    {
        ModelPhase {
            backend: self,
            held,
        }
    }

    async fn drive<'p, 'id>(
        driver: DriverLoan<'p, Self::Driver>,
        held: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> DriverExit<(), ModelFault> {
        poll_fn(|cx| {
            held.air.passes.set(held.air.passes.get().saturating_add(1));
            if held.air.failed() || retire.is_requested() || retire.is_abandoned() {
                return Poll::Ready(());
            }
            if matches!(held.air.stream.get(), ModelStream::Draining) {
                held.draining.set(true);
            }
            held.driver_state.borrow_mut().serve(held);
            if held.air.failed() {
                return Poll::Ready(());
            }
            *held.air.driver.borrow_mut() = Some(cx.waker().clone());
            // Retirement has its own wake, independent of air traffic.
            let wait = retire.wait();
            let mut wait = core::pin::pin!(wait);
            wait.as_mut().poll(cx)
        })
        .await;
        if held.air.failed() {
            held.wake_body();
            let (cause, published) = held.poison_state.borrow_mut().publish(ModelFault);
            DriverExit::Faulted {
                state: (),
                cause: cause.cause,
                poisoned: driver.fail_stop(published),
            }
        } else {
            retire.retired(driver, ()).await
        }
    }

    fn retire_bound(&self, _call: Call) -> impl Future<Output = ()> + use<C, X> {
        // A model step never awaits hardware. Retirement's registered wake
        // ends it in the core's next poll.
        core::future::pending()
    }

    fn detach<'id>(&mut self, _call: Detach<'_>, _: &ModelHeld<'id, X>) {}

    fn uninstall<'id>(
        self,
        held: ModelHeld<'id, X>,
        local: ModelLocal<'id>,
        retired: Retired<'id, ((), ())>,
        detached: Detached<'id>,
        (): (),
    ) -> Rc<ModelAir> {
        let ((), ()) = retired.into_state();
        // Only Quarantined retains undelivered owners. Drained has returned
        // them all and may release its now-empty queues during uninstall.
        if matches!(
            held.air.stream.get(),
            ModelStream::Ended(EventEnd::Quarantined { .. })
        ) {
            let _quarantined = oer_radio_port::Quarantined::new((held, local, detached));
        }
        self.air.end_installation();
        self.air
    }

    fn abandon(&mut self, call: Quarantine) -> Abandon {
        // One host thread owns every step; no ISR or device can outlive it.
        // Held queues remain in the core's quarantine even after this stop.
        self.air.poison();
        self.air.end_installation();
        Abandon::Stopped(call.stopped())
    }
}

/// The depth of the model driver's software queue of each access
/// category: the composition's initial value.
pub const MODEL_TX_QUEUE_DEPTH: core::num::NonZeroU8 = match core::num::NonZeroU8::new(2) {
    Some(depth) => depth,
    None => panic!("a queue holds an attempt"),
};

impl<C: Ieee80211Capabilities, X: ModelContext> LowerMacBackend for ModelBackend<C, X> {
    type Caps = C;
    const TX_QUEUE_DEPTH: core::num::NonZeroU8 = MODEL_TX_QUEUE_DEPTH;
    const PUBLICATION_BOUND: oer_time::Duration = MODEL_PUBLICATION_BOUND;
    const GROUP_COMMAND_BOUND: oer_time::Duration = MODEL_GROUP_COMMAND_BOUND;
}

/// The driver retains the unique open place throughout the closing barrier.
enum DriverGate<'id> {
    Open,
    Closing(GateOpen<'id, ModelPool<0>>),
    Closed,
}

struct Removal<'id, X: ModelContext> {
    key: KeyIdentity,
    removed: Removed<'id, ModelTypes<X>>,
    group_admitted_at: Option<u64>,
}

/// The model's hardware, which only the driver owns.
struct Driver<'id, X: ModelContext> {
    /// The prepared attempts of each queue, the head first.
    queues: [TxQueue<'id, X>; WmmAccessCategory::ALL.len()],
    /// The installed hardware keys.
    keys: Vec<KeyIdentity>,
    removing: Vec<Removal<'id, X>>,
    gate: DriverGate<'id>,
}

impl<'id, X: ModelContext> Driver<'id, X> {
    /// Serve everything posted: the settings, the commands in order, the
    /// queues, then the air.
    fn serve(&mut self, held: &ModelHeld<'id, X>) {
        loop {
            if held.air.failed() {
                return;
            }
            // The settings come before the next command.
            let settings = core::mem::take(&mut *held.settings.borrow_mut());
            for (slot, wanted) in settings.block_acks {
                let mut applied = held.air.receive_agreements.borrow_mut();
                match wanted {
                    Some(agreement) => {
                        applied.insert(slot, agreement);
                    }
                    None => {
                        applied.remove(&slot);
                    }
                }
            }
            while let Some(mut buffer) = held.recycled.pop() {
                buffer.recycled = true;
                held.release_rx();
            }
            if settings.other {
                held.air
                    .settings
                    .set(held.air.settings.get().saturating_add(1));
            }
            let command = held.commands.pop();
            let Some(command) = command else {
                break;
            };
            held.air
                .commands
                .set(held.air.commands.get().saturating_add(1));
            match command {
                Command::Configure(permission) => {
                    let role = permission.role();
                    // Preserve each AC's terminal order. Its published head
                    // completes before queued owners are withdrawn; a retry of
                    // the old interface configuration cannot pass this barrier.
                    self.publish_before(held, |candidate| candidate == role);
                    if self.queues.iter().any(|queue| {
                        queue
                            .air
                            .as_ref()
                            .is_some_and(|publication| publication.work.role() == role)
                    }) {
                        held.commands
                            .items
                            .borrow_mut()
                            .push_front(Command::Configure(permission));
                        break;
                    }
                    held.air
                        .settings
                        .set(held.air.settings.get().saturating_add(1));
                    held.post(LowerMacEvent::Terminal(LowerMacTerminal::Configured(
                        permission,
                    )));
                }
                Command::Submit(prepared) => {
                    let queue = prepared.queue;
                    queue
                        .select_mut(&mut self.queues)
                        .waiting
                        .push_back(Work::Single(prepared));
                }
                Command::SubmitAmpdu(aggregate) => {
                    let queue = aggregate.queue;
                    queue
                        .select_mut(&mut self.queues)
                        .waiting
                        .push_back(Work::Aggregate(aggregate));
                }
                Command::InstallKey { spp, key, material } => {
                    held.air.apply_association(key.0.role, spp);
                    held.air.install_material(&key, material);
                    self.keys.push(key);
                }
                Command::Group {
                    admitted_at,
                    change,
                } => {
                    if admitted_at
                        .checked_add(MODEL_GROUP_COMMAND_BOUND.as_micros())
                        .is_none_or(|deadline| held.air.now_micros.get() >= deadline)
                        || held.air.group_failure.get() == Some(GroupFaultStage::Before)
                    {
                        let _capacity_fault = held.failed_group.push(change);
                        held.poison();
                        return;
                    }
                    if held.air.group_held.get() && !held.draining.get() {
                        // We popped exactly this cell; restore it without creating
                        // capacity or allowing a later command to pass it.
                        held.commands.items.borrow_mut().push_front(Command::Group {
                            admitted_at,
                            change,
                        });
                        break;
                    }
                    self.group_change(held, admitted_at, change);
                    if held.air.failed() {
                        return;
                    }
                }
                Command::RemoveKey { key, removed } => {
                    self.removing.push(Removal {
                        key,
                        removed,
                        group_admitted_at: None,
                    });
                }
                Command::CloseGate(gate) => self.gate = DriverGate::Closing(gate),
                Command::OpenGate => self.gate = DriverGate::Open,
            }
            self.publish(held);
        }
        // A result released by the world makes progress without a command.
        self.publish(held);
        let idle = self
            .queues
            .iter()
            .all(|queue| queue.air.is_none() && queue.waiting.is_empty());
        if idle {
            self.gate = match core::mem::replace(&mut self.gate, DriverGate::Closed) {
                DriverGate::Closing(gate) => {
                    let closed = held.gate_issuer.borrow_mut().close(gate);
                    held.post(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(
                        closed,
                    )));
                    DriverGate::Closed
                }
                state => state,
            };
        }
        if held.air.failed() {
            return;
        }
        self.finish_removals(held);
        if held.air.failed() {
            return;
        }
        if idle
            && self.removing.is_empty()
            && held.commands.is_empty()
            && held.draining.replace(false)
        {
            held.wake_body();
        }
        if matches!(held.air.stream.get(), ModelStream::Running) {
            self.receive(held);
        }
    }

    fn activate_group(&mut self, held: &ModelHeld<'id, X>, identity: &KeyIdentity, write: bool) {
        let Some(role) = identity.0.group_role else {
            return;
        };
        for key in &self.keys {
            if key.0.key_id == identity.0.key_id && key.0.group_role == Some(role) {
                key.0.rx_active.set(false);
            }
        }
        identity.0.rx_active.set(true);
        if write {
            held.air
                .group_writes
                .set(held.air.group_writes.get().saturating_add(1));
            self.keys.push(identity.clone());
        }
    }

    /// Apply the mutation without waking the consumer. The installed owner
    /// stays private until all frozen old RX precedes its FIFO terminal.
    fn install_group<R>(
        &mut self,
        held: &ModelHeld<'id, X>,
        pending: PendingGroupInstall<'id, R>,
    ) -> LowerMacEvent<'id, ModelTypes<X>>
    where
        R: ModelRole + RoleKeys<'id, ModelTypes<X>, Keys = ModelKeys<'id, R>>,
    {
        held.air.apply_association(R::ROLE, pending.spp);
        let identity = KeyIdentity::group(
            pending.slot.index(),
            pending.key_id,
            R::ROLE,
            pending.key,
            &held.air,
        );
        self.activate_group(held, &identity, true);
        LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(R::installed_group(
            ModelGroupKey {
                identity,
                slot: pending.slot,
            },
            pending.permission,
        )))
    }

    fn group_change(
        &mut self,
        held: &ModelHeld<'id, X>,
        admitted_at: u64,
        change: GroupChange<'id, X>,
    ) {
        // Snapshot and mutate before any wake can reenter ModelAir. This
        // single-threaded model has no other interleaving during these steps.
        // New-key ingress captured by delivery callbacks stays in the live
        // queues until after this command's terminal. No drain-to-idle loop
        // can postpone the boundary under continuous traffic.
        let received = core::mem::take(&mut *held.air.rx.borrow_mut());
        let captured = core::mem::take(&mut *held.air.group_rx.borrow_mut());
        let terminal = match change {
            GroupChange::Install(GroupInstallationCommand::Station(pending)) => {
                Some(self.install_group(held, pending))
            }
            GroupChange::Install(GroupInstallationCommand::AccessPoint(pending)) => {
                Some(self.install_group(held, pending))
            }
            GroupChange::Replace {
                permission,
                installed,
                key_id,
                key,
            } => {
                installed.identity.0.active.set(false);
                installed.identity.0.rx_active.set(false);
                held.air.remove_key_material(&installed.identity);
                self.keys.retain(|key| *key != installed.identity);
                let identity = KeyIdentity::group(
                    installed.slot.index(),
                    key_id,
                    VifRole::Station,
                    key,
                    &held.air,
                );
                self.activate_group(held, &identity, true);
                Some(LowerMacEvent::Terminal(
                    LowerMacTerminal::GroupKeyReplaced {
                        key: ModelGroupKey {
                            identity,
                            slot: installed.slot,
                        },
                        permission,
                    },
                ))
            }
            GroupChange::Select {
                permission,
                installed,
            } => {
                self.activate_group(held, &installed.identity, false);
                Some(LowerMacEvent::Terminal(
                    LowerMacTerminal::GroupKeySelected {
                        key: installed,
                        permission,
                    },
                ))
            }
            GroupChange::Remove { key, removed } => {
                key.0.active.set(false);
                key.0.rx_active.set(false);
                self.removing.push(Removal {
                    key,
                    removed,
                    group_admitted_at: Some(admitted_at),
                });
                // AP removal can still wait for a published TX to end. Its
                // permission remains in removing, not in a success terminal.
                None
            }
        };
        for (frame, meta) in received {
            self.deliver(held, frame, meta);
        }
        for received in captured {
            self.deliver(held, received.frame, received.meta);
        }
        let Some(terminal) = terminal else {
            return;
        };
        if held.air.group_failure.get() == Some(GroupFaultStage::After) {
            let _capacity_fault = held.failed_group_terminal.push(terminal);
            held.poison();
        } else {
            held.post(terminal);
        }
    }

    /// Keep each published head until its result; barriers stop the next
    /// publication, not just admission of new plans.
    fn publish(&mut self, held: &ModelHeld<'id, X>) {
        self.publish_before(held, |_| false);
    }

    fn publish_before(&mut self, held: &ModelHeld<'id, X>, reconfigure: impl Fn(VifRole) -> bool) {
        for ((queue, busy), result_held) in self
            .queues
            .iter_mut()
            .zip(held.air.held.get())
            .zip(held.air.held_results.get())
        {
            loop {
                if let Some(publication) = queue.air.take() {
                    let elapsed = held
                        .air
                        .now_micros
                        .get()
                        .saturating_sub(publication.started);
                    if result_held && elapsed < MODEL_PUBLICATION_BOUND.as_micros() {
                        queue.air = Some(publication);
                        break;
                    }
                    let cause =
                        withdrawal(held, &self.gate, publication.work.key(), &self.removing)
                            .or_else(|| {
                                reconfigure(publication.work.role())
                                    .then_some(WithdrawCause::GateClosed)
                            });
                    let work = if result_held {
                        publication.work.timed_out()
                    } else {
                        publication.work
                    };
                    queue.air = work.complete(held, &self.keys, cause);
                    if held.air.failed() {
                        return;
                    }
                    continue;
                }
                let Some(work) = queue.waiting.pop_front() else {
                    break;
                };
                if let Some(cause) = withdrawal(held, &self.gate, work.key(), &self.removing)
                    .or_else(|| reconfigure(work.role()).then_some(WithdrawCause::GateClosed))
                {
                    withdrawn(held, work, cause);
                } else if busy {
                    queue.waiting.push_front(work);
                    break;
                } else {
                    queue.air = start_work(held, &self.keys, work);
                    if held.air.failed() {
                        return;
                    }
                }
            }
        }
    }

    fn finish_removals(&mut self, held: &ModelHeld<'id, X>) {
        // The single reference belongs to KeyInstall. Additional owners are
        // actual prepared/queued/published attempts. Returned Sent identities
        // do not keep a preparation alive and do not delay key-slot reuse.
        let mut pending = Vec::new();
        for removal in self.removing.drain(..) {
            let overdue = removal.group_admitted_at.is_some_and(|admitted| {
                admitted
                    .checked_add(MODEL_GROUP_COMMAND_BOUND.as_micros())
                    .is_none_or(|deadline| held.air.now_micros.get() >= deadline)
            });
            let uncertain = removal.group_admitted_at.is_some()
                && held.air.group_failure.get() == Some(GroupFaultStage::After);
            if overdue || uncertain || held.air.failed() {
                pending.push(removal);
                held.poison();
            } else if Rc::strong_count(&removal.key.0.preparations) == 1 {
                held.air.remove_key_material(&removal.key);
                self.keys.retain(|installed| *installed != removal.key);
                held.post(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(
                    removal.removed,
                )));
            } else {
                pending.push(removal);
            }
        }
        self.removing = pending;
    }

    fn deliver(&mut self, held: &ModelHeld<'id, X>, frame: Vec<u8>, meta: ModelRxMeta) {
        if held.charge_rx() {
            held.post(LowerMacEvent::Received {
                frame: ModelRxBuffer {
                    bytes: frame,
                    recycled: false,
                    air: Rc::downgrade(&held.air),
                    streamed: false,
                },
                meta,
            });
        } else {
            held.lost.set(true);
            held.wake_body();
        }
    }

    /// Capture only this finite batch; newly arriving traffic belongs to the
    /// next pass. Delayed ingress is forced through before a GTK boundary.
    fn receive(&mut self, held: &ModelHeld<'id, X>) {
        let captured = core::mem::take(&mut *held.air.rx.borrow_mut());
        for (frame, meta) in captured {
            self.deliver(held, frame, meta);
        }
        let captured = core::mem::take(&mut *held.air.group_rx.borrow_mut());
        for received in captured {
            if received.stage == GroupRxStage::Ready {
                // Integrity was established at capture under this owner.
                // Retain it through delivery even if the slot is later reused.
                let _installation = received.installation;
                self.deliver(held, received.frame, received.meta);
            } else {
                held.air.group_rx.borrow_mut().push_back(received);
            }
        }
    }
}

/// Synthetic model latency, in model time; hardware backends declare their
/// own PPDU + response/timeout bound. It does not scale with retry-plan length.
pub const MODEL_PUBLICATION_BOUND: oer_time::Duration = oer_time::Duration::from_millis(1);
/// Synthetic admission-to-GTK-terminal bound, driven by `ModelAir::set_time`.
pub const MODEL_GROUP_COMMAND_BOUND: oer_time::Duration = oer_time::Duration::from_millis(2);

struct TxQueue<'id, X: ModelContext> {
    waiting: VecDeque<Work<'id, X>>,
    air: Option<Publication<'id, X>>,
}
struct Publication<'id, X: ModelContext> {
    started: u64,
    work: Published<'id, X>,
}
struct SinglePlan<'id, X: ModelContext> {
    prepared: Box<Prepared<'id, X>>,
    remaining: VecDeque<PhyRate>,
    last: Option<(NonZeroU16, PhyRate)>,
    rate: PhyRate,
}
enum Published<'id, X: ModelContext> {
    Single {
        plan: SinglePlan<'id, X>,
        answer: SingleAnswer,
    },
    Aggregate {
        prepared: PreparedAggregate<'id, X>,
        report: BlockAckReport,
        answer: AggregateAnswer,
    },
}
/// Derive the terminal from the actual lifecycle barrier and key owner;
/// callers cannot exchange three unrelated boolean arguments. Disable wins
/// over key removal, which wins over a closing or closed gate.
fn withdrawal<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    gate: &DriverGate<'id>,
    key: Option<&KeyIdentity>,
    removals: &[Removal<'id, X>],
) -> Option<WithdrawCause> {
    if held.draining.get() {
        Some(WithdrawCause::Disabled)
    } else if key.is_some_and(|key| removals.iter().any(|removal| &removal.key == key)) {
        Some(WithdrawCause::KeyRemoved)
    } else {
        match gate {
            DriverGate::Open => None,
            DriverGate::Closing(_) | DriverGate::Closed => Some(WithdrawCause::GateClosed),
        }
    }
}
impl<'id, X: ModelContext> Published<'id, X> {
    fn role(&self) -> VifRole {
        match self {
            Self::Single { plan, .. } => plan.prepared.role,
            Self::Aggregate { prepared, .. } => prepared.role,
        }
    }
    fn key(&self) -> Option<&KeyIdentity> {
        match self {
            Self::Single { plan, .. } => plan.prepared.key.as_ref(),
            Self::Aggregate { prepared, .. } => prepared.key.as_ref(),
        }
        .map(|key| &key.identity)
    }
    /// The publication with its answer replaced by the hardware timeout.
    fn timed_out(self) -> Self {
        match self {
            Self::Single { plan, .. } => Self::Single {
                plan,
                answer: SingleAnswer::HardwareTimeout,
            },
            Self::Aggregate {
                prepared, report, ..
            } => Self::Aggregate {
                prepared,
                report,
                answer: AggregateAnswer::HardwareTimeout,
            },
        }
    }
    fn complete(
        self,
        held: &ModelHeld<'id, X>,
        keys: &[KeyIdentity],
        cause: Option<WithdrawCause>,
    ) -> Option<Publication<'id, X>> {
        match self {
            Self::Single {
                mut plan,
                answer: result,
            } => {
                let (outcome, last) = single_outcome(result, plan.last, plan.rate, cause);
                plan.last = last;
                if matches!(
                    outcome,
                    TxOutcome::Ended(SingleEnd::AckTimeout(_) | SingleEnd::ContentionLimit(_))
                ) && let Some(rate) = plan.remaining.pop_front()
                {
                    held.air.edca.borrow_mut().failure(plan.prepared.queue);
                    plan.rate = rate;
                    return publish_single(held, keys, plan);
                }
                held.air.edca.borrow_mut().reset(plan.prepared.queue);
                terminal(held, *plan.prepared, outcome);
            }
            Self::Aggregate {
                prepared,
                report,
                answer,
            } => {
                let outcome = aggregate_outcome(answer, prepared.rate, report, cause);
                held.air.edca.borrow_mut().reset(prepared.queue);
                terminal_aggregate(held, prepared, outcome);
            }
        }
        None
    }
}

/// A single MPDU's outcome after `answer`, and its transmissions so far:
/// `last` before this publication, which went out at `rate`.
fn single_outcome(
    answer: SingleAnswer,
    last: Option<(NonZeroU16, PhyRate)>,
    rate: PhyRate,
    cause: Option<WithdrawCause>,
) -> (ModelSingleOutcome, Option<(NonZeroU16, PhyRate)>) {
    let before = last.map(|(transmissions, rate)| Air {
        transmissions,
        rate,
    });
    let transmitted = Air {
        transmissions: last.map_or(NonZeroU16::MIN, |(count, _)| count.saturating_add(1)),
        rate,
    };
    let ack = ModelAck {
        snr_db: MODEL_ACK_SNR_DB,
    };
    let delivered = |response| {
        TxOutcome::Ended(SingleEnd::Delivered {
            air: transmitted,
            response,
        })
    };
    let went_out = Some((transmitted.transmissions, rate));
    match (answer, cause) {
        (SingleAnswer::HardwareTimeout, _) => (
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
            last,
        ),
        (SingleAnswer::AirDenied | SingleAnswer::Collision, Some(cause)) => {
            (TxOutcome::Withdrawn(cause), last)
        }
        (SingleAnswer::Unanswered, Some(cause)) => (TxOutcome::Withdrawn(cause), went_out),
        (SingleAnswer::AirDenied, None) => (
            TxOutcome::Ended(SingleEnd::AirDenied {
                denial: ModelAirDenied,
                air: before,
            }),
            last,
        ),
        (SingleAnswer::Collision, None) => {
            (TxOutcome::Ended(SingleEnd::ContentionLimit(before)), last)
        }
        (SingleAnswer::Unanswered, None) => (
            TxOutcome::Ended(SingleEnd::AckTimeout(transmitted)),
            went_out,
        ),
        (SingleAnswer::Unsolicited, _) => (delivered(SingleResponse::Unsolicited), went_out),
        (SingleAnswer::Ack, _) => (delivered(SingleResponse::Ack(ack)), went_out),
        (SingleAnswer::BlockAck(report), _) => (
            delivered(SingleResponse::BlockAck { report, ack }),
            went_out,
        ),
    }
}

/// An aggregate's outcome after `answer`, published once at `rate`.
fn aggregate_outcome(
    answer: AggregateAnswer,
    rate: PhyRate,
    report: BlockAckReport,
    cause: Option<WithdrawCause>,
) -> ModelAggregateOutcome {
    let air = Air {
        transmissions: NonZeroU16::MIN,
        rate,
    };
    match (answer, cause) {
        (AggregateAnswer::HardwareTimeout, _) => {
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
        }
        (AggregateAnswer::BlockAck { evidence, .. }, _) => {
            TxOutcome::Ended(AggregateEnd::Delivered {
                air,
                report,
                ack: evidence,
            })
        }
        (
            AggregateAnswer::Unanswered | AggregateAnswer::Collision | AggregateAnswer::AirDenied,
            Some(cause),
        ) => TxOutcome::Withdrawn(cause),
        (AggregateAnswer::Unanswered, None) => TxOutcome::Ended(AggregateEnd::AckTimeout(air)),
        (AggregateAnswer::Collision, None) => TxOutcome::Ended(AggregateEnd::ContentionLimit(None)),
        (AggregateAnswer::AirDenied, None) => TxOutcome::Ended(AggregateEnd::AirDenied {
            denial: ModelAirDenied,
            air: None,
        }),
    }
}

fn start_work<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    keys: &[KeyIdentity],
    work: Work<'id, X>,
) -> Option<Publication<'id, X>> {
    match work {
        Work::Single(prepared) => {
            let rate = prepared.plan.first().rate;
            let remaining = prepared
                .plan
                .stages()
                .flat_map(|stage| {
                    core::iter::repeat_n(stage.rate, usize::from(stage.transmissions.get()))
                })
                .skip(1)
                .collect();
            publish_single(
                held,
                keys,
                SinglePlan {
                    prepared,
                    remaining,
                    last: None,
                    rate,
                },
            )
        }
        Work::Aggregate(prepared) => publish_aggregate(held, keys, prepared),
    }
}
fn publish_single<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    keys: &[KeyIdentity],
    mut plan: SinglePlan<'id, X>,
) -> Option<Publication<'id, X>> {
    let answer = match held
        .air
        .single_result(plan.prepared.operation.payload().response)
    {
        Ok(answer) => answer,
        Err(AnswerShapeMismatch) => {
            let _fault = held.quarantined.push(Work::Single(plan.prepared));
            held.poison();
            return None;
        }
    };
    if plan
        .prepared
        .operation
        .execute()
        .publish(
            plan.prepared.role,
            plan.prepared.key.as_ref().map(|key| &key.identity),
        )
        .is_err()
    {
        let _fault = held.quarantined.push(Work::Single(plan.prepared));
        held.poison();
        return None;
    }
    if plan.last.is_some() {
        plan.prepared.operation.execute().retry();
    }
    let frame = match plan.prepared.operation.payload().on_air(
        plan.prepared.role,
        keys,
        plan.prepared.key.as_ref().map(|key| &key.identity),
        plan.rate,
    ) {
        Ok(frame) => frame,
        Err(_) => {
            let _fault = held.quarantined.push(Work::Single(plan.prepared));
            held.poison();
            return None;
        }
    };
    held.air
        .edca
        .borrow_mut()
        .draw(plan.prepared.role, plan.prepared.queue);
    // A denied PPDU never reached the air the journal records.
    if !matches!(answer, SingleAnswer::AirDenied) {
        held.air.sent.borrow_mut().push(frame);
    }
    Some(Publication {
        started: held.air.now_micros.get(),
        work: Published::Single { plan, answer },
    })
}

/// Preserve the submission shape and every owner on withdrawal.
fn withdrawn<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    work: Work<'id, X>,
    cause: WithdrawCause,
) {
    match work {
        Work::Single(prepared) => {
            terminal(held, *prepared, TxOutcome::Withdrawn(cause));
        }
        Work::Aggregate(aggregate) => {
            terminal_aggregate(held, aggregate, TxOutcome::Withdrawn(cause))
        }
    }
}

fn terminal_aggregate<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    aggregate: PreparedAggregate<'id, X>,
    completion: ModelAggregateOutcome,
) {
    held.post(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
        TxCompleted::Aggregate(aggregate.operation.complete(completion)),
    )));
}

fn publish_aggregate<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    keys: &[KeyIdentity],
    mut aggregate: PreparedAggregate<'id, X>,
) -> Option<Publication<'id, X>> {
    let answer = match held.air.aggregate_result() {
        Ok(answer) => answer,
        Err(AnswerShapeMismatch) => {
            let _fault = held.quarantined.push(Work::Aggregate(aggregate));
            held.poison();
            return None;
        }
    };
    let acknowledged = match answer {
        AggregateAnswer::BlockAck { bitmap, .. } => bitmap,
        AggregateAnswer::Unanswered
        | AggregateAnswer::Collision
        | AggregateAnswer::AirDenied
        | AggregateAnswer::HardwareTimeout => 0,
    };
    let frames = match aggregate.operation.execute().publish(
        aggregate.role,
        keys,
        aggregate.key.as_ref().map(|key| &key.identity),
        aggregate.rate,
    ) {
        Ok(frames) => frames,
        Err(_) => {
            let _fault = held.quarantined.push(Work::Aggregate(aggregate));
            held.poison();
            return None;
        }
    };
    let mut bitmap = 0_u64;
    for (index, subframe) in aggregate.operation.payload().0.iter().enumerate() {
        if acknowledged & (1 << index) != 0 {
            bitmap |= subframe.acknowledgement_bit;
        }
    }
    held.air
        .edca
        .borrow_mut()
        .draw(aggregate.role, aggregate.queue);
    if !matches!(answer, AggregateAnswer::AirDenied) {
        held.air.sent.borrow_mut().extend(frames);
    }
    // NonEmptyAggregate carries the first sequence into PreparedAggregate below.
    let report = BlockAckReport {
        start_sequence: aggregate.start_sequence,
        bitmap,
    };
    Some(Publication {
        started: held.air.now_micros.get(),
        work: Published::Aggregate {
            prepared: aggregate,
            report,
            answer,
        },
    })
}

/// Post the terminal of `prepared`, with its tokens.
fn terminal<'id, X: ModelContext>(
    held: &ModelHeld<'id, X>,
    prepared: Prepared<'id, X>,
    outcome: ModelSingleOutcome,
) {
    held.post(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
        TxCompleted::Single(prepared.operation.complete(outcome)),
    )));
}

/// The model's running phase: the body's handle over the queues.
pub struct ModelPhase<'a, 'id, C = ModelCaps, X: ModelContext = ()> {
    backend: &'a mut ModelBackend<C, X>,
    held: &'a ModelHeld<'id, X>,
}

impl<'id, C, X: ModelContext> ModelPhase<'_, 'id, C, X> {
    /// Deliberately violate the backend contract while retaining actual minted
    /// owners, for consumer contract regressions outside the conformance suite.
    pub fn inject_terminal(&mut self, terminal: LowerMacTerminal<'id, ModelTypes<X>>) {
        self.held.post(LowerMacEvent::Terminal(terminal));
    }
}

impl<'id, C, X: ModelContext> PortPhase for ModelPhase<'_, 'id, C, X> {
    type Event = LowerMacEvent<'id, ModelTypes<X>>;
    type Fault = ModelFault;

    fn next_event(
        &mut self,
    ) -> impl Future<Output = Result<Result<Self::Event, EventsLost>, EventEnd<ModelFault>>> + '_
    {
        let held = self.held;
        // Cancel-safe: the event stays queued until a poll takes it.
        poll_fn(move |cx| {
            if let ModelStream::Ended(end) = held.air.stream.get() {
                return Poll::Ready(Err(end));
            }
            if let Some(event) = held.take_event() {
                return Poll::Ready(Ok(Ok(event)));
            }
            if held.lost.replace(false) {
                return Poll::Ready(Ok(Err(EventsLost)));
            }
            if matches!(held.air.stream.get(), ModelStream::Draining) {
                let driver = held.driver_state.borrow();
                let returned = driver
                    .queues
                    .iter()
                    .all(|queue| queue.air.is_none() && queue.waiting.is_empty())
                    && driver.removing.is_empty()
                    && !matches!(driver.gate, DriverGate::Closing(_));
                if returned
                    && !held.draining.get()
                    && held.commands.is_empty()
                    && held.recycled.is_empty()
                {
                    let end = EventEnd::Drained { cause: ModelFault };
                    held.air.stream.set(ModelStream::Ended(end));
                    held.air.wake_driver();
                    return Poll::Ready(Err(end));
                }
                *held.body.borrow_mut() = Some(cx.waker().clone());
                held.air.wake_driver();
                return Poll::Pending;
            }
            if let Some((bytes, meta)) = held.air.rx_stream.borrow().as_ref()
                && held.charge_rx()
            {
                return Poll::Ready(Ok(Ok(LowerMacEvent::Received {
                    frame: ModelRxBuffer {
                        bytes: bytes.clone(),
                        recycled: false,
                        air: Rc::downgrade(&held.air),
                        streamed: true,
                    },
                    meta: *meta,
                })));
            }
            *held.body.borrow_mut() = Some(cx.waker().clone());
            Poll::Pending
        })
    }
}

impl<'p, 'id: 'p, C: Ieee80211Capabilities, X: ModelContext> LowerMacPhase<'p, 'id>
    for ModelPhase<'p, 'id, C, X>
{
    type Caps = C;
    type Types = ModelTypes<X>;
    type Station<'v>
        = ModelInterface<'p, 'v, 'id, Station, C, X>
    where
        Self: 'v;
    type AccessPoint<'v>
        = ModelInterface<'p, 'v, 'id, AccessPoint, C, X>
    where
        Self: 'v;

    fn channel(&self) -> Channel {
        self.backend.channel
    }

    fn recycle(&mut self, buffer: ModelRxBuffer) {
        if buffer.streamed {
            let mut buffer = buffer;
            // The independent source observes this return immediately. Its
            // next poll can reuse the same bounded RX capacity without a
            // body/driver task switch, exactly the starvation case.
            buffer.recycled = true;
            self.held.release_rx();
            return;
        }
        if self.held.recycled.push(buffer).is_err() {
            self.held.poison();
        }
        self.held.air.wake_driver();
    }

    fn close_gate(&mut self, gate: GateOpen<'id, ModelPool<0>>) {
        self.held.command(Command::CloseGate(gate));
    }

    fn open_gate(&mut self, gate: GateClosed<'id, ModelPool<0>>) -> GateOpen<'id, ModelPool<0>> {
        let open = self.held.gate_issuer.borrow_mut().open(gate);
        self.held.command(Command::OpenGate);
        open
    }

    fn station(&mut self) -> ModelInterface<'p, '_, 'id, Station, C, X> {
        let [station, _] = self.backend.addresses;
        ModelInterface {
            held: self.held,
            address: station,
            role: PhantomData,
        }
    }

    fn access_point(&mut self) -> ModelInterface<'p, '_, 'id, AccessPoint, C, X> {
        let [_, access_point] = self.backend.addresses;
        ModelInterface {
            held: self.held,
            address: access_point,
            role: PhantomData,
        }
    }
}

/// One interface of the model's phase.
pub struct ModelInterface<'p, 'v, 'id, R, C = ModelCaps, X: ModelContext = ()> {
    held: &'v ModelHeld<'id, X>,
    address: IndividualAddress,
    role: PhaseProof<'p, (R, C)>,
}

/// A key the model installed.
pub struct ModelPairwiseKey<'id, R: ModelRole> {
    identity: KeyIdentity,
    slot: PairwiseKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>,
}

/// A group key the model installed.
pub struct ModelGroupKey<'id, R: ModelRole> {
    identity: KeyIdentity,
    slot: GroupKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>,
}

/// A receive Block Ack agreement the model keeps.
pub struct ModelBlockAck<'id> {
    slot: ModelBlockAckSlot<'id>,
}

/// A retry plan's stages, as the model's hardware takes them.
fn stages<C: Ieee80211Capabilities>(
    frame: &TxFrame<'_, '_, C>,
) -> oer_ieee80211_port::RetryPlan<PhyRate> {
    match frame {
        TxFrame::Individual { plan, .. } => plan.map(|rate| rate.get()),
        TxFrame::IndividualNoAck { rate, .. } => one_stage(rate.rate().get()),
        TxFrame::Group { rate, .. } => one_stage(rate.get()),
        TxFrame::BlockAckRequest { plan, .. } => plan.map(|rate| rate.rate().get()),
    }
}
fn one_stage(rate: PhyRate) -> oer_ieee80211_port::RetryPlan<PhyRate> {
    oer_ieee80211_port::RetryPlan::new(oer_ieee80211_port::RetryStage {
        rate,
        transmissions: core::num::NonZeroU8::MIN,
        protection: oer_ieee80211_port::Protection::None,
    })
}

fn retry_stages<K>(
    layout: &MpduLayout,
    plan: &RetransmissionPlan<'_, '_, K>,
) -> Result<oer_ieee80211_port::RetryPlan<PhyRate>, PreparationError> {
    match (layout.kind(), plan) {
        (MpduKind::Individual, RetransmissionPlan::Individual(plan)) => {
            Ok(plan.map(|rate| rate.get()))
        }
        (MpduKind::IndividualNoAck, RetransmissionPlan::IndividualNoAck(rate)) => {
            Ok(one_stage(rate.rate().get()))
        }
        (MpduKind::Group, RetransmissionPlan::Group(rate)) => Ok(one_stage(rate.get())),
        _ => Err(PreparationError::RetransmissionKind),
    }
}

type PhaseProof<'p, T> = PhantomData<fn(&'p ()) -> (&'p (), T)>;
type TypeProof<T> = PhantomData<fn() -> T>;
type ModelSource<'k, 'p, 'id, C, K, R, X> = TxSource<
    ModelBuffer<'id, C>,
    ModelSent<'id, C, <X as ModelContext>::Body>,
    oer_ieee80211_port::ClassFrame<'p, 'id, C, K>,
    <X as ModelContext>::Body,
    <R as RoleKeys<'id, ModelTypes<X>>>::TxKey<'k>,
    RetransmissionPlan<'p, 'id, K>,
>;
type ModelCheckedSource<'k, 'p, 'id, C, K, R, X> = CheckedTxSource<
    ModelBuffer<'id, C>,
    ModelSent<'id, C, <X as ModelContext>::Body>,
    oer_ieee80211_port::ClassFrame<'p, 'id, C, K>,
    <X as ModelContext>::Body,
    <R as RoleKeys<'id, ModelTypes<X>>>::TxKey<'k>,
    RetransmissionPlan<'p, 'id, K>,
>;
type ModelPreparation<'k, 'p, 'id, R, K, X, C> = Result<
    ModelPrepared<'k, 'p, 'id, R, K, X>,
    PreparationRefused<ModelSource<'k, 'p, 'id, C, K, R, X>>,
>;

/// Closed preparation proof retaining role, phase, install and the key borrow.
/// Dropping or submitting it ends that borrow; a queued operation owns only
/// the backend identity, never a reference to a portable machine.
pub struct ModelPrepared<'k, 'p, 'id, R, K, X: ModelContext = ()> {
    mpdu: ModelMpdu<'id, X::Body>,
    plan: oer_ieee80211_port::RetryPlan<PhyRate>,
    key: Option<PreparedKey>,
    key_borrow: PhantomData<&'k ()>,
    proof: PhaseProof<'p, (R, K, X)>,
}
/// Metadata lowered before single-MPDU admission; it owns no payload.
pub struct ModelTxExecution {
    plan: oer_ieee80211_port::RetryPlan<PhyRate>,
    key: Option<PreparedKey>,
}
impl<'id, R, K, X: ModelContext> PreparedTx<'id> for ModelPrepared<'_, '_, 'id, R, K, X> {
    type Types = ModelTypes<X>;
    type Execution = ModelTxExecution;
    fn into_admission(self) -> (ModelMpdu<'id, X::Body>, Self::Execution) {
        (
            self.mpdu,
            ModelTxExecution {
                plan: self.plan,
                key: self.key,
            },
        )
    }
    fn into_parts(self) -> (ModelAnyBuffer<'id, X::Body>, Option<X::Body>) {
        self.mpdu.into_parts()
    }
}

/// A model buffer class and its unique pool within the installation layout.
pub trait ModelClass: TxBufferClass + Sized {
    /// Nominal component of the backing memory slots.
    type Path: oer_radio_port::PoolPath;
    /// Return fresh memory through the terminal's exhaustive buffer enum.
    fn erase<'id, D>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id, D>;
    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent<'id, D>(mpdu: ModelSent<'id, Self, D>) -> ModelAnyBuffer<'id, D>;
    /// Recover fresh memory of this class, or preserve the other owner.
    fn class<'id, D>(
        buffer: ModelAnyBuffer<'id, D>,
    ) -> Result<ModelBuffer<'id, Self>, ModelAnyBuffer<'id, D>>;
    /// Recover a published MPDU of this class, or preserve the other owner.
    fn sent<'id, D>(
        buffer: ModelAnyBuffer<'id, D>,
    ) -> Result<ModelSent<'id, Self, D>, ModelAnyBuffer<'id, D>>;
}

impl ModelClass for ModelSmall {
    type Path = ModelPool<2>;
    fn erase<'id, D>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id, D> {
        ModelAnyBuffer::Small(buffer)
    }
    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent<'id, D>(mpdu: ModelSent<'id, Self, D>) -> ModelAnyBuffer<'id, D> {
        ModelAnyBuffer::SmallSent(mpdu)
    }
    /// Recover fresh memory of this class, or preserve the other owner.
    fn class<'id, D>(
        buffer: ModelAnyBuffer<'id, D>,
    ) -> Result<ModelBuffer<'id, Self>, ModelAnyBuffer<'id, D>> {
        match buffer {
            ModelAnyBuffer::Small(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
    /// Recover a published MPDU of this class, or preserve the other owner.
    fn sent<'id, D>(
        buffer: ModelAnyBuffer<'id, D>,
    ) -> Result<ModelSent<'id, Self, D>, ModelAnyBuffer<'id, D>> {
        match buffer {
            ModelAnyBuffer::SmallSent(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
}
impl ModelClass for ModelLarge {
    type Path = ModelPool<3>;
    fn erase<'id, D>(buffer: ModelBuffer<'id, Self>) -> ModelAnyBuffer<'id, D> {
        ModelAnyBuffer::Large(buffer)
    }
    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent<'id, D>(mpdu: ModelSent<'id, Self, D>) -> ModelAnyBuffer<'id, D> {
        ModelAnyBuffer::LargeSent(mpdu)
    }
    /// Recover fresh memory of this class, or preserve the other owner.
    fn class<'id, D>(
        buffer: ModelAnyBuffer<'id, D>,
    ) -> Result<ModelBuffer<'id, Self>, ModelAnyBuffer<'id, D>> {
        match buffer {
            ModelAnyBuffer::Large(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
    /// Recover a published MPDU of this class, or preserve the other owner.
    fn sent<'id, D>(
        buffer: ModelAnyBuffer<'id, D>,
    ) -> Result<ModelSent<'id, Self, D>, ModelAnyBuffer<'id, D>> {
        match buffer {
            ModelAnyBuffer::LargeSent(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
}

#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::result_large_err,
        reason = "a refusal returns the whole operation by value"
    )
)]
fn prepare_model<
    'k,
    'p,
    'id: 'k,
    R: RoleKeys<'id, ModelTypes<X>>,
    K: Ieee80211Capabilities,
    X: ModelContext,
    C: ModelClass,
>(
    source: ModelCheckedSource<'k, 'p, 'id, C, K, R, X>,
) -> ModelPreparation<'k, 'p, 'id, R, K, X, C> {
    let (mpdu, plan, key) = match source {
        CheckedTxSource::Fresh(fresh) => {
            let (
                FreshSource {
                    buffer,
                    frame,
                    body,
                    protection,
                },
                layout,
            ) = fresh.into_parts();
            let (key, packet_number) = match protection {
                TxProtection::Open => (None, None),
                TxProtection::Ccmp(numbered) => (
                    Some(R::tx_identity(&numbered.key)),
                    Some(numbered.packet_number),
                ),
            };
            let plan = stages(frame.frame());
            let response = ResponsePolicy::of_frame(frame.frame());
            (
                ModelMpdu {
                    buffer: C::erase(buffer),
                    layout,
                    response,
                    body,
                    published: None,
                    packet_number,
                },
                plan,
                key,
            )
        }
        CheckedTxSource::Retransmission { mpdu, plan } => {
            let checked = if mpdu.role != R::ROLE {
                Err(PreparationError::RetransmissionKind)
            } else if mpdu.key.as_ref().is_some_and(|key| !key.0.active.get()) {
                Err(PreparationError::RetransmissionKey)
            } else {
                retry_stages(&mpdu.layout, &plan)
            };
            let stages = match checked {
                Ok(stages) => stages,
                Err(reason) => {
                    return Err(PreparationRefused {
                        reason,
                        operation: TxSource::Retransmission { mpdu, plan },
                    });
                }
            };
            let key = mpdu.key.clone();
            let layout = mpdu.layout;
            let response = mpdu.response;
            (
                ModelMpdu::from_sent(C::erase_sent(mpdu), layout, response),
                stages,
                key,
            )
        }
    };
    Ok(ModelPrepared {
        mpdu,
        plan,
        key: key.map(PreparedKey::new),
        key_borrow: PhantomData,
        proof: PhantomData,
    })
}

impl<'p, 'v, 'id: 'p, R, K: Ieee80211Capabilities, X: ModelContext> InterfacePhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, R, K, X>
where
    R: GroupInstallationRole + RoleKeys<'id, ModelTypes<X>, Keys = ModelKeys<'id, R>>,
{
    type Caps = K;
    type Types = ModelTypes<X>;
    type Role = R;

    fn address(&self) -> IndividualAddress {
        self.address
    }

    /// The model applies each interface's set exactly.
    fn set_edca(&mut self, parameters: SupportedEdca<K>) -> SupportedEdca<K> {
        *self.held.air.edca.borrow_mut().parameters_mut(R::ROLE) = parameters.get();
        self.held.settings.borrow_mut().other = true;
        self.held.air.wake_driver();
        parameters
    }

    fn submit<'k>(
        &mut self,
        attempt: oer_ieee80211_port::InterfaceSubmission<'k, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<ModelFault>> {
        let ModelTxExecution { plan, key } = attempt.execution;
        self.held.command(Command::Submit(Box::new(Prepared {
            operation: attempt.operation,
            role: R::ROLE,
            queue: attempt.access_category,
            plan,
            key,
        })));
        if self.held.air.admission_closed() {
            Err(Poisoned { cause: ModelFault })
        } else {
            Ok(())
        }
    }

    fn install_pairwise_key(
        &mut self,
        association: &ModelAssociation<'id, R>,
        slot: PairwiseKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> ModelPairwiseKey<'id, R> {
        let identity = KeyIdentity::new(slot.index(), 0, R::ROLE, peer, &self.held.air);
        self.held.command(Command::InstallKey {
            spp: association.owner.spp(),
            material: key,
            key: identity.clone(),
        });
        ModelPairwiseKey { identity, slot }
    }

    fn install_group_key(
        &mut self,
        association: &ModelAssociation<'id, R>,
        permission: Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath>,
        slot: GroupKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.now_micros.get(),
            change: GroupChange::Install(R::installation(PendingGroupInstall {
                spp: association.owner.spp(),
                slot,
                permission,
                key_id,
                key,
            })),
        });
    }

    fn remove_pairwise_key(
        &mut self,
        association: ModelAssociation<'id, R>,
        key: ModelPairwiseKey<'id, R>,
    ) {
        key.identity.0.active.set(false);
        self.held.command(Command::RemoveKey {
            key: key.identity,
            removed: R::removed(RemovedKey::Pairwise {
                slot: key.slot,
                association,
            }),
        });
    }

    fn remove_group_key(
        &mut self,
        permission: Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath>,
        key: ModelGroupKey<'id, R>,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.now_micros.get(),
            change: GroupChange::Remove {
                key: key.identity,
                removed: R::removed(RemovedKey::Group {
                    slot: key.slot,
                    permission,
                }),
            },
        });
    }

    fn add_block_ack(
        &mut self,
        slot: ModelBlockAckSlot<'id>,
        agreement: SupportedBlockAck<K>,
    ) -> ModelBlockAck<'id> {
        self.held.settings.borrow_mut().block_acks.insert(
            slot.index(),
            Some(ModelReceiveAgreement::checked(R::ROLE, agreement)),
        );
        self.held.air.wake_driver();
        ModelBlockAck { slot }
    }

    fn replace_block_ack(
        &mut self,
        running: ModelBlockAck<'id>,
        agreement: SupportedBlockAck<K>,
    ) -> ModelBlockAck<'id> {
        self.held.settings.borrow_mut().block_acks.insert(
            running.slot.index(),
            Some(ModelReceiveAgreement::checked(R::ROLE, agreement)),
        );
        self.held.air.wake_driver();
        running
    }

    fn remove_block_ack(&mut self, agreement: ModelBlockAck<'id>) -> ModelBlockAckSlot<'id> {
        self.held
            .settings
            .borrow_mut()
            .block_acks
            .insert(agreement.slot.index(), None);
        self.held.air.wake_driver();
        agreement.slot
    }
}

/// The subframes one model A-MPDU holds.
pub const MODEL_AMPDU_SUBFRAMES: usize = 8;

/// Builder retaining receiver, TID, role and one installed protection.
/// Only `push_aggregate` can create the nonempty state accepted by submission.
///
/// Role, phase and install stay in the builder through submission:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_model::ModelAggregate;
/// use oer_ieee80211_port::{Station, AccessPoint};
/// fn role<'k, 'p, 'id>(a: ModelAggregate<'k, 'p, 'id, Station>) -> ModelAggregate<'k, 'p, 'id, AccessPoint> { a }
/// ```
///
/// ```compile_fail
/// use oer_ieee80211_model::ModelAggregate;
/// use oer_ieee80211_port::Station;
/// fn phase<'k, 'p, 'q, 'id>(a: ModelAggregate<'k, 'p, 'id, Station>) -> ModelAggregate<'k, 'q, 'id, Station> { a }
/// ```
///
/// ```compile_fail
/// use oer_ieee80211_model::ModelAggregate;
/// use oer_ieee80211_port::Station;
/// fn install<'k, 'p, 'id, 'other>(a: ModelAggregate<'k, 'p, 'id, Station>) -> ModelAggregate<'k, 'p, 'other, Station> { a }
/// ```
///
/// ```
/// use oer_ieee80211_model::ModelAggregate;
/// fn same<'k, 'p, 'id, R>(a: ModelAggregate<'k, 'p, 'id, R>) -> ModelAggregate<'k, 'p, 'id, R> { a }
/// ```
pub struct ModelAggregate<
    'k,
    'p,
    'id,
    R,
    K = ModelCaps,
    X: ModelContext = (),
    S: AggregateState = EmptyAggregate,
> {
    subframes: Vec<Subframe<'id, X::Body>>,
    first: S::Head<SequenceNumber>,
    receiver: IndividualAddress,
    tid: Tid,
    rate: AmpduRate<'p, 'id, K>,
    key: Option<PreparedKey>,
    key_borrow: PhantomData<&'k ()>,
    caps: TypeProof<(R, K, X, S)>,
}
impl<R, K, X: ModelContext, S: AggregateState> ModelAggregate<'_, '_, '_, R, K, X, S> {
    fn acknowledgement_bit(&self, sequence: SequenceNumber) -> Option<u64> {
        let first = S::retain_head(&self.first, sequence);
        let distance = first.forward_distance(sequence);
        let bit = 1_u64.checked_shl(u32::from(distance))?;
        (!self
            .subframes
            .iter()
            .any(|subframe| subframe.acknowledgement_bit == bit))
        .then_some(bit)
    }
}
/// Metadata lowered before aggregate admission; subframes stay in the sealed owner.
pub struct ModelAggregateExecution {
    first: SequenceNumber,
    tid: Tid,
    rate: PhyRate,
    key: Option<PreparedKey>,
}
impl<'id, R, K, X: ModelContext, S: AggregateState> PhaseAggregate<'id>
    for ModelAggregate<'_, '_, 'id, R, K, X, S>
{
    type Types = ModelTypes<X>;
    type State = S;
    type Execution = ModelAggregateExecution;
    fn into_admission(self) -> (ModelSubframes<'id, X::Body>, Self::Execution)
    where
        S: oer_ieee80211_port::NonEmptyState,
    {
        (
            ModelSubframes(self.subframes),
            ModelAggregateExecution {
                first: S::into_head(self.first),
                tid: self.tid,
                rate: self.rate.rate().get(),
                key: self.key,
            },
        )
    }
    type Cancelled = ModelSubframeIter<'id, X::Body>;
    fn receiver(&self) -> IndividualAddress {
        self.receiver
    }
    fn tid(&self) -> Tid {
        self.tid
    }
    fn protection(&self) -> oer_ieee80211_port::MpduProtection {
        match &self.key {
            None => oer_ieee80211_port::MpduProtection::Open,
            Some(_) => oer_ieee80211_port::MpduProtection::Ccmp,
        }
    }
    fn subframes(&self) -> usize {
        self.subframes.len()
    }
    fn into_parts(self) -> Self::Cancelled {
        ModelSubframeIter(self.subframes.into_iter())
    }
}
type ModelSubframe<'id, C, K, D> =
    SubframeSource<ModelBuffer<'id, C>, ModelSent<'id, C, D>, ClassSubframe<C, K>, D>;
type ModelCheckedAggregate<'k, 'p, 'id, R, K, X, S, C> = CheckedAggregate<
    ModelAggregate<'k, 'p, 'id, R, K, X, S>,
    ModelBuffer<'id, C>,
    ModelSent<'id, C, <X as ModelContext>::Body>,
    ClassSubframe<C, K>,
    <X as ModelContext>::Body,
>;
type ModelAggregatePush<'k, 'p, 'id, R, K, X, S, C> = Result<
    ModelAggregate<'k, 'p, 'id, R, K, X, NonEmptyAggregate>,
    AggregateRefused<
        ModelAggregate<'k, 'p, 'id, R, K, X, S>,
        ModelSubframe<'id, C, K, <X as ModelContext>::Body>,
    >,
>;

#[cfg_attr(
    target_pointer_width = "64",
    expect(
        clippy::result_large_err,
        reason = "a refusal returns the whole operation by value"
    )
)]
fn push_model<
    'k,
    'p,
    'id,
    R: oer_ieee80211_port::InterfaceRole,
    K: Ieee80211Capabilities,
    X: ModelContext,
    S: AggregateState,
    C: ModelClass,
>(
    operation: ModelCheckedAggregate<'k, 'p, 'id, R, K, X, S, C>,
) -> ModelAggregatePush<'k, 'p, 'id, R, K, X, S, C> {
    let (mut aggregate, source) = operation.into_parts();
    let refused = if aggregate.subframes.len() >= MODEL_AMPDU_SUBFRAMES {
        Some(PreparationError::AggregateFull)
    } else if let CheckedSubframe::Sent(mpdu) = &source {
        if mpdu.key.as_ref() != aggregate.key.as_ref().map(|key| &key.identity)
            || mpdu.key.as_ref().is_some_and(|key| !key.0.active.get())
        {
            Some(PreparationError::RetransmissionKey)
        } else if mpdu.role != R::ROLE
            || mpdu.layout.recipient() != Some((aggregate.receiver, aggregate.tid))
        {
            Some(PreparationError::RetransmissionKind)
        } else {
            None
        }
    } else {
        None
    };
    if let Some(reason) = refused {
        return Err(AggregateRefused {
            reason,
            aggregate,
            subframe: source.into_source(),
        });
    }
    let (mpdu, acknowledgement_bit) = match source {
        CheckedSubframe::Fresh(fresh) => {
            let (
                FreshSubframe {
                    numbering,
                    buffer,
                    frame,
                    body,
                },
                layout,
            ) = fresh.into_parts();
            let Some(bit) = aggregate.acknowledgement_bit(buffer.memory.sequence()) else {
                return Err(AggregateRefused {
                    reason: PreparationError::BlockAckWindow,
                    aggregate,
                    subframe: SubframeSource::Fresh {
                        numbering,
                        buffer,
                        frame,
                        body,
                    },
                });
            };
            (
                ModelMpdu {
                    buffer: C::erase(buffer),
                    layout,
                    response: ResponsePolicy::Ack,
                    body,
                    published: None,
                    packet_number: numbering.packet_number(),
                },
                bit,
            )
        }
        CheckedSubframe::Sent(mpdu) => {
            let Some(bit) = aggregate.acknowledgement_bit(mpdu.buffer.memory.sequence()) else {
                return Err(AggregateRefused {
                    reason: PreparationError::BlockAckWindow,
                    aggregate,
                    subframe: SubframeSource::Sent(mpdu),
                });
            };
            let layout = mpdu.layout;
            let response = mpdu.response;
            (
                ModelMpdu::from_sent(C::erase_sent(mpdu), layout, response),
                bit,
            )
        }
    };
    let first = S::retain_head(&aggregate.first, mpdu.buffer.sequence());
    aggregate.subframes.push(Subframe {
        mpdu,
        acknowledgement_bit,
    });
    let ModelAggregate {
        subframes,
        receiver,
        tid,
        rate,
        key,
        ..
    } = aggregate;
    Ok(ModelAggregate {
        first,
        subframes,
        receiver,
        tid,
        rate,
        key,
        key_borrow: PhantomData,
        caps: PhantomData,
    })
}

impl<'p, 'v, 'id: 'p, R, K: Ieee80211Capabilities, X: ModelContext> AmpduPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, R, K, X>
where
    R: GroupInstallationRole + RoleKeys<'id, ModelTypes<X>, Keys = ModelKeys<'id, R>>,
{
    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        AmpduCapabilities {
            max_subframes: MODEL_AMPDU_SUBFRAMES as u16,
            formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
            max_length: 65_535,
        }
    }
    fn aggregate<'k>(
        &mut self,
        receiver: IndividualAddress,
        tid: Tid,
        rate: AmpduRate<'p, 'id, K>,
        protection: InterfaceProtection<'k, 'p, 'id, Self>,
    ) -> ModelAggregate<'k, 'p, 'id, R, K, X>
    where
        'id: 'k,
    {
        let key = match &protection {
            TxProtection::Open => None,
            TxProtection::Ccmp(key) => Some(R::tx_identity(key)),
        };
        ModelAggregate {
            subframes: Vec::new(),
            first: (),
            receiver,
            tid,
            rate,
            key: key.map(PreparedKey::new),
            key_borrow: PhantomData,
            caps: PhantomData,
        }
    }
    fn submit_ampdu<'k>(
        &mut self,
        attempt: oer_ieee80211_port::InterfaceAggregateSubmission<'k, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<ModelFault>>
    where
        'id: 'k,
    {
        let ModelAggregateExecution {
            first,
            tid,
            rate,
            key,
        } = attempt.execution;
        let queue = match oer_ieee80211_wire::qos::WmmUserPriority::new(tid.get()) {
            Some(priority) => priority.access_category(),
            None => oer_ieee80211_wire::qos::WmmAccessCategory::BestEffort,
        };
        self.held.command(Command::SubmitAmpdu(PreparedAggregate {
            operation: attempt.operation,
            role: R::ROLE,
            start_sequence: first,
            queue,
            rate,
            key,
        }));
        if self.held.air.admission_closed() {
            Err(Poisoned { cause: ModelFault })
        } else {
            Ok(())
        }
    }
}

impl<'p, 'v, 'id: 'p, C: Ieee80211Capabilities, X: ModelContext> StationPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, Station, C, X>
{
    fn replace_group_key(
        &mut self,
        permission: StationGroupPermission<'id>,
        installed: ModelGroupKey<'id, Station>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.now_micros.get(),
            change: GroupChange::Replace {
                permission,
                installed,
                key_id,
                key,
            },
        });
    }

    fn select_group_key(
        &mut self,
        permission: StationGroupPermission<'id>,
        installed: ModelGroupKey<'id, Station>,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.now_micros.get(),
            change: GroupChange::Select {
                permission,
                installed,
            },
        });
    }

    fn configure(
        &mut self,
        permission: ConfigurePermission<
            'id,
            Station,
            oer_radio_port::Child<ModelPool<9>, 1>,
            X::StationConfigureContext<'id>,
        >,
        _: StationReceive<C>,
    ) {
        self.held
            .command(Command::Configure(Configured::Station(permission)));
    }

    fn set_bss_color(&mut self, _: BssColor) {
        self.held.settings.borrow_mut().other = true;
        self.held.air.wake_driver();
    }
}

impl<'p, 'v, 'id: 'p, C: Ieee80211Capabilities, X: ModelContext> AccessPointPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, AccessPoint, C, X>
{
    fn configure(
        &mut self,
        permission: ConfigurePermission<
            'id,
            AccessPoint,
            oer_radio_port::Child<ModelPool<9>, 2>,
            X::AccessPointConfigureContext<'id>,
        >,
        _: SupportedFilter<C, AccessPoint>,
    ) {
        self.held
            .command(Command::Configure(Configured::AccessPoint(permission)));
    }
}

#[cfg(test)]
mod tests;

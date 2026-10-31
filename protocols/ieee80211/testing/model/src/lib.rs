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
    clippy::disallowed_macros,
    clippy::disallowed_methods
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
        clippy::disallowed_methods,
    )
)]

//! An in-memory lower-MAC backend on the phase shape
//! (`oer_ieee80211_contracts_port::phase`), for host tests of the shape and of
//! the code that drives it.
//!
//! [`ModelBackend::new`] takes the install's channel, interface addresses
//! and monotonic clock, and returns the P2 owner and the [`ModelAir`] it
//! plays on. A caller places its generative local slot outside the frontend
//! future, registers it with `LocalExecution::scope`, and calls `run_port`
//! with that frontend. The air answers each transmission, delivers received frames
//! and records what the hardware saw. Receive Block Ack agreements are
//! per-slot desired state: updates coalesce before the driver applies them,
//! without occupying the command queue. Configure and TSF sampling consume
//! their affine permissions in the FIFO; their terminals return those same
//! permissions, including through disable's drain.
//!
//! The backend has the phase's shape: the phase
//! is the body's handle over queues to the model's driver
//! (`SlotOwner::run`), which alone owns the model's hardware (its
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
//! terminal or cancellation. `KeyOwnedRemoved` waits for these owners; a returned
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
//! `ModelContext` selects owned contexts by slot Id before lending their
//! public body views. A station context retains both its nominal `LinkEpoch`
//! identity and its separate body loan through every queue and terminal. The model grants one epoch-source slot in its resources.
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
//! The model's 1 ms publication bound uses the supplied image clock. Only
//! that clock's owner advances time. Radio instants are projections through
//! an affine, zero-drift relation with a sampled nonzero offset. Hardware
//! backends declare their own PPDU plus response/timeout bound.
//!
//! Unit tests and library consumers use the same implementation,
//! including continuous RX and immediate recycling of its bounded slots. Only the unit-test module is under `cfg(test)`.
//! Runtime's `tests/model_feature.rs` exercises these paths through the public port
//! from a separate test crate, so the model itself is compiled without that cfg.

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::disallowed_methods, clippy::disallowed_macros)]
mod fixture;
#[cfg(any(test, feature = "test-support"))]
use fixture::plain_context;

mod contention;
mod loan;
mod owned;
use owned::*;

extern crate alloc;
#[cfg(test)]
extern crate std;
#[cfg(test)]
use core::cell::RefCell;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    rc::{Rc, Weak},
    vec::Vec,
};
use contention::ModelWindow;
use core::{
    cell::Cell,
    future::{Future, poll_fn},
    marker::PhantomData,
    num::NonZeroU16,
    task::{Poll, Waker},
};
use oer_ieee80211_contracts_port::{EdcaAcParameters, EdcaParameterSet};
use oer_ieee80211_contracts_port::{
    association,
    association::{
        Association, AssociationPool, AssociationStart, BlockAckSlot, BlockAckSlots, GroupKeySlot,
        KeyBankParts, PairwiseKeySlot, VacantAssociation, VacantKeyBank,
    },
};
use oer_ieee80211_contracts_security::Ccmp128Key;
use oer_ieee80211_wire::ccmp::{CcmpHeader, CcmpKeyId, GroupKeyId};
use oer_ieee80211_wire::frame::{sequence_control, set_retry_bit};
use oer_radio_port::{
    BodyLoan, LoanedContext, ModelSlotOwner, SlotBodyOwner, SlotIdentity, SlotOwner, SlotRetire,
    SlotStopped, SlotWakes,
};
use oer_radio_port::{ClockRelation, GenerationSource, RelationSample};
use oer_time::{Clock, Duration, Instant};

use oer_ieee80211_wire::phy::{HeMcs, HtMcs, PhyRate};
use oer_radio_port::{
    Abandon, Call, ClockError, EnableResult, EventEnd, InstallParts, PortBackend, PortPhase,
    Quarantine,
};
use oer_radio_port::{EventsLost, Mint, Poisoned, PortResult, Slot};
use oer_time::RadioInstant;

use oer_ieee80211_contracts_port::{
    AccessPoint, AccessPointPhase, BandSet, BodyPayload, BssColor, Channel, CheckedAggregate,
    CheckedPoolSource, CheckedSubframe, CheckedTxSource, ConfigurePermission, ConnectionEpochKind,
    ExecutionPayload, FreshSource, FreshSubframe, GateClosed, GateIssuer, GateOpen,
    HardwareServices, Ieee80211Capabilities, Ieee80211Radio, InFlight, IndeterminateCause,
    InterfaceCommandPool, InterfacePermissions, InterfacePhase, KeyTypes, LinkMetric,
    LowerMacBackend, LowerMacCapabilities, LowerMacPhase, LowerMacSetup, MpduKind, MpduLayout,
    PhaseTypes, PhyFormatSet, PreparationError, PreparationRefused, PreparedTx, RateSupport,
    ReceiveFilter, RemovedKey, RetransmissionPlan, RoleKeys, RxBuffer, RxEvidence, RxMeta,
    RxMetadata, SignalMetadata, Station, StationPhase, StationReceive, SupportedBlockAck,
    SupportedChannel, SupportedEdca, SupportedFilter, TxBody, TxBufferClass, TxBufferMemory,
    TxBufferPool, TxFrame, TxMpdu, TxOutcome, TxProtection, TxSource, VifRole, WidthSet,
    WithdrawCause,
};
use oer_ieee80211_contracts_port::{
    AckSnr, AggregateEnd, AggregateOutcome, Air, BlockAckSnr, SingleEnd, SingleOutcome,
    SingleResponse, SnrSample,
};
use oer_ieee80211_contracts_port::{
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
pub type ModelBlockAckSlot<'id, B> = BlockAckSlot<
    'id,
    ModelBlockAcks,
    MODEL_BLOCK_ACK_SLOTS,
    ModelPool<<B as ModelComposition>::Identity, 7>,
>;
/// The key pool of one role's association domain.
pub type ModelAssociationPool<R> = AssociationPool<R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS>;

/// FIFO commands: each ticket, an install or remove for every pairwise
/// slot (a removal needs the key only the installation terminal returns),
/// one GTK mutation per interface permission, and close/open of the gate.
/// Configure and TSF each reserve their own FIFO cell and terminal.
/// Receive agreements remain coalesced settings.
pub const MODEL_COMMANDS: usize = MODEL_TICKETS
    + 2 * MODEL_KEY_SLOTS
    + 2
    + 2
    + InterfaceCommandPool::<Station>::COMMAND_CAPACITY
    + InterfaceCommandPool::<AccessPoint>::COMMAND_CAPACITY;
/// Reserved terminals: tickets, a pairwise installation or removal per
/// slot, the two GTK permissions,
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
    items: OwnedQueue<T>,
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
            items: OwnedQueue::with_capacity(capacity),
            capacity,
            name,
        }
    }

    fn push(&self, item: T) -> Result<(), QueueCapacityViolation> {
        let failure = (self.items.len() >= self.capacity).then_some(QueueCapacityViolation {
            queue: self.name,
            capacity: self.capacity,
        });
        // On a backend accounting violation retain every owner in this same
        // queue for fail-stop. No terminal or command is dropped or delivered
        // after the caller latches poisoning. Healthy queues stay bounded by
        // their tickets/slots; Rust cannot connect a VecDeque length to them.
        self.items.push_back(item);
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn pop(&self) -> Option<T> {
        self.items.pop_front()
    }

    fn is_empty(&self) -> bool {
        self.items.is_empty()
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
pub type ModelPool<Id, const I: u8> = oer_radio_port::Child<oer_radio_port::Root<Id>, I>;

/// The distinct key pools belonging to each modeled interface role.
pub trait ModelRole: oer_ieee80211_contracts_port::InterfaceRole {
    /// Component containing the role's pairwise slots.
    type AssociationPath<Id: SlotIdentity>: oer_radio_port::PoolPath<Identity = Id>;
    /// Component containing the unique GTK mutation permission.
    type GroupPermissionPath<Id: SlotIdentity>: oer_radio_port::PoolPath<Identity = Id>;
}
impl ModelRole for Station {
    type AssociationPath<Id: SlotIdentity> = ModelPool<Id, 5>;
    type GroupPermissionPath<Id: SlotIdentity> = ModelPool<Id, 8>;
}
impl ModelRole for AccessPoint {
    type AssociationPath<Id: SlotIdentity> = ModelPool<Id, 6>;
    type GroupPermissionPath<Id: SlotIdentity> = oer_radio_port::Child<ModelPool<Id, 9>, 0>;
}

/// The kind of the one GTK mutation permission per interface.
pub struct ModelGroupPermission<R>(PhantomData<fn() -> R>);
/// The kind of the receive Block Ack slots.
pub enum ModelBlockAcks {}

/// A transmit buffer of the class `C`: its token and its memory, which
/// moves with it; only the install mints one.
pub struct ModelBuffer<'id, B: ModelComposition, C: ModelClass> {
    slot: Slot<'id, C, C::Path<B::Identity>>,
    loan: Option<BodyLoan<'id, B::Identity, B>>,
    memory: ModelMemory,
    admitted_at: Option<RadioInstant<Ieee80211Radio>>,
    admitted: Option<Instant>,
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
impl ModelMemory {
    fn sequence(&self) -> SequenceNumber {
        let bytes: &[u8] = match self {
            Self::Small(bytes) => bytes.as_ref(),
            Self::Large(bytes) => bytes.as_ref(),
        };
        // Both memory classes hold at least a MAC header.
        sequence_control(bytes).map_or(SequenceNumber::ZERO, SequenceNumber::from_sequence_control)
    }
    fn retry(&mut self) {
        let bytes: &mut [u8] = match self {
            Self::Small(bytes) => bytes.as_mut(),
            Self::Large(bytes) => bytes.as_mut(),
        };
        // Both memory classes hold at least a MAC header.
        let _marked: bool = set_retry_bit(bytes);
    }
}

impl<B: ModelComposition, C: ModelClass> ModelBuffer<'_, B, C> {
    fn admit(&mut self, at: Instant) {
        self.admitted.get_or_insert(at);
    }
    fn project_admission(&mut self, air: &ModelAir) -> Result<(), ClockError> {
        if self.admitted_at.is_none()
            && let Some(at) = self.admitted
        {
            self.admitted_at = Some(air.project_time(at)?);
        }
        Ok(())
    }
    fn admission(&self) -> Option<RadioInstant<Ieee80211Radio>> {
        self.admitted_at
    }
}

impl<B: ModelComposition, C: ModelClass> TxBufferMemory for ModelBuffer<'_, B, C> {
    fn renew_msdu(&mut self) {
        self.admitted = None;
        self.admitted_at = None;
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.memory
    }
}

impl<B: ModelComposition, C: ModelClass> core::fmt::Debug for ModelBuffer<'_, B, C> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("ModelBuffer")
            .field(&self.slot.index())
            .finish()
    }
}

/// A returned published MPDU (open or protected). Its checked layout, installed-key identity,
/// and external body remain inseparable from the immutable buffer.
pub struct ModelSent<'id, B: ModelComposition, C: ModelClass> {
    role: VifRole,
    buffer: ModelBuffer<'id, B, C>,
    layout: MpduLayout,
    response: ResponsePolicy,
    key: Option<RawKeyIdentity>,
    body: Option<ModelBodyOf<B>>,
    packet_number: Option<u64>,
}

impl<'id, C: ModelClass, B: ModelComposition> ModelSent<'id, B, C> {
    /// The original PN for a protected MPDU, otherwise `None`; a caller supplies no unchecked header offset.
    pub const fn packet_number(&self) -> Option<u64> {
        self.packet_number
    }
    /// The retained external body, available read-only until renewal.
    pub fn body(&self) -> Option<&ModelBodyOf<B>> {
        self.body.as_ref()
    }
}

/// A transmit buffer of either class, fresh or published.
pub enum ModelAnyBuffer<'id, B: ModelComposition> {
    /// Fresh small memory.
    Small(ModelBuffer<'id, B, ModelSmall>),
    /// Fresh large memory.
    Large(ModelBuffer<'id, B, ModelLarge>),
    /// The original small published MPDU, including its body.
    SmallSent(ModelSent<'id, B, ModelSmall>),
    /// The original large published MPDU, including its body.
    LargeSent(ModelSent<'id, B, ModelLarge>),
}

impl<B: ModelComposition, C: ModelClass> core::fmt::Debug for ModelSent<'_, B, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ModelSent")
            .field("buffer", &self.buffer)
            .field("packet_number", &self.packet_number)
            .finish_non_exhaustive()
    }
}
impl<B: ModelComposition> core::fmt::Debug for ModelAnyBuffer<'_, B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Small(buffer) => f.debug_tuple("Small").field(buffer).finish(),
            Self::Large(buffer) => f.debug_tuple("Large").field(buffer).finish(),
            Self::SmallSent(buffer) => f.debug_tuple("SmallSent").field(buffer).finish(),
            Self::LargeSent(buffer) => f.debug_tuple("LargeSent").field(buffer).finish(),
        }
    }
}

impl<B: ModelComposition> ModelAnyBuffer<'_, B> {
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

/// Opaque installed-key identity under this body's loan and nominal slot Id.
/// Cloning identity retains neither capacity nor live material past stopping.
type KeyIdentityLoan<'p, Id> = PhantomData<fn(&'p (), Id) -> (&'p (), Id)>;

pub struct KeyIdentity<'p, Id: SlotIdentity> {
    owned: RawKeyIdentity,
    loan: KeyIdentityLoan<'p, Id>,
}
impl<Id: SlotIdentity> Clone for KeyIdentity<'_, Id> {
    fn clone(&self) -> Self {
        Self {
            owned: self.owned.clone(),
            loan: PhantomData,
        }
    }
}
impl<Id: SlotIdentity> PartialEq for KeyIdentity<'_, Id> {
    fn eq(&self, other: &Self) -> bool {
        self.owned == other.owned
    }
}
impl<Id: SlotIdentity> Eq for KeyIdentity<'_, Id> {}
impl<Id: SlotIdentity> core::fmt::Debug for KeyIdentity<'_, Id> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.owned.fmt(f)
    }
}
impl<'p, Id: SlotIdentity> KeyIdentity<'p, Id> {
    fn into_owned<S: SlotOwner<Id>>(self, _: &BodyLoan<'p, Id, S>) -> RawKeyIdentity {
        self.owned
    }
}

/// Identity of an installation of a key, not its reusable slot index.
/// Rc is the host model's allocation identity; only the driver allocates PN.
/// Cloning an identity never retains secret material past removal or teardown.
#[derive(Clone, Debug)]
struct RawKeyIdentity(Rc<KeyInstall>);

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

impl PartialEq for RawKeyIdentity {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for RawKeyIdentity {}

impl RawKeyIdentity {
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
    identity: RawKeyIdentity,
    _lease: Rc<()>,
}
impl PreparedKey {
    fn new(identity: RawKeyIdentity) -> Self {
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
pub struct ModelMpdu<'id, B: ModelComposition> {
    buffer: ModelAnyBuffer<'id, B>,
    layout: MpduLayout,
    response: ResponsePolicy,
    // Fresh memory keeps its body here, including during publication. Only
    // consuming the terminal moves both into the returned Sent owner.
    body: Option<ModelBodyOf<B>>,
    published: Option<PublishedMpdu>,
    packet_number: Option<oer_ieee80211_wire::ccmp::CcmpPacketNumber>,
}

struct PublishedMpdu {
    role: VifRole,
    key: Option<RawKeyIdentity>,
    packet_number: Option<u64>,
}

/// Execution access cannot replace or take the MPDU owner.
pub struct ModelMpduExecution<'a, 'id, B: ModelComposition> {
    mpdu: &'a mut ModelMpdu<'id, B>,
}
impl<'id, B: ModelComposition> ExecutionPayload for ModelMpdu<'id, B> {
    type Execution<'a>
        = ModelMpduExecution<'a, 'id, B>
    where
        Self: 'a;
    fn execution(&mut self) -> Self::Execution<'_> {
        ModelMpduExecution { mpdu: self }
    }
}
impl<B: ModelComposition> ModelMpduExecution<'_, '_, B> {
    fn publish(&mut self, role: VifRole, key: Option<&RawKeyIdentity>) -> Result<(), ModelFault> {
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
                let pn = mpdu.packet_number.ok_or(ModelFault)?;
                let key_id = CcmpKeyId::new(key.0.key_id).ok_or(ModelFault)?;
                *header = CcmpHeader::new(pn, key_id).encode();
                Some(pn.value())
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
impl<'id, B: ModelComposition> TxMpdu for ModelMpdu<'id, B> {
    type Buffer = ModelAnyBuffer<'id, B>;
    type Body = ModelBodyOf<B>;
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
impl<'id, B: ModelComposition> ModelMpdu<'id, B> {
    fn admit(&mut self, at: Instant) {
        match &mut self.buffer {
            ModelAnyBuffer::Small(buffer) => buffer.admit(at),
            ModelAnyBuffer::Large(buffer) => buffer.admit(at),
            ModelAnyBuffer::SmallSent(_) | ModelAnyBuffer::LargeSent(_) => {}
        }
    }
    fn project_admission(&mut self, air: &ModelAir) -> Result<(), ClockError> {
        match &mut self.buffer {
            ModelAnyBuffer::Small(buffer) => buffer.project_admission(air),
            ModelAnyBuffer::Large(buffer) => buffer.project_admission(air),
            ModelAnyBuffer::SmallSent(mpdu) => mpdu.buffer.project_admission(air),
            ModelAnyBuffer::LargeSent(mpdu) => mpdu.buffer.project_admission(air),
        }
    }
    fn expired(&self, now: Instant, lifetime: Duration) -> bool {
        let admitted = match &self.buffer {
            ModelAnyBuffer::Small(buffer) => buffer.admitted,
            ModelAnyBuffer::Large(buffer) => buffer.admitted,
            ModelAnyBuffer::SmallSent(mpdu) => mpdu.buffer.admitted,
            ModelAnyBuffer::LargeSent(mpdu) => mpdu.buffer.admitted,
        };
        admitted.is_some_and(|at| now.saturating_duration_since(at) >= lifetime)
    }
    /// Original admission projected by P2 when dequeuing, retained even when
    /// an unpublished member is returned for aggregate re-formation. MSDU
    /// age uses its separately retained monotonic instant.
    pub fn admitted_at(&self) -> Option<RadioInstant<Ieee80211Radio>> {
        match &self.buffer {
            ModelAnyBuffer::Small(buffer) => buffer.admission(),
            ModelAnyBuffer::Large(buffer) => buffer.admission(),
            ModelAnyBuffer::SmallSent(mpdu) => mpdu.buffer.admission(),
            ModelAnyBuffer::LargeSent(mpdu) => mpdu.buffer.admission(),
        }
    }

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
        mut buffer: ModelAnyBuffer<'id, B>,
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
        ticket: u8,
        role: VifRole,
        keys: &[RawKeyIdentity],
        key: Option<&RawKeyIdentity>,
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
            let installations = air.key_installations.take();
            let result = (|| {
                let material = installations
                    .iter()
                    .find(|entry| entry.identity.ptr_eq(&Rc::downgrade(&identity.0)))
                    .ok_or(ModelFault)?;
                wire.extend_from_slice(&[0; 8]);
                air.associations
                    .get()
                    .get(identity.0.role)
                    .seal(&material.key, &mut wire)
                    .map_err(|_| ModelFault)
            })();
            air.key_installations.set(installations);
            result?;
        }
        Ok(AirFrame {
            ticket,
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

/// Composition-selected owned contexts and their separate public body loans.
/// The model never interprets or clones the contexts, and adds no `Send` bound.
pub trait ModelContext {
    /// Whole external payload owners, selected before the body loan.
    type Body: TxBody + core::fmt::Debug;
    /// Owned operation representation retained in the command/event queues.
    type OwnedContext<Id: SlotIdentity>;
    /// Public operation context; conversion never receives backend authority.
    type Context<'p, Id: SlotIdentity + 'p>: LoanedContext<'p, Id, Owned = Self::OwnedContext<Id>>
        + 'p
    where
        Self: 'p;
    /// Owned station-configuration context.
    type OwnedStationConfigureContext<Id: SlotIdentity>;
    /// Public station-configuration context.
    type StationConfigureContext<'p, Id: SlotIdentity + 'p>: LoanedContext<'p, Id, Owned = Self::OwnedStationConfigureContext<Id>>
        + 'p
    where
        Self: 'p;
    /// Owned access-point configuration context.
    type OwnedAccessPointConfigureContext<Id: SlotIdentity>;
    /// Public access-point configuration context.
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p>: LoanedContext<'p, Id, Owned = Self::OwnedAccessPointConfigureContext<Id>>
        + 'p
    where
        Self: 'p;
}

impl ModelContext for () {
    type Body = ModelBody;
    type OwnedContext<Id: SlotIdentity> = ();
    type Context<'p, Id: SlotIdentity + 'p> = ();
    type OwnedStationConfigureContext<Id: SlotIdentity> = ();
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}

/// The concrete slot owner selecting this model's nominal identity and data.
pub trait ModelComposition: SlotBodyOwner<Self::Identity> {
    /// Nominal slot identity, independent of every body loan.
    type Identity: SlotIdentity;
    /// Capabilities of this concrete backend.
    type Caps: Ieee80211Capabilities;
    /// Context and external payload representations fixed before the body loan.
    type Contexts: ModelContext;
}

type ModelBodyOf<B> = <<B as ModelComposition>::Contexts as ModelContext>::Body;
type ModelContextOf<B> = <<B as ModelComposition>::Contexts as ModelContext>::OwnedContext<
    <B as ModelComposition>::Identity,
>;
type ModelStationContextOf<B> =
    <<B as ModelComposition>::Contexts as ModelContext>::OwnedStationConfigureContext<
        <B as ModelComposition>::Identity,
    >;
type ModelAccessPointContextOf<B> =
    <<B as ModelComposition>::Contexts as ModelContext>::OwnedAccessPointConfigureContext<
        <B as ModelComposition>::Identity,
    >;

/// The types the model's events carry.
pub struct ModelTypes<B: ModelComposition>(PhantomData<fn() -> B>);

impl<'id, B: ModelComposition + 'id> PhaseTypes<'id> for ModelTypes<B> {
    const COMMAND_CAPACITY: usize = MODEL_COMMANDS;
    type StationCommandPath =
        oer_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 1>;
    type AccessPointCommandPath =
        oer_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 2>;
    type StationConfigureContext =
        <B::Contexts as ModelContext>::StationConfigureContext<'id, B::Identity>;
    type AccessPointConfigureContext =
        <B::Contexts as ModelContext>::AccessPointConfigureContext<'id, B::Identity>;
    const TERMINAL_CAPACITY: usize = MODEL_TERMINALS;
    type EpochPath = ModelPool<<B as ModelComposition>::Identity, 4>;
    type GatePath = ModelPool<<B as ModelComposition>::Identity, 0>;
    type TicketPath = ModelPool<<B as ModelComposition>::Identity, 1>;
    type BlockAckPath = ModelPool<<B as ModelComposition>::Identity, 7>;
    type Fault = ModelFault;
    type Context = <B::Contexts as ModelContext>::Context<'id, B::Identity>;
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities> = ModelPrepared<'k, 'p, 'id, R, K, B>;
    type KeyIdentity = KeyIdentity<'id, B::Identity>;
    type Aggregate<'k, 'p, R, K, S>
        = ModelAggregate<'k, 'p, 'id, R, K, B, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k;
    type TxBody = ModelBodyOf<B>;
    type TicketKind = ModelTickets;
    type AckEvidence = ModelAck;
    type BlockAckEvidence = ModelBlockAckEvidence;
    type AirDenied = ModelAirDenied;
    type AnyTxBuffer = ModelAnyBuffer<'id, B>;
    type TxMpdu = ModelMpdu<'id, B>;
    type TxAggregate = ModelSubframes<'id, B>;
    type RxBuffer = ModelRxBuffer<'id, B::Identity>;
    type RxMeta = ModelRxMeta;
    type StationKeys = ModelKeys<'id, B, Station>;
    type AccessPointKeys = ModelKeys<'id, B, AccessPoint>;
    type BlockAckKind = ModelBlockAcks;
    type BlockAckPool = ModelBlockAckPool;
    type BlockAck = ModelBlockAck<'id, B>;
}

/// The unique protocol owner selecting an interface's immutable association.
/// Installed keys retain only their interface identity; the modeled hardware
/// holds exactly one CCMP record at that interface's index.
pub struct ModelAssociation<'id, B: ModelComposition, R: ModelRole> {
    owner: Association<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath<B::Identity>>,
}

/// The keys of the role `R` and their slots.
type KeyTypesLoan<'p, B, R> = PhantomData<fn(&'p (), B) -> (&'p (), B, R)>;

pub struct ModelKeys<'id, B: ModelComposition, R>(KeyTypesLoan<'id, B, R>);

impl<'id, B: ModelComposition + 'id, R: ModelRole + 'id> KeyTypes<'id> for ModelKeys<'id, B, R> {
    type Pool = ModelAssociationPool<R>;
    type AssociationStart =
        AssociationStart<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath<B::Identity>>;
    type Association = ModelAssociation<'id, B, R>;
    type VacantKeyBank =
        VacantKeyBank<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath<B::Identity>>;
    type VacantAssociation = VacantAssociation<
        'id,
        R,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        R::AssociationPath<B::Identity>,
    >;
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
        PairwiseKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath<B::Identity>>;
    type GroupSlot =
        GroupKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath<B::Identity>>;
    type PairwiseKey = ModelPairwiseKey<'id, B, R>;
    type GroupKey = ModelGroupKey<'id, B, R>;
    type GroupPermission = Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath<B::Identity>>;
    type Identity = KeyIdentity<'id, B::Identity>;
    type GroupSlotIdentity = u8;
    fn pairwise_identity(key: &Self::PairwiseKey) -> Self::Identity {
        KeyIdentity {
            owned: key.identity.clone(),
            loan: PhantomData,
        }
    }
    fn group_identity(key: &Self::GroupKey) -> Self::Identity {
        KeyIdentity {
            owned: key.identity.clone(),
            loan: PhantomData,
        }
    }
    fn group_slot_identity(key: &Self::GroupKey) -> u8 {
        key.slot.index()
    }
    fn vacant_group_slot_identity(slot: &Self::GroupSlot) -> u8 {
        slot.index()
    }
}

impl<'id, B: ModelComposition + 'id, C: ModelClass> TxBufferPool<'id, C> for ModelTypes<B> {
    type Buffer = ModelBuffer<'id, B, C>;
    type Sent = ModelSent<'id, B, C>;

    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: CheckedPoolSource<'k, 'p, 'id, Self, C, K, R>,
    ) -> oer_ieee80211_contracts_port::PoolPreparation<'k, 'p, 'id, Self, C, K, R>
    where
        'id: 'k,
    {
        prepare_model(source)
    }

    fn push_aggregate<'k, 'p, R, K, S>(
        operation: oer_ieee80211_contracts_port::CheckedPoolAggregate<
            'k,
            'p,
            'id,
            Self,
            C,
            R,
            K,
            S,
        >,
    ) -> oer_ieee80211_contracts_port::AggregatePushResult<'k, 'p, 'id, Self, C, R, K, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: AggregateState,
        'id: 'k + 'p,
    {
        push_model(operation)
    }

    fn erase(buffer: Self::Buffer) -> ModelAnyBuffer<'id, B> {
        C::erase(buffer)
    }

    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent(buffer: Self::Sent) -> ModelAnyBuffer<'id, B> {
        C::erase_sent(buffer)
    }

    fn class(buffer: ModelAnyBuffer<'id, B>) -> Result<Self::Buffer, ModelAnyBuffer<'id, B>> {
        C::class(buffer)
    }

    fn sent(buffer: ModelAnyBuffer<'id, B>) -> Result<Self::Sent, ModelAnyBuffer<'id, B>> {
        C::sent(buffer)
    }

    fn renew(mut mpdu: Self::Sent) -> (Self::Buffer, Option<ModelBodyOf<B>>) {
        mpdu.buffer.renew_msdu();
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
        crypto: RxEvidence<oer_ieee80211_contracts_port::RxCryptoStatus>,
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

/// A received allocation under the consumer's invariant body loan and Id.
/// Dropping it requests poison; only `LowerMacPhase::recycle` returns capacity.
pub struct ModelRxBuffer<'p, Id: SlotIdentity> {
    owner: OwnedRxBuffer<Id>,
    loan: PhantomData<fn(&'p ()) -> &'p ()>,
}

struct OwnedRxBuffer<Id: SlotIdentity> {
    bytes: Vec<u8>,
    recycled: bool,
    air: Weak<ModelAir>,
    streamed: bool,
    identity: PhantomData<fn(Id) -> Id>,
}

impl<Id: SlotIdentity> RxBuffer for ModelRxBuffer<'_, Id> {
    fn bytes(&self) -> &[u8] {
        &self.owner.bytes
    }
}
impl<Id: SlotIdentity> core::fmt::Debug for ModelRxBuffer<'_, Id> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ModelRxBuffer")
            .field("bytes", &self.owner.bytes.len())
            .finish_non_exhaustive()
    }
}
impl<Id: SlotIdentity> OwnedRxBuffer<Id> {
    fn lend<'p, S: SlotOwner<Id>>(self, _: &BodyLoan<'p, Id, S>) -> ModelRxBuffer<'p, Id> {
        ModelRxBuffer {
            owner: self,
            loan: PhantomData,
        }
    }
}
impl<Id: SlotIdentity> Drop for OwnedRxBuffer<Id> {
    fn drop(&mut self) {
        if !self.recycled
            && let Some(air) = self.air.upgrade()
        {
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
pub struct ModelResources<'id, B: ModelComposition> {
    /// Station's unique association-mode selection permission.
    pub station_association: AssociationStart<
        'id,
        Station,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 5>,
    >,
    /// AP's interface-wide association-mode selection permission.
    pub access_point_association: AssociationStart<
        'id,
        AccessPoint,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 6>,
    >,
    /// Station command admission from its own installation component.
    pub station_commands: InterfacePermissions<
        'id,
        Station,
        oer_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 1>,
    >,
    /// Access-point command admission from its own installation component.
    pub access_point_commands: InterfacePermissions<
        'id,
        AccessPoint,
        oer_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 2>,
    >,
    /// The install's single epoch-source slot; Station constructs its source.
    pub connection_epoch:
        Slot<'id, ConnectionEpochKind, ModelPool<<B as ModelComposition>::Identity, 4>>,
    /// The transmit tickets.
    pub tickets: Vec<Slot<'id, ModelTickets, ModelPool<<B as ModelComposition>::Identity, 1>>>,
    /// Sole permission for the body to unpack TX terminals.
    pub terminal_access: oer_ieee80211_contracts_port::TerminalAccess<
        'id,
        ModelTickets,
        ModelPool<<B as ModelComposition>::Identity, 1>,
    >,
    /// The small transmit buffers.
    pub small: Vec<ModelBuffer<'id, B, ModelSmall>>,
    /// The large transmit buffers.
    pub large: Vec<ModelBuffer<'id, B, ModelLarge>>,
    /// The station's pairwise key slots.
    pub station_pairwise: [PairwiseKeySlot<
        'id,
        Station,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 5>,
    >; MODEL_KEY_SLOTS],
    /// The station's group key slots.
    pub station_group: [GroupKeySlot<
        'id,
        Station,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 5>,
    >; MODEL_KEY_SLOTS],
    /// The access point's pairwise key slots.
    pub access_point_pairwise: [PairwiseKeySlot<
        'id,
        AccessPoint,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 6>,
    >; MODEL_KEY_SLOTS],
    /// The access point's group key slots.
    pub access_point_group: [GroupKeySlot<
        'id,
        AccessPoint,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 6>,
    >; MODEL_KEY_SLOTS],
    /// The station's sole GTK mutation permission.
    pub station_group_permission:
        Slot<'id, ModelGroupPermission<Station>, ModelPool<<B as ModelComposition>::Identity, 8>>,
    /// The access point's sole GTK mutation permission.
    pub access_point_group_permission: Slot<
        'id,
        ModelGroupPermission<AccessPoint>,
        oer_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 0>,
    >,
    /// The receive Block Ack slots both roles share.
    pub block_acks: [ModelBlockAckSlot<'id, B>; MODEL_BLOCK_ACK_SLOTS],
    /// The sole authority to construct this installation's arbiter.
    pub arbiter_authority: oer_ieee80211_contracts_port::ArbiterAuthority<
        'id,
        ModelPool<<B as ModelComposition>::Identity, 0>,
    >,
    /// The open transmit gate.
    pub gate: GateOpen<'id, ModelPool<<B as ModelComposition>::Identity, 0>>,
}

/// One transmission the air saw.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AirFrame {
    /// Ticket of the operation that actually published this MPDU.
    pub ticket: u8,
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
    /// Reject the replacement write, then restore the exact old GTK and fail-stop.
    ReplacementWrite,
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
    installation: RawKeyIdentity,
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
    queues: [ModelWindow; 4],
    heads: [Option<VifRole>; 4],
    entropy: u32,
    draws: Vec<ModelContention>,
}
impl Default for ModelEdca {
    fn default() -> Self {
        Self {
            parameters: [EdcaParameterSet::STANDARD_DEFAULT; 2],
            queues: WmmAccessCategory::ALL.map(|ac| {
                ModelWindow::from_wmm(EdcaParameterSet::STANDARD_DEFAULT.access_category(ac).wmm)
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
        queue.reconfigure(parameters.wmm.ecw_min.get(), parameters.wmm.ecw_max.get());
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
            backoff_slots: queue.draw(self.entropy),
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

struct ModelTime {
    clock: Rc<dyn Clock>,
    relation: ClockRelation<Instant, RadioInstant<Ieee80211Radio>>,
    sample: Cell<RelationSample<Instant, RadioInstant<Ieee80211Radio>>>,
}
impl ModelTime {
    fn new(clock: Rc<dyn Clock>) -> Self {
        let source = clock.now();
        // Toggling one epoch bit guarantees a representable, non-zero offset.
        let target = RadioInstant::from_micros(source.as_micros() ^ (1_u64 << 32));
        Self {
            clock,
            relation: ClockRelation::affine(0),
            sample: Cell::new(RelationSample {
                source,
                target,
                uncertainty: oer_time::RadioDuration::ZERO,
                generation: GenerationSource::new(0).current(),
            }),
        }
    }
}

/// Non-owning observation of one GTK removal retained by P2.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupRemovalObservation {
    /// Interface retaining the removal.
    pub role: VifRole,
    /// Vacant slot retained inside its whole removal command.
    pub slot: u8,
    /// Index of the original mutation permission.
    pub permission: u8,
}

/// Snapshot published by P2, containing counts and indices, never owners.
#[derive(Clone, Debug, Default)]
pub struct DriverObservation {
    /// Publications awaiting their actual results.
    pub publications: usize,
    /// Identities retained in the installed-key table.
    pub installed_keys: usize,
    /// GTK removals waiting for earlier admitted uses.
    pub group_removals: Vec<GroupRemovalObservation>,
}

/// The air and the world of a model: shared by the backend's driver and
/// the test.
pub struct ModelAir {
    observation: Cell<DriverObservation>,
    sent: Cell<Vec<AirFrame>>,
    /// The answers of the next transmissions, in order; once none is
    /// queued, each is acknowledged.
    answers: OwnedQueue<AirAnswer>,
    /// Conformance air input: an absent peer supplies no response to any PPDU.
    #[cfg(feature = "test-support")]
    absent_peer: Cell<bool>,
    rx: OwnedQueue<(Vec<u8>, ModelRxMeta)>,
    rx_stream: Cell<Option<(Vec<u8>, ModelRxMeta)>>,
    refuse_enable: Cell<bool>,
    group_failure: Cell<Option<GroupFaultStage>>,
    group_rollback: Cell<Option<oer_ieee80211_testing_conformance::group_keys::Rollback>>,
    group_held: Cell<bool>,
    // Secret material belongs to this installation table, never to the
    // cloneable identity carried by captured RX or returned Sent owners.
    key_installations: Cell<Vec<KeyInstallation>>,
    associations: Cell<InterfaceAssociations>,
    crypto_operations: Cell<Vec<ModelCryptoOperation>>,
    group_rx: OwnedQueue<CapturedGroupRx>,
    group_writes: Cell<usize>,
    stream: Cell<ModelStream>,
    disables: Cell<u32>,
    driver: Cell<Option<Waker>>,
    /// The driver's passes.
    passes: Cell<u32>,
    /// The commands the driver took, of every kind.
    commands: Cell<u32>,
    /// The settings the driver applied.
    settings: Cell<u32>,
    /// The EDCA set each interface's attempts contend with, by role.
    edca: Cell<ModelEdca>,
    /// The queues the air keeps busy: their attempts wait unpublished.
    held: Cell<[bool; WmmAccessCategory::ALL.len()]>,
    /// Keep a published PPDU's result pending, independently of prepublication CCA.
    held_results: Cell<[bool; WmmAccessCategory::ALL.len()]>,
    time: ModelTime,
    receive_agreements: Cell<BTreeMap<u8, ModelReceiveAgreement>>,
    expired_msdus: Cell<u32>,
}
impl ModelAir {
    /// Inspect P2's last published snapshot without borrowing its driver.
    pub fn driver_observation(&self) -> DriverObservation {
        self.observation.snapshot()
    }

    fn new(clock: Rc<dyn Clock>) -> Self {
        Self {
            observation: Cell::default(),
            sent: Default::default(),
            answers: Default::default(),
            #[cfg(feature = "test-support")]
            absent_peer: Cell::new(false),
            rx: Default::default(),
            rx_stream: Default::default(),
            refuse_enable: Default::default(),
            group_failure: Default::default(),
            group_rollback: Default::default(),
            group_held: Default::default(),
            key_installations: Default::default(),
            associations: Default::default(),
            crypto_operations: Default::default(),
            group_rx: Default::default(),
            group_writes: Default::default(),
            stream: Default::default(),
            disables: Default::default(),
            driver: Default::default(),
            passes: Default::default(),
            commands: Default::default(),
            settings: Default::default(),
            edca: Default::default(),
            held: Default::default(),
            held_results: Default::default(),
            time: ModelTime::new(clock),
            receive_agreements: Default::default(),
            expired_msdus: Default::default(),
        }
    }

    fn monotonic_now(&self) -> Instant {
        self.time.clock.now()
    }

    /// Wake the driver after the clock owner advances virtual time.
    /// This notification never changes the clock.
    pub fn clock_advanced(&self) {
        self.wake_driver();
    }

    /// Project the image clock into the model radio domain.
    pub fn radio_now(&self) -> Result<RadioInstant<Ieee80211Radio>, ClockError> {
        self.project_time(self.monotonic_now())
    }

    fn project_time(&self, at: Instant) -> Result<RadioInstant<Ieee80211Radio>, ClockError> {
        let sample = self.time.sample.get();
        self.time
            .relation
            .project(at, sample.generation, &sample)
            .map(|p| p.at)
            .map_err(|_| ClockError::EpochExhausted)
    }

    /// The sampled affine relation used by `radio_now()`.
    pub fn clock_relation(
        &self,
    ) -> (
        ClockRelation<Instant, RadioInstant<Ieee80211Radio>>,
        RelationSample<Instant, RadioInstant<Ieee80211Radio>>,
    ) {
        (self.time.relation, self.time.sample.get())
    }
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
    Authentication(oer_ieee80211_core_security_cipher::ccmp_mpdu::CcmpMpduError),
}

// ModelCaps declares Interface scope. These are hardware policy records,
// indexed by the same interface identity that keys already carry. The
// association owner is never cloned. A queued install takes its decision
// from that owner and the driver applies it before publishing material.
// Like key-table hardware, vacant interfaces retain their last mode; changing
// it requires a new owner obtained through the complete returned slot bank.
#[derive(Clone, Copy)]
struct InterfaceAssociations {
    station: oer_ieee80211_wire::security::Spp,
    access_point: oer_ieee80211_wire::security::Spp,
}
impl Default for InterfaceAssociations {
    fn default() -> Self {
        use oer_ieee80211_wire::security::Spp;
        Self {
            station: Spp::NotNegotiated,
            access_point: Spp::NotNegotiated,
        }
    }
}
impl InterfaceAssociations {
    fn get(&self, role: VifRole) -> oer_ieee80211_core_security_cipher::ccmp_mpdu::CcmpAssociation {
        oer_ieee80211_core_security_cipher::ccmp_mpdu::CcmpAssociation::new(match role {
            VifRole::Station => self.station,
            VifRole::AccessPoint => self.access_point,
        })
    }
    fn apply(&mut self, role: VifRole, spp: oer_ieee80211_wire::security::Spp) {
        let record = match role {
            VifRole::Station => &mut self.station,
            VifRole::AccessPoint => &mut self.access_point,
        };
        *record = spp;
    }
}

impl ModelAir {
    fn install_material(&self, identity: &RawKeyIdentity, key: Ccmp128Key) {
        self.crypto_operations.record(ModelCryptoOperation::Key {
            role: identity.0.role,
            slot: identity.0.index,
        });
        self.key_installations.record(KeyInstallation {
            identity: Rc::downgrade(&identity.0),
            key,
        });
    }

    fn apply_association(&self, role: VifRole, spp: oer_ieee80211_wire::security::Spp) {
        self.associations.apply(role, spp);
        self.crypto_operations
            .record(ModelCryptoOperation::Association { role, spp });
    }

    /// Programming observations, in the same order as the driver's FIFO.
    pub fn crypto_operations(&self) -> Vec<ModelCryptoOperation> {
        self.crypto_operations.snapshot()
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
        let group = oer_ieee80211_contracts_port::GroupAddress::new(header.receiver()).is_some();
        let installations = self.key_installations.take();
        let result = (|| {
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
                .get()
                .get(identity.role)
                .open(&material.key, &mut plaintext)
                .map_err(ModelCcmpError::Authentication)?;
            Ok(plaintext)
        })();
        self.key_installations.set(installations);
        let plaintext = result?;
        let (plaintext, _mic) = plaintext
            .split_last_chunk::<8>()
            .ok_or(ModelCcmpError::MalformedHeader)?;
        self.receive(
            plaintext,
            meta.with_crypto(RxEvidence::HardwareObserved(
                oer_ieee80211_contracts_port::RxCryptoStatus::DecryptedAndIntegrityVerified,
            )),
        );
        Ok(())
    }

    fn remove_key_material(&self, identity: &RawKeyIdentity) {
        let mut installations = self.key_installations.take();
        installations.retain(|entry| !entry.identity.ptr_eq(&Rc::downgrade(&identity.0)));
        self.key_installations.set(installations);
    }

    fn end_installation(&self) {
        let mut installations = self.key_installations.take();
        for entry in installations.iter() {
            if let Some(identity) = entry.identity.upgrade() {
                identity.active.set(false);
                identity.rx_active.set(false);
            }
        }
        self.group_rx.clear();
        // Ccmp128Key clears its bytes even if an external identity still exists.
        installations.clear();
        self.key_installations.set(installations);
    }

    /// Inject uncertainty before or after a GTK hardware mutation. No success
    /// owner or ordinary permission leaves the poisoned installation.
    pub fn fail_next_group_change(&self, stage: GroupFaultStage) {
        self.group_failure.set(Some(stage));
        self.wake_driver();
    }

    /// Actual internal rollback observed before poisoned teardown.
    pub fn group_rollback(
        &self,
    ) -> Option<oer_ieee80211_testing_conformance::group_keys::Rollback> {
        self.group_rollback.get()
    }

    /// Delay commands while RX continues. The supplied clock's owner calls
    /// `clock_advanced` for the deadline wake; the driver poisons at
    /// GROUP_COMMAND_BOUND. Normal disable finishes
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
        let installations = self.key_installations.take();
        let installation = installations
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
        self.key_installations.set(installations);
        let Some(installation) = installation else {
            return false;
        };
        self.group_rx.push_back(CapturedGroupRx {
            frame: frame.into(),
            meta,
            installation: RawKeyIdentity(installation),
            stage,
        });
        self.wake_driver();
        true
    }

    /// Every publication so far, including collisions and unknown outcomes.
    pub fn sent(&self) -> Vec<AirFrame> {
        self.sent.snapshot()
    }

    /// Supply exactly one result for the next publication. A Block Ack cannot
    /// be left behind for a later aggregate when a Single is ahead of it.
    pub fn answer(&self, answer: AirAnswer) {
        self.answers.push_back(answer);
    }

    /// Applied receive agreements by hardware slot. Pending writes remain
    /// invisible until the driver takes the latest value of each slot.
    pub fn receive_agreements(&self) -> Vec<(u8, ModelReceiveAgreement)> {
        let agreements = self.receive_agreements.take();
        let result = agreements
            .iter()
            .map(|(&slot, &agreement)| (slot, agreement))
            .collect();
        self.receive_agreements.set(agreements);
        result
    }

    /// Keep reception ready on every service poll, like an RX interrupt
    /// source on another executor. This exercises scheduling against a
    /// continuously ready source, rather than a finite queued burst.
    pub fn receive_continuously(&self, frame: &[u8], meta: ModelRxMeta) {
        self.rx_stream.set(Some((frame.into(), meta)));
        self.wake_driver();
    }

    /// Stop generating RX events. Already returned buffers still require
    /// recycling through the phase.
    pub fn stop_continuous_receive(&self) {
        self.rx_stream.take();
    }

    /// Deliver `frame` to the radio.
    pub fn receive(&self, frame: &[u8], meta: ModelRxMeta) {
        self.rx.push_back((frame.into(), meta));
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

    /// MSDUs that ended with Expired, counted separately from air failures.
    pub fn expired_msdus(&self) -> u32 {
        self.expired_msdus.get()
    }

    fn expired(&self, mask: u64) {
        self.expired_msdus
            .set(self.expired_msdus.get().saturating_add(mask.count_ones()));
    }

    /// The complete EDCA set currently installed for this interface.
    pub fn edca(&self, role: VifRole) -> EdcaParameterSet {
        self.edca.parameters(role)
    }

    /// Actual contention draws, one per PPDU publication attempt.
    pub fn contentions(&self) -> Vec<ModelContention> {
        self.edca.contentions()
    }

    /// Inject a deterministic entropy word for subsequent backoff draws.
    pub fn set_backoff_entropy(&self, entropy: u32) {
        self.edca.set_entropy(entropy);
    }

    /// The frames the air holds for the driver yet.
    pub fn pending_rx(&self) -> usize {
        self.rx.len()
    }

    fn wake_driver(&self) {
        if let Some(waker) = self.driver.take() {
            waker.wake();
        }
    }

    fn next_answer(&self, default: AirAnswer) -> AirAnswer {
        #[cfg(feature = "test-support")]
        if self.absent_peer.get() {
            return AirAnswer::NoResponse;
        }
        self.answers.pop_front().unwrap_or(default)
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
enum Command<'id, B: ModelComposition> {
    Configure(OwnedConfigured<'id, B>),
    Submit(Box<Prepared<'id, B>>),
    SubmitAmpdu(PreparedAggregate<'id, B>),
    InstallKey {
        spp: oer_ieee80211_wire::security::Spp,
        key: RawKeyIdentity,
        material: Ccmp128Key,
        /// The installed owner its terminal returns.
        installed: OwnedInstalledPairwise<'id, B>,
    },
    Group {
        admitted_at: u64,
        change: GroupChange<'id, B>,
    },
    RemoveKey {
        key: RawKeyIdentity,
        removed: OwnedRemoved<'id, B>,
    },
    CloseGate(GateOpen<'id, ModelPool<<B as ModelComposition>::Identity, 0>>),
    OpenGate,
}

type StationGroupPermission<'id, B> =
    Slot<'id, ModelGroupPermission<Station>, ModelPool<<B as ModelComposition>::Identity, 8>>;

struct PendingGroupInstall<'id, B: ModelComposition, R: ModelRole> {
    spp: oer_ieee80211_wire::security::Spp,
    slot: GroupKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath<B::Identity>>,
    permission: Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath<B::Identity>>,
    key_id: GroupKeyId,
    key: Ccmp128Key,
}

enum GroupInstallationCommand<'id, B: ModelComposition> {
    Station(PendingGroupInstall<'id, B, Station>),
    AccessPoint(PendingGroupInstall<'id, B, AccessPoint>),
}

trait GroupInstallationRole: ModelRole + Sized {
    fn installation<B: ModelComposition>(
        pending: PendingGroupInstall<'_, B, Self>,
    ) -> GroupInstallationCommand<'_, B>;
    fn installed<'o, B: ModelComposition>(
        key: ModelGroupKey<'o, B, Self>,
        permission: Slot<'o, ModelGroupPermission<Self>, Self::GroupPermissionPath<B::Identity>>,
    ) -> OwnedInstalledGroup<'o, B>;
    fn pairwise_installed<'o, B: ModelComposition>(
        key: ModelPairwiseKey<'o, B, Self>,
    ) -> OwnedInstalledPairwise<'o, B>;
}
impl GroupInstallationRole for Station {
    fn installation<B: ModelComposition>(
        pending: PendingGroupInstall<'_, B, Self>,
    ) -> GroupInstallationCommand<'_, B> {
        GroupInstallationCommand::Station(pending)
    }
    fn installed<'o, B: ModelComposition>(
        key: ModelGroupKey<'o, B, Self>,
        permission: StationGroupPermission<'o, B>,
    ) -> OwnedInstalledGroup<'o, B> {
        OwnedInstalledGroup::Station { key, permission }
    }
    fn pairwise_installed<'o, B: ModelComposition>(
        key: ModelPairwiseKey<'o, B, Self>,
    ) -> OwnedInstalledPairwise<'o, B> {
        OwnedInstalledPairwise::Station(key)
    }
}
impl GroupInstallationRole for AccessPoint {
    fn installation<B: ModelComposition>(
        pending: PendingGroupInstall<'_, B, Self>,
    ) -> GroupInstallationCommand<'_, B> {
        GroupInstallationCommand::AccessPoint(pending)
    }
    fn installed<'o, B: ModelComposition>(
        key: ModelGroupKey<'o, B, Self>,
        permission: Slot<
            'o,
            ModelGroupPermission<Self>,
            oer_radio_port::Child<ModelPool<B::Identity, 9>, 0>,
        >,
    ) -> OwnedInstalledGroup<'o, B> {
        OwnedInstalledGroup::AccessPoint { key, permission }
    }
    fn pairwise_installed<'o, B: ModelComposition>(
        key: ModelPairwiseKey<'o, B, Self>,
    ) -> OwnedInstalledPairwise<'o, B> {
        OwnedInstalledPairwise::AccessPoint(key)
    }
}

/// Whole command custody, including permission, material and physical owner.
enum GroupChange<'id, B: ModelComposition> {
    Install(GroupInstallationCommand<'id, B>),
    Replace {
        permission: StationGroupPermission<'id, B>,
        installed: ModelGroupKey<'id, B, Station>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    },
    Select {
        permission: StationGroupPermission<'id, B>,
        installed: ModelGroupKey<'id, B, Station>,
    },
    Remove {
        key: RawKeyIdentity,
        removed: OwnedRemoved<'id, B>,
    },
}

/// The work of a queue: one MPDU, or one A-MPDU.
enum Work<'id, B: ModelComposition> {
    Single(Box<Prepared<'id, B>>),
    Aggregate(PreparedAggregate<'id, B>),
}

impl<B: ModelComposition> Work<'_, B> {
    fn role(&self) -> VifRole {
        match self {
            Self::Single(prepared) => prepared.role,
            Self::Aggregate(prepared) => prepared.role,
        }
    }
    fn key(&self) -> Option<&RawKeyIdentity> {
        match self {
            Self::Single(prepared) => prepared.key.as_ref(),
            Self::Aggregate(prepared) => prepared.key.as_ref(),
        }
        .map(|key| &key.identity)
    }
}

/// One subframe of an aggregate, in its buffer.
struct Subframe<'id, B: ModelComposition> {
    mpdu: ModelMpdu<'id, B>,
    /// Checked at admission against the first subframe's BA window.
    acknowledgement_bit: u64,
}

/// Complete aggregate storage; each subframe retains its own MPDU/body.
pub struct ModelSubframes<'id, B: ModelComposition>(Vec<Subframe<'id, B>>);
impl<B: ModelComposition> ModelSubframes<'_, B> {
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
pub struct ModelSubframeIter<'id, B: ModelComposition>(alloc::vec::IntoIter<Subframe<'id, B>>);
impl<B: ModelComposition> ModelSubframeIter<'_, B> {
    /// Whether cancellation or terminal iteration has any MPDUs left.
    pub fn is_empty(&self) -> bool {
        self.0.as_slice().is_empty()
    }
}
impl<'id, B: ModelComposition> Iterator for ModelSubframeIter<'id, B> {
    type Item = ModelMpdu<'id, B>;
    fn next(&mut self) -> Option<Self::Item> {
        self.0.next().map(|subframe| subframe.mpdu)
    }
}
impl<'id, B: ModelComposition> IntoIterator for ModelSubframes<'id, B> {
    type Item = ModelMpdu<'id, B>;
    type IntoIter = ModelSubframeIter<'id, B>;
    fn into_iter(self) -> Self::IntoIter {
        ModelSubframeIter(self.0.into_iter())
    }
}
/// Execution lends subframe storage without exposing replaceable owners.
pub struct ModelSubframesExecution<'a, 'id, B: ModelComposition> {
    subframes: &'a mut [Subframe<'id, B>],
}
impl<'id, B: ModelComposition> ExecutionPayload for ModelSubframes<'id, B> {
    type Execution<'a>
        = ModelSubframesExecution<'a, 'id, B>
    where
        Self: 'a;
    fn execution(&mut self) -> Self::Execution<'_> {
        ModelSubframesExecution {
            subframes: &mut self.0,
        }
    }
}
impl<B: ModelComposition> ModelSubframesExecution<'_, '_, B> {
    fn publish(
        &mut self,
        ticket: u8,
        role: VifRole,
        keys: &[RawKeyIdentity],
        key: Option<&RawKeyIdentity>,
        rate: PhyRate,
    ) -> Result<Vec<AirFrame>, ModelFault> {
        for subframe in &*self.subframes {
            subframe.mpdu.preflight(key.is_some())?;
        }
        let mut frames = Vec::with_capacity(self.subframes.len());
        for subframe in &mut *self.subframes {
            subframe.mpdu.execution().publish(role, key)?;
            frames.push(subframe.mpdu.on_air(ticket, role, keys, key, rate)?);
        }
        Ok(frames)
    }
}

type ModelInFlight<'id, B, M> = InFlight<
    ModelContextOf<B>,
    Slot<'id, ModelTickets, ModelPool<<B as ModelComposition>::Identity, 1>>,
    M,
>;

/// An A-MPDU attempt prepared for its queue.
struct PreparedAggregate<'id, B: ModelComposition> {
    operation: ModelInFlight<'id, B, ModelSubframes<'id, B>>,
    role: VifRole,
    start_sequence: SequenceNumber,
    queue: oer_ieee80211_wire::qos::WmmAccessCategory,
    rate: PhyRate,
    key: Option<PreparedKey>,
}

/// An attempt prepared for its queue.
struct Prepared<'id, B: ModelComposition> {
    operation: ModelInFlight<'id, B, ModelMpdu<'id, B>>,
    role: VifRole,
    queue: oer_ieee80211_wire::qos::WmmAccessCategory,
    plan: oer_ieee80211_contracts_port::RetryPlan<PhyRate>,
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
pub struct ModelHeld<'id, B: ModelComposition> {
    air: Rc<ModelAir>,
    commands: Bounded<Command<'id, B>>,
    /// The received buffers the body gives back, at most one per buffer.
    recycled: Bounded<OwnedRxBuffer<B::Identity>>,
    settings: Cell<Settings>,
    events: Bounded<OwnedLowerMacEvent<'id, B>>,
    quarantined: Bounded<Work<'id, B>>,
    failed_group: Bounded<GroupChange<'id, B>>,
    failed_group_terminal: Bounded<OwnedLowerMacEvent<'id, B>>,
    /// The receive buffers out with the body, queued or held.
    rx_out: Cell<usize>,
    /// Received frames dropped since the stream last said so.
    lost: Cell<bool>,
    body: Cell<Option<Waker>>,
    /// The backend alone attests the hardware barrier with the one gate place.
    gate_issuer: GateIssuer<'id, ModelPool<<B as ModelComposition>::Identity, 0>>,
    /// Disable asks the driver to return all queued operations before the
    /// owner ends the phase. Unlike poison, this is an ordinary terminal.
    draining: Cell<bool>,
    returned: Cell<bool>,
}

impl<'id, B: ModelComposition> ModelHeld<'id, B> {
    fn command(&self, command: Command<'id, B>) {
        self.returned.set(false);
        if self.commands.push(command).is_err() {
            self.poison();
        }
        self.air.wake_driver();
    }

    fn post(&self, event: OwnedLowerMacEvent<'id, B>) {
        if self.events.push(event).is_err() {
            self.poison();
        }
        self.wake_body();
    }

    fn take_event(&self) -> Option<OwnedLowerMacEvent<'id, B>> {
        self.events.pop()
    }

    fn wake_body(&self) {
        if let Some(waker) = self.body.take() {
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

mod lifecycle;
pub use lifecycle::{ModelBackend, ModelFrontend, ModelSetup, ModelVacant};

/// The composition's initial software depth for each access category.
pub const MODEL_TX_QUEUE_DEPTH: core::num::NonZeroU8 = core::num::NonZeroU8::MIN.saturating_add(1);

/// The driver retains the unique open place throughout the closing barrier.
enum DriverGate<'id, B: ModelComposition> {
    Open,
    Closing(GateOpen<'id, ModelPool<<B as ModelComposition>::Identity, 0>>),
    Closed,
}

struct Removal<'id, B: ModelComposition> {
    key: RawKeyIdentity,
    removed: OwnedRemoved<'id, B>,
    group_admitted_at: Option<u64>,
}

/// The model's hardware, which only the driver owns.
struct Driver<'id, B: ModelComposition> {
    /// The prepared attempts of each queue, the head first.
    queues: [TxQueue<'id, B>; WmmAccessCategory::ALL.len()],
    /// The installed hardware keys.
    keys: Vec<RawKeyIdentity>,
    removing: Vec<Removal<'id, B>>,
    gate: DriverGate<'id, B>,
}

impl<'id, B: ModelComposition> Driver<'id, B> {
    fn observation(&self) -> DriverObservation {
        DriverObservation {
            publications: self
                .queues
                .iter()
                .filter(|queue| queue.air.is_some())
                .count(),
            installed_keys: self.keys.len(),
            group_removals: self
                .removing
                .iter()
                .filter_map(|removal| match &removal.removed {
                    OwnedRemoved::Station(OwnedRemovedKey::Group { slot, permission }) => {
                        Some(GroupRemovalObservation {
                            role: VifRole::Station,
                            slot: slot.index(),
                            permission: permission.index(),
                        })
                    }
                    OwnedRemoved::AccessPoint(OwnedRemovedKey::Group { slot, permission }) => {
                        Some(GroupRemovalObservation {
                            role: VifRole::AccessPoint,
                            slot: slot.index(),
                            permission: permission.index(),
                        })
                    }
                    _ => None,
                })
                .collect(),
        }
    }

    fn new() -> Self {
        Self {
            queues: core::array::from_fn(|_| TxQueue {
                waiting: VecDeque::new(),
                air: None,
            }),
            keys: Vec::new(),
            removing: Vec::new(),
            gate: DriverGate::Open,
        }
    }

    /// Serve everything posted: the settings, the commands in order, the
    /// queues, then the air.
    fn serve(&mut self, held: &ModelHeld<'id, B>) {
        loop {
            if held.air.failed() {
                return;
            }
            // The settings come before the next command.
            let settings = held.settings.take();
            for (slot, wanted) in settings.block_acks {
                let mut applied = held.air.receive_agreements.take();
                match wanted {
                    Some(agreement) => {
                        applied.insert(slot, agreement);
                    }
                    None => {
                        applied.remove(&slot);
                    }
                }
                held.air.receive_agreements.set(applied);
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
                            .push_front(Command::Configure(permission));
                        break;
                    }
                    held.air
                        .settings
                        .set(held.air.settings.get().saturating_add(1));
                    held.post(OwnedLowerMacEvent::Terminal(
                        OwnedLowerMacTerminal::Configured(permission),
                    ));
                }
                Command::Submit(mut prepared) => {
                    if prepared
                        .operation
                        .execute()
                        .mpdu
                        .project_admission(&held.air)
                        .is_err()
                    {
                        let _fault = held.quarantined.push(Work::Single(prepared));
                        held.poison();
                        return;
                    }
                    let queue = prepared.queue;
                    queue
                        .select_mut(&mut self.queues)
                        .waiting
                        .push_back(Work::Single(prepared));
                }
                Command::SubmitAmpdu(mut aggregate) => {
                    if aggregate
                        .operation
                        .execute()
                        .subframes
                        .iter_mut()
                        .any(|subframe| subframe.mpdu.project_admission(&held.air).is_err())
                    {
                        let _fault = held.quarantined.push(Work::Aggregate(aggregate));
                        held.poison();
                        return;
                    }
                    let queue = aggregate.queue;
                    queue
                        .select_mut(&mut self.queues)
                        .waiting
                        .push_back(Work::Aggregate(aggregate));
                }
                Command::InstallKey {
                    spp,
                    key,
                    material,
                    installed,
                } => {
                    held.air.apply_association(key.0.role, spp);
                    held.air.install_material(&key, material);
                    self.keys.push(key);
                    held.post(OwnedLowerMacEvent::Terminal(
                        OwnedLowerMacTerminal::PairwiseKeyInstalled(installed),
                    ));
                }
                Command::Group {
                    admitted_at,
                    change,
                } => {
                    if admitted_at
                        .checked_add(MODEL_GROUP_COMMAND_BOUND.as_micros())
                        .is_none_or(|deadline| held.air.monotonic_now().as_micros() >= deadline)
                        || held.air.group_failure.get() == Some(GroupFaultStage::Before)
                    {
                        let _capacity_fault = held.failed_group.push(change);
                        held.poison();
                        return;
                    }
                    if held.air.group_held.get() && !held.draining.get() {
                        // We popped exactly this cell; restore it without creating
                        // capacity or allowing a later command to pass it.
                        held.commands.items.push_front(Command::Group {
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
                    let closed = held.gate_issuer.close(gate);
                    held.post(OwnedLowerMacEvent::Terminal(
                        OwnedLowerMacTerminal::GateClosed(closed),
                    ));
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

    fn activate_group(&mut self, held: &ModelHeld<'id, B>, identity: &RawKeyIdentity, write: bool) {
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
        held: &ModelHeld<'id, B>,
        pending: PendingGroupInstall<'id, B, R>,
    ) -> OwnedLowerMacEvent<'id, B>
    where
        R: GroupInstallationRole,
    {
        held.air.apply_association(R::ROLE, pending.spp);
        let identity = RawKeyIdentity::group(
            pending.slot.index(),
            pending.key_id,
            R::ROLE,
            pending.key,
            &held.air,
        );
        self.activate_group(held, &identity, true);
        OwnedLowerMacEvent::Terminal(OwnedLowerMacTerminal::GroupKeyInstalled(R::installed(
            ModelGroupKey {
                identity,
                slot: pending.slot,
            },
            pending.permission,
        )))
    }

    fn group_change(
        &mut self,
        held: &ModelHeld<'id, B>,
        admitted_at: u64,
        change: GroupChange<'id, B>,
    ) {
        // Snapshot and mutate before any wake can reenter ModelAir. This
        // single-threaded model has no other interleaving during these steps.
        // New-key ingress captured by delivery callbacks stays in the live
        // queues until after this command's terminal. No drain-to-idle loop
        // can postpone the boundary under continuous traffic.
        let received = held.air.rx.take_batch();
        let captured = held.air.group_rx.take_batch();
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
                if held.air.group_failure.get() == Some(GroupFaultStage::ReplacementWrite) {
                    let mut table = held.air.key_installations.take();
                    let Some(index) = table.iter().position(|entry| {
                        entry.identity.ptr_eq(&Rc::downgrade(&installed.identity.0))
                    }) else {
                        held.air.key_installations.set(table);
                        let _capacity_fault = held.failed_group.push(GroupChange::Replace {
                            permission,
                            installed,
                            key_id,
                            key,
                        });
                        held.poison();
                        return;
                    };
                    let previous = table.remove(index);
                    installed.identity.0.active.set(false);
                    installed.identity.0.rx_active.set(false);
                    // Failed new publication leaves no new epoch. The old
                    // material is written back before fail-stop, not returned
                    // as an ordinary installed owner.
                    table.push(previous);
                    installed.identity.0.active.set(true);
                    installed.identity.0.rx_active.set(true);
                    held.air
                        .group_writes
                        .set(held.air.group_writes.get().saturating_add(1));
                    held.air.group_rollback.set(Some(
                        oer_ieee80211_testing_conformance::group_keys::Rollback {
                            previous_key_id: installed.identity.0.key_id,
                            attempted_key_id: key_id.get(),
                            restored: table.iter().any(|entry| {
                                entry.identity.ptr_eq(&Rc::downgrade(&installed.identity.0))
                            }),
                        },
                    ));
                    held.air.key_installations.set(table);
                    let _capacity_fault = held.failed_group.push(GroupChange::Replace {
                        permission,
                        installed,
                        key_id,
                        key,
                    });
                    held.poison();
                    return;
                }
                installed.identity.0.active.set(false);
                installed.identity.0.rx_active.set(false);
                held.air.remove_key_material(&installed.identity);
                self.keys.retain(|key| *key != installed.identity);
                let identity = RawKeyIdentity::group(
                    installed.slot.index(),
                    key_id,
                    VifRole::Station,
                    key,
                    &held.air,
                );
                self.activate_group(held, &identity, true);
                Some(OwnedLowerMacEvent::Terminal(
                    OwnedLowerMacTerminal::GroupKeyReplaced {
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
                Some(OwnedLowerMacEvent::Terminal(
                    OwnedLowerMacTerminal::GroupKeySelected {
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
    fn publish(&mut self, held: &ModelHeld<'id, B>) {
        self.publish_before(held, |_| false);
    }

    fn publish_before(&mut self, held: &ModelHeld<'id, B>, reconfigure: impl Fn(VifRole) -> bool) {
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
                        .monotonic_now()
                        .as_micros()
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

    fn finish_removals(&mut self, held: &ModelHeld<'id, B>) {
        // The single reference belongs to KeyInstall. Additional owners are
        // actual prepared/queued/published attempts. Returned Sent identities
        // do not keep a preparation alive and do not delay key-slot reuse.
        let mut pending = Vec::new();
        for removal in self.removing.drain(..) {
            let overdue = removal.group_admitted_at.is_some_and(|admitted| {
                admitted
                    .checked_add(MODEL_GROUP_COMMAND_BOUND.as_micros())
                    .is_none_or(|deadline| held.air.monotonic_now().as_micros() >= deadline)
            });
            let uncertain = removal.group_admitted_at.is_some()
                && held.air.group_failure.get() == Some(GroupFaultStage::After);
            if overdue || uncertain || held.air.failed() {
                pending.push(removal);
                held.poison();
            } else if Rc::strong_count(&removal.key.0.preparations) == 1 {
                held.air.remove_key_material(&removal.key);
                self.keys.retain(|installed| *installed != removal.key);
                held.post(OwnedLowerMacEvent::Terminal(
                    OwnedLowerMacTerminal::KeyRemoved(removal.removed),
                ));
            } else {
                pending.push(removal);
            }
        }
        self.removing = pending;
    }

    fn deliver(&mut self, held: &ModelHeld<'id, B>, frame: Vec<u8>, meta: ModelRxMeta) {
        if held.charge_rx() {
            held.post(OwnedLowerMacEvent::Received {
                frame: OwnedRxBuffer {
                    identity: PhantomData,
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
    fn receive(&mut self, held: &ModelHeld<'id, B>) {
        let captured = held.air.rx.take_batch();
        for (frame, meta) in captured {
            self.deliver(held, frame, meta);
        }
        let captured = held.air.group_rx.take_batch();
        for received in captured {
            if received.stage == GroupRxStage::Ready {
                // Integrity was established at capture under this owner.
                // Retain it through delivery even if the slot is later reused.
                let _installation = received.installation;
                self.deliver(held, received.frame, received.meta);
            } else {
                held.air.group_rx.push_back(received);
            }
        }
    }
}

/// Synthetic model latency, in model time; hardware backends declare their
/// own PPDU + response/timeout bound. It does not scale with retry-plan length.
pub const MODEL_PUBLICATION_BOUND: oer_time::Duration = oer_time::Duration::from_millis(1);
/// Synthetic admission-to-GTK-terminal bound, driven by the supplied clock's owner.
pub const MODEL_GROUP_COMMAND_BOUND: oer_time::Duration = oer_time::Duration::from_millis(2);

struct TxQueue<'id, B: ModelComposition> {
    waiting: VecDeque<Work<'id, B>>,
    air: Option<Publication<'id, B>>,
}
struct Publication<'id, B: ModelComposition> {
    started: u64,
    work: Published<'id, B>,
}
struct SinglePlan<'id, B: ModelComposition> {
    prepared: Box<Prepared<'id, B>>,
    remaining: VecDeque<PhyRate>,
    last: Option<(NonZeroU16, PhyRate)>,
    rate: PhyRate,
}
enum Published<'id, B: ModelComposition> {
    Single {
        plan: SinglePlan<'id, B>,
        answer: SingleAnswer,
    },
    Aggregate {
        prepared: PreparedAggregate<'id, B>,
        report: BlockAckReport,
        answer: AggregateAnswer,
    },
}
/// Derive the terminal from the actual lifecycle barrier and key owner;
/// callers cannot exchange three unrelated boolean arguments. Disable wins
/// over key removal, which wins over a closing or closed gate.
fn withdrawal<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    gate: &DriverGate<'id, B>,
    key: Option<&RawKeyIdentity>,
    removals: &[Removal<'id, B>],
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
impl<'id, B: ModelComposition> Published<'id, B> {
    fn role(&self) -> VifRole {
        match self {
            Self::Single { plan, .. } => plan.prepared.role,
            Self::Aggregate { prepared, .. } => prepared.role,
        }
    }
    fn key(&self) -> Option<&RawKeyIdentity> {
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
        held: &ModelHeld<'id, B>,
        keys: &[RawKeyIdentity],
        cause: Option<WithdrawCause>,
    ) -> Option<Publication<'id, B>> {
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
                    held.air.edca.failure(plan.prepared.queue);
                    plan.rate = rate;
                    return publish_single(held, keys, plan);
                }
                held.air.edca.reset(plan.prepared.queue);
                terminal(held, *plan.prepared, outcome);
            }
            Self::Aggregate {
                prepared,
                report,
                answer,
            } => {
                let outcome = aggregate_outcome(answer, prepared.rate, report, cause);
                held.air.edca.reset(prepared.queue);
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

fn start_work<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    keys: &[RawKeyIdentity],
    work: Work<'id, B>,
) -> Option<Publication<'id, B>> {
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
fn publish_single<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    keys: &[RawKeyIdentity],
    mut plan: SinglePlan<'id, B>,
) -> Option<Publication<'id, B>> {
    let now = held.air.monotonic_now();
    let lifetime = held
        .air
        .edca(plan.prepared.role)
        .access_category(plan.prepared.queue)
        .msdu_lifetime;
    if plan.prepared.operation.payload().expired(now, lifetime) {
        held.air.expired(1);
        held.air.edca.reset(plan.prepared.queue);
        terminal(held, *plan.prepared, TxOutcome::Expired { msdus: 1 });
        return None;
    }
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
        plan.prepared.operation.ticket().index(),
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
    held.air.edca.draw(plan.prepared.role, plan.prepared.queue);
    // A denied PPDU never reached the air the journal records.
    if !matches!(answer, SingleAnswer::AirDenied) {
        held.air.sent.record(frame);
    }
    Some(Publication {
        started: held.air.monotonic_now().as_micros(),
        work: Published::Single { plan, answer },
    })
}

/// Preserve the submission shape and every owner on withdrawal.
fn withdrawn<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    work: Work<'id, B>,
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

fn terminal_aggregate<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    aggregate: PreparedAggregate<'id, B>,
    completion: ModelAggregateOutcome,
) {
    held.post(OwnedLowerMacEvent::Terminal(
        OwnedLowerMacTerminal::TxCompleted(OwnedTxCompleted::Aggregate(
            aggregate.operation.complete(completion),
        )),
    ));
}

fn subframe_bit(index: usize) -> Result<u64, ModelFault> {
    let shift = u32::try_from(index).map_err(|_| ModelFault)?;
    1_u64.checked_shl(shift).ok_or(ModelFault)
}

fn publish_aggregate<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    keys: &[RawKeyIdentity],
    mut aggregate: PreparedAggregate<'id, B>,
) -> Option<Publication<'id, B>> {
    let now = held.air.monotonic_now();
    let lifetime = held
        .air
        .edca(aggregate.role)
        .access_category(aggregate.queue)
        .msdu_lifetime;
    let expired = aggregate.operation.payload().0.iter().enumerate().try_fold(
        0_u64,
        |mask, (index, subframe)| {
            let bit = subframe_bit(index)?;
            Ok(if subframe.mpdu.expired(now, lifetime) {
                mask | bit
            } else {
                mask
            })
        },
    );
    let expired = match expired {
        Ok(expired) => expired,
        Err(ModelFault) => {
            let _fault = held.quarantined.push(Work::Aggregate(aggregate));
            held.poison();
            return None;
        }
    };
    if expired != 0 {
        held.air.expired(expired);
        held.air.edca.reset(aggregate.queue);
        terminal_aggregate(held, aggregate, TxOutcome::Expired { msdus: expired });
        return None;
    }
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
    let ticket = aggregate.operation.ticket().index();
    let frames = match aggregate.operation.execute().publish(
        ticket,
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
        let bit = match subframe_bit(index) {
            Ok(bit) => bit,
            Err(ModelFault) => {
                let _fault = held.quarantined.push(Work::Aggregate(aggregate));
                held.poison();
                return None;
            }
        };
        if acknowledged & bit != 0 {
            bitmap |= subframe.acknowledgement_bit;
        }
    }
    held.air.edca.draw(aggregate.role, aggregate.queue);
    if !matches!(answer, AggregateAnswer::AirDenied) {
        held.air.sent.record_all(frames);
    }
    // NonEmptyAggregate carries the first sequence into PreparedAggregate below.
    let report = BlockAckReport {
        start_sequence: aggregate.start_sequence,
        bitmap,
    };
    Some(Publication {
        started: held.air.monotonic_now().as_micros(),
        work: Published::Aggregate {
            prepared: aggregate,
            report,
            answer,
        },
    })
}

/// Post the terminal of `prepared`, with its tokens.
fn terminal<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    prepared: Prepared<'id, B>,
    outcome: ModelSingleOutcome,
) {
    held.post(OwnedLowerMacEvent::Terminal(
        OwnedLowerMacTerminal::TxCompleted(OwnedTxCompleted::Single(
            prepared.operation.complete(outcome),
        )),
    ));
}

/// The model's running phase: the body's handle over the queues.
pub struct ModelPhase<'a, 'id, C, B: ModelComposition + 'id> {
    frontend: &'a mut ModelFrontend<'id, B>,
    caps: PhantomData<fn() -> C>,
    held: &'a ModelHeld<'static, B>,
}

impl<'id, C, B: ModelComposition + 'id> ModelPhase<'_, 'id, C, B> {
    /// Deliberately violate the backend contract while retaining actual minted
    /// owners, for consumer contract regressions outside the conformance suite.
    pub fn inject_terminal(
        &mut self,
        terminal: oer_ieee80211_contracts_port::LowerMacTerminal<'id, ModelTypes<B>>,
    ) {
        self.held
            .post(OwnedLowerMacEvent::Terminal(OwnedLowerMacTerminal::retain(
                terminal,
                &self.frontend.loan,
            )));
    }
}

/// Install a station PTK and take the key its installation terminal
/// returns, recycling RX delivered before it.
#[cfg(any(test, feature = "test-support"))]
pub(crate) async fn installed_station_pairwise<'p, 'id: 'p, C, B: ModelComposition + 'id>(
    phase: &mut ModelPhase<'p, 'id, C, B>,
    association: &ModelAssociation<'id, B, Station>,
    slot: PairwiseKeySlot<
        'id,
        Station,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        <Station as ModelRole>::AssociationPath<B::Identity>,
    >,
    peer: IndividualAddress,
    key: Ccmp128Key,
) -> Result<ModelPairwiseKey<'id, B, Station>, &'static str>
where
    C: Ieee80211Capabilities,
{
    phase
        .station()
        .install_pairwise_key(association, slot, peer, key);
    loop {
        match phase.next_event().await {
            Ok(Ok(oer_ieee80211_contracts_port::LowerMacEvent::Terminal(
                oer_ieee80211_contracts_port::LowerMacTerminal::PairwiseKeyInstalled(
                    oer_ieee80211_contracts_port::InstalledPairwise::Station(key),
                ),
            ))) => return Ok(key),
            Ok(Ok(oer_ieee80211_contracts_port::LowerMacEvent::Received { frame, .. })) => {
                phase.recycle(frame)
            }
            _ => return Err("the station PTK installation terminal"),
        }
    }
}

/// Install an access point PTK and take the key its installation terminal
/// returns, recycling RX delivered before it.
#[cfg(test)]
pub(crate) async fn installed_access_point_pairwise<'p, 'id: 'p, C, B: ModelComposition + 'id>(
    phase: &mut ModelPhase<'p, 'id, C, B>,
    association: &ModelAssociation<'id, B, AccessPoint>,
    slot: PairwiseKeySlot<
        'id,
        AccessPoint,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        <AccessPoint as ModelRole>::AssociationPath<B::Identity>,
    >,
    peer: IndividualAddress,
    key: Ccmp128Key,
) -> Result<ModelPairwiseKey<'id, B, AccessPoint>, &'static str>
where
    C: Ieee80211Capabilities,
{
    phase
        .access_point()
        .install_pairwise_key(association, slot, peer, key);
    loop {
        match phase.next_event().await {
            Ok(Ok(oer_ieee80211_contracts_port::LowerMacEvent::Terminal(
                oer_ieee80211_contracts_port::LowerMacTerminal::PairwiseKeyInstalled(
                    oer_ieee80211_contracts_port::InstalledPairwise::AccessPoint(key),
                ),
            ))) => return Ok(key),
            Ok(Ok(oer_ieee80211_contracts_port::LowerMacEvent::Received { frame, .. })) => {
                phase.recycle(frame)
            }
            _ => return Err("the access point PTK installation terminal"),
        }
    }
}

impl<'id, C, B: ModelComposition + 'id> PortPhase for ModelPhase<'_, 'id, C, B> {
    type Event = oer_ieee80211_contracts_port::LowerMacEvent<'id, ModelTypes<B>>;
    type Fault = ModelFault;

    fn next_event(
        &mut self,
    ) -> impl Future<Output = Result<Result<Self::Event, EventsLost>, EventEnd<ModelFault>>> + '_
    {
        let held = self.held;
        let loan = self.frontend.loan;
        // Cancel-safe: the event stays queued until a poll takes it.
        poll_fn(move |cx| {
            if let ModelStream::Ended(end) = held.air.stream.get() {
                return Poll::Ready(Err(end));
            }
            if let Some(event) = held.take_event() {
                return Poll::Ready(Ok(Ok(event.lend(&loan))));
            }
            if held.lost.replace(false) {
                return Poll::Ready(Ok(Err(EventsLost)));
            }
            if matches!(held.air.stream.get(), ModelStream::Draining) {
                if held.returned.get()
                    && !held.draining.get()
                    && held.commands.is_empty()
                    && held.recycled.is_empty()
                {
                    let end = EventEnd::Drained { cause: ModelFault };
                    held.air.stream.set(ModelStream::Ended(end));
                    held.air.wake_driver();
                    return Poll::Ready(Err(end));
                }
                held.body.set(Some(cx.waker().clone()));
                held.air.wake_driver();
                return Poll::Pending;
            }
            if held.rx_out.get() < MODEL_RX_EVENTS
                && let Some((bytes, meta)) = held.air.rx_stream.snapshot()
                && held.charge_rx()
            {
                return Poll::Ready(Ok(Ok(OwnedLowerMacEvent::Received {
                    frame: OwnedRxBuffer {
                        identity: PhantomData,
                        bytes,
                        recycled: false,
                        air: Rc::downgrade(&held.air),
                        streamed: true,
                    },
                    meta,
                }
                .lend(&loan))));
            }
            held.body.set(Some(cx.waker().clone()));
            Poll::Pending
        })
    }
}

impl<'p, 'id: 'p, C: Ieee80211Capabilities, B: ModelComposition + 'id> LowerMacPhase<'p, 'id>
    for ModelPhase<'p, 'id, C, B>
{
    type Caps = C;
    type Types = ModelTypes<B>;
    type Station<'v>
        = ModelInterface<'p, 'v, 'id, Station, C, B>
    where
        Self: 'v;
    type AccessPoint<'v>
        = ModelInterface<'p, 'v, 'id, AccessPoint, C, B>
    where
        Self: 'v;

    fn channel(&self) -> Channel {
        self.frontend.channel.get()
    }

    fn recycle(&mut self, buffer: ModelRxBuffer<'id, B::Identity>) {
        let buffer = buffer.owner;
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

    fn close_gate(&mut self, gate: GateOpen<'id, ModelPool<<B as ModelComposition>::Identity, 0>>) {
        self.held
            .command(Command::CloseGate(gate.into_owned(&self.frontend.loan)));
    }

    fn open_gate(
        &mut self,
        gate: GateClosed<'id, ModelPool<<B as ModelComposition>::Identity, 0>>,
    ) -> GateOpen<'id, ModelPool<<B as ModelComposition>::Identity, 0>> {
        let open = self
            .held
            .gate_issuer
            .open(gate.into_owned(&self.frontend.loan));
        self.held.command(Command::OpenGate);
        open.lend(&self.frontend.loan)
    }

    fn station(&mut self) -> ModelInterface<'p, '_, 'id, Station, C, B> {
        let [station, _] = self.frontend.addresses;
        ModelInterface {
            held: self.held,
            loan: self.frontend.loan,
            address: station,
            role: PhantomData,
        }
    }

    fn access_point(&mut self) -> ModelInterface<'p, '_, 'id, AccessPoint, C, B> {
        let [_, access_point] = self.frontend.addresses;
        ModelInterface {
            held: self.held,
            loan: self.frontend.loan,
            address: access_point,
            role: PhantomData,
        }
    }
}

/// One interface of the model's phase.
pub struct ModelInterface<'p, 'v, 'id, R, C, B: ModelComposition + 'id> {
    held: &'v ModelHeld<'static, B>,
    loan: BodyLoan<'id, B::Identity, B>,
    address: IndividualAddress,
    role: PhaseProof<'p, (R, C)>,
}

/// A key the model installed.
pub struct ModelPairwiseKey<'id, B: ModelComposition, R: ModelRole> {
    identity: RawKeyIdentity,
    slot:
        PairwiseKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath<B::Identity>>,
}

/// A group key the model installed.
pub struct ModelGroupKey<'id, B: ModelComposition, R: ModelRole> {
    identity: RawKeyIdentity,
    slot: GroupKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath<B::Identity>>,
}

/// A receive Block Ack agreement the model keeps.
pub struct ModelBlockAck<'id, B: ModelComposition> {
    slot: ModelBlockAckSlot<'id, B>,
}

/// A retry plan's stages, as the model's hardware takes them.
fn stages<C: Ieee80211Capabilities>(
    frame: &TxFrame<'_, '_, C>,
) -> oer_ieee80211_contracts_port::RetryPlan<PhyRate> {
    match frame {
        TxFrame::Individual { plan, .. } => plan.map(|rate| rate.get()),
        TxFrame::IndividualNoAck { rate, .. } => one_stage(rate.rate().get()),
        TxFrame::Group { rate, .. } => one_stage(rate.get()),
        TxFrame::BlockAckRequest { plan, .. } => plan.map(|rate| rate.rate().get()),
    }
}
fn one_stage(rate: PhyRate) -> oer_ieee80211_contracts_port::RetryPlan<PhyRate> {
    oer_ieee80211_contracts_port::RetryPlan::new(oer_ieee80211_contracts_port::RetryStage {
        rate,
        transmissions: core::num::NonZeroU8::MIN,
        protection: oer_ieee80211_contracts_port::Protection::None,
    })
}

fn retry_stages<K>(
    layout: &MpduLayout,
    plan: &RetransmissionPlan<'_, '_, K>,
) -> Result<oer_ieee80211_contracts_port::RetryPlan<PhyRate>, PreparationError> {
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
type ModelSource<'k, 'p, 'id, C, K, R, B> = TxSource<
    ModelBuffer<'id, B, C>,
    ModelSent<'id, B, C>,
    oer_ieee80211_contracts_port::ClassFrame<'p, 'id, C, K>,
    ModelBodyOf<B>,
    <R as RoleKeys<'id, ModelTypes<B>>>::TxKey<'k>,
    RetransmissionPlan<'p, 'id, K>,
>;
type ModelCheckedSource<'k, 'p, 'id, C, K, R, B> = CheckedTxSource<
    ModelBuffer<'id, B, C>,
    ModelSent<'id, B, C>,
    oer_ieee80211_contracts_port::ClassFrame<'p, 'id, C, K>,
    ModelBodyOf<B>,
    <R as RoleKeys<'id, ModelTypes<B>>>::TxKey<'k>,
    RetransmissionPlan<'p, 'id, K>,
>;
type ModelPreparation<'k, 'p, 'id, R, K, B, C> = Result<
    ModelPrepared<'k, 'p, 'id, R, K, B>,
    PreparationRefused<ModelSource<'k, 'p, 'id, C, K, R, B>>,
>;

/// Closed preparation proof retaining role, phase, install and the key borrow.
/// Dropping or submitting it ends that borrow; a queued operation owns only
/// the backend identity, never a reference to a portable machine.
pub struct ModelPrepared<'k, 'p, 'id, R, K, B: ModelComposition> {
    loan: BodyLoan<'id, B::Identity, B>,
    mpdu: ModelMpdu<'id, B>,
    plan: oer_ieee80211_contracts_port::RetryPlan<PhyRate>,
    key: Option<PreparedKey>,
    key_borrow: PhantomData<&'k ()>,
    proof: PhaseProof<'p, (R, K, B)>,
}
/// Metadata lowered before single-MPDU admission; it owns no payload.
pub struct ModelTxExecution {
    plan: oer_ieee80211_contracts_port::RetryPlan<PhyRate>,
    key: Option<PreparedKey>,
}
impl<'id, R, K, B: ModelComposition + 'id> PreparedTx<'id> for ModelPrepared<'_, '_, 'id, R, K, B> {
    type Types = ModelTypes<B>;
    type Backend = B;
    type OwnedContext = ModelContextOf<B>;
    type OwnedPayload = ModelMpdu<'static, B>;
    type Execution = ModelTxExecution;
    fn into_admission(
        self,
        context: <Self::Types as PhaseTypes<'id>>::Context,
        ticket: Slot<'id, ModelTickets, ModelPool<B::Identity, 1>>,
        _: oer_ieee80211_contracts_port::Admission<Self>,
    ) -> oer_ieee80211_contracts_port::AdmissionOwners<
        Self::OwnedContext,
        Slot<'static, ModelTickets, ModelPool<B::Identity, 1>>,
        Self::OwnedPayload,
        Self::Execution,
    > {
        let loan = self.loan;
        oer_ieee80211_contracts_port::AdmissionOwners {
            context: context.enclose().into_owned(&loan),
            ticket: loan.reclaim_slot(ticket),
            payload: self.mpdu.into_owned(&loan),
            execution: ModelTxExecution {
                plan: self.plan,
                key: self.key,
            },
        }
    }
    fn into_parts(self) -> (ModelAnyBuffer<'id, B>, Option<ModelBodyOf<B>>) {
        self.mpdu.into_parts()
    }
}

/// A model buffer class and its unique pool within the installation layout.
pub trait ModelClass: TxBufferClass + Sized {
    /// Nominal component of the backing memory slots.
    type Path<Id: SlotIdentity>: oer_radio_port::PoolPath<Identity = Id>;
    /// Return fresh memory through the terminal's exhaustive buffer enum.
    fn erase<'id, B: ModelComposition>(buffer: ModelBuffer<'id, B, Self>)
    -> ModelAnyBuffer<'id, B>;
    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent<'id, B: ModelComposition>(
        mpdu: ModelSent<'id, B, Self>,
    ) -> ModelAnyBuffer<'id, B>;
    /// Recover fresh memory of this class, or preserve the other owner.
    fn class<'id, B: ModelComposition>(
        buffer: ModelAnyBuffer<'id, B>,
    ) -> Result<ModelBuffer<'id, B, Self>, ModelAnyBuffer<'id, B>>;
    /// Recover a published MPDU of this class, or preserve the other owner.
    fn sent<'id, B: ModelComposition>(
        buffer: ModelAnyBuffer<'id, B>,
    ) -> Result<ModelSent<'id, B, Self>, ModelAnyBuffer<'id, B>>;
}

impl ModelClass for ModelSmall {
    type Path<Id: SlotIdentity> = ModelPool<Id, 2>;
    fn erase<'id, B: ModelComposition>(
        buffer: ModelBuffer<'id, B, Self>,
    ) -> ModelAnyBuffer<'id, B> {
        ModelAnyBuffer::Small(buffer)
    }
    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent<'id, B: ModelComposition>(
        mpdu: ModelSent<'id, B, Self>,
    ) -> ModelAnyBuffer<'id, B> {
        ModelAnyBuffer::SmallSent(mpdu)
    }
    /// Recover fresh memory of this class, or preserve the other owner.
    fn class<'id, B: ModelComposition>(
        buffer: ModelAnyBuffer<'id, B>,
    ) -> Result<ModelBuffer<'id, B, Self>, ModelAnyBuffer<'id, B>> {
        match buffer {
            ModelAnyBuffer::Small(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
    /// Recover a published MPDU of this class, or preserve the other owner.
    fn sent<'id, B: ModelComposition>(
        buffer: ModelAnyBuffer<'id, B>,
    ) -> Result<ModelSent<'id, B, Self>, ModelAnyBuffer<'id, B>> {
        match buffer {
            ModelAnyBuffer::SmallSent(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
}
impl ModelClass for ModelLarge {
    type Path<Id: SlotIdentity> = ModelPool<Id, 3>;
    fn erase<'id, B: ModelComposition>(
        buffer: ModelBuffer<'id, B, Self>,
    ) -> ModelAnyBuffer<'id, B> {
        ModelAnyBuffer::Large(buffer)
    }
    /// Return a published MPDU through the terminal buffer enum.
    fn erase_sent<'id, B: ModelComposition>(
        mpdu: ModelSent<'id, B, Self>,
    ) -> ModelAnyBuffer<'id, B> {
        ModelAnyBuffer::LargeSent(mpdu)
    }
    /// Recover fresh memory of this class, or preserve the other owner.
    fn class<'id, B: ModelComposition>(
        buffer: ModelAnyBuffer<'id, B>,
    ) -> Result<ModelBuffer<'id, B, Self>, ModelAnyBuffer<'id, B>> {
        match buffer {
            ModelAnyBuffer::Large(buffer) => Ok(buffer),
            buffer => Err(buffer),
        }
    }
    /// Recover a published MPDU of this class, or preserve the other owner.
    fn sent<'id, B: ModelComposition>(
        buffer: ModelAnyBuffer<'id, B>,
    ) -> Result<ModelSent<'id, B, Self>, ModelAnyBuffer<'id, B>> {
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
    R: RoleKeys<'id, ModelTypes<B>>,
    K: Ieee80211Capabilities,
    B: ModelComposition + 'id,
    C: ModelClass,
>(
    source: ModelCheckedSource<'k, 'p, 'id, C, K, R, B>,
) -> ModelPreparation<'k, 'p, 'id, R, K, B, C> {
    let (mpdu, plan, key, loan) = match source {
        CheckedTxSource::Fresh(fresh) => {
            let (fresh, layout) = fresh.into_parts();
            let Some(loan) = fresh.buffer.loan else {
                return Err(PreparationRefused {
                    reason: PreparationError::MissingBodyLoan,
                    operation: {
                        let FreshSource {
                            buffer,
                            frame,
                            body,
                            protection,
                        } = fresh;
                        TxSource::Fresh {
                            buffer,
                            frame,
                            body,
                            protection,
                        }
                    },
                });
            };
            let FreshSource {
                buffer,
                frame,
                body,
                protection,
            } = fresh;
            let (key, packet_number) = match protection {
                TxProtection::Open => (None, None),
                TxProtection::Ccmp(numbered) => (
                    Some(R::tx_identity(&numbered.key).into_owned(&loan)),
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
                loan,
            )
        }
        CheckedTxSource::Retransmission { mpdu, plan } => {
            let Some(loan) = mpdu.buffer.loan else {
                return Err(PreparationRefused {
                    reason: PreparationError::MissingBodyLoan,
                    operation: TxSource::Retransmission { mpdu, plan },
                });
            };
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
                loan,
            )
        }
    };
    Ok(ModelPrepared {
        loan,
        mpdu,
        plan,
        key: key.map(PreparedKey::new),
        key_borrow: PhantomData,
        proof: PhantomData,
    })
}

impl<'p, 'v, 'id: 'p, R, K: Ieee80211Capabilities, B: ModelComposition + 'id>
    InterfacePhase<'p, 'id> for ModelInterface<'p, 'v, 'id, R, K, B>
where
    R: GroupInstallationRole + RoleKeys<'id, ModelTypes<B>, Keys = ModelKeys<'id, B, R>>,
{
    type Caps = K;
    type Types = ModelTypes<B>;
    type Role = R;

    fn address(&self) -> IndividualAddress {
        self.address
    }

    /// The model applies each interface's set exactly.
    fn set_edca(&mut self, parameters: SupportedEdca<K>) -> SupportedEdca<K> {
        self.held.air.edca.set_parameters(R::ROLE, parameters.get());
        self.held.settings.mark_other();
        self.held.air.wake_driver();
        parameters
    }

    fn submit<'k>(
        &mut self,
        attempt: oer_ieee80211_contracts_port::InterfaceSubmission<'k, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<ModelFault>> {
        let mut attempt = attempt.into_parts(&self.loan);
        attempt
            .operation
            .execute()
            .mpdu
            .admit(self.held.air.monotonic_now());
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
        association: &ModelAssociation<'id, B, R>,
        slot: PairwiseKeySlot<
            'id,
            R,
            MODEL_KEY_SLOTS,
            MODEL_KEY_SLOTS,
            R::AssociationPath<B::Identity>,
        >,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) {
        let identity = RawKeyIdentity::new(slot.index(), 0, R::ROLE, peer, &self.held.air);
        self.held.command(Command::InstallKey {
            spp: association.owner.spp(),
            material: key,
            key: identity.clone(),
            installed: R::pairwise_installed(ModelPairwiseKey {
                identity,
                slot: slot.into_owned(&self.loan),
            }),
        });
    }

    fn install_group_key(
        &mut self,
        association: &ModelAssociation<'id, B, R>,
        permission: Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath<B::Identity>>,
        slot: GroupKeySlot<
            'id,
            R,
            MODEL_KEY_SLOTS,
            MODEL_KEY_SLOTS,
            R::AssociationPath<B::Identity>,
        >,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.monotonic_now().as_micros(),
            change: GroupChange::Install(R::installation(PendingGroupInstall {
                spp: association.owner.spp(),
                slot: slot.into_owned(&self.loan),
                permission: self.loan.reclaim_slot(permission),
                key_id,
                key,
            })),
        });
    }

    fn remove_pairwise_key(
        &mut self,
        association: ModelAssociation<'id, B, R>,
        key: ModelPairwiseKey<'id, B, R>,
    ) {
        key.identity.0.active.set(false);
        self.held.command(Command::RemoveKey {
            key: key.identity,
            removed: OwnedRemoved::retain(
                R::removed(RemovedKey::Pairwise {
                    slot: key.slot,
                    association,
                }),
                &self.loan,
            ),
        });
    }

    fn remove_group_key(
        &mut self,
        permission: Slot<'id, ModelGroupPermission<R>, R::GroupPermissionPath<B::Identity>>,
        key: ModelGroupKey<'id, B, R>,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.monotonic_now().as_micros(),
            change: GroupChange::Remove {
                key: key.identity,
                removed: OwnedRemoved::retain(
                    R::removed(RemovedKey::Group {
                        slot: key.slot,
                        permission,
                    }),
                    &self.loan,
                ),
            },
        });
    }

    fn add_block_ack(
        &mut self,
        slot: ModelBlockAckSlot<'id, B>,
        agreement: SupportedBlockAck<K>,
    ) -> ModelBlockAck<'id, B> {
        self.held.settings.set_block_ack(
            slot.index(),
            Some(ModelReceiveAgreement::checked(R::ROLE, agreement)),
        );
        self.held.air.wake_driver();
        ModelBlockAck { slot }
    }

    fn replace_block_ack(
        &mut self,
        running: ModelBlockAck<'id, B>,
        agreement: SupportedBlockAck<K>,
    ) -> ModelBlockAck<'id, B> {
        self.held.settings.set_block_ack(
            running.slot.index(),
            Some(ModelReceiveAgreement::checked(R::ROLE, agreement)),
        );
        self.held.air.wake_driver();
        running
    }

    fn remove_block_ack(&mut self, agreement: ModelBlockAck<'id, B>) -> ModelBlockAckSlot<'id, B> {
        self.held
            .settings
            .set_block_ack(agreement.slot.index(), None);
        self.held.air.wake_driver();
        agreement.slot
    }
}

/// The subframes one model A-MPDU holds.
pub const MODEL_AMPDU_SUBFRAMES: usize = 8;

/// Builder retaining receiver, TID, role and one installed protection.
/// Only `push_aggregate` can create the nonempty state accepted by submission.
///
/// Role, phase, nominal identity and body loan stay in the builder:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_testing_model::{ModelAggregate, ModelComposition};
/// use oer_ieee80211_contracts_port::{Station, AccessPoint};
/// fn role<'k, 'phase, 'p, K, B: ModelComposition>(a: ModelAggregate<'k, 'phase, 'p, Station, K, B>) -> ModelAggregate<'k, 'phase, 'p, AccessPoint, K, B> { a }
/// ```
///
/// ```compile_fail
/// use oer_ieee80211_testing_model::{ModelAggregate, ModelComposition};
/// fn phase<'k, 'old, 'new, 'p, R, K, B: ModelComposition>(a: ModelAggregate<'k, 'old, 'p, R, K, B>) -> ModelAggregate<'k, 'new, 'p, R, K, B> { a }
/// ```
///
/// ```compile_fail
/// use oer_ieee80211_testing_model::{ModelAggregate, ModelComposition};
/// fn body<'k, 'phase, 'old, 'new, R, K, B: ModelComposition>(a: ModelAggregate<'k, 'phase, 'old, R, K, B>) -> ModelAggregate<'k, 'phase, 'new, R, K, B> { a }
/// ```
///
/// ```compile_fail,E0308
/// use oer_ieee80211_testing_model::{ModelAggregate, ModelBackend};
/// use oer_radio_port::SlotIdentity;
/// fn identity<'k, 'phase, 'p, R, K, Old: SlotIdentity, New: SlotIdentity>(a: ModelAggregate<'k, 'phase, 'p, R, K, ModelBackend<Old>>) -> ModelAggregate<'k, 'phase, 'p, R, K, ModelBackend<New>> { a }
/// ```
///
/// ```
/// use oer_ieee80211_testing_model::{ModelAggregate, ModelComposition};
/// fn same<'k, 'phase, 'p, R, K, B: ModelComposition>(a: ModelAggregate<'k, 'phase, 'p, R, K, B>) -> ModelAggregate<'k, 'phase, 'p, R, K, B> { a }
/// ```
pub struct ModelAggregate<
    'k,
    'p,
    'id,
    R,
    K,
    B: ModelComposition,
    S: AggregateState = EmptyAggregate,
> {
    subframes: Vec<Subframe<'id, B>>,
    loan: BodyLoan<'id, B::Identity, B>,
    first: S::Head<SequenceNumber>,
    receiver: IndividualAddress,
    tid: Tid,
    rate: AmpduRate<'p, 'id, K>,
    key: Option<PreparedKey>,
    key_borrow: PhantomData<&'k ()>,
    caps: TypeProof<(R, K, B, S)>,
}
impl<R, K, B: ModelComposition, S: AggregateState> ModelAggregate<'_, '_, '_, R, K, B, S> {
    fn acknowledgement_bit(&self, sequence: SequenceNumber) -> Option<u64> {
        let first = S::retain_head(&self.first, sequence);
        // The modulo-4096 distance (IEEE Std 802.11-2020, 10.24.7).
        let distance =
            SequenceNumber::from_low_bits(sequence.get().wrapping_sub(first.get())).get();
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
impl<'id, R, K, B: ModelComposition + 'id, S: AggregateState> PhaseAggregate<'id>
    for ModelAggregate<'_, '_, 'id, R, K, B, S>
{
    type Types = ModelTypes<B>;
    type Backend = B;
    type OwnedContext = ModelContextOf<B>;
    type OwnedPayload = ModelSubframes<'static, B>;
    type State = S;
    type Execution = ModelAggregateExecution;
    fn into_admission(
        self,
        context: <Self::Types as PhaseTypes<'id>>::Context,
        ticket: Slot<'id, ModelTickets, ModelPool<B::Identity, 1>>,
        _: oer_ieee80211_contracts_port::Admission<Self>,
    ) -> oer_ieee80211_contracts_port::AdmissionOwners<
        Self::OwnedContext,
        Slot<'static, ModelTickets, ModelPool<B::Identity, 1>>,
        Self::OwnedPayload,
        Self::Execution,
    >
    where
        S: oer_ieee80211_contracts_port::NonEmptyState,
    {
        oer_ieee80211_contracts_port::AdmissionOwners {
            context: context.enclose().into_owned(&self.loan),
            ticket: self.loan.reclaim_slot(ticket),
            payload: ModelSubframes(self.subframes).into_owned(&self.loan),
            execution: ModelAggregateExecution {
                first: S::into_head(self.first),
                tid: self.tid,
                rate: self.rate.rate().get(),
                key: self.key,
            },
        }
    }
    type Cancelled = ModelSubframeIter<'id, B>;
    fn receiver(&self) -> IndividualAddress {
        self.receiver
    }
    fn tid(&self) -> Tid {
        self.tid
    }
    fn protection(&self) -> oer_ieee80211_contracts_port::MpduProtection {
        match &self.key {
            None => oer_ieee80211_contracts_port::MpduProtection::Open,
            Some(_) => oer_ieee80211_contracts_port::MpduProtection::Ccmp,
        }
    }
    fn subframes(&self) -> usize {
        self.subframes.len()
    }
    fn into_parts(self) -> Self::Cancelled {
        ModelSubframeIter(self.subframes.into_iter())
    }
}
type ModelSubframe<'id, B, C, K> = SubframeSource<
    ModelBuffer<'id, B, C>,
    ModelSent<'id, B, C>,
    ClassSubframe<C, K>,
    ModelBodyOf<B>,
>;
type ModelCheckedAggregate<'k, 'p, 'id, R, K, B, S, C> = CheckedAggregate<
    ModelAggregate<'k, 'p, 'id, R, K, B, S>,
    ModelBuffer<'id, B, C>,
    ModelSent<'id, B, C>,
    ClassSubframe<C, K>,
    ModelBodyOf<B>,
>;
type ModelAggregatePush<'k, 'p, 'id, R, K, B, S, C> = Result<
    ModelAggregate<'k, 'p, 'id, R, K, B, NonEmptyAggregate>,
    AggregateRefused<ModelAggregate<'k, 'p, 'id, R, K, B, S>, ModelSubframe<'id, B, C, K>>,
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
    R: oer_ieee80211_contracts_port::InterfaceRole,
    K: Ieee80211Capabilities,
    B: ModelComposition,
    S: AggregateState,
    C: ModelClass,
>(
    operation: ModelCheckedAggregate<'k, 'p, 'id, R, K, B, S, C>,
) -> ModelAggregatePush<'k, 'p, 'id, R, K, B, S, C> {
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
        loan,
        receiver,
        tid,
        rate,
        key,
        ..
    } = aggregate;
    Ok(ModelAggregate {
        first,
        loan,
        subframes,
        receiver,
        tid,
        rate,
        key,
        key_borrow: PhantomData,
        caps: PhantomData,
    })
}

impl<'p, 'v, 'id: 'p, R, K: Ieee80211Capabilities, B: ModelComposition + 'id> AmpduPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, R, K, B>
where
    R: GroupInstallationRole + RoleKeys<'id, ModelTypes<B>, Keys = ModelKeys<'id, B, R>>,
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
    ) -> ModelAggregate<'k, 'p, 'id, R, K, B>
    where
        'id: 'k,
    {
        let key = match &protection {
            TxProtection::Open => None,
            TxProtection::Ccmp(key) => Some(R::tx_identity(key).into_owned(&self.loan)),
        };
        ModelAggregate {
            subframes: Vec::new(),
            loan: self.loan,
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
        attempt: oer_ieee80211_contracts_port::InterfaceAggregateSubmission<'k, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<ModelFault>>
    where
        'id: 'k,
    {
        let mut attempt = attempt.into_parts(&self.loan);
        let at = self.held.air.monotonic_now();
        for subframe in attempt.operation.execute().subframes {
            subframe.mpdu.admit(at);
        }
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

impl<'p, 'v, 'id: 'p, C: Ieee80211Capabilities, B: ModelComposition + 'id> StationPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, Station, C, B>
{
    fn replace_group_key(
        &mut self,
        permission: StationGroupPermission<'id, B>,
        installed: ModelGroupKey<'id, B, Station>,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.monotonic_now().as_micros(),
            change: GroupChange::Replace {
                permission: self.loan.reclaim_slot(permission),
                installed: installed.into_owned(&self.loan),
                key_id,
                key,
            },
        });
    }

    fn select_group_key(
        &mut self,
        permission: StationGroupPermission<'id, B>,
        installed: ModelGroupKey<'id, B, Station>,
    ) {
        self.held.command(Command::Group {
            admitted_at: self.held.air.monotonic_now().as_micros(),
            change: GroupChange::Select {
                permission: self.loan.reclaim_slot(permission),
                installed: installed.into_owned(&self.loan),
            },
        });
    }

    fn configure(
        &mut self,
        permission: ConfigurePermission<
            'id,
            Station,
            oer_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 1>,
            <B::Contexts as ModelContext>::StationConfigureContext<'id, B::Identity>,
        >,
        _: StationReceive<C>,
    ) {
        self.held
            .command(Command::Configure(OwnedConfigured::Station(
                permission.into_owned(&self.loan),
            )));
    }

    fn set_bss_color(&mut self, _: BssColor) {
        self.held.settings.mark_other();
        self.held.air.wake_driver();
    }
}

impl<'p, 'v, 'id: 'p, C: Ieee80211Capabilities, B: ModelComposition + 'id> AccessPointPhase<'p, 'id>
    for ModelInterface<'p, 'v, 'id, AccessPoint, C, B>
{
    fn configure(
        &mut self,
        permission: ConfigurePermission<
            'id,
            AccessPoint,
            oer_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 2>,
            <B::Contexts as ModelContext>::AccessPointConfigureContext<'id, B::Identity>,
        >,
        _: SupportedFilter<C, AccessPoint>,
    ) {
        self.held
            .command(Command::Configure(OwnedConfigured::AccessPoint(
                permission.into_owned(&self.loan),
            )));
    }
}

#[cfg(test)]
mod tests;

/// Reusable reference fixtures for the physical conformance runner.
#[cfg(feature = "test-support")]
#[allow(clippy::disallowed_methods, clippy::disallowed_macros)]
pub mod conformance;

mod values;
use values::*;

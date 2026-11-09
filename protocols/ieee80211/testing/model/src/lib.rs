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

//! An in-memory lower-MAC backend as a data port
//! (`oer_protocols_ieee80211_contracts_port::DataPort`), for host tests of
//! the code that drives it.
//!
//! [`ModelPort::install`] takes the installation's channel, interface
//! addresses and monotonic clock, and returns the port one actor owns by
//! value, the lifecycle's issuer of its installation commands
//! ([`ModelCommands`]), the installation's tokens ([`ModelResources`]) and
//! the [`ModelAir`] it plays on. Its installations come from
//! `Installations` under the identity's brand; [`ModelBackend`] is the
//! composition tag that names the identity, capabilities and contexts. The
//! actor calls the port synchronously with the instant of its turn. The air
//! answers each transmission, delivers received frames and records what the
//! hardware saw; the port's own queues, key table and receive rules are
//! cells of the one host thread a test runs on.
//!
//! Publications are answered at once and take no time, unless the test
//! gives the air a channel ([`ModelAir::set_air_time`], [`air_time`]): each
//! then ends with its exchange on a shared medium, an access point sends the
//! other way and the receive path takes its limits as separate inputs. The
//! clock's owner drives that air ([`ModelAir::next_air_event`],
//! [`ModelAir::clock_advanced`]).
//!
//! `ModelAir::answer` queues one `AirAnswer` per publication: ACK, BA
//! bitmap, missing response, collision or hardware timeout. A single MPDU's
//! checked frame determines whether it expects no response, an ACK or a
//! Block Ack (a BAR's). A mismatched response quarantines the installation
//! before publication.
//!
//! The station's data goes out on its two data arenas
//! ([`ModelDataArena`], lent as [`ModelPortArena`]) of up to
//! [`MODEL_AMPDU_SUBFRAMES`] frames each, unless a timed air sets another
//! number, as single MPDUs or A-MPDUs. Each
//! publication takes the next `AirAnswer` of the same FIFO: a BlockAck's
//! bit n acknowledges arena position n, by the frames' sequence numbers,
//! and no queued answer acknowledges every frame. The air records each sent
//! frame without a ticket, sealed under the installed station pairwise key
//! the arena holds until it empties; a removal waits for that use. A gate,
//! configuration or stop barrier refuses publication and waits for the
//! arena on the air, and a stop aborts it. [`ModelArena`] is the same
//! `TxArena` as a standalone host arena with scripted answers ([`arena`]).
//!
//! Each concrete class pool uses the portable `MpduLayout` proof before
//! writing headers or copying an external body. Its allocation is privately
//! minted at the class capacity; `ClassFrame` proves the MPDU fits it.
//! Small and large classes share one pool implementation. Their private
//! `ModelClass` dispatch only erases or recovers the appropriate owner
//! variant, returning every other class unchanged on refusal.
//! `TxProtection` alone determines Protected and the MAC/CCMP prefix. Sent
//! returns own that layout, body and the identity of the original key
//! installation. Retry supplies only a new attempt's rates, never an
//! independently reconstructed MPDU or body. Key identity is an allocation
//! identity in this host model; a reused slot cannot match an earlier
//! installation. A refused attempt returns all its resources synchronously
//! and allocates no PN or completion event. Prepared operations retain a
//! use of their key installation through their terminal or cancellation; a
//! key removal waits for these owners, while a returned Sent retains only
//! identity and therefore permits removal and slot reuse. PTK and GTK
//! material live in the model's installation table, separately from
//! cloneable identities. Removal clears its material after the last
//! prepared use ends; a reinstallation ends the installed keys.
//! The unique `ModelAssociation` owns the protocol's decision. Modeled
//! hardware holds one CCMP record per interface; keys select it with their
//! interface identity, without sharing ownership of the association. The
//! port applies its SPP mode before publishing material. `AirFrame::frame`
//! contains actual CCMP ciphertext and MIC under that mode; `plaintext` is
//! the DMA image before encryption. A pending GTK installation owns only a
//! vacant slot, permission, KeyID and material. The port creates its
//! identity and installed owner after the hardware step, then puts the
//! success terminal at the RX boundary: the frozen old DMA/ISR/software
//! ingress (`ModelAir::receive_group`) goes to the sink before it.
//!
//! `ModelContext` selects owned contexts by slot Id before lending their
//! public views. A station context retains both its nominal `LinkEpoch`
//! identity and its separate loan through every queue and terminal. The
//! model grants one epoch-source slot in its resources.
//!
//! Capacity is the tokens': every terminal returns a ticket, so no terminal
//! is ever lost; at most [`MODEL_RX_EVENTS`] received frames are out at
//! once, unless a timed air sets another limit. The host fixture's VecDeque
//! cannot express the token/capacity equation in its type: a broken equation
//! retains the offending owner and quarantines the installation. A received owner dropped without recycle
//! likewise quarantines this fixture.
//!
//! Publication and completion are separate owners. Tests may hold a result
//! while sending gate, key or stop barriers; only that publication can
//! finish. The model's publication bound uses the supplied image clock.
//! Only that clock's owner advances time. Radio instants are projections
//! through an affine, zero-drift relation with a sampled nonzero offset.
//! Hardware backends declare their own PPDU plus response/timeout bound.
//!
//! The access point uses the same port ([`ModelPort`] implements
//! `AccessPointPort`): its attempts share the queues and the key table, its
//! commands keep their own held bound, its configuration withdraws and waits
//! for its own interface only, and [`ModelAir::access_point`] shows its
//! receive rules and TSF. A confirmed stop leaves its interface receiving
//! nothing and its TSF stopped. [`ModelResources`] groups the shared tokens
//! and each role's own permissions. Every token of its pools, its group
//! permissions included, names its installation; both roles' commands, and
//! beacons and keyed attempts at `submit`, refuse a token of an earlier
//! installation with its owners.

pub mod air_time;
pub use air_time::{
    AirAccess, AirConfig, AirConfigRefused, AirExchange, AirHolder, AirPeer, AirProgress,
    AirReport, AirResponse, AirSideReport, AirSpan, AirTicket, AirTimeError, AirTiming,
    BusyChannel, ModelRxLimits, Offered, PeerBounds, PeerWindow, ampdu_psdu_octets,
};
use air_time::{AirWorld, Medium};
pub mod arena;
pub use arena::{
    MODEL_ARENA_CHANNEL, MODEL_ARENA_FRAMES, MODEL_ARENA_TIMEOUT, ModelArena, ModelArenaAnswer,
    ModelArenaFault, ModelArenaPublication, ModelArenas, ModelDataArena,
};
mod loan;
mod owned;
use owned::*;

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
    cell::Cell,
    marker::PhantomData,
    num::NonZeroU16,
    task::{Poll, Waker},
};
use oer_portable_time::{Clock, Duration, Instant};
use oer_protocols_ieee80211_contracts_port::EdcaParameterSet;
use oer_protocols_ieee80211_contracts_port::{
    association,
    association::{
        Association, AssociationPool, AssociationStart, BlockAckSlot, BlockAckSlots, GroupKeySlot,
        KeyBankParts, PairwiseKeySlot, VacantAssociation, VacantKeyBank,
    },
};
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_wire::ccmp::{CcmpHeader, CcmpKeyId, GroupKeyId};
use oer_protocols_ieee80211_wire::frame::set_retry_bit;
use oer_protocols_radio_port::{BodyLoan, LoanedContext, SlotIdentity};
use oer_protocols_radio_port::{ClockRelation, GenerationSource, RelationSample};

use oer_portable_time::RadioInstant;
use oer_protocols_ieee80211_wire::phy::{HeMcs, HtMcs, PhyRate};
use oer_protocols_radio_port::ClockError;
use oer_protocols_radio_port::{Mint, Slot};

use oer_protocols_ieee80211_contracts_port::{
    AccessPoint, BandSet, BodyPayload, BufferMemoryUnavailable, Channel, CheckedPoolSource,
    CheckedTxSource, ConfigurePermission, ConnectionEpochKind, ExecutionPayload, FreshSource,
    GateClosed, GateIssuer, GateOpen, HardwareServices, Ieee80211Capabilities, Ieee80211Radio,
    InFlight, IndeterminateCause, InterfaceCommandPool, InterfacePermissions, KeyTypes, LinkMetric,
    LowerMacCapabilities, MpduKind, MpduLayout, PhyFormatSet, PortTypes, PreparationError,
    PreparationRefused, PreparedTx, RateSupport, ReceiveFilter, RetransmissionPlan, RoleKeys,
    RxBuffer, RxEvidence, RxMeta, RxMetadata, SignalMetadata, Station, SupportedBlockAck,
    SupportedChannel, TxBody, TxBufferClass, TxBufferMemory, TxBufferPool, TxFrame, TxMpdu,
    TxOutcome, TxProtection, TxSource, VifRole, WidthSet, WithdrawCause,
};
use oer_protocols_ieee80211_contracts_port::{
    AckSnr, Air, BlockAckSnr, SingleEnd, SingleOutcome, SingleResponse, SnrSample,
};
use oer_protocols_ieee80211_contracts_port::{
    AmpduCapabilities, AmpduLengthTable, BlockAckReport, Protection, Tid,
};
use oer_protocols_ieee80211_wire::address::IndividualAddress;
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

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
    rx_amsdu_in_ampdu: true,
    edca_max_aifsn: 15,
    edca_max_cw_exponent: 10,
    edca_max_txop_limit_units_32_us: u16::MAX,
};

/// The model's capabilities marker.
pub struct ModelCaps;

impl Ieee80211Capabilities for ModelCaps {
    type SppScope = oer_protocols_ieee80211_wire::security::spp::Interface;
    const CAPS: LowerMacCapabilities = MODEL_CAPABILITIES;
}

/// What the model serves of the access-point role: the field of a beacon
/// goes on its air as the caller wrote it, and its TSF restarts and stops
/// as the commands say. These are the model's own rules, not a chip's.
pub const MODEL_ACCESS_POINT: oer_protocols_ieee80211_contracts_port::AccessPointServices =
    oer_protocols_ieee80211_contracts_port::AccessPointServices {
        beacon_timestamp: Some(oer_protocols_ieee80211_contracts_port::BeaconTimestamp::Caller),
        tsf_control: true,
    };

impl oer_protocols_ieee80211_contracts_port::AccessPointCapabilities for ModelCaps {
    const ACCESS_POINT: oer_protocols_ieee80211_contracts_port::AccessPointServices =
        MODEL_ACCESS_POINT;
}

impl oer_protocols_ieee80211_contracts_port::AccessPointCapabilities for ModelCaps24 {
    const ACCESS_POINT: oer_protocols_ieee80211_contracts_port::AccessPointServices =
        MODEL_ACCESS_POINT;
}

/// The model's capabilities without its 5 GHz band, for host tests of
/// station rate selection constrained to a single band.
pub struct ModelCaps24;

impl Ieee80211Capabilities for ModelCaps24 {
    type SppScope = oer_protocols_ieee80211_wire::security::spp::Interface;
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
pub type ModelPool<Id, const I: u8> =
    oer_protocols_radio_port::Child<oer_protocols_radio_port::Root<Id>, I>;

/// The distinct key pools belonging to each modeled interface role.
pub trait ModelRole: oer_protocols_ieee80211_contracts_port::InterfaceRole {
    /// Component containing the role's pairwise slots.
    type AssociationPath<Id: SlotIdentity>: oer_protocols_radio_port::PoolPath<Identity = Id>;
    /// Component containing the unique GTK mutation permission.
    type GroupPermissionPath<Id: SlotIdentity>: oer_protocols_radio_port::PoolPath<Identity = Id>;
}
impl ModelRole for Station {
    type AssociationPath<Id: SlotIdentity> = ModelPool<Id, 5>;
    type GroupPermissionPath<Id: SlotIdentity> = ModelPool<Id, 8>;
}
impl ModelRole for AccessPoint {
    type AssociationPath<Id: SlotIdentity> = ModelPool<Id, 6>;
    type GroupPermissionPath<Id: SlotIdentity> =
        oer_protocols_radio_port::Child<ModelPool<Id, 9>, 0>;
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
/// The memory a network fills with one data frame, which a model data
/// arena's slot holds ([`PortTypes::DataMemory`]).
pub type ModelNetworkMemory = Box<[u8; ModelLarge::CAPACITY]>;
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
    fn retry(&mut self) {
        let bytes: &mut [u8] = self;
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
    fn renew_msdu(&mut self) -> Result<(), BufferMemoryUnavailable> {
        self.admitted = None;
        self.admitted_at = None;
        Ok(())
    }
    fn bytes_mut(&mut self) -> Result<&mut [u8], BufferMemoryUnavailable> {
        Ok(&mut self.memory)
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
    /// An earlier transmission could have reached the air: a retransmission
    /// carries the Retry bit.
    retry: bool,
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
    /// A published MPDU's transmission could have reached the air.
    fn owe_retry(&mut self) {
        match self {
            Self::SmallSent(mpdu) => mpdu.retry = true,
            Self::LargeSent(mpdu) => mpdu.retry = true,
            Self::Small(_) | Self::Large(_) => {}
        }
    }
    /// Whether a retransmission of this published MPDU carries Retry.
    fn retry_owed(&self) -> bool {
        match self {
            Self::SmallSent(mpdu) => mpdu.retry,
            Self::LargeSent(mpdu) => mpdu.retry,
            Self::Small(_) | Self::Large(_) => false,
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
    fn into_owned<S>(self, _: &BodyLoan<'p, Id, S>) -> RawKeyIdentity {
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
    /// The installation that issued the key's slot.
    generation: u32,
    key_id: u8,
    active: Cell<bool>,
    group_role: Option<VifRole>,
    role: VifRole,
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
    fn new(index: u8, generation: u32, key_id: u8, role: VifRole, air: &Rc<ModelAir>) -> Self {
        Self(Rc::new(KeyInstall {
            index,
            generation,
            key_id,
            active: Cell::new(true),
            group_role: None,
            role,
            rx_active: Cell::new(false),
            preparations: Rc::new(()),
            air: Rc::downgrade(air),
        }))
    }
    fn group(
        index: u8,
        generation: u32,
        key_id: GroupKeyId,
        role: VifRole,
        key: Ccmp128Key,
        air: &Rc<ModelAir>,
    ) -> Self {
        let identity = Self(Rc::new(KeyInstall {
            index,
            generation,
            key_id: key_id.get(),
            active: Cell::new(true),
            group_role: Some(role),
            role,
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

/// One closed MPDU, retaining its memory, external body and publication state.
pub struct ModelMpdu<'id, B: ModelComposition> {
    buffer: ModelAnyBuffer<'id, B>,
    layout: MpduLayout,
    response: ResponsePolicy,
    // Fresh memory keeps its body here, including during publication. Only
    // consuming the terminal moves both into the returned Sent owner.
    body: Option<ModelBodyOf<B>>,
    published: Option<PublishedMpdu>,
    /// A transmission of it could have reached the air; its `Sent` keeps it.
    retry: bool,
    packet_number: Option<oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber>,
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
        }
        .map_err(|_| ModelFault)?;
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
            retry,
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
                    retry,
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
                    retry,
                }),
                None,
            ),
            // A retransmission's `Sent` keeps its own decision and gains this
            // one's.
            (mut buffer, _) => {
                if retry {
                    buffer.owe_retry();
                }
                (buffer, body)
            }
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
    /// The monotonic instant the MPDU was admitted at, once it was.
    fn admitted_monotonic(&self) -> Option<Instant> {
        match &self.buffer {
            ModelAnyBuffer::Small(buffer) => buffer.admitted,
            ModelAnyBuffer::Large(buffer) => buffer.admitted,
            ModelAnyBuffer::SmallSent(mpdu) => mpdu.buffer.admitted,
            ModelAnyBuffer::LargeSent(mpdu) => mpdu.buffer.admitted,
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
    /// Original admission projected by P2 when dequeuing. MSDU age uses its
    /// separately retained monotonic instant.
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
        buffer: ModelAnyBuffer<'id, B>,
        layout: MpduLayout,
        response: ResponsePolicy,
    ) -> Self {
        Self {
            buffer,
            layout,
            response,
            body: None,
            published: None,
            retry: false,
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
        let wire = match key {
            Some(identity) => seal(identity, bytes)?,
            None => bytes.into(),
        };
        Ok(AirFrame {
            ticket: Some(ticket),
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

/// `bytes` as the air carries them under the installed key `identity`:
/// encrypted, with its MIC.
fn seal(identity: &RawKeyIdentity, bytes: &[u8]) -> Result<Vec<u8>, ModelFault> {
    let mut wire: Vec<u8> = bytes.into();
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
            .seal(
                &mut oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128,
                &material.key,
                &mut wire,
            )
            .map_err(|_| ModelFault)
    })();
    air.key_installations.set(installations);
    result?;
    Ok(wire)
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

/// The composition tag selecting this model's nominal identity and data.
pub trait ModelComposition: Sized {
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

impl<'id, B: ModelComposition + 'id> PortTypes<'id> for ModelTypes<B> {
    const COMMAND_CAPACITY: usize = MODEL_COMMANDS;
    type StationCommandPath =
        oer_protocols_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 1>;
    type AccessPointCommandPath =
        oer_protocols_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 2>;
    type StationConfigureContext =
        <B::Contexts as ModelContext>::StationConfigureContext<'id, B::Identity>;
    type AccessPointConfigureContext =
        <B::Contexts as ModelContext>::AccessPointConfigureContext<'id, B::Identity>;
    const TERMINAL_CAPACITY: usize = MODEL_TERMINALS;
    type EpochPath = ModelPool<<B as ModelComposition>::Identity, 4>;
    type GatePath = ModelPool<<B as ModelComposition>::Identity, 0>;
    type TicketPath = ModelPool<<B as ModelComposition>::Identity, 1>;
    type BlockAckPath = ModelPool<<B as ModelComposition>::Identity, 7>;
    type BeaconPath = ModelBeaconPath<B>;
    type Fault = ModelFault;
    type Context = <B::Contexts as ModelContext>::Context<'id, B::Identity>;
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities> = ModelPrepared<'k, 'p, 'id, R, K, B>;
    type KeyIdentity = KeyIdentity<'id, B::Identity>;
    type TxBody = ModelBodyOf<B>;
    type TicketKind = ModelTickets;
    type AckEvidence = ModelAck;
    type BlockAckEvidence = ModelBlockAckEvidence;
    type AirDenied = ModelAirDenied;
    type AnyTxBuffer = ModelAnyBuffer<'id, B>;
    type TxMpdu = ModelMpdu<'id, B>;
    type DataMemory = ModelNetworkMemory;
    type RxBuffer = ModelRxBuffer<'id, B::Identity>;
    type LentRx = ModelLentRx;
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
        spp: oer_protocols_ieee80211_wire::security::Spp,
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
    type GroupPermission = oer_protocols_ieee80211_contracts_port::IssuedSlot<
        'id,
        ModelGroupPermission<R>,
        R::GroupPermissionPath<B::Identity>,
    >;
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
    ) -> oer_protocols_ieee80211_contracts_port::PoolPreparation<'k, 'p, 'id, Self, C, K, R>
    where
        'id: 'k,
    {
        prepare_model(source)
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

    fn renew(mpdu: Self::Sent) -> (Self::Buffer, Option<ModelBodyOf<B>>) {
        // The MPDU and its PN end here; `renew_msdu` releases its admission.
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
        crypto: RxEvidence<oer_protocols_ieee80211_contracts_port::RxCryptoStatus>,
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
/// Dropping it requests poison; only `DataPort::recycle` returns capacity.
pub struct ModelRxBuffer<'p, Id: SlotIdentity> {
    owner: OwnedRxBuffer<Id>,
    loan: PhantomData<fn(&'p ()) -> &'p ()>,
}

struct OwnedRxBuffer<Id: SlotIdentity> {
    bytes: Vec<u8>,
    recycled: bool,
    air: Weak<ModelAir>,
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
    fn lend<'p, S>(self, _: &BodyLoan<'p, Id, S>) -> ModelRxBuffer<'p, Id> {
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

/// The bytes a lent model buffer holds: its MPDU, then spare allocation up
/// to this length, as a DMA receive buffer has.
pub const MODEL_RX_STORAGE: usize = 1664;

/// The model's count of the receive buffers the network holds.
#[derive(Default)]
struct ModelNetwork {
    held: core::sync::atomic::AtomicUsize,
    waker: oer_portable_waker::AtomicWaker,
}

impl ModelNetwork {
    fn held(&self) -> usize {
        self.held.load(core::sync::atomic::Ordering::Acquire)
    }
}

/// A model receive buffer lent to the network
/// ([`DataPort::lend`](oer_protocols_ieee80211_contracts_port::DataPort::lend)).
pub struct ModelLentRx {
    bytes: Vec<u8>,
    mpdu_len: usize,
    network: alloc::sync::Arc<ModelNetwork>,
}

impl oer_protocols_ieee80211_contracts_port::LentRxBuffer for ModelLentRx {
    fn storage(&self) -> &[u8] {
        &self.bytes
    }
    fn storage_mut(&mut self) -> &mut [u8] {
        &mut self.bytes
    }
    fn mpdu_len(&self) -> usize {
        self.mpdu_len
    }
    fn advance(
        &mut self,
        by: usize,
    ) -> Result<(), oer_protocols_ieee80211_contracts_port::AdvanceRefused> {
        let refused = oer_protocols_ieee80211_contracts_port::AdvanceRefused {
            by,
            mpdu_len: self.mpdu_len,
        };
        self.mpdu_len = self.mpdu_len.checked_sub(by).ok_or(refused)?;
        self.bytes = core::mem::take(&mut self.bytes)
            .into_iter()
            .skip(by)
            .collect();
        Ok(())
    }
}

impl Drop for ModelLentRx {
    fn drop(&mut self) {
        self.network
            .held
            .fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
        self.network.waker.wake();
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

/// The tokens the install gives the body once: what both roles share of
/// the one physical owner, and each role's own permissions.
pub struct ModelResources<'id, B: ModelComposition> {
    /// The transmit capacity, the receive Block Ack slots and the gate both
    /// roles share.
    pub shared: ModelShared<'id, B>,
    /// The station's own permissions.
    pub station: ModelStationPermissions<'id, B>,
    /// The access point's own permissions.
    pub access_point: ModelAccessPointPermissions<'id, B>,
}

/// What both roles share of the one physical owner: tickets, buffers, the
/// receive Block Ack slots and the gate its arbiter holds.
pub struct ModelShared<'id, B: ModelComposition> {
    /// The transmit tickets.
    pub tickets: Vec<Slot<'id, ModelTickets, ModelPool<<B as ModelComposition>::Identity, 1>>>,
    /// Sole permission for the body to unpack TX terminals.
    pub terminal_access: oer_protocols_ieee80211_contracts_port::TerminalAccess<
        'id,
        ModelTickets,
        ModelPool<<B as ModelComposition>::Identity, 1>,
    >,
    /// The small transmit buffers.
    pub small: Vec<ModelBuffer<'id, B, ModelSmall>>,
    /// The large transmit buffers.
    pub large: Vec<ModelBuffer<'id, B, ModelLarge>>,
    /// The receive Block Ack slots both roles share.
    pub block_acks: [ModelBlockAckSlot<'id, B>; MODEL_BLOCK_ACK_SLOTS],
    /// The sole authority to construct this installation's arbiter.
    pub arbiter_authority: oer_protocols_ieee80211_contracts_port::ArbiterAuthority<
        'id,
        ModelPool<<B as ModelComposition>::Identity, 0>,
    >,
    /// The open transmit gate.
    pub gate: GateOpen<'id, ModelPool<<B as ModelComposition>::Identity, 0>>,
}

/// The station's own permissions.
pub struct ModelStationPermissions<'id, B: ModelComposition> {
    /// Its unique association-mode selection permission.
    pub association: AssociationStart<
        'id,
        Station,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 5>,
    >,
    /// Its command admission from its own installation component.
    pub commands: InterfacePermissions<
        'id,
        Station,
        oer_protocols_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 1>,
    >,
    /// The install's single epoch-source slot; Station constructs its source.
    pub connection_epoch:
        Slot<'id, ConnectionEpochKind, ModelPool<<B as ModelComposition>::Identity, 4>>,
    /// Its pairwise key slots.
    pub pairwise: [PairwiseKeySlot<
        'id,
        Station,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 5>,
    >; MODEL_KEY_SLOTS],
    /// Its group key slots.
    pub group: [GroupKeySlot<
        'id,
        Station,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 5>,
    >; MODEL_KEY_SLOTS],
    /// Its sole GTK mutation permission.
    pub group_permission: oer_protocols_ieee80211_contracts_port::IssuedSlot<
        'id,
        ModelGroupPermission<Station>,
        ModelPool<<B as ModelComposition>::Identity, 8>,
    >,
}

/// The access point's own permissions.
pub struct ModelAccessPointPermissions<'id, B: ModelComposition> {
    /// Its interface-wide association-mode selection permission.
    pub association: AssociationStart<
        'id,
        AccessPoint,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 6>,
    >,
    /// Its command admission from its own installation component.
    pub commands: InterfacePermissions<
        'id,
        AccessPoint,
        oer_protocols_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 2>,
    >,
    /// Its pairwise key slots, one per association identifier.
    pub pairwise: [PairwiseKeySlot<
        'id,
        AccessPoint,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 6>,
    >; MODEL_KEY_SLOTS],
    /// Its group key slots.
    pub group: [GroupKeySlot<
        'id,
        AccessPoint,
        MODEL_KEY_SLOTS,
        MODEL_KEY_SLOTS,
        ModelPool<<B as ModelComposition>::Identity, 6>,
    >; MODEL_KEY_SLOTS],
    /// Its sole GTK mutation permission.
    pub group_permission: oer_protocols_ieee80211_contracts_port::IssuedSlot<
        'id,
        ModelGroupPermission<AccessPoint>,
        oer_protocols_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 0>,
    >,
    /// The installation's one beacon permission.
    pub beacon: oer_protocols_ieee80211_contracts_port::BeaconPermission<'id, ModelBeaconPath<B>>,
}

/// The model's beacon component.
pub type ModelBeaconPath<B> =
    oer_protocols_radio_port::Child<ModelPool<<B as ModelComposition>::Identity, 9>, 3>;

/// The access point's interface as the modeled hardware holds it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModelAccessPointInterface {
    /// Its receive rules, `None` while it receives nothing.
    pub receive: Option<ReceiveFilter>,
    /// Its TSF.
    pub tsf: ModelAccessPointTsf,
}

/// The access point's TSF in the model.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ModelAccessPointTsf {
    /// Stopped.
    #[default]
    Stopped,
    /// Running from zero since the instant of its restart.
    Running {
        /// The model instant it restarted at.
        since: Instant,
    },
}

/// One transmission the air saw.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AirFrame {
    /// Ticket of the operation that actually published this MPDU; `None`
    /// for a frame of the station's data arenas, which take no ticket.
    pub ticket: Option<u8>,
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
    /// A measured BlockAck. Bit n acknowledges position n of this one data
    /// arena's A-MPDU, or is a BAR's bitmap.
    BlockAck(u64),
    /// An A-MPDU's BlockAck whose completion carries no SNR measurement,
    /// as an S31 BlockAck accepted with a failed hardware status does.
    BlockAckUnmeasured(u64),
    NoResponse,
    /// The protection exchange drew no CTS: the data frame did not go out.
    CtsTimeout,
    Collision,
    /// Coexistence denied the air before the PPDU went out.
    AirDenied,
    /// The hardware refused the frame's key: a single MPDU's end is
    /// indeterminate, a data arena's publication is aborted.
    KeyRefused,
    /// The hardware timed out: a single MPDU's end is indeterminate, a
    /// data arena's publication is aborted.
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
    fn snr_sample(&self) -> SnrSample {
        SnrSample::Measured(self.snr_db)
    }
}

/// What the model observed of a data arena's ACK or BlockAck.
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
/// A data arena's end in the model.
pub type ModelArenaCompleted =
    oer_protocols_ieee80211_contracts_port::ArenaCompleted<ModelBlockAckEvidence, ModelAirDenied>;

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
    stage: GroupRxStage,
}

/// Whether the model admits work: it runs, closed admission to return its
/// owners ([`ModelAir::fail_after_terminals`]), or quarantined
/// ([`ModelAir::poison`]).
#[derive(Clone, Copy, Default)]
enum ModelStream {
    #[default]
    Running,
    Draining,
    Ended,
}

/// The EDCA set each interface's attempts contend with.
struct ModelEdca {
    parameters: [EdcaParameterSet; 2],
}
impl Default for ModelEdca {
    fn default() -> Self {
        Self {
            parameters: [EdcaParameterSet::STANDARD_DEFAULT; 2],
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
                uncertainty: oer_portable_time::RadioDuration::ZERO,
                generation: GenerationSource::new(0).current(),
            }),
        }
    }
}

/// The air and the world of a model: shared by the backend's driver and
/// the test.
pub struct ModelAir {
    /// What the access point's interface receives and its TSF.
    access_point: Cell<ModelAccessPointInterface>,
    sent: Cell<Vec<AirFrame>>,
    /// The answers of the next transmissions, in order; once none is
    /// queued, each is acknowledged.
    answers: OwnedQueue<AirAnswer>,
    /// Conformance air input: an absent peer supplies no response to any PPDU.
    #[cfg(feature = "test-support")]
    absent_peer: Cell<bool>,
    rx: OwnedQueue<(Vec<u8>, ModelRxMeta)>,
    rx_stream: Cell<Option<(Vec<u8>, ModelRxMeta)>>,
    refuse_enable: Cell<u8>,
    refuse_access_point: Cell<u8>,
    group_held: Cell<bool>,
    // Secret material belongs to this installation table, never to the
    // cloneable identity carried by captured RX or returned Sent owners.
    key_installations: Cell<Vec<KeyInstallation>>,
    associations: Cell<InterfaceAssociations>,
    group_rx: OwnedQueue<CapturedGroupRx>,
    group_writes: Cell<usize>,
    stream: Cell<ModelStream>,
    /// The port may take a poison back for a stop.
    recovery: Cell<bool>,
    disables: Cell<u32>,
    /// The EDCA set each interface's attempts contend with, by role.
    edca: Cell<ModelEdca>,
    /// The queues the air keeps busy: their attempts wait unpublished.
    held: Cell<[bool; WmmAccessCategory::ALL.len()]>,
    /// Keep a published PPDU's result pending, independently of prepublication CCA.
    held_results: Cell<[bool; WmmAccessCategory::ALL.len()]>,
    /// The MAC does not go quiet: a stop stays unconfirmed while set.
    stop_held: Cell<bool>,
    /// Arena ends served ahead of the port's transmit service.
    relayed: Cell<usize>,
    /// What [`TxArena::event_waiting`] of the port's arenas answers.
    event_waiting: Cell<bool>,
    time: ModelTime,
    receive_agreements: Cell<BTreeMap<u8, ModelReceiveAgreement>>,
    expired_msdus: Cell<u32>,
    /// The instant a data port call was given: the model's time for the
    /// call, read instead of the image clock while it is set.
    turn: Cell<Option<Instant>>,
    /// The data port's actor, which the air's world wakes; the port's own
    /// steps never do.
    actor: Cell<Option<Waker>>,
    /// The data port's receive pulls answer that the ring has work from
    /// this instant on: before it, and at least once (whether it answered).
    rx_later: Cell<Option<(Instant, bool)>>,
    /// The data port's deadline stays at this instant across its services.
    kept_due: Cell<Option<Instant>>,
    /// Frames that land once a receive pull found the ring empty, without
    /// a wake.
    landing: OwnedQueue<(Vec<u8>, ModelRxMeta)>,
    /// A TBTT that fires at the data port's next receive call, without a
    /// wake, and the one fired for the port's next facts.
    tbtt_landing: Cell<bool>,
    tbtt: Cell<bool>,
    /// The shared channel of a timed air: without it, publications are
    /// answered at once.
    medium: Cell<Option<Box<Medium>>>,
    /// The rule the medium could not compute, which poisoned the model.
    air_fault: Cell<Option<AirTimeError>>,
    /// The limits of the receive path.
    rx_limits: Cell<ModelRxLimits>,
    /// The subframes one data arena holds.
    arena_frames: Cell<u8>,
}
impl ModelAir {
    fn new(clock: Rc<dyn Clock>) -> Self {
        Self {
            access_point: Default::default(),
            sent: Default::default(),
            answers: Default::default(),
            #[cfg(feature = "test-support")]
            absent_peer: Cell::new(false),
            rx: Default::default(),
            rx_stream: Default::default(),
            refuse_enable: Default::default(),
            refuse_access_point: Default::default(),
            group_held: Default::default(),
            key_installations: Default::default(),
            associations: Default::default(),
            group_rx: Default::default(),
            group_writes: Default::default(),
            stream: Default::default(),
            recovery: Default::default(),
            disables: Default::default(),
            edca: Default::default(),
            held: Default::default(),
            held_results: Default::default(),
            stop_held: Cell::new(false),
            relayed: Cell::new(0),
            event_waiting: Cell::new(false),
            time: ModelTime::new(clock),
            receive_agreements: Default::default(),
            expired_msdus: Default::default(),
            turn: Cell::new(None),
            actor: Cell::new(None),
            rx_later: Cell::new(None),
            kept_due: Cell::new(None),
            landing: Default::default(),
            tbtt_landing: Cell::new(false),
            tbtt: Cell::new(false),
            medium: Default::default(),
            air_fault: Default::default(),
            rx_limits: Cell::new(ModelRxLimits {
                ring: usize::MAX,
                held: MODEL_RX_EVENTS,
                lent: MODEL_RX_EVENTS,
            }),
            arena_frames: Cell::new(arena::MODEL_ARENA_FRAMES as u8),
        }
    }

    fn monotonic_now(&self) -> Instant {
        self.turn.get().unwrap_or_else(|| self.time.clock.now())
    }

    /// Wake the data port's actor: the world changed.
    fn wake_world(&self) {
        if let Some(waker) = self.actor.take() {
            waker.wake();
        }
    }

    /// Wake the data port's actor after the clock owner advances virtual
    /// time, and on a timed air serve what happened on the channel up to
    /// that time. This notification never changes the clock.
    pub fn clock_advanced(&self) {
        let now = self.monotonic_now().as_micros();
        if let Some(Err(fault)) = self.with_medium(|medium| medium.advance(now, self)) {
            self.air_fault.set(Some(fault));
            self.poison();
        }
        self.wake_world();
    }

    /// Run every later publication on a channel with time, from now on
    /// ([`air_time`]).
    ///
    /// # Errors
    ///
    /// The input the medium cannot run on; nothing changes.
    pub fn set_air_time(&self, config: AirConfig) -> Result<(), AirConfigRefused> {
        let AirConfig {
            seed,
            timing,
            arena_frames,
            rx_limits,
            busy,
            peer,
        } = config;
        if arena_frames == 0
            || arena_frames > oer_protocols_ieee80211_contracts_port::arena::ARENA_FRAMES
        {
            return Err(AirConfigRefused::ArenaFrames);
        }
        if busy.is_some_and(|busy| busy.busy_micros >= busy.period_micros.get()) {
            return Err(AirConfigRefused::BusyPeriod);
        }
        if peer.as_ref().is_some_and(|peer| peer.max_subframes == 0) {
            return Err(AirConfigRefused::PeerSubframes);
        }
        let medium = self.medium.take();
        if medium.is_some() {
            self.medium.set(medium);
            return Err(AirConfigRefused::AlreadyTimed);
        }
        let now = self.monotonic_now().as_micros();
        self.medium
            .set(Some(Box::new(Medium::new(now, seed, timing, busy, peer))));
        self.rx_limits.set(rx_limits);
        self.arena_frames.set(arena_frames);
        Ok(())
    }

    /// The next instant the channel changes, for the clock's owner, who
    /// calls [`Self::clock_advanced`] there; `None` on an air without time
    /// or a channel with nothing left to do.
    pub fn next_air_event(&self) -> Option<Instant> {
        self.with_medium(|medium| medium.next_event())
            .flatten()
            .map(Instant::from_micros)
    }

    /// What the medium did since its last reset; `None` without time.
    pub fn air_report(&self) -> Option<AirReport> {
        self.with_medium(|medium| medium.report())
    }

    /// Start the medium's counts and spans anew.
    pub fn reset_air_report(&self) {
        let _reset: Option<()> = self.with_medium(Medium::reset_report);
    }

    /// Every transmission since the last reset, in order.
    pub fn air_spans(&self) -> Vec<AirSpan> {
        self.with_medium(|medium| medium.spans().to_vec())
            .unwrap_or_default()
    }

    /// The rule the medium could not compute, when it poisoned the model.
    pub fn air_fault(&self) -> Option<AirTimeError> {
        self.air_fault.get()
    }

    /// `run` over the medium, which leaves its cell meanwhile: nothing it
    /// calls reenters this method.
    fn with_medium<R>(&self, run: impl FnOnce(&mut Medium) -> R) -> Option<R> {
        let mut medium = self.medium.take()?;
        let result = run(&mut medium);
        self.medium.set(Some(medium));
        Some(result)
    }

    /// Ask the medium for a station exchange at the turn's instant; `None`
    /// on an air without time.
    fn request_air(
        &self,
        rate: PhyRate,
        psdu_octets: u32,
        protection: Protection,
        response: AirResponse,
        access_category: WmmAccessCategory,
        subframes: u8,
    ) -> Result<Option<AirTicket>, AirTimeError> {
        let now = self.monotonic_now().as_micros();
        let parameters = self
            .edca(VifRole::Station)
            .access_category(access_category)
            .wmm;
        self.with_medium(|medium| {
            medium.request(
                now,
                rate,
                psdu_octets,
                protection,
                response,
                parameters,
                subframes,
            )
        })
        .transpose()
    }

    fn air_progress(&self, ticket: AirTicket) -> AirProgress {
        self.with_medium(|medium| medium.progress(ticket))
            .unwrap_or(AirProgress::Answered)
    }

    fn forget_air(&self, ticket: AirTicket) {
        let _forgotten: Option<()> = self.with_medium(|medium| medium.forget(ticket));
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
}

impl AirWorld for ModelAir {
    fn agreed_window(&self, tid: Tid) -> Option<u16> {
        let agreements = self.receive_agreements.take();
        let window = agreements
            .values()
            .find(|agreement| agreement.role == VifRole::Station && agreement.tid == tid)
            .map(|agreement| agreement.window);
        self.receive_agreements.set(agreements);
        window
    }

    fn land(&self, frame: Vec<u8>, meta: ModelRxMeta) -> bool {
        if self.rx.len() >= self.rx_limits.get().ring {
            return false;
        }
        self.rx.push_back((frame, meta));
        true
    }
}

// ModelCaps declares Interface scope. These are hardware policy records,
// indexed by the same interface identity that keys already carry. The
// association owner is never cloned. A queued install takes its decision
// from that owner and the driver applies it before publishing material.
// Like key-table hardware, vacant interfaces retain their last mode; changing
// it requires a new owner obtained through the complete returned slot bank.
#[derive(Clone, Copy)]
struct InterfaceAssociations {
    station: oer_protocols_ieee80211_wire::security::Spp,
    access_point: oer_protocols_ieee80211_wire::security::Spp,
}
impl Default for InterfaceAssociations {
    fn default() -> Self {
        use oer_protocols_ieee80211_wire::security::Spp;
        Self {
            station: Spp::NotNegotiated,
            access_point: Spp::NotNegotiated,
        }
    }
}
impl InterfaceAssociations {
    fn get(
        &self,
        role: VifRole,
    ) -> oer_protocols_ieee80211_core_security_cipher::ccmp_mpdu::CcmpAssociation {
        oer_protocols_ieee80211_core_security_cipher::ccmp_mpdu::CcmpAssociation::new(match role {
            VifRole::Station => self.station,
            VifRole::AccessPoint => self.access_point,
        })
    }
    fn apply(&mut self, role: VifRole, spp: oer_protocols_ieee80211_wire::security::Spp) {
        let record = match role {
            VifRole::Station => &mut self.station,
            VifRole::AccessPoint => &mut self.access_point,
        };
        *record = spp;
    }
}

impl ModelAir {
    fn install_material(&self, identity: &RawKeyIdentity, key: Ccmp128Key) {
        self.key_installations.record(KeyInstallation {
            identity: Rc::downgrade(&identity.0),
            key,
        });
    }

    fn apply_association(&self, role: VifRole, spp: oer_protocols_ieee80211_wire::security::Spp) {
        self.associations.apply(role, spp);
    }

    fn remove_key_material(&self, identity: &RawKeyIdentity) {
        let mut installations = self.key_installations.take();
        installations.retain(|entry| !entry.identity.ptr_eq(&Rc::downgrade(&identity.0)));
        self.key_installations.set(installations);
    }

    /// Delay commands while RX continues. The supplied clock's owner calls
    /// `clock_advanced` for the deadline wake; the driver poisons at
    /// GROUP_COMMAND_BOUND. Normal disable finishes
    /// admitted commands instead of waiting for this test hold to be released.
    pub fn hold_group_commands(&self, held: bool) {
        self.group_held.set(held);
        self.wake_world();
    }

    /// Keep the MAC from going quiet: a stop stays unconfirmed, past its
    /// bound, until the hold ends.
    pub fn hold_stop(&self, held: bool) {
        self.stop_held.set(held);
        self.wake_world();
    }

    /// Actual material writes, including replacement and excluding selection.
    pub fn group_writes(&self) -> usize {
        self.group_writes.get()
    }

    /// The pairwise keys of `role` the hardware key table holds now.
    pub fn installed_pairwise(&self, role: VifRole) -> usize {
        let installations = self.key_installations.take();
        let count = installations
            .iter()
            .filter_map(|entry| entry.identity.upgrade())
            .filter(|identity| {
                identity.active.get() && identity.group_role.is_none() && identity.role == role
            })
            .count();
        self.key_installations.set(installations);
        count
    }

    /// Capture a protected individually addressed frame as the hardware
    /// does: decrypted with the pairwise key of `role` that the port
    /// installed and that opens it, delivered as its header, its CCMP
    /// header and its plaintext with the decryption observed. Without such
    /// a key the frame is not captured: `false`.
    pub fn receive_protected(&self, frame: &[u8], meta: ModelRxMeta, role: VifRole) -> bool {
        const HEADER: usize = 24;
        const CCMP: usize = oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN;
        let installations = self.key_installations.take();
        let mut opened = None;
        for entry in &installations {
            let Some(identity) = entry.identity.upgrade() else {
                continue;
            };
            if !identity.active.get() || identity.group_role.is_some() || identity.role != role {
                continue;
            }
            let mut wire: Vec<u8> = frame.into();
            let Ok(body) = self.associations.get().get(role).open(
                &mut oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128,
                &entry.key,
                &mut wire,
            ) else {
                continue;
            };
            let body: Vec<u8> = body.into();
            let mut delivered: Vec<u8> = frame.get(..HEADER + CCMP).unwrap_or_default().into();
            delivered.extend_from_slice(&body);
            opened = Some(delivered);
            break;
        }
        self.key_installations.set(installations);
        let Some(delivered) = opened else {
            return false;
        };
        self.receive(
            &delivered,
            meta.with_crypto(RxEvidence::HardwareObserved(
                oer_protocols_ieee80211_contracts_port::RxCryptoStatus::DecryptedAndIntegrityVerified,
            )),
        );
        true
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
        if installation.is_none() {
            return false;
        }
        self.group_rx.push_back(CapturedGroupRx {
            frame: frame.into(),
            meta,
            stage,
        });
        self.wake_world();
        true
    }

    /// Every publication so far, including collisions and unknown outcomes.
    pub fn sent(&self) -> Vec<AirFrame> {
        self.sent.snapshot()
    }

    /// Supply exactly one result for the next publication, a single MPDU's
    /// or a data arena's, in publication order.
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
        self.wake_world();
    }

    /// Stop generating received frames. Units already out still require
    /// recycling through the port.
    pub fn stop_continuous_receive(&self) {
        self.rx_stream.take();
    }

    /// Make the data port's receive pulls answer that the ring has work
    /// only from `at` on, as a ring that waits for its continuation does:
    /// every pull before `at`, and the next one in any case (`at` at the
    /// pull's instant is work beyond the one call). The first pull from
    /// `at` on after that answer takes the ring's units again.
    pub fn defer_receive(&self, at: Instant) {
        self.rx_later.set(Some((at, false)));
    }

    /// The deferred receive's answer to a pull at `now`, if it still
    /// defers.
    fn receive_later(&self, now: Instant) -> Option<Instant> {
        let (at, answered) = self.rx_later.get()?;
        if answered && now >= at {
            self.rx_later.set(None);
            return None;
        }
        self.rx_later.set(Some((at, true)));
        Some(at)
    }

    /// Keep the data port's deadline at `at` or earlier across its
    /// services, as a port whose service leaves work due does; `None`
    /// ends it.
    pub fn keep_due(&self, at: Option<Instant>) {
        self.kept_due.set(at);
        self.wake_world();
    }

    /// Deliver `frame` to the radio once the data port's next receive pull
    /// found the ring empty, waking nobody: a unit that lands during the
    /// actor's turn, whose signal's wake the actor's own next poll of the
    /// facts takes.
    pub fn receive_during_pull(&self, frame: &[u8], meta: ModelRxMeta) {
        self.landing.push_back((frame.into(), meta));
    }

    /// Fire the station's TBTT at the data port's next receive call, waking
    /// nobody: an event that comes during the actor's turn, whose signal's
    /// wake the actor's own next poll of the facts takes. The port's next
    /// facts give it, and the transmit service that takes it announces it.
    pub fn tbtt_during_receive(&self) {
        self.tbtt_landing.set(true);
    }

    /// Move the landing frames to the ring; whether one landed.
    fn land(&self) -> bool {
        let landing = self.landing.take_batch();
        let landed = !landing.is_empty();
        for frame in landing {
            self.rx.push_back(frame);
        }
        landed
    }

    /// Deliver `frame` to the radio.
    pub fn receive(&self, frame: &[u8], meta: ModelRxMeta) {
        self.rx.push_back((frame.into(), meta));
        self.wake_world();
    }

    /// Keep the queue of `access_category` busy, or free it: its attempts
    /// wait unpublished meanwhile, as behind a long transmission.
    pub fn hold(
        &self,
        access_category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory,
        held: bool,
    ) {
        let mut queues = self.held.get();
        *access_category.select_mut(&mut queues) = held;
        self.held.set(queues);
        self.wake_world();
    }

    /// Say that an event waits, or no longer waits, for the port's next
    /// poll.
    pub fn set_event_waiting(&self, waiting: bool) {
        self.event_waiting.set(waiting);
    }

    /// How many arena ends the port served ahead of its transmit service.
    pub fn relayed_arenas(&self) -> usize {
        self.relayed.get()
    }

    /// Delay the current publication's result until released or its bound
    /// expires. A queued successor cannot pass this publication.
    pub fn hold_completion(
        &self,
        category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory,
        held: bool,
    ) {
        let mut results = self.held_results.get();
        *category.select_mut(&mut results) = held;
        self.held_results.set(results);
        self.wake_world();
    }
    /// Refuse the next enable.
    pub fn refuse_enable(&self) {
        self.refuse_enables(1);
    }

    /// Refuse the next `count` access-point commands of a running radio as
    /// not enabled, each with its owners returned.
    pub fn refuse_access_point_commands(&self, count: u8) {
        self.refuse_access_point.set(count);
    }

    /// Whether this access-point command is refused: one of the refusals
    /// asked for.
    pub(crate) fn take_access_point_refusal(&self) -> bool {
        let left = self.refuse_access_point.get();
        if left == 0 {
            return false;
        }
        self.refuse_access_point.set(left.saturating_sub(1));
        true
    }

    /// Refuse the next `count` enables.
    pub fn refuse_enables(&self, count: u8) {
        self.refuse_enable.set(count);
    }

    /// Whether this enable is refused: one of the refusals asked for.
    pub(crate) fn take_enable_refusal(&self) -> bool {
        let left = self.refuse_enable.get();
        self.refuse_enable.set(left.saturating_sub(1));
        left > 0
    }

    /// Poison the backend.
    pub fn poison(&self) {
        self.stream.set(ModelStream::Ended);
    }

    /// Let the port take a poison back for a stop.
    pub fn allow_recovery(&self) {
        self.recovery.set(true);
    }

    /// Take a poison back, when the port may.
    pub(crate) fn recover(&self) -> bool {
        let allowed = self.recovery.get();
        if allowed {
            self.stream.set(ModelStream::Running);
        }
        allowed
    }

    /// An ordinary fault closes admission and returns every admitted owner
    /// before the event stream reports Drained. This differs from `poison`,
    /// which models an accounting/publication uncertainty and retains owners.
    pub fn fail_after_terminals(&self) {
        if matches!(self.stream.get(), ModelStream::Running) {
            self.stream.set(ModelStream::Draining);
        }
    }

    fn failed(&self) -> bool {
        matches!(self.stream.get(), ModelStream::Ended)
    }

    fn admission_closed(&self) -> bool {
        !matches!(self.stream.get(), ModelStream::Running)
    }

    /// Number of actual disable calls; fail-stop is a different transition.
    pub fn disable_calls(&self) -> u32 {
        self.disables.get()
    }

    /// MSDUs that ended with Expired, counted separately from air failures.
    pub fn expired_msdus(&self) -> u32 {
        self.expired_msdus.get()
    }

    fn expired(&self, mask: u64) {
        self.expired_msdus
            .set(self.expired_msdus.get().saturating_add(mask.count_ones()));
    }

    /// What the access point's interface receives and its TSF, as the
    /// modeled hardware holds them.
    pub fn access_point(&self) -> ModelAccessPointInterface {
        self.access_point.get()
    }

    /// The complete EDCA set currently installed for this interface.
    pub fn edca(&self, role: VifRole) -> EdcaParameterSet {
        self.edca.parameters(role)
    }

    /// The frames the air holds for the driver yet.
    pub fn pending_rx(&self) -> usize {
        self.rx.len()
    }

    fn next_answer(&self, default: AirAnswer) -> AirAnswer {
        #[cfg(feature = "test-support")]
        if self.absent_peer.get() {
            return AirAnswer::NoResponse;
        }
        self.answers.pop_front().unwrap_or(default)
    }

    /// The next answer to a data arena's publication: one MPDU (`single`)
    /// soliciting an ACK, or an A-MPDU soliciting a BlockAck of every frame
    /// unless an answer is queued.
    fn arena_result(&self, single: bool) -> AirAnswer {
        self.next_answer(if single {
            AirAnswer::Ack
        } else {
            AirAnswer::BlockAck(u64::MAX)
        })
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
            (_, AirAnswer::CtsTimeout) => Ok(SingleAnswer::CtsTimeout),
            (_, AirAnswer::KeyRefused) => Ok(SingleAnswer::KeyRefused),
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
    CtsTimeout,
    Collision,
    AirDenied,
    HardwareTimeout,
    KeyRefused,
}

#[derive(Clone, Copy, Debug)]
enum ResponsePolicy {
    NoResponseExpected,
    Ack,
    BlockAck { start_sequence: SequenceNumber },
}

impl ResponsePolicy {
    fn of_frame<K>(frame: &TxFrame<'_, K>) -> Self {
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

type StationGroupPermission<'id, B> = oer_protocols_ieee80211_contracts_port::IssuedSlot<
    'id,
    ModelGroupPermission<Station>,
    ModelPool<<B as ModelComposition>::Identity, 8>,
>;

struct PendingGroupInstall<'id, B: ModelComposition, R: ModelRole> {
    spp: oer_protocols_ieee80211_wire::security::Spp,
    /// The installation that issued the association.
    association: u32,
    slot: GroupKeySlot<'id, R, MODEL_KEY_SLOTS, MODEL_KEY_SLOTS, R::AssociationPath<B::Identity>>,
    permission: oer_protocols_ieee80211_contracts_port::IssuedSlot<
        'id,
        ModelGroupPermission<R>,
        R::GroupPermissionPath<B::Identity>,
    >,
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
        permission: oer_protocols_ieee80211_contracts_port::IssuedSlot<
            'o,
            ModelGroupPermission<Self>,
            Self::GroupPermissionPath<B::Identity>,
        >,
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
        permission: oer_protocols_ieee80211_contracts_port::IssuedSlot<
            'o,
            ModelGroupPermission<Self>,
            oer_protocols_radio_port::Child<ModelPool<B::Identity, 9>, 0>,
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

/// The work of a queue: one prepared MPDU.
type Work<'id, B> = Box<Prepared<'id, B>>;

type ModelInFlight<'id, B, M> = InFlight<
    ModelContextOf<B>,
    Slot<'id, ModelTickets, ModelPool<<B as ModelComposition>::Identity, 1>>,
    M,
>;

/// An attempt prepared for its queue.
struct Prepared<'id, B: ModelComposition> {
    operation: ModelInFlight<'id, B, ModelMpdu<'id, B>>,
    role: VifRole,
    queue: oer_protocols_ieee80211_wire::qos::WmmAccessCategory,
    plan: oer_protocols_ieee80211_contracts_port::RetryPlan<PhyRate>,
    key: Option<PreparedKey>,
    /// A beacon's permission, which its terminal returns.
    beacon: Option<OwnedBeacon<B>>,
    /// A beacon's deadline: not published by then, it ends expired.
    deadline: Option<Instant>,
}

impl<B: ModelComposition> Prepared<'_, B> {
    const fn role(&self) -> VifRole {
        self.role
    }
    /// Whether it aged past its MSDU `lifetime` or its deadline at `now`.
    fn expired(&self, now: Instant, lifetime: Duration) -> bool {
        self.operation.payload().expired(now, lifetime)
            || self.deadline.is_some_and(|deadline| now >= deadline)
    }
    fn key(&self) -> Option<&RawKeyIdentity> {
        self.key.as_ref().map(|key| &key.identity)
    }
}

/// The installation's owned queues: the terminals and received units the
/// port puts in its sink, and the owners it retains. RX capacity stays
/// charged until recycle.
pub struct ModelHeld<'id, B: ModelComposition> {
    air: Rc<ModelAir>,
    events: Bounded<OwnedLowerMacEvent<'id, B>>,
    quarantined: Bounded<Work<'id, B>>,
    /// The receive buffers out with the caller, queued or held.
    rx_out: Cell<usize>,
    /// The receive buffers lent to the network.
    network: alloc::sync::Arc<ModelNetwork>,
    /// The backend alone attests the hardware barrier with the one gate place.
    gate_issuer: GateIssuer<'id, ModelPool<<B as ModelComposition>::Identity, 0>>,
    /// A stop returns all queued operations before the radio stops. Unlike
    /// poison, this is an ordinary terminal.
    draining: Cell<bool>,
    /// The station's two data arenas, which the port lends for one turn
    /// and serves.
    arenas: core::cell::RefCell<arena::StationArenas>,
    /// Station data publications wait: the gate is closed or closing, or a
    /// station configuration is pending.
    gate_closed: Cell<bool>,
    configuring_station: Cell<bool>,
}

impl<'id, B: ModelComposition> ModelHeld<'id, B> {
    fn post(&self, event: OwnedLowerMacEvent<'id, B>) {
        if self.events.push(event).is_err() {
            self.poison();
        }
    }

    fn take_event(&self) -> Option<OwnedLowerMacEvent<'id, B>> {
        self.events.pop()
    }

    fn poison(&self) {
        self.air.poison();
    }

    /// Charge one received buffer to the bounded RX capacity, when a place
    /// is left. The bound is checked before the count moves; a count that
    /// could not move is a broken model invariant and poisons, as a return
    /// with none charged does.
    fn charge_rx(&self) -> bool {
        let out = self.rx_out.get();
        if out >= self.air.rx_limits.get().held {
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

mod installation;
pub use installation::{ModelBackend, ModelPools};
mod data_port;
pub use data_port::{
    MODEL_HELD_COMMAND_BOUND, MODEL_STOP_BOUND, ModelAccessPointGroupInstall,
    ModelAccessPointPairwiseInstall, ModelAdmitted, ModelCommands, ModelGroupInstall, ModelInstall,
    ModelPairwiseInstall, ModelPort, ModelPortArena, ModelPortCall, ModelPortFault,
    ModelPortObservation, ModelReinstallRefused,
};

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
            pending.slot.generation(),
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
                installed.identity.0.active.set(false);
                installed.identity.0.rx_active.set(false);
                held.air.remove_key_material(&installed.identity);
                self.keys.retain(|key| *key != installed.identity);
                let identity = RawKeyIdentity::group(
                    installed.slot.index(),
                    installed.slot.generation(),
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
        held.post(terminal);
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
                    let progress = publication.air.map(|ticket| held.air.air_progress(ticket));
                    let on_air = progress == Some(AirProgress::Pending) && !held.draining.get();
                    if on_air || (result_held && elapsed < MODEL_PUBLICATION_BOUND.as_micros()) {
                        queue.air = Some(publication);
                        break;
                    }
                    if let Some(ticket) = publication.air {
                        held.air.forget_air(ticket);
                    }
                    let cause =
                        withdrawal(held, &self.gate, publication.work.key(), &self.removing)
                            .or_else(|| {
                                reconfigure(publication.work.role())
                                    .then_some(WithdrawCause::GateClosed)
                            });
                    let work = if result_held {
                        publication.work.timed_out()
                    } else if progress == Some(AirProgress::Collided) {
                        publication.work.unanswered()
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
            if overdue || held.air.failed() {
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

    /// Deliver a unit of a group mutation's receive cut, or drop it when
    /// the receive capacity is full.
    fn deliver(&mut self, held: &ModelHeld<'id, B>, frame: Vec<u8>, meta: ModelRxMeta) {
        if held.charge_rx() {
            held.post(OwnedLowerMacEvent::Received {
                frame: OwnedRxBuffer {
                    identity: PhantomData,
                    bytes: frame,
                    recycled: false,
                    air: Rc::downgrade(&held.air),
                },
                meta,
            });
        }
    }
}

/// Synthetic model latency, in model time; hardware backends declare their
/// own PPDU + response/timeout bound. It does not scale with retry-plan length.
pub const MODEL_PUBLICATION_BOUND: oer_portable_time::Duration =
    oer_portable_time::Duration::from_millis(1);
/// Synthetic admission-to-GTK-terminal bound, driven by the supplied clock's owner.
pub const MODEL_GROUP_COMMAND_BOUND: oer_portable_time::Duration =
    oer_portable_time::Duration::from_millis(2);

struct TxQueue<'id, B: ModelComposition> {
    waiting: VecDeque<Work<'id, B>>,
    air: Option<Publication<'id, B>>,
}
struct Publication<'id, B: ModelComposition> {
    started: u64,
    /// Its exchange on a timed air.
    air: Option<AirTicket>,
    work: Published<'id, B>,
}
struct SinglePlan<'id, B: ModelComposition> {
    prepared: Box<Prepared<'id, B>>,
    remaining: VecDeque<PhyRate>,
    last: Option<(NonZeroU16, PhyRate)>,
    rate: PhyRate,
}
struct Published<'id, B: ModelComposition> {
    plan: SinglePlan<'id, B>,
    answer: SingleAnswer,
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
        self.plan.prepared.role
    }
    fn key(&self) -> Option<&RawKeyIdentity> {
        self.plan.prepared.key()
    }
    /// The publication with its answer replaced by the hardware timeout.
    fn timed_out(self) -> Self {
        Self {
            plan: self.plan,
            answer: SingleAnswer::HardwareTimeout,
        }
    }
    /// The publication after a collision on the air: no response came.
    fn unanswered(self) -> Self {
        Self {
            plan: self.plan,
            answer: SingleAnswer::Unanswered,
        }
    }
    fn complete(
        self,
        held: &ModelHeld<'id, B>,
        keys: &[RawKeyIdentity],
        cause: Option<WithdrawCause>,
    ) -> Option<Publication<'id, B>> {
        let Self {
            mut plan,
            answer: result,
        } = self;
        let unknown = matches!(
            result,
            SingleAnswer::HardwareTimeout | SingleAnswer::KeyRefused
        );
        let (outcome, last) = single_outcome(result, plan.last, plan.rate, cause);
        plan.last = last;
        // A transmission went out, or an indeterminate end (a hardware timeout,
        // a refused key) leaves it possible.
        if last.is_some() || unknown {
            plan.prepared.operation.execute().mpdu.retry = true;
        }
        if matches!(
            outcome,
            TxOutcome::Ended(
                SingleEnd::AckTimeout(_) | SingleEnd::CtsTimeout(_) | SingleEnd::ContentionLimit(_)
            )
        ) && let Some(rate) = plan.remaining.pop_front()
        {
            plan.rate = rate;
            return publish_single(held, keys, plan);
        }
        terminal(held, *plan.prepared, outcome);
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
        (SingleAnswer::KeyRefused, _) => (
            TxOutcome::Indeterminate(IndeterminateCause::KeyRefused),
            last,
        ),
        (SingleAnswer::CtsTimeout, Some(cause)) => (TxOutcome::Withdrawn(cause), last),
        (SingleAnswer::CtsTimeout, None) => (TxOutcome::Ended(SingleEnd::CtsTimeout(before)), last),
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

fn start_work<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    keys: &[RawKeyIdentity],
    prepared: Work<'id, B>,
) -> Option<Publication<'id, B>> {
    let rate = prepared.plan.first().rate;
    let remaining = prepared
        .plan
        .stages()
        .flat_map(|stage| core::iter::repeat_n(stage.rate, usize::from(stage.transmissions.get())))
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
    if plan.prepared.expired(now, lifetime) {
        held.air.expired(1);
        terminal(held, *plan.prepared, TxOutcome::Expired { msdus: 1 });
        return None;
    }
    let answer = match held
        .air
        .single_result(plan.prepared.operation.payload().response)
    {
        Ok(answer) => answer,
        Err(AnswerShapeMismatch) => {
            let _fault = held.quarantined.push(plan.prepared);
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
        let _fault = held.quarantined.push(plan.prepared);
        held.poison();
        return None;
    }
    // Retry once an earlier transmission could have reached the air: in this
    // plan, or before it, as the `Sent` this retransmission came from holds.
    if plan.last.is_some() || plan.prepared.operation.payload().buffer.retry_owed() {
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
            let _fault = held.quarantined.push(plan.prepared);
            held.poison();
            return None;
        }
    };
    let air = match single_air(held, &plan, &frame, answer) {
        Ok(air) => air,
        Err(fault) => {
            held.air.air_fault.set(Some(fault));
            let _fault = held.quarantined.push(plan.prepared);
            held.poison();
            return None;
        }
    };
    // A denied PPDU or a data frame behind a missing CTS never reached the
    // air the journal records.
    if !matches!(answer, SingleAnswer::AirDenied | SingleAnswer::CtsTimeout) {
        held.air.sent.record(frame);
    }
    Some(Publication {
        started: held.air.monotonic_now().as_micros(),
        air,
        work: Published { plan, answer },
    })
}

/// The exchange of a single MPDU on a timed air: `frame` as it went, under
/// the plan's first protection. An answer that ends the attempt before the
/// air takes no time.
fn single_air<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    plan: &SinglePlan<'id, B>,
    frame: &AirFrame,
    answer: SingleAnswer,
) -> Result<Option<AirTicket>, AirTimeError> {
    let response = match answer {
        SingleAnswer::Unsolicited | SingleAnswer::Unanswered => AirResponse::None,
        SingleAnswer::Ack => AirResponse::Ack,
        SingleAnswer::BlockAck(_) => AirResponse::BlockAck,
        // A missing CTS ends the attempt before its data reached the air;
        // the RTS's own time is not modelled.
        SingleAnswer::Collision
        | SingleAnswer::AirDenied
        | SingleAnswer::HardwareTimeout
        | SingleAnswer::CtsTimeout
        | SingleAnswer::KeyRefused => {
            return Ok(None);
        }
    };
    let octets = u32::try_from(frame.frame.len())
        .ok()
        .and_then(|length| length.checked_add(air_time::FCS_OCTETS))
        .ok_or(AirTimeError::Unrepresentable)?;
    held.air.request_air(
        plan.rate,
        octets,
        plan.prepared.plan.first().protection,
        response,
        plan.prepared.queue,
        1,
    )
}

/// Preserve every owner of `work` on withdrawal.
fn withdrawn<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    work: Work<'id, B>,
    cause: WithdrawCause,
) {
    terminal(held, *work, TxOutcome::Withdrawn(cause));
}

/// Post the terminal of `prepared`, with its tokens.
fn terminal<'id, B: ModelComposition>(
    held: &ModelHeld<'id, B>,
    prepared: Prepared<'id, B>,
    outcome: ModelSingleOutcome,
) {
    held.post(OwnedLowerMacEvent::Terminal(
        OwnedLowerMacTerminal::TxCompleted(OwnedTxCompleted::of(
            prepared.operation.complete(outcome),
            prepared.beacon,
        )),
    ));
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
    frame: &TxFrame<'_, C>,
) -> oer_protocols_ieee80211_contracts_port::RetryPlan<PhyRate> {
    match frame {
        TxFrame::Individual { plan, .. } => plan.map(|rate| rate.get()),
        TxFrame::IndividualNoAck { rate, .. } => one_stage(rate.rate().get()),
        TxFrame::Group { rate, .. } => one_stage(rate.get()),
        TxFrame::BlockAckRequest { plan, .. } => plan.map(|rate| rate.rate().get()),
    }
}
fn one_stage(rate: PhyRate) -> oer_protocols_ieee80211_contracts_port::RetryPlan<PhyRate> {
    oer_protocols_ieee80211_contracts_port::RetryPlan::new(
        oer_protocols_ieee80211_contracts_port::RetryStage {
            rate,
            transmissions: core::num::NonZeroU8::MIN,
            protection: oer_protocols_ieee80211_contracts_port::Protection::None,
        },
    )
}

fn retry_stages<K>(
    layout: &MpduLayout,
    plan: &RetransmissionPlan<'_, K>,
) -> Result<oer_protocols_ieee80211_contracts_port::RetryPlan<PhyRate>, PreparationError> {
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
type ModelSource<'k, 'p, 'id, C, K, R, B> = TxSource<
    ModelBuffer<'id, B, C>,
    ModelSent<'id, B, C>,
    oer_protocols_ieee80211_contracts_port::ClassFrame<'p, C, K>,
    ModelBodyOf<B>,
    <R as RoleKeys<'id, ModelTypes<B>>>::TxKey<'k>,
    RetransmissionPlan<'p, K>,
>;
type ModelCheckedSource<'k, 'p, 'id, C, K, R, B> = CheckedTxSource<
    ModelBuffer<'id, B, C>,
    ModelSent<'id, B, C>,
    oer_protocols_ieee80211_contracts_port::ClassFrame<'p, C, K>,
    ModelBodyOf<B>,
    <R as RoleKeys<'id, ModelTypes<B>>>::TxKey<'k>,
    RetransmissionPlan<'p, K>,
>;
type ModelPreparation<'k, 'p, 'id, R, K, B, C> = Result<
    ModelPrepared<'k, 'p, 'id, R, K, B>,
    PreparationRefused<ModelSource<'k, 'p, 'id, C, K, R, B>>,
>;

/// Closed preparation proof retaining role, port borrow, install and the key
/// borrow.
/// Dropping or submitting it ends that borrow; a queued operation owns only
/// the backend identity, never a reference to a portable machine.
pub struct ModelPrepared<'k, 'p, 'id, R, K, B: ModelComposition> {
    loan: BodyLoan<'id, B::Identity, B>,
    role: VifRole,
    mpdu: ModelMpdu<'id, B>,
    plan: oer_protocols_ieee80211_contracts_port::RetryPlan<PhyRate>,
    key: Option<PreparedKey>,
    key_borrow: PhantomData<&'k ()>,
    proof: PhaseProof<'p, (R, K, B)>,
}
/// Metadata lowered before single-MPDU admission; it owns no payload. A
/// beacon's admission adds the beacon permission its terminal returns.
pub struct ModelTxExecution<B: ModelComposition> {
    plan: oer_protocols_ieee80211_contracts_port::RetryPlan<PhyRate>,
    key: Option<PreparedKey>,
    role: VifRole,
    beacon: Option<OwnedBeacon<B>>,
    /// A beacon's deadline: not published by then, it ends expired.
    deadline: Option<Instant>,
}

/// The beacon permission while the model holds its beacon.
type OwnedBeacon<B> =
    oer_protocols_ieee80211_contracts_port::BeaconPermission<'static, ModelBeaconPath<B>>;
impl<'id, R, K, B: ModelComposition + 'id> PreparedTx<'id> for ModelPrepared<'_, '_, 'id, R, K, B> {
    type Types = ModelTypes<B>;
    type Backend = B;
    type OwnedContext = ModelContextOf<B>;
    type OwnedPayload = ModelMpdu<'static, B>;
    type Execution = ModelTxExecution<B>;
    fn into_admission(
        self,
        context: <Self::Types as PortTypes<'id>>::Context,
        ticket: Slot<'id, ModelTickets, ModelPool<B::Identity, 1>>,
        _: oer_protocols_ieee80211_contracts_port::Admission<Self>,
    ) -> oer_protocols_ieee80211_contracts_port::AdmissionOwners<
        Self::OwnedContext,
        Slot<'static, ModelTickets, ModelPool<B::Identity, 1>>,
        Self::OwnedPayload,
        Self::Execution,
    > {
        let loan = self.loan;
        oer_protocols_ieee80211_contracts_port::AdmissionOwners {
            context: context.enclose().into_owned(&loan),
            ticket: loan.reclaim_slot(ticket),
            payload: self.mpdu.into_owned(&loan),
            execution: ModelTxExecution {
                plan: self.plan,
                key: self.key,
                role: self.role,
                beacon: None,
                deadline: None,
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
    type Path<Id: SlotIdentity>: oer_protocols_radio_port::PoolPath<Identity = Id>;
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
                    retry: false,
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
        role: R::ROLE,
        mpdu,
        plan,
        key: key.map(PreparedKey::new),
        key_borrow: PhantomData,
        proof: PhantomData,
    })
}

/// The subframes one model A-MPDU holds.
pub const MODEL_AMPDU_SUBFRAMES: usize = 8;

/// The longest model A-MPDU at every HT and HE rate: the 65 535 octets of
/// the HT-SIG HT Length field.
pub static MODEL_AMPDU_LENGTHS: AmpduLengthTable = AmpduLengthTable::uniform(65_535);

mod values;
use values::*;

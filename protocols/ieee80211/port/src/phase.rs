//! A running lower-MAC phase: one object under `&mut`
//! ([`PortPhase`]), whose event stream and
//! operations a body's one loop drives.
//!
//! # Tokens
//!
//! Every token carries the install's brand `'id`, is minted once by the
//! install's resources and travels by value: an operation takes it, and
//! the event that ends the operation gives it back. So a portable core
//! holds the tokens of what it may do, and what it holds is what it may
//! do: a transmission it has no ticket for, a key it has no slot for or a
//! gate it has not closed does not compile.
//!
//! | Token | Taken by | Given back by |
//! | --- | --- | --- |
//! | a transmit ticket ([`PhaseTypes::TicketKind`]) | [`InterfacePhase::submit`] | [`LowerMacTerminal::TxCompleted`] |
//! | a transmit buffer of a class ([`TxBufferClass`]), its memory ([`TxBufferMemory`]) | [`InterfacePhase::submit`], together with its [`ClassFrame`] | [`LowerMacTerminal::TxCompleted`], every buffer of the attempt |
//! | a key slot of a role's kind | [`InterfacePhase::install_pairwise_key`], [`InterfacePhase::install_group_key`] (the installed key holds it) | [`LowerMacTerminal::KeyRemoved`], after a removal |
//! | one GTK mutation permission per interface ([`KeyTypes::GroupPermission`]) | install/replace/select/remove GTK | the corresponding FIFO success terminal; uncertainty keeps it for teardown |
//! | one configure permission per interface ([`crate::ConfigurePermission`]) | `configure(permission, receive)` | [`LowerMacTerminal::Configured`], with the same permission and opaque context |
//! | one TSF permission per interface ([`crate::TsfPermission`]) | `sample_tsf(permission)` | [`LowerMacTerminal::TsfSampled`], with the same permission |
//! | a Block Ack slot | [`InterfacePhase::add_block_ack`] (the agreement holds it) | [`InterfacePhase::remove_block_ack`] |
//! | the open gate ([`GateOpen`]) | [`LowerMacPhase::close_gate`] | [`LowerMacPhase::open_gate`], which takes the closed gate ([`GateClosed`]) that [`LowerMacTerminal::GateClosed`] gave |
//! | a received frame's buffer | [`LowerMacEvent::Received`] | [`LowerMacPhase::recycle`] |
//!
//! A ticket and a buffer are different tokens.
//! A ticket is a place in flight and a place for its terminal; their
//! number is bounded by the hardware. A buffer is memory of one class,
//! sized by the composition, from a pool the body holds: the caller
//! encodes its frame straight into it. A submission
//! takes one ticket and the buffers of its frames (an A-MPDU: one ticket
//! and a buffer per subframe), and its terminal gives all of them back. So
//! a submission never waits: an empty pool is the caller's own capacity,
//! and what the hardware cannot take at once (an occupied queue) the
//! backend keeps until it can. Queue permits are the backend's own and
//! never reach a caller. How many tickets and buffers each role holds,
//! with a reserve for its management frames, is the composition's
//! choice: the tokens are values it hands out.
//!
//! Settings without a terminal (EDCA, BSS color, TBTT plan, monitor and
//! Block Ack desired state) take effect before the next submission; the latest
//! value of a kind wins. Receive configuration and TSF sampling each consume
//! their interface's unique permission and occupy a FIFO command until their
//! terminal returns that same permission; neither can replace a prior request.
//!
//! An operation that waits for the air or for the attempts in flight (a
//! gate's closing, an interface's receive configuration, a key's removal)
//! returns at once, and its end arrives as an event of the stream. No
//! operation of the phase awaits.
//!
//! # Order
//!
//! The phase's operations reach the hardware in the order the caller made
//! them, through one queue to the driver: an attempt that names a key is
//! published after that key's install, and a key's removal ends after
//! every attempt that names it. A backend's conformance tests check both.
//!
//! The queue is strictly first in, first out: no command replaces another.
//! Its capacity is the tokens': one attempt per ticket,
//! two commands per pairwise slot, one command per interface's unique GTK
//! mutation permission, and two commands for the gate. Pairwise install and
//! removal, or gate closing and opening, may wait together; a GTK permission
//! returns only at its terminal. Thus the command capacity is
//! `tickets + 2 × pairwise slots + GTK permissions + configure permissions + TSF permissions + 2`.
//! There is one configure and one TSF permission per
//! interface, minted by `InterfaceCommandPool`. Block Ack is per-slot desired state;
//! repeated writes coalesce and never enter the command queue.
//!
//! # Protected frames
//!
//! Preparation derives the Protected bit from [`TxProtection`] and
//! leaves the 8 octets of its CCMP header after its MAC header; its length
//! counts them, not the MIC. At the first transmission of the MPDU the
//! backend writes the CCMP header with a new packet number of the key into
//! the buffer, and the hardware appends the MIC; the plan's transmissions
//! keep it. The completion gives the buffer back as `Sent`
//! ([`TxBufferPool::Sent`]): a retransmission of the same MPDU
//! ([`TxSource::Retransmission`]) keeps its packet number, and only a
//! renewed buffer ([`TxBufferPool::renew`]) is numbered anew. A received
//! frame whose crypto status is
//! [`RxCryptoStatus::DecryptedAndIntegrityVerified`](crate::RxCryptoStatus)
//! keeps its CCMP header, for the receiver's replay check, and loses its
//! MIC.
//!
//! # Compile-fail tests
//!
//! No caller mints a token: a gate, a ticket or a buffer comes only from
//! the install's resources.
//!
//! ```compile_fail,E0432
//! use oer_ieee80211_port::GatePlace;
//! ```
//!
//! Nor copies one:
//!
//! ```compile_fail,E0382
//! # use oer_ieee80211_port::GateOpen;
//! fn twice<'id>(gate: GateOpen<'id>) -> (GateOpen<'id>, GateOpen<'id>) {
//!     (gate, gate)
//! }
//! ```
//!
//! ```compile_fail,E0599
//! # use oer_radio_port::Slot;
//! fn copy<'id, K>(ticket: &Slot<'id, K>) -> Slot<'id, K> {
//!     ticket.clone()
//! }
//! ```
//!
//! Its compiling twin moves the token on, as an operation takes it:
//!
//! ```
//! # use oer_ieee80211_port::{GateClosed, GateOpen, LowerMacPhase, PhaseTypes};
//! fn close<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P, gate: GateOpen<'id, <P::Types as PhaseTypes<'id>>::GatePath>) {
//!     phase.close_gate(gate);
//! }
//! fn open<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P, gate: GateClosed<'id, <P::Types as PhaseTypes<'id>>::GatePath>) -> GateOpen<'id, <P::Types as PhaseTypes<'id>>::GatePath> {
//!     phase.open_gate(gate)
//! }
//! ```
//!
//! The open gate does not open a gate, nor the closed one close it:
//!
//! ```compile_fail,E0308
//! # use oer_ieee80211_port::{GateOpen, LowerMacPhase, PhaseTypes};
//! fn reopen<'p, 'id: 'p, P: LowerMacPhase<'p, 'id>>(phase: &mut P, gate: GateOpen<'id, <P::Types as PhaseTypes<'id>>::GatePath>) {
//!     let _ = phase.open_gate(gate);
//! }
//! ```
//!
//! A frame a buffer's class does not fit has no [`ClassFrame`], whose
//! fields no caller sets:
//!
//! ```compile_fail,E0451
//! # use oer_ieee80211_port::{ClassFrame, TxFrame};
//! fn forge<'p, 'id, C, K>(frame: TxFrame<'p, 'id, K>) -> ClassFrame<'p, 'id, C, K> {
//!     ClassFrame { frame, class: core::marker::PhantomData }
//! }
//! ```
//!
//! Its compiling twin checks the frame against the class:
//!
//! ```
//! # use oer_ieee80211_port::{ClassFrame, Ieee80211Capabilities, TxBufferClass, TxFrame};
//! fn fit<'p, 'id, C: TxBufferClass, K: Ieee80211Capabilities>(
//!     frame: TxFrame<'p, 'id, K>,
//! ) -> Result<ClassFrame<'p, 'id, C, K>, TxFrame<'p, 'id, K>> {
//!     ClassFrame::new(frame)
//! }
//! ```
//!
//! An attempt moves its context, ticket and buffer once:
//!
//! ```compile_fail,E0382
//! # use oer_ieee80211_port::*;
//! fn twice<'p, 'id: 'p, I>(interface: &mut I, attempt: InterfaceAttempt<'_, 'p, 'id, I>)
//! where I: InterfacePhase<'p, 'id> {
//!     let _ = interface.submit(attempt.admit());
//!     let _ = interface.submit(attempt.admit());
//! }
//! ```
//!
//! Its compiling twin moves each operation once, with no `Clone` bound:
//!
//! ```
//! # use oer_ieee80211_port::*;
//! fn one<'p, 'id: 'p, I>(interface: &mut I, attempt: InterfaceAttempt<'_, 'p, 'id, I>)
//! where I: InterfacePhase<'p, 'id> {
//!     let _ = interface.submit(attempt.admit());
//! }
//! ```
//!
//! A frame checked on one phase cannot be submitted on an unrelated one:
//!
//! ```compile_fail
//! # use oer_ieee80211_port::*;
//! fn another_phase<'p, 'q, 'id: 'p + 'q, I, J>(
//!     interface: &mut J, attempt: InterfaceAttempt<'_, 'p, 'id, I>,
//! ) where
//!     I: InterfacePhase<'p, 'id>,
//!     J: InterfacePhase<'q, 'id, Caps = I::Caps, Types = I::Types, Role = I::Role>,
//! {
//!     let _ = interface.submit(attempt.admit());
//! }
//! ```
//!
//! # Event stream end
//!
//! A command's [`Poisoned`] result closes admission; earlier terminals may
//! still be waiting. `next_event` ends with [`oer_radio_port::EventEnd`]:
//! `Drained` promises every admitted terminal was returned, while `Quarantined`
//! retains undelivered owners in the common lifecycle's permanent quarantine.
//! The body checks its outstanding identities before accepting `Drained`.
//! On `Quarantined` it fail-stops immediately, without disable or another
//! drain. Subsequent event calls, including through a new phase loan, return
//! the same end and cause. Dropping a quarantine receipt releases no owners.

use oer_ieee80211_wire::ccmp::{Ccmp128Key, GroupKeyId};
use oer_radio_port::PortPhase;
use oer_radio_port::{Poisoned, PoolPath, Root, Slot};

use crate::{
    Channel, PhyRate, Protection, TbttEvent, TxOperation,
    owner::{
        AccessPoint, BssColor, Ieee80211Capabilities, InterfaceRole, PhaseRate, Station,
        StationReceive, SupportedBlockAck, SupportedEdca, SupportedFilter, SupportedNoAck,
        SupportedPower, TxFrame,
    },
};
use oer_ieee80211_wire::address::IndividualAddress;

enum GateKind {}

/// The gate component of a lower-MAC backend's pool set.
///
/// The backend receives its open gate and issuer together when its one
/// [`oer_radio_port::Mint`] is consumed. The unique place remains private.
///
/// ```compile_fail,E0432
/// use oer_ieee80211_port::GatePlace;
/// ```
///
/// ```
/// use oer_ieee80211_port::{GateIssuer, GateOpen, GatePool};
/// use oer_radio_port::Mint;
/// fn gate<'id>(mint: Mint<'id, GatePool>) -> (GateOpen<'id>, GateIssuer<'id>) {
///     mint.into_pools()
/// }
/// ```
pub struct GatePool;

impl oer_radio_port::PoolSet for GatePool {
    type Pools<'id, Path: PoolPath> = (GateOpen<'id, Path>, GateIssuer<'id, Path>);

    fn build<'id, Path: PoolPath>(
        builder: oer_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        GatePlace::mint(builder.into_pools::<oer_radio_port::Slots<GateKind, 1>>())
    }
}

/// The install's unique gate place, held inside exactly one gate state.
#[must_use = "the unique gate place stays in one gate state"]
struct GatePlace<'id, Path: PoolPath = Root> {
    slot: Slot<'id, GateKind, Path>,
}

impl<'id, Path: PoolPath> GatePlace<'id, Path> {
    fn mint(
        [slot]: [Slot<'id, GateKind, Path>; 1],
    ) -> (GateOpen<'id, Path>, GateIssuer<'id, Path>) {
        (
            GateOpen {
                place: Self { slot },
            },
            GateIssuer {
                brand: core::marker::PhantomData,
            },
        )
    }
}

impl<Path: PoolPath> core::fmt::Debug for GatePlace<'_, Path> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GatePlace")
            .field("slot", &self.slot.index())
            .finish()
    }
}

/// The open transmit gate. Closing transfers its unique place to the backend.
#[derive(Debug)]
#[must_use = "close_gate consumes the open gate and returns its closed evidence"]
pub struct GateOpen<'id, Path: PoolPath = Root> {
    place: GatePlace<'id, Path>,
}

/// Evidence that the backend publishes no attempt; it does not prove quiet air.
/// Only the retained backend issuer constructs this state after its barrier.
///
/// ```compile_fail,E0616
/// # use oer_ieee80211_port::{GateClosed, GateOpen};
/// fn forge<'id>(gate: GateOpen<'id>) -> GateClosed<'id> {
///     GateClosed { place: gate.place }
/// }
/// ```
#[derive(Debug)]
#[must_use = "open_gate consumes the closed evidence and returns the open gate"]
pub struct GateClosed<'id, Path: PoolPath = Root> {
    place: GatePlace<'id, Path>,
}

/// The backend's authority to convert its one gate place between states.
///
/// One issuer is retained by the backend per install and never reaches the
/// body. Closing is allowed only after the current publication's result and
/// the withdrawal of queued attempts. Opening consumes that very evidence.
///
/// ```compile_fail,E0382
/// # use oer_ieee80211_port::{GateClosed, GateIssuer, GateOpen};
/// fn both<'id>(issuer: &mut GateIssuer<'id>, gate: GateOpen<'id>)
///     -> (GateOpen<'id>, GateClosed<'id>)
/// {
///     let closed = issuer.close(gate);
///     (gate, closed)
/// }
/// ```
#[must_use = "the backend retains the issuer for the whole install"]
pub struct GateIssuer<'id, Path: PoolPath = Root> {
    brand: core::marker::PhantomData<fn(&'id (), Path) -> &'id ()>,
}

impl<'id, Path: PoolPath> GateIssuer<'id, Path> {
    /// Attest the completed hardware barrier, retaining the same place.
    pub fn close(&mut self, gate: GateOpen<'id, Path>) -> GateClosed<'id, Path> {
        GateClosed { place: gate.place }
    }

    /// Consume the closed evidence to open the same gate.
    pub fn open(&mut self, gate: GateClosed<'id, Path>) -> GateOpen<'id, Path> {
        GateOpen { place: gate.place }
    }
}

/// An event of a running lower-MAC phase, in the order the events
/// occurred; every token an operation took comes back in one.
pub enum LowerMacEvent<'id, P: PhaseTypes<'id>> {
    /// One received MPDU and its metadata.
    Received {
        /// The frame, from its header to the end of its body, without the
        /// FCS.
        frame: P::RxBuffer,
        /// What the hardware observed of it.
        meta: P::RxMeta,
    },
    /// A TBTT of an interface's schedule.
    Tbtt(TbttEvent),
    /// A command completion, carrying every returned owner.
    Terminal(LowerMacTerminal<'id, P>),
}

/// Completions which a disabled port can return without lending an RX phase.
/// Unsolicited RX and TBTT are not representable here.
pub enum LowerMacTerminal<'id, P: PhaseTypes<'id>> {
    /// The terminal preserves the submitted shape and all of its owners.
    TxCompleted(TxCompleted<'id, P>),
    /// The paired TSF reading a
    /// [`BeaconTimingPhase::sample_tsf`](crate::BeaconTimingPhase::sample_tsf)
    /// asked for.
    TsfSampled {
        /// The paired hardware and radio clock reading.
        sample: crate::TsfSample,
        /// The request's original interface admission.
        permission: crate::SampledTsfPermission<'id, P>,
    },
    /// The transmit gate closed: no attempt is published.
    GateClosed(GateClosed<'id, P::GatePath>),
    /// An interface's receive configuration took effect.
    Configured(crate::Configured<'id, P>),
    /// A removed key's slot, of its role and kind, once no attempt in
    /// flight uses the key.
    KeyRemoved(Removed<'id, P>),
    /// A station GTK replacement completed in the same slot. RX before this
    /// terminal belongs to the previous installation; RX after it belongs to
    /// this one. Replay state is owned above the port.
    GroupKeyReplaced {
        /// The installed owner, returned exactly once.
        key: <P::StationKeys as KeyTypes<'id>>::GroupKey,
        /// Exclusive admission for the next GTK mutation.
        permission: <P::StationKeys as KeyTypes<'id>>::GroupPermission,
    },
    /// A new installation is effective. This terminal is an RX boundary,
    /// including frames already captured by DMA/ISR/software queues.
    GroupKeyInstalled(InstalledGroup<'id, P>),
    /// A retained installation is selected without writing material or
    /// resetting counters. It is the sole RX-active installation of its ID.
    GroupKeySelected {
        /// The installed owner, returned exactly once.
        key: <P::StationKeys as KeyTypes<'id>>::GroupKey,
        /// Exclusive admission for the next GTK mutation.
        permission: <P::StationKeys as KeyTypes<'id>>::GroupPermission,
    },
}

/// An accepted attempt's terminal, retaining its closed operation owner.
/// The higher layer consumes `Completed::into_parts` to recover context,
/// ticket and the complete MPDU or aggregate. Queues cannot exchange their
/// components before that terminal boundary.
pub enum TxCompleted<'id, P: PhaseTypes<'id>> {
    /// Exactly one submitted MPDU, including its original body owner.
    Single(SingleCompleted<'id, P>),
    /// Every submitted subframe, in order, each owning its memory and body.
    Aggregate(AggregateCompleted<'id, P>),
}

/// A backend's single terminal, with its original accepted owner.
pub type SingleCompleted<'id, P> = crate::Completed<
    <P as PhaseTypes<'id>>::Context,
    Slot<'id, <P as PhaseTypes<'id>>::TicketKind, <P as PhaseTypes<'id>>::TicketPath>,
    <P as PhaseTypes<'id>>::TxMpdu,
    PhaseSingleOutcome<'id, P>,
>;
/// A backend's aggregate terminal, with every complete submitted subframe.
pub type AggregateCompleted<'id, P> = crate::Completed<
    <P as PhaseTypes<'id>>::Context,
    Slot<'id, <P as PhaseTypes<'id>>::TicketKind, <P as PhaseTypes<'id>>::TicketPath>,
    <P as PhaseTypes<'id>>::TxAggregate,
    PhaseAggregateOutcome<'id, P>,
>;
/// A receive Block Ack slot of the phase types `P`.
pub type PhaseBlockAckSlot<'id, P> =
    <<P as PhaseTypes<'id>>::BlockAckPool as crate::association::RxBlockAckPool>::Slot<
        'id,
        <P as PhaseTypes<'id>>::BlockAckPath,
    >;
/// The outcome of a single MPDU in the phase types `P`.
pub type PhaseSingleOutcome<'id, P> =
    crate::SingleOutcome<<P as PhaseTypes<'id>>::AckEvidence, <P as PhaseTypes<'id>>::AirDenied>;
/// The outcome of an aggregate in the phase types `P`.
pub type PhaseAggregateOutcome<'id, P> = crate::AggregateOutcome<
    <P as PhaseTypes<'id>>::BlockAckEvidence,
    <P as PhaseTypes<'id>>::AirDenied,
>;

/// The keys of one role and their slots, which a phase's types name: they
/// depend on the install alone, so a caller holds them across the phase's
/// lends.
pub trait KeyTypes<'id> {
    /// The installation pool of this role's association domain: its
    /// [`KeyPool`](crate::association::KeyPool) constants are the key
    /// capacity.
    type Pool: crate::association::KeyPool;
    /// The one affine permission to choose this association's SPP mode.
    type AssociationStart: 'id;
    /// Immutable association owner required by every PTK/GTK installation.
    type Association: 'id;
    /// Complete vacant bank, containing every slot of the association domain.
    type VacantKeyBank: 'id;
    /// Returned association permission together with its complete bank.
    type VacantAssociation: 'id;
    /// Pure selection; the backend applies it before publishing key material.
    fn begin_association(
        start: Self::AssociationStart,
        spp: oer_ieee80211_wire::security::Spp,
    ) -> Self::Association;
    /// A partial collection cannot cross the association-end boundary.
    fn collect_key_bank<const N: usize>(
        parts: crate::association::KeyBankParts<Self::PairwiseSlot, Self::GroupSlot, N>,
    ) -> Result<
        Self::VacantKeyBank,
        crate::association::KeyBankParts<Self::PairwiseSlot, Self::GroupSlot, N>,
    >;
    /// Pure, infallible end after all key-removal terminals. No new command or
    /// terminal exists; the returned association retains its complete bank.
    fn end_association(
        association: Self::Association,
        vacant: Self::VacantKeyBank,
    ) -> Self::VacantAssociation;
    /// Distribute the retained bank for the next exchange.
    fn split_vacant<const N: usize>(
        vacant: Self::VacantAssociation,
    ) -> crate::association::DistributedKeyBank<
        Self::AssociationStart,
        Self::PairwiseSlot,
        Self::GroupSlot,
        N,
    >;
    /// The role's affine pairwise key slots (an access point's: its
    /// association identifiers).
    type PairwiseSlot: crate::association::AssociationSlot<Pool = Self::Pool> + 'id;
    /// The role's affine group key slots, retained through removal.
    type GroupSlot: crate::association::AssociationSlot<Pool = Self::Pool> + 'id;
    /// An installed pairwise key, which holds its slot.
    type PairwiseKey: 'id;
    /// An installed group key, which holds its slot.
    type GroupKey: 'id;
    /// The installation mints exactly one affine mutation permission per
    /// interface. It has no public constructor, Copy or Clone implementation.
    /// Every group operation holds it until its terminal; RX needs no loan.
    type GroupPermission: 'id;
    /// Backend identity of one installation, distinct from its reusable slot.
    type Identity;
    /// Stable identity of a group slot within this installation, retained
    /// across replacement. It correlates the consumed key with its terminal.
    type GroupSlotIdentity: Copy + Eq;
    /// Retain the installed pairwise identity while preparing an operation.
    fn pairwise_identity(key: &Self::PairwiseKey) -> Self::Identity;
    /// Retain the installed group identity (transmit-capable on an AP only).
    fn group_identity(key: &Self::GroupKey) -> Self::Identity;
    /// The group key's slot identity, independent of its key installation.
    fn group_slot_identity(key: &Self::GroupKey) -> Self::GroupSlotIdentity;
    /// Correlate a pending installation before an installed owner exists.
    fn vacant_group_slot_identity(slot: &Self::GroupSlot) -> Self::GroupSlotIdentity;
}

/// A removed key's slot, of its kind.
pub enum RemovedKey<'id, K: KeyTypes<'id>> {
    /// A pairwise key's slot.
    Pairwise {
        /// Vacant physical key slot, after its last admitted use.
        slot: K::PairwiseSlot,
        /// Association owner retained through removal; the full bank is still required to end it.
        association: K::Association,
    },
    /// A group key's slot.
    Group {
        /// The now-vacant physical slot.
        slot: K::GroupSlot,
        /// Exclusive admission for the next GTK mutation.
        permission: K::GroupPermission,
    },
}

/// The successful installation and the permission for the next mutation.
/// Only the driver creates this proof after establishing the RX boundary.
pub enum InstalledGroup<'id, P: PhaseTypes<'id>> {
    /// The station's group installation.
    Station {
        /// The installed owner, returned exactly once.
        key: <P::StationKeys as KeyTypes<'id>>::GroupKey,
        /// Exclusive admission for the next GTK mutation.
        permission: <P::StationKeys as KeyTypes<'id>>::GroupPermission,
    },
    /// The access point's group installation.
    AccessPoint {
        /// The installed owner, returned exactly once.
        key: <P::AccessPointKeys as KeyTypes<'id>>::GroupKey,
        /// Exclusive admission for the next GTK mutation.
        permission: <P::AccessPointKeys as KeyTypes<'id>>::GroupPermission,
    },
}

/// A removed key's slot, of its role.
pub enum Removed<'id, P: PhaseTypes<'id>> {
    /// The station's.
    Station(RemovedKey<'id, P::StationKeys>),
    /// The access point's.
    AccessPoint(RemovedKey<'id, P::AccessPointKeys>),
}

/// The keys of a role in the phase types `P`.
pub trait RoleKeys<'id, P: PhaseTypes<'id>>: InterfaceRole + Sized {
    /// Installation component of this role's FIFO command permissions.
    type CommandPath: PoolPath;
    /// Preserve the request's interface when returning its sampling permission.
    fn sampled(
        permission: crate::TsfPermission<'id, Self, Self::CommandPath>,
    ) -> crate::SampledTsfPermission<'id, P>;
    /// The role's keys.
    type Keys: KeyTypes<'id, Identity = P::KeyIdentity>;
    /// Keys this role may use for TX. Station has only a pairwise key.
    type TxKey<'k>
    where
        'id: 'k;
    /// Retain an opaque backend identity, without inspecting key material.
    fn tx_identity<'k>(key: &Self::TxKey<'k>) -> P::KeyIdentity
    where
        'id: 'k;

    /// The removed key's slot, as the event names it.
    fn removed(key: RemovedKey<'id, Self::Keys>) -> Removed<'id, P>;
    /// Preserve role when publishing an actual group installation terminal.
    fn installed_group(
        key: <Self::Keys as KeyTypes<'id>>::GroupKey,
        permission: <Self::Keys as KeyTypes<'id>>::GroupPermission,
    ) -> InstalledGroup<'id, P>;
}

impl<'id, P: PhaseTypes<'id>> RoleKeys<'id, P> for Station {
    type CommandPath = P::StationCommandPath;
    fn sampled(
        permission: crate::TsfPermission<'id, Self, Self::CommandPath>,
    ) -> crate::SampledTsfPermission<'id, P> {
        crate::SampledTsfPermission::Station(permission)
    }
    type Keys = P::StationKeys;
    type TxKey<'k>
        = &'k <P::StationKeys as KeyTypes<'id>>::PairwiseKey
    where
        'id: 'k;
    fn tx_identity<'k>(key: &Self::TxKey<'k>) -> P::KeyIdentity
    where
        'id: 'k,
    {
        P::StationKeys::pairwise_identity(key)
    }

    fn removed(key: RemovedKey<'id, Self::Keys>) -> Removed<'id, P> {
        Removed::Station(key)
    }
    fn installed_group(
        key: <Self::Keys as KeyTypes<'id>>::GroupKey,
        permission: <Self::Keys as KeyTypes<'id>>::GroupPermission,
    ) -> InstalledGroup<'id, P> {
        InstalledGroup::Station { key, permission }
    }
}

impl<'id, P: PhaseTypes<'id>> RoleKeys<'id, P> for AccessPoint {
    type CommandPath = P::AccessPointCommandPath;
    fn sampled(
        permission: crate::TsfPermission<'id, Self, Self::CommandPath>,
    ) -> crate::SampledTsfPermission<'id, P> {
        crate::SampledTsfPermission::AccessPoint(permission)
    }
    type Keys = P::AccessPointKeys;
    type TxKey<'k>
        = AttemptKey<
        'k,
        <P::AccessPointKeys as KeyTypes<'id>>::PairwiseKey,
        <P::AccessPointKeys as KeyTypes<'id>>::GroupKey,
    >
    where
        'id: 'k;
    fn tx_identity<'k>(key: &Self::TxKey<'k>) -> P::KeyIdentity
    where
        'id: 'k,
    {
        match key {
            AttemptKey::Pairwise(key) => P::AccessPointKeys::pairwise_identity(key),
            AttemptKey::Group(key) => P::AccessPointKeys::group_identity(key),
        }
    }

    fn removed(key: RemovedKey<'id, Self::Keys>) -> Removed<'id, P> {
        Removed::AccessPoint(key)
    }
    fn installed_group(
        key: <Self::Keys as KeyTypes<'id>>::GroupKey,
        permission: <Self::Keys as KeyTypes<'id>>::GroupPermission,
    ) -> InstalledGroup<'id, P> {
        InstalledGroup::AccessPoint { key, permission }
    }
}

/// The keys of the interface `I`.
pub type InterfaceKeys<'p, 'id, I> = <<I as InterfacePhase<'p, 'id>>::Role as RoleKeys<
    'id,
    <I as InterfacePhase<'p, 'id>>::Types,
>>::Keys;

/// The types of one backend's phase that its events carry.
pub trait PhaseTypes<'id>: Sized {
    /// Maximum queued commands, derived in const evaluation from the mint's pools.
    const COMMAND_CAPACITY: usize;
    /// Pool component owning the station's configure and TSF permissions.
    type StationCommandPath: PoolPath;
    /// Pool component owning the access point's configure and TSF permissions.
    type AccessPointCommandPath: PoolPath;
    /// Opaque composition context moved with configuration and returned unchanged.
    type StationConfigureContext: 'id;
    /// Opaque context of an access-point configuration request.
    type AccessPointConfigureContext: 'id;
    /// Maximum unconsumed command terminals of the installation's pools.
    /// Includes TX tickets, pairwise removals, GTK permissions, gate and
    /// configuration/TSF requests; excludes unsolicited RX and TBTT events.
    const TERMINAL_CAPACITY: usize;
    /// Component owning the station connection epoch source.
    type EpochPath: PoolPath;
    /// Nominal authority of this backend's gate component.
    type GatePath: PoolPath;
    /// Nominal authority of this backend's transmit ticket component.
    type TicketPath: PoolPath;
    /// Nominal authority of this backend's receive Block Ack component.
    type BlockAckPath: PoolPath;
    /// The backend fault shared by the event stream and every interface.
    type Fault: Copy + core::fmt::Debug;
    /// The composition's operation context. The backend owns and returns
    /// it unchanged, without interpreting or cloning it. The install bound
    /// retains branded values while excluding references to machines local
    /// to the generic `PortBody::run`. Portable contexts need not be `Send`;
    /// a backend crossing execution contexts adds that bound itself.
    type Context: 'id;
    /// A prepared MPDU retaining its phase, install and intended interface role.
    type Prepared<'k, 'p, R, K: Ieee80211Capabilities>: PreparedTx<'id, Types = Self>;
    /// Opaque installed-key identity, retained by prepared and sent MPDUs.
    type KeyIdentity;
    /// Aggregate storage with a compile-time empty/nonempty state.
    type Aggregate<'k, 'p, R, K, S>: crate::PhaseAggregate<'id, Types = Self, State = S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: crate::AggregateState,
        'id: 'k;
    /// An external MPDU body. A `Sent` MPDU retains its body internally
    /// for a retry; renewing that MPDU returns the body with its memory.
    type TxBody: crate::TxBody;
    /// The kind of the transmit tickets.
    type TicketKind;
    /// What the backend observed of a single MPDU's solicited response (an
    /// ACK, or the BlockAck to a BlockAckReq): a policy that needs the
    /// response's SNR requires [`AckSnr`](crate::AckSnr) of it.
    type AckEvidence: Copy + core::fmt::Debug + Eq;
    /// What the backend observed of an aggregate's BlockAck: a policy that
    /// needs its SNR requires [`BlockAckSnr`](crate::BlockAckSnr) of it.
    type BlockAckEvidence: Copy + core::fmt::Debug + Eq;
    /// The backend's evidence that coexistence denied an attempt the air;
    /// [`core::convert::Infallible`] for a backend that cannot tell a
    /// coexistence abort from a failed exchange.
    type AirDenied: Copy + core::fmt::Debug + Eq;
    /// A transmit buffer of any class, as a completion gives it back.
    type AnyTxBuffer;
    /// One closed MPDU, with its fresh body or its complete Sent owner.
    type TxMpdu: crate::TxMpdu<Buffer = Self::AnyTxBuffer, Body = Self::TxBody>
        + crate::ExecutionPayload;
    /// The complete submitted aggregate. Its subframes retain their bodies;
    /// no independently indexed body collection exists.
    type TxAggregate: IntoIterator<Item = Self::TxMpdu> + crate::ExecutionPayload;
    /// The memory of one received MPDU, which goes back through
    /// [`LowerMacPhase::recycle`].
    type RxBuffer: crate::RxBuffer;
    /// The metadata of one received MPDU: the portable fields and what the
    /// backend's type guarantees beyond them.
    type RxMeta: crate::RxMetadata;
    /// The station's keys and their slots.
    type StationKeys: KeyTypes<'id, Identity = Self::KeyIdentity>;
    /// The access point's keys and their slots.
    type AccessPointKeys: KeyTypes<'id, Identity = Self::KeyIdentity>;
    /// The kind of the receive Block Ack slots: one pool the backend's
    /// hardware shares between its roles.
    type BlockAckKind;
    /// The install's pool of receive Block Ack slots, `BlockAckSlots<K, N>`:
    /// its slots name its size, so its
    /// [`RxBlockAckPool::LEN`](crate::association::RxBlockAckPool::LEN) is the
    /// capacity of the slots the backend actually mints. No agreement is
    /// accepted without a slot. Key capacity is likewise each role's
    /// [`KeyTypes::Pool`], which its slot types name.
    type BlockAckPool: crate::association::RxBlockAckPool<Kind = Self::BlockAckKind>;
    /// A running receive Block Ack agreement, which holds its slot.
    type BlockAck;
}

/// A running lower-MAC phase of the install `'id`, lent for `'p`.
pub trait LowerMacPhase<'p, 'id: 'p>:
    PortPhase<
        Event = LowerMacEvent<'id, Self::Types>,
        Fault = <Self::Types as PhaseTypes<'id>>::Fault,
    > + Sized
{
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
    /// The types the phase's events carry.
    type Types: PhaseTypes<'id>;
    /// The station interface of the phase.
    type Station<'v>: StationPhase<'p, 'id, Caps = Self::Caps, Types = Self::Types>
    where
        Self: 'v;
    /// The access-point interface of the phase.
    type AccessPoint<'v>: AccessPointPhase<'p, 'id, Caps = Self::Caps, Types = Self::Types>
    where
        Self: 'v;

    /// The channel the phase runs on.
    fn channel(&self) -> Channel;

    /// The rate on the phase's channel, when the capabilities send it
    /// there. Only this check mints a [`PhaseRate`].
    fn rate(&self, rate: PhyRate) -> Option<PhaseRate<'p, 'id, Self::Caps>> {
        if Self::Caps::CAPS.supports_rate(rate, self.channel()) {
            Some(PhaseRate::checked(rate))
        } else {
            None
        }
    }

    /// The rate, when the capabilities carry an individual frame without
    /// an ACK at it.
    ///
    /// # Errors
    ///
    /// The rate, when they do not.
    fn no_ack(
        &self,
        rate: PhaseRate<'p, 'id, Self::Caps>,
    ) -> Result<SupportedNoAck<'p, 'id, Self::Caps>, PhaseRate<'p, 'id, Self::Caps>> {
        if Self::Caps::CAPS.individual_no_ack.contains_rate(rate.get()) {
            Ok(SupportedNoAck(rate))
        } else {
            Err(rate)
        }
    }

    /// Return an owned receive buffer to the backend's receive pool. Keeping
    /// it occupies that capacity; dropping it does not recycle it. The host
    /// model poisons the installation when an unrecycled owner is dropped.
    fn recycle(&mut self, buffer: <Self::Types as PhaseTypes<'id>>::RxBuffer);

    /// Close publication in every queue. A current publication is waited for,
    /// at most the backend's PUBLICATION_BOUND, before GateClosed is emitted.
    /// ACK returns `Delivered`; unknown timeout returns Indeterminate.
    /// When a retry would be needed, withdraw the attempt with its Sent MPDU
    /// instead of starting that retry. Queued fresh MPDUs return Fresh.
    /// Terminals retain each AC's order and precede the barrier confirmation.
    fn close_gate(&mut self, gate: GateOpen<'id, <Self::Types as PhaseTypes<'id>>::GatePath>);

    /// Open the closed gate, a register write that cannot fail.
    fn open_gate(
        &mut self,
        gate: GateClosed<'id, <Self::Types as PhaseTypes<'id>>::GatePath>,
    ) -> GateOpen<'id, <Self::Types as PhaseTypes<'id>>::GatePath>;

    /// The station interface.
    fn station(&mut self) -> Self::Station<'_>;

    /// The access-point interface.
    fn access_point(&mut self) -> Self::AccessPoint<'_>;
}

/// One attempt awaiting preparation: its ticket, its MPDU in a
/// buffer, its context, body, protection, key, power and the operation it asks the air for.
/// The frame's kind sets its response and
/// its retry plan ([`RetryPlan`]) or one rate; its access category picks
/// its queue, whose contention the backend draws.
pub struct Attempt<'id, T, M, K, X, Path: PoolPath = Root> {
    /// The original operation's context, returned with its terminal.
    pub context: X,
    /// The attempt's place in flight.
    pub ticket: Slot<'id, T, Path>,
    /// Fresh memory with its checked description and body, or the original
    /// returned MPDU together with this attempt's phase-bound rates.
    pub source: M,
    /// The access category the attempt contends as.
    pub access_category: oer_ieee80211_wire::qos::WmmAccessCategory,
    /// The control exchange that protects its first transmission; a
    /// retry plan's stages name their own.
    pub protection: Protection,
    /// Its transmit power.
    pub power: SupportedPower<K>,
    /// What the attempt is, for the radio it shares the air with.
    pub coex_operation: TxOperation,
}

/// An operation sealed before it crosses into backend custody. Execution
/// metadata is separate; context, ticket and the complete MPDU remain together.
pub struct AdmittedAttempt<'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath = Root> {
    /// Indivisible owner which the backend executes and completes.
    pub operation: crate::InFlight<X, Slot<'id, T, Path>, <M::Types as PhaseTypes<'id>>::TxMpdu>,
    /// Backend execution metadata, without independently owned MPDUs.
    pub execution: M::Execution,
    /// Access category selected by the caller.
    pub access_category: oer_ieee80211_wire::qos::WmmAccessCategory,
    /// Protection selected by the caller.
    pub protection: Protection,
    /// Transmit power selected by the caller.
    pub power: SupportedPower<K>,
    /// Operation selected by the caller.
    pub coex_operation: TxOperation,
    proof: core::marker::PhantomData<fn(M) -> M>,
}

impl<'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath> Attempt<'id, T, M, K, X, Path> {
    /// Seal the whole prepared operation before passing it to the backend.
    pub fn admit(self) -> AdmittedAttempt<'id, T, M, K, X, Path> {
        let (payload, execution) = self.source.into_admission();
        AdmittedAttempt {
            operation: crate::InFlight::admit(self.context, self.ticket, payload),
            execution,
            access_category: self.access_category,
            protection: self.protection,
            power: self.power,
            coex_operation: self.coex_operation,
            proof: core::marker::PhantomData,
        }
    }
}

/// The installed key an attempt names.
pub enum AttemptKey<'k, P, G> {
    /// A pairwise key.
    Pairwise(&'k P),
    /// A group key.
    Group(&'k G),
}

/// The only source of the MPDU's Protected bit and installed TX key.
/// A station's `Key` is a borrowed pairwise key; it cannot supply a group key.
///
/// A station's receive group key is not a transmit key:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_port::*;
/// fn group<'k, 'id: 'k, T: PhaseTypes<'id>>(
///     key: &'k <T::StationKeys as KeyTypes<'id>>::GroupKey,
/// ) -> TxProtection<<Station as RoleKeys<'id, T>>::TxKey<'k>> {
///     TxProtection::Ccmp(key)
/// }
/// ```
pub enum TxProtection<Key> {
    /// No encryption; the backend clears Protected even in an encoded input header.
    Open,
    /// CCMP under this installation; the backend sets Protected.
    Ccmp(Key),
}

impl<Key> TxProtection<Key> {
    /// Derive the MAC/CCMP layout from this operation's key selection.
    pub(crate) const fn layout(&self) -> crate::MpduProtection {
        match self {
            Self::Open => crate::MpduProtection::Open,
            Self::Ccmp(_) => crate::MpduProtection::Ccmp,
        }
    }
}

/// The class-specific input to preparation; refusal returns this exact type.
pub type PoolSource<'k, 'p, 'id, T, C, K, R> = TxSource<
    <T as TxBufferPool<'id, C>>::Buffer,
    <T as TxBufferPool<'id, C>>::Sent,
    ClassFrame<'p, 'id, C, K>,
    <T as PhaseTypes<'id>>::TxBody,
    <R as RoleKeys<'id, T>>::TxKey<'k>,
    RetransmissionPlan<'p, 'id, K>,
>;

/// Class-specific source whose portable layout has already been checked.
pub type CheckedPoolSource<'k, 'p, 'id, T, C, K, R> = crate::CheckedTxSource<
    <T as TxBufferPool<'id, C>>::Buffer,
    <T as TxBufferPool<'id, C>>::Sent,
    ClassFrame<'p, 'id, C, K>,
    <T as PhaseTypes<'id>>::TxBody,
    <R as RoleKeys<'id, T>>::TxKey<'k>,
    RetransmissionPlan<'p, 'id, K>,
>;

/// Preparation returns a phase-bound MPDU or its unchanged class-specific source.
pub type PoolPreparation<'k, 'p, 'id, T, C, K, R> = Result<
    <T as PhaseTypes<'id>>::Prepared<'k, 'p, R, K>,
    PreparationRefused<PoolSource<'k, 'p, 'id, T, C, K, R>>,
>;

/// A class's fresh allocation or its retained published MPDU.
pub type PoolMpdu<'id, T, C> =
    ReturnedMpdu<<T as TxBufferPool<'id, C>>::Buffer, <T as TxBufferPool<'id, C>>::Sent>;

/// A backend's closed preparation proof. Cancellation recovers its owners
/// without creating a TX terminal; submission consumes the proof itself.
pub trait PreparedTx<'id> {
    /// The backend's resource family.
    type Types: PhaseTypes<'id>;
    /// Metadata needed by the backend to execute this MPDU.
    type Execution;
    /// Lower preparation into the complete MPDU and execution metadata. Called
    /// by `Attempt::admit`, before the backend receives any accepted owners.
    fn into_admission(self) -> (<Self::Types as PhaseTypes<'id>>::TxMpdu, Self::Execution);
    /// Recover the buffer and any external body. A `Sent` MPDU keeps its
    /// body nested until renewal.
    fn into_parts(
        self,
    ) -> (
        <Self::Types as PhaseTypes<'id>>::AnyTxBuffer,
        Option<<Self::Types as PhaseTypes<'id>>::TxBody>,
    );
}

/// The attempt of the interface `I`, after its class pool prepared the MPDU.
/// It remains bound to this phase, install, capabilities and role.
pub type InterfaceAttempt<'k, 'p, 'id, I> = Attempt<
    'id,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketKind,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::Prepared<
        'k,
        'p,
        <I as InterfacePhase<'p, 'id>>::Role,
        <I as InterfacePhase<'p, 'id>>::Caps,
    >,
    <I as InterfacePhase<'p, 'id>>::Caps,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::Context,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketPath,
>;

/// The sealed operation accepted by an interface backend.
pub type InterfaceSubmission<'k, 'p, 'id, I> = AdmittedAttempt<
    'id,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketKind,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::Prepared<
        'k,
        'p,
        <I as InterfacePhase<'p, 'id>>::Role,
        <I as InterfacePhase<'p, 'id>>::Caps,
    >,
    <I as InterfacePhase<'p, 'id>>::Caps,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::Context,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketPath,
>;

/// The MPDU supplied to a submission. Fresh memory travels with its
/// `ClassFrame` and body; a `Sent` MPDU already owns that description and
/// body, and receives only the new attempt's phase-bound rate plan.
pub enum TxSource<B, N, F, O, Key, Plan> {
    /// A new MPDU, numbered at its first protected transmission.
    Fresh {
        /// Caller-owned memory, writable only before admission.
        buffer: B,
        /// The checked, phase-bound frame description.
        frame: F,
        /// External octets at the end of the MPDU, retained until its return.
        body: Option<O>,
        /// The role's TX protection; the encoded Protected bit is derived from it.
        protection: TxProtection<Key>,
    },
    /// The same published MPDU again: keep its SN and PN, set Retry.
    Retransmission {
        /// The returned MPDU owns its description, payload and body.
        mpdu: N,
        /// Only the transmission parameters change, not the MPDU.
        plan: Plan,
    },
}

/// The phase-bound rates of a new attempt of a `Sent` MPDU. The backend
/// checks that its response kind matches the original MPDU's kind.
#[derive(Debug)]
pub enum RetransmissionPlan<'p, 'id, K> {
    /// An individually acknowledged MPDU.
    Individual(RetryPlan<PhaseRate<'p, 'id, K>>),
    /// An individually addressed MPDU with no ACK.
    IndividualNoAck(crate::SupportedNoAck<'p, 'id, K>),
    /// A group-addressed MPDU.
    Group(PhaseRate<'p, 'id, K>),
}

/// Preparation failed before admission; the original operation is returned
/// without allocating a PN or creating a TX terminal.
pub struct PreparationRefused<A> {
    /// The dynamic incompatibility.
    pub reason: PreparationError,
    /// Every resource and parameter of the rejected operation.
    pub operation: A,
}

/// Dynamic preparation failures, distinct from the outcome of accepted TX.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparationError {
    /// A retry names a different key installation, even in the same slot.
    RetransmissionKey,
    /// The new rate plan requires a different response kind.
    RetransmissionKind,
    /// The aggregate cannot take another subframe.
    AggregateFull,
    /// A subframe lies outside the compressed BA window starting at the
    /// aggregate's first sequence number. Neither source has been published.
    BlockAckWindow,
}

/// What one interface of a running phase does: its own frames, keys and
/// Block Ack agreements.
///
/// The body and fault belong to the phase's resource family, with no
/// independent interface-level declarations:
///
/// ```compile_fail,E0220
/// use oer_ieee80211_port::InterfacePhase;
/// fn body<'p, 'id: 'p, I: InterfacePhase<'p, 'id>>() -> Option<I::TxBody> { None }
/// ```
///
/// ```compile_fail,E0220
/// use oer_ieee80211_port::InterfacePhase;
/// fn fault<'p, 'id: 'p, I: InterfacePhase<'p, 'id>>() -> Option<I::Fault> { None }
/// ```
///
/// ```
/// use oer_ieee80211_port::{InterfaceAttempt, InterfacePhase, PhaseTypes, Poisoned};
/// fn body<'p, 'id: 'p, I: InterfacePhase<'p, 'id>>()
///     -> Option<<I::Types as PhaseTypes<'id>>::TxBody> { None }
/// fn submit<'k, 'p, 'id: 'k + 'p, I: InterfacePhase<'p, 'id>>(
///     interface: &mut I, attempt: InterfaceAttempt<'k, 'p, 'id, I>,
/// ) -> Result<(), Poisoned<<I::Types as PhaseTypes<'id>>::Fault>> {
///     interface.submit(attempt.admit())
/// }
/// ```
pub trait InterfacePhase<'p, 'id: 'p> {
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
    /// The types the phase's events carry.
    type Types: PhaseTypes<'id>;
    /// The interface's role, which names its keys among the phase's types.
    type Role: RoleKeys<'id, Self::Types>;
    /// The interface's own address, which the install gave it.
    fn address(&self) -> IndividualAddress;

    /// Accept the already sealed `attempt`. Its closed preparation proof
    /// retains phase, install and role; submission has no buffer-class
    /// parameter. A retransmission retains SN, PN and payload. An optional body remains
    /// owned by the attempt until its terminal, even if copied into DMA memory.
    /// The backend publishes the attempt once its queue is free; its
    /// context, ticket, buffers and bodies return together, exactly once.
    ///
    /// # Errors
    ///
    /// Only a poisoned backend refuses submission. Preparation already
    /// established the layout and key; fail-stop retains the operation until
    /// the installation ends, without inventing a terminal.
    fn submit<'k>(
        &mut self,
        attempt: InterfaceSubmission<'k, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<<Self::Types as PhaseTypes<'id>>::Fault>>;

    /// The EDCA parameters this interface's attempts contend with, from
    /// their next publication on. Each interface keeps its own set; the
    /// backend reconciles the sets of all interfaces into its queues and
    /// returns the set this interface's attempts contend with from then on.
    fn set_edca(&mut self, parameters: SupportedEdca<Self::Caps>) -> SupportedEdca<Self::Caps>;

    /// Install `key` for frames with `peer` in `slot`.
    fn install_pairwise_key(
        &mut self,
        association: &<InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::Association,
        slot: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::PairwiseKey;

    /// Enqueue a group installation, consuming its vacant slot and the
    /// interface's unique permission. The call reserves a FIFO terminal.
    /// Only `GroupKeyInstalled` returns an installed owner and permission.
    ///
    /// Exactly one installation per interface/association and wire KeyID
    /// participates in decryption; other IDs stay active. At the terminal,
    /// older installations of this ID remain retained but become inactive.
    /// All delivered old-installation RX, including delayed DMA/ISR facts,
    /// precedes it; selected-installation RX follows it. Traffic cannot delay
    /// completion indefinitely: `LowerMacBackend::GROUP_COMMAND_BOUND` covers
    /// admission through terminal or Poisoned, with a backend-owned wake.
    ///
    /// Hardware uncertainty retains every resource for teardown and returns
    /// no ordinary permission. Disable completes accepted commands within
    /// the same bound. This contract exposes no cancellation command: teardown
    /// retains any operation that cannot complete, never fabricating success.
    fn install_group_key(
        &mut self,
        association: &<InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::Association,
        permission: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupPermission,
        slot: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupSlot,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    );

    /// Remove `key`: queued attempts that use it return as
    /// `Withdrawn(KeyRemoved)`, a published one is waited for, and
    /// [`LowerMacTerminal::KeyRemoved`] gives its slot and association owner back.
    /// The owner is retained through queued, published and quarantined uses;
    /// receiving it alone cannot end the association without the full bank.
    fn remove_pairwise_key(
        &mut self,
        association: <<Self::Role as RoleKeys<'id, Self::Types>>::Keys as KeyTypes<'id>>::Association,
        key: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::PairwiseKey,
    );

    /// Remove `key`, as [`Self::remove_pairwise_key`].
    fn remove_group_key(
        &mut self,
        permission: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupPermission,
        key: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupKey,
    );

    /// Start acknowledging A-MPDUs of `agreement` in `slot`; the agreement
    /// holds the slot. A caller holds one agreement per peer and TID: it
    /// replaces one through [`Self::replace_block_ack`]. This writes the
    /// slot's desired state; add, replace and remove never enqueue commands.
    /// The latest value is applied before the driver's next command.
    #[must_use = "an agreement holds its slot; dropping it loses the slot until the next install"]
    fn add_block_ack(
        &mut self,
        slot: PhaseBlockAckSlot<'id, Self::Types>,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> <Self::Types as PhaseTypes<'id>>::BlockAck;

    /// Replace `running` with `agreement` of the same peer and TID, in its
    /// slot.
    #[must_use = "an agreement holds its slot; dropping it loses the slot until the next install"]
    fn replace_block_ack(
        &mut self,
        running: <Self::Types as PhaseTypes<'id>>::BlockAck,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> <Self::Types as PhaseTypes<'id>>::BlockAck;

    /// Clear the desired state of `agreement`, giving its slot back. Reuse
    /// before the driver runs replaces the pending removal with the new
    /// agreement; no intermediate hardware installation is promised.
    #[must_use = "an ended agreement's slot is capacity; dropping it loses it until the next install"]
    fn remove_block_ack(
        &mut self,
        agreement: <Self::Types as PhaseTypes<'id>>::BlockAck,
    ) -> PhaseBlockAckSlot<'id, Self::Types>;
}

/// The station interface of a running phase.
pub trait StationPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id, Role = Station> {
    /// Consume an installed GTK and enqueue its replacement in command order.
    /// Its unique interface permission reserves one terminal place at this
    /// call. No removal or competing mutation is possible while the operation
    /// holds that permission and the installed owner. The same finite RX cut
    /// and GROUP_COMMAND_BOUND as ordinary group installation apply.
    ///
    /// [`LowerMacTerminal::GroupKeyReplaced`] returns the new installed key with
    /// the same slot. Its position in the event stream is the installation
    /// boundary for RX. A hardware write failure poisons the backend; any
    /// internal hardware rollback is not a successful port terminal.
    fn replace_group_key(
        &mut self,
        permission: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupPermission,
        installed: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupKey,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    );

    /// Select this concrete retained installation. No material write, new
    /// installation or counter reset occurs, including when already current.
    /// The same RX boundary and completion bound as installation apply.
    /// The owner and permission return exactly once in `GroupKeySelected`.
    /// An evicted hardware entry is not an installed owner and cannot be
    /// restored by this operation, regardless of the backend's slot count.
    fn select_group_key(
        &mut self,
        permission: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupPermission,
        installed: <InterfaceKeys<'p, 'id, Self> as KeyTypes<'id>>::GroupKey,
    );

    /// Set what the station receives once none of its frames is in flight:
    /// a frame the closed gate holds ends as aborted at once, a published
    /// one is waited for, and [`LowerMacTerminal::Configured`] returns the same
    /// permission and context. The install mints one permission per interface;
    /// another request cannot exist before its terminal returns that owner.
    fn configure(
        &mut self,
        permission: crate::ConfigurePermission<
            'id,
            Station,
            <Self::Types as PhaseTypes<'id>>::StationCommandPath,
            <Self::Types as PhaseTypes<'id>>::StationConfigureContext,
        >,
        receive: StationReceive<Self::Caps>,
    );

    /// The BSS color of the station's HE BSS.
    fn set_bss_color(&mut self, color: BssColor);
}

/// The access-point interface of a running phase, whose BSSID is its
/// address.
pub trait AccessPointPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id, Role = AccessPoint> {
    /// Set the access point's receive rules, as
    /// [`StationPhase::configure`].
    fn configure(
        &mut self,
        permission: crate::ConfigurePermission<
            'id,
            AccessPoint,
            <Self::Types as PhaseTypes<'id>>::AccessPointCommandPath,
            <Self::Types as PhaseTypes<'id>>::AccessPointConfigureContext,
        >,
        receive: SupportedFilter<Self::Caps, AccessPoint>,
    );
}

/// A class of transmit buffers: memory of one size, which the composition
/// sets.
pub trait TxBufferClass {
    /// The longest MPDU a buffer of the class holds, without its FCS.
    const CAPACITY: usize;
}

/// The phase types of a backend that keeps a pool of the buffer class `C`.
pub trait TxBufferPool<'id, C: TxBufferClass>: PhaseTypes<'id> {
    /// A buffer of the class: its memory, which the body holds and writes
    /// while it is not submitted. The backend owns its type and mints it
    /// only in the install's resources.
    type Buffer: TxBufferMemory;

    /// Prepare a source while its buffer class is concrete. The resulting
    /// closed value retains phase, install and role until `submit` or an
    /// aggregate consumes it. A refusal returns the original typed source.
    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: CheckedPoolSource<'k, 'p, 'id, Self, C, K, R>,
    ) -> PoolPreparation<'k, 'p, 'id, Self, C, K, R>
    where
        'id: 'k;

    /// Prepare and append through this builder's receiver, TID and protection.
    /// Fresh sources have no protection field. Refusal returns both the entire
    /// builder and the unchanged class-specific source, before admission or PN.
    fn push_aggregate<'k, 'p, R, K, S>(
        operation: crate::CheckedPoolAggregate<'k, 'p, 'id, Self, C, R, K, S>,
    ) -> crate::AggregatePushResult<'k, 'p, 'id, Self, C, R, K, S>
    where
        R: RoleKeys<'id, Self>,
        K: Ieee80211Capabilities,
        S: crate::AggregateState,
        'id: 'k + 'p;

    /// The buffer, as any class's.
    fn erase(buffer: Self::Buffer) -> Self::AnyTxBuffer;

    /// A published MPDU, protected or open. It owns the immutable encoded
    /// header, payload, checked description and external body. A protected
    /// MPDU also retains its PN and original key-install identity.
    /// A withdrawal returns Fresh only if never published; otherwise Sent.
    /// Retransmission preserves SN, PN and payload and sets Retry.
    /// Only renewal releases the old MPDU for a different frame.
    type Sent;

    /// The buffer, as any class's.
    fn erase_sent(buffer: Self::Sent) -> Self::AnyTxBuffer;

    /// The fresh buffer of the class `C` a completion gave back.
    ///
    /// # Errors
    ///
    /// The buffer, when it is of another class or `Sent`.
    fn class(buffer: Self::AnyTxBuffer) -> Result<Self::Buffer, Self::AnyTxBuffer>;

    /// The `Sent` buffer of the class `C` a completion gave back.
    ///
    /// # Errors
    ///
    /// The buffer, when it is of another class or fresh.
    fn sent(buffer: Self::AnyTxBuffer) -> Result<Self::Sent, Self::AnyTxBuffer>;

    /// The memory of a `Sent` buffer for a new MPDU: its packet number is
    /// forgotten, and the next protected attempt in it is numbered anew.
    /// The original external body is returned explicitly beside the memory.
    fn renew(buffer: Self::Sent) -> (Self::Buffer, Option<Self::TxBody>);
}

/// A returned MPDU retains whether it has ever been published, independently
/// of encryption. `Sent` owns the immutable MPDU, its description and body.
/// Only the owning machine decides to renew it for a different MPDU.
#[derive(Debug)]
pub enum ReturnedMpdu<B, S> {
    /// Original writable allocation; this MPDU was never published.
    Fresh(B),
    /// Published MPDU, including an unprotected one.
    Sent(S),
}

impl<B, S> ReturnedMpdu<B, S> {
    /// Release the old MPDU through its owner's renewal policy. A fresh
    /// allocation needs no renewal. The operation consumes either owner once.
    pub fn into_fresh(self, renew: impl FnOnce(S) -> B) -> B {
        match self {
            Self::Fresh(buffer) => buffer,
            Self::Sent(mpdu) => renew(mpdu),
        }
    }
}

/// The memory of a transmit buffer the body holds: the caller encodes a
/// frame into it, from its header on, before it hands the buffer in.
///
/// A backend's buffer token keeps four rules:
///
/// - it opens only the frame's octets, never the DMA descriptor;
/// - it is written only while the body holds it, and a submission takes
///   it by value;
/// - when its memory is cacheable, the backend makes it coherent at the
///   submission, before the DMA reads it;
/// - it is a handle on pinned memory: moving the token never moves the
///   buffer.
pub trait TxBufferMemory {
    /// The buffer's octets, as long as its class holds.
    fn bytes_mut(&mut self) -> &mut [u8];
}

/// A frame that a buffer of the class `C` fits; only [`ClassFrame::new`]
/// makes one.
pub struct ClassFrame<'p, 'id, C, K> {
    frame: TxFrame<'p, 'id, K>,
    class: core::marker::PhantomData<C>,
}

impl<'p, 'id, C: TxBufferClass, K: Ieee80211Capabilities> ClassFrame<'p, 'id, C, K> {
    /// `frame`, when a buffer of the class `C` fits it.
    ///
    /// # Errors
    ///
    /// The frame, when it is longer than the class holds.
    pub fn new(frame: TxFrame<'p, 'id, K>) -> Result<Self, TxFrame<'p, 'id, K>> {
        if frame.mpdu_len() <= C::CAPACITY {
            Ok(Self {
                frame,
                class: core::marker::PhantomData,
            })
        } else {
            Err(frame)
        }
    }

    /// The frame.
    pub fn into_frame(self) -> TxFrame<'p, 'id, K> {
        self.frame
    }

    /// Borrow the checked description for preparation without consuming
    /// the original operation, so a refusal can return it intact.
    pub const fn frame(&self) -> &TxFrame<'p, 'id, K> {
        &self.frame
    }
}

/// A phase-independent checked description that fits its buffer class.
/// Portable machines retain it in the operation context. Adding the phase's
/// rates cannot lose or re-check the established size proof.
#[derive(Debug)]
pub struct ClassSizedFrame<C, H, K> {
    frame: crate::SizedFrame<H, K>,
    class: core::marker::PhantomData<C>,
}
impl<C, H: Copy, K> Copy for ClassSizedFrame<C, H, K> {}
impl<C, H: Copy, K> Clone for ClassSizedFrame<C, H, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<C: TxBufferClass, H: crate::FrameHeader, K: Ieee80211Capabilities> ClassSizedFrame<C, H, K> {
    /// Establish the class-size proof while constructing the operation.
    pub fn new(frame: crate::SizedFrame<H, K>) -> Result<Self, crate::SizedFrame<H, K>> {
        if frame.len().get() <= C::CAPACITY {
            Ok(Self {
                frame,
                class: core::marker::PhantomData,
            })
        } else {
            Err(frame)
        }
    }
    /// The already checked description.
    pub const fn frame(&self) -> &crate::SizedFrame<H, K> {
        &self.frame
    }
}
impl<'p, 'id, C, K> ClassFrame<'p, 'id, C, K> {
    /// Add only the phase-bound retry plan to an individually acknowledged MPDU.
    pub fn individual(
        frame: ClassSizedFrame<C, crate::AckHeader, K>,
        plan: RetryPlan<PhaseRate<'p, 'id, K>>,
    ) -> Self {
        Self {
            frame: TxFrame::Individual {
                frame: frame.frame,
                plan,
            },
            class: core::marker::PhantomData,
        }
    }
    /// Add only the phase-bound rate to a group MPDU.
    pub fn group(
        frame: ClassSizedFrame<C, crate::HeaderSpec<crate::GroupAddress>, K>,
        rate: PhaseRate<'p, 'id, K>,
    ) -> Self {
        Self {
            frame: TxFrame::Group {
                frame: frame.frame,
                rate,
            },
            class: core::marker::PhantomData,
        }
    }
}

/// The stages a retry plan holds at most.
pub const RETRY_STAGES: usize = 4;

/// One stage of a retry plan: a rate, the transmissions at it and their
/// protection.
#[derive(Debug)]
pub struct RetryStage<R> {
    /// The rate of the stage's transmissions.
    pub rate: R,
    /// The transmissions at the rate.
    pub transmissions: core::num::NonZeroU8,
    /// The control exchange that protects each of them.
    pub protection: Protection,
}

/// The retry plan of an individual frame: its stages in order, as the
/// rate table mac80211 hands its drivers.
/// The backend transmits the head of its queue by the plan before the next
/// attempt of that queue, keeps the frame's sequence number and sets the
/// Retry bit of each retransmission; an exhausted plan ends the attempt
/// unacknowledged. A backend whose hardware retries carries the plan out
/// there.
#[derive(Debug)]
pub struct RetryPlan<R> {
    /// The first stage: a plan always has one.
    first: RetryStage<R>,
    /// The stages after the first, in order. The occupied cells form a
    /// prefix, which every operation keeps; the array's length is the bound.
    rest: [Option<RetryStage<R>>; RETRY_STAGES - 1],
}

impl<R> RetryPlan<R> {
    /// The plan of `first` alone.
    pub fn new(first: RetryStage<R>) -> Self {
        Self {
            first,
            rest: core::array::from_fn(|_| None),
        }
    }

    /// The plan with `stage` after its last.
    ///
    /// # Errors
    ///
    /// The plan and the stage, when the plan holds [`RETRY_STAGES`].
    pub fn then(mut self, stage: RetryStage<R>) -> Result<Self, (Self, RetryStage<R>)> {
        let unplaced = match self.rest.iter_mut().find(|cell| cell.is_none()) {
            Some(cell) => {
                *cell = Some(stage);
                None
            }
            None => Some(stage),
        };
        match unplaced {
            None => Ok(self),
            Some(stage) => Err((self, stage)),
        }
    }

    /// The first stage.
    pub const fn first(&self) -> &RetryStage<R> {
        &self.first
    }

    /// The first stage, by value: a frame of one rate takes it.
    pub fn into_first(self) -> RetryStage<R> {
        self.first
    }

    /// The stages in order.
    pub fn stages(&self) -> impl Iterator<Item = &RetryStage<R>> {
        core::iter::once(&self.first).chain(self.rest.iter().flatten())
    }

    /// The transmissions the plan makes at most.
    pub fn transmissions(&self) -> u16 {
        self.stages()
            .map(|stage| u16::from(stage.transmissions.get()))
            .sum()
    }

    /// The plan's first `transmissions` transmissions: its stages in order,
    /// the last one cut and the ones after it dropped.
    ///
    /// ```
    /// # use core::num::NonZeroU8;
    /// # use oer_ieee80211_port::{Protection, RetryPlan, RetryStage};
    /// let stage = |rate, transmissions| RetryStage {
    ///     rate,
    ///     transmissions: NonZeroU8::new(transmissions).unwrap(),
    ///     protection: Protection::None,
    /// };
    /// let plan = RetryPlan::new(stage('a', 2)).then(stage('b', 3)).ok().unwrap();
    /// let plan = plan.then(stage('c', 25)).ok().unwrap();
    /// let cut = plan.limit(NonZeroU8::new(4).unwrap());
    /// let left: Vec<_> = cut.stages().map(|s| (s.rate, s.transmissions.get())).collect();
    /// assert_eq!(left, [('a', 2), ('b', 2)]);
    /// assert_eq!(cut.transmissions(), 4);
    /// // Each stage's rate turns; a refused one stops the plan.
    /// let numbers = cut.try_map(|rate| Ok::<_, char>(rate as u8)).unwrap();
    /// assert_eq!(numbers.stages().next().unwrap().rate, b'a');
    /// let refused = numbers.try_map(|rate| if rate == b'b' { Err(rate) } else { Ok(rate) });
    /// assert_eq!(refused.err(), Some(b'b'));
    /// ```
    pub fn limit(mut self, transmissions: core::num::NonZeroU8) -> Self {
        let first = self.first.transmissions.min(transmissions);
        // The budget left after the first stage; it only shrinks, so once a
        // stage is dropped every later one is too: the occupied cells stay a
        // prefix.
        // `first` is at most `transmissions`, so this is their difference.
        let mut left = transmissions.get().abs_diff(first.get());
        self.first.transmissions = first;
        for cell in &mut self.rest {
            let taken = cell
                .as_ref()
                .map_or(0, |stage| stage.transmissions.get().min(left));
            // `taken` is at most `left`.
            left = left.abs_diff(taken);
            match (cell.as_mut(), core::num::NonZeroU8::new(taken)) {
                (Some(stage), Some(taken)) => stage.transmissions = taken,
                (_, None) => *cell = None,
                (None, Some(_)) => {}
            }
        }
        self
    }

    /// Map each rate without changing the nonempty plan or its budget.
    pub fn map<S>(&self, mut rate: impl FnMut(&R) -> S) -> RetryPlan<S> {
        let mut stage = |stage: &RetryStage<R>| RetryStage {
            rate: rate(&stage.rate),
            transmissions: stage.transmissions,
            protection: stage.protection,
        };
        RetryPlan {
            first: stage(&self.first),
            rest: self
                .rest
                .each_ref()
                .map(|cell| cell.as_ref().map(&mut stage)),
        }
    }

    /// Keep the stages whose rates `rate` accepts, in their original order.
    /// Each retained stage keeps its exact transmission count and protection;
    /// `None` means every stage was refused.
    pub fn filter_map<S>(self, mut rate: impl FnMut(R) -> Option<S>) -> Option<RetryPlan<S>> {
        let mut accepted = core::iter::once(self.first)
            .chain(self.rest.into_iter().flatten())
            .filter_map(|stage| {
                Some(RetryStage {
                    rate: rate(stage.rate)?,
                    transmissions: stage.transmissions,
                    protection: stage.protection,
                })
            });
        let first = accepted.next()?;
        // Taking the first accepted stage leaves at most RETRY_STAGES - 1 of
        // the input's stages, one per cell: the zip places every one.
        let mut rest: [Option<RetryStage<S>>; RETRY_STAGES - 1] = core::array::from_fn(|_| None);
        for (cell, stage) in rest.iter_mut().zip(accepted) {
            *cell = Some(stage);
        }
        Some(RetryPlan { first, rest })
    }

    /// The plan with each stage's rate turned by `rate`, the stages and
    /// their transmissions kept.
    ///
    /// # Errors
    ///
    /// The first error `rate` returned.
    pub fn try_map<S, E>(self, mut rate: impl FnMut(R) -> Result<S, E>) -> Result<RetryPlan<S>, E> {
        let mut turn = |stage: RetryStage<R>| {
            Ok(RetryStage {
                rate: rate(stage.rate)?,
                transmissions: stage.transmissions,
                protection: stage.protection,
            })
        };
        let first = turn(self.first)?;
        let mut rest: [Option<RetryStage<S>>; RETRY_STAGES - 1] = core::array::from_fn(|_| None);
        for (cell, stage) in rest.iter_mut().zip(self.rest) {
            if let Some(stage) = stage {
                *cell = Some(turn(stage)?);
            }
        }
        Ok(RetryPlan { first, rest })
    }
}

#[cfg(test)]
mod tests;

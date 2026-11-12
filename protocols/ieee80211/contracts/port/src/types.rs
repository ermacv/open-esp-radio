//! The types a data port's owners and terminals carry ([`PortTypes`]),
//! and the values that move through it: tokens, attempts, keys and
//! terminals.
//!
//! # Tokens
//!
//! Every token carries the install's brand `'id`, is minted once by the
//! install's resources and travels by value: an operation takes it, and
//! the terminal that ends the operation gives it back. So a portable core
//! holds the tokens of what it may do, and what it holds is what it may
//! do: a transmission it has no ticket for, a key it has no slot for or a
//! gate it has not closed does not compile.
//!
//! | Token | Taken by | Given back by |
//! | --- | --- | --- |
//! | a transmit ticket ([`PortTypes::TicketKind`]) | [`DataPort::submit`](crate::DataPort::submit) | [`LowerMacTerminal::TxCompleted`] |
//! | a transmit buffer of a class ([`TxBufferClass`]), its memory ([`TxBufferMemory`]) | [`DataPort::submit`](crate::DataPort::submit), together with its [`ClassFrame`] | [`LowerMacTerminal::TxCompleted`], every buffer of the attempt |
//! | a key slot of a role's kind | [`PortCommands`](crate::PortCommands)' installations (the installed key, returned by its installation terminal, holds it) | [`LowerMacTerminal::KeyRemoved`], after a removal |
//! | one GTK mutation permission per interface ([`KeyTypes::GroupPermission`]) | install/replace/select/remove GTK | the corresponding FIFO success terminal |
//! | one configure permission per interface ([`crate::ConfigurePermission`]) | [`PortCommand::Configure`](crate::PortCommand::Configure) | [`LowerMacTerminal::Configured`], with the same permission and opaque context |
//! | a Block Ack slot | [`DataPort::add_block_ack`](crate::DataPort::add_block_ack) (the agreement holds it) | [`DataPort::remove_block_ack`](crate::DataPort::remove_block_ack) |
//! | the open gate ([`GateOpen`]) | [`PortCommand::CloseGate`](crate::PortCommand::CloseGate) | [`DataPort::open_gate`](crate::DataPort::open_gate), which takes the closed gate ([`GateClosed`]) that [`LowerMacTerminal::GateClosed`] gave |
//! | a received frame's buffer | [`DataPort::receive`](crate::DataPort::receive) | [`DataPort::recycle`](crate::DataPort::recycle) |
//!
//! A ticket and a buffer are different tokens.
//! A ticket is a place in flight and a place for its terminal; their
//! number is bounded by the hardware. A buffer is memory of one class,
//! sized by the composition, from a pool the caller holds: the caller
//! encodes its frame straight into it. A submission takes one ticket and
//! the buffer of its frame, and its terminal gives both back. So a
//! submission never waits: an empty pool is the caller's own capacity,
//! and what the hardware cannot take at once (an occupied queue) the
//! backend keeps until it can. Queue permits are the backend's own and
//! never reach a caller. How many tickets and buffers each role holds,
//! with a reserve for its management frames, is the composition's
//! choice: the tokens are values it hands out.
//!
//! # Protected frames
//!
//! Preparation derives the Protected bit from [`TxProtection`] and
//! leaves the 8 octets of its CCMP header after its MAC header; its length
//! counts them, not the MIC. At the first transmission of the MPDU the
//! backend writes the CCMP header with the caller-allocated packet number into
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
//! use oer_protocols_ieee80211_contracts_port::GatePlace;
//! ```
//!
//! Nor copies one:
//!
//! ```compile_fail,E0382
//! # use oer_protocols_ieee80211_contracts_port::GateOpen;
//! fn twice<'id>(gate: GateOpen<'id>) -> (GateOpen<'id>, GateOpen<'id>) {
//!     (gate, gate)
//! }
//! ```
//!
//! ```compile_fail,E0599
//! # use oer_protocols_radio_port::Slot;
//! fn copy<'id, K>(ticket: &Slot<'id, K>) -> Slot<'id, K> {
//!     ticket.clone()
//! }
//! ```
//!
//! Its compiling twin moves the token on, as an operation takes it:
//!
//! ```
//! # use oer_protocols_ieee80211_contracts_port::{DataPort, GateClosed, GateOpen, PortTypes, PortCommand};
//! fn close<'id, P: DataPort<'id>>(
//!     gate: GateOpen<'id, <P::Types as PortTypes<'id>>::GatePath>,
//! ) -> PortCommand<'id, P::Types, P::Caps> {
//!     PortCommand::CloseGate(gate)
//! }
//! fn open<'id, P: DataPort<'id>>(port: &mut P, gate: GateClosed<'id, <P::Types as PortTypes<'id>>::GatePath>) -> GateOpen<'id, <P::Types as PortTypes<'id>>::GatePath> {
//!     port.open_gate(gate)
//! }
//! ```
//!
//! The open gate does not open a gate:
//!
//! ```compile_fail,E0308
//! # use oer_protocols_ieee80211_contracts_port::{DataPort, GateOpen, PortTypes};
//! fn reopen<'id, P: DataPort<'id>>(port: &mut P, gate: GateOpen<'id, <P::Types as PortTypes<'id>>::GatePath>) {
//!     let _ = port.open_gate(gate);
//! }
//! ```
//!
//! A frame a buffer's class does not fit has no [`ClassFrame`], whose
//! fields no caller sets:
//!
//! ```compile_fail,E0451
//! # use oer_protocols_ieee80211_contracts_port::{ClassFrame, TxFrame};
//! fn forge<'t, C, K>(frame: TxFrame<'t, K>) -> ClassFrame<'t, C, K> {
//!     ClassFrame { frame, class: core::marker::PhantomData }
//! }
//! ```
//!
//! Its compiling twin checks the frame against the class:
//!
//! ```
//! # use oer_protocols_ieee80211_contracts_port::{ClassFrame, Ieee80211Capabilities, TxBufferClass, TxFrame};
//! fn fit<'t, C: TxBufferClass, K: Ieee80211Capabilities>(
//!     frame: TxFrame<'t, K>,
//! ) -> Result<ClassFrame<'t, C, K>, TxFrame<'t, K>> {
//!     ClassFrame::new(frame)
//! }
//! ```
//!
//! An attempt seals its context, ticket and buffer once:
//!
//! ```compile_fail,E0382
//! # use oer_protocols_ieee80211_contracts_port::*;
//! # use oer_protocols_radio_port::PoolPath;
//! fn twice<'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath>(attempt: Attempt<'id, T, M, K, X, Path>)
//! where M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X> {
//!     let _ = attempt.admit();
//!     let _ = attempt.admit();
//! }
//! ```
//!
//! Its compiling twin seals each attempt once, with no `Clone` bound:
//!
//! ```
//! # use oer_protocols_ieee80211_contracts_port::*;
//! # use oer_protocols_radio_port::PoolPath;
//! fn one<'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath>(attempt: Attempt<'id, T, M, K, X, Path>)
//! where M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X> {
//!     let _ = attempt.admit();
//! }
//! ```

use oer_protocols_radio_port::{PoolPath, Root, Slot};

use crate::{
    Protection, TxOperation,
    owner::{
        AccessPoint, Ieee80211Capabilities, InterfaceRole, PortRate, Station, SupportedPower,
        TxFrame,
    },
};

enum GateKind {}

/// The gate component of a lower-MAC backend's pool set.
///
/// The backend receives its open gate and issuer together when its one
/// [`oer_protocols_radio_port::Mint`] is consumed. The unique place remains private.
///
/// ```compile_fail,E0432
/// use oer_protocols_ieee80211_contracts_port::GatePlace;
/// ```
///
/// ```
/// use oer_protocols_ieee80211_contracts_port::{GateIssuer, GateOpen, GatePool};
/// use oer_protocols_radio_port::Mint;
/// fn gate<'id>(mint: Mint<'id, GatePool>) -> (GateOpen<'id>, GateIssuer<'id>) {
///     mint.into_pools()
/// }
/// ```
pub struct GatePool;

impl oer_protocols_radio_port::PoolSet for GatePool {
    type Pools<'id, Path: PoolPath> = (GateOpen<'id, Path>, GateIssuer<'id, Path>);

    fn build<'id, Path: PoolPath>(
        builder: oer_protocols_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        GatePlace::mint(builder.into_pools::<oer_protocols_radio_port::Slots<GateKind, 1>>())
    }
}

/// The install's unique gate place, held inside exactly one gate state.
#[must_use = "the unique gate place stays in one gate state"]
struct GatePlace<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    slot: Slot<'id, oer_protocols_radio_port::Slots<GateKind, 1>, Path>,
}

impl<'id, Path: PoolPath> GatePlace<'id, Path> {
    fn mint(
        [slot]: [Slot<'id, oer_protocols_radio_port::Slots<GateKind, 1>, Path>; 1],
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
pub struct GateOpen<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    place: GatePlace<'id, Path>,
}

/// Evidence that the backend publishes no attempt; it does not prove quiet air.
/// Only the retained backend issuer constructs this state after its barrier.
///
/// ```compile_fail,E0616
/// # use oer_protocols_ieee80211_contracts_port::{GateClosed, GateOpen};
/// fn forge<'id>(gate: GateOpen<'id>) -> GateClosed<'id> {
///     GateClosed { place: gate.place }
/// }
/// ```
#[derive(Debug)]
#[must_use = "open_gate consumes the closed evidence and returns the open gate"]
pub struct GateClosed<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    place: GatePlace<'id, Path>,
}

/// The backend's authority to convert its one gate place between states.
///
/// One issuer is retained by the backend per install and never reaches the
/// body. Closing is allowed only after the current publication's result and
/// the withdrawal of queued attempts. Opening consumes that very evidence.
/// Backend endpoints may share this stateless authority immutably: each
/// transition still consumes the installation's one affine gate place.
///
/// ```compile_fail,E0382
/// # use oer_protocols_ieee80211_contracts_port::{GateClosed, GateIssuer, GateOpen};
/// fn both<'id>(issuer: &mut GateIssuer<'id>, gate: GateOpen<'id>)
///     -> (GateOpen<'id>, GateClosed<'id>)
/// {
///     let closed = issuer.close(gate);
///     (gate, closed)
/// }
/// ```
#[must_use = "the backend retains the issuer for the whole install"]
pub struct GateIssuer<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    brand: core::marker::PhantomData<fn(&'id (), Path) -> &'id ()>,
}

impl<'id, Path: PoolPath> GateIssuer<'id, Path> {
    /// Attest the completed hardware barrier, retaining the same place.
    pub fn close(&self, gate: GateOpen<'id, Path>) -> GateClosed<'id, Path> {
        GateClosed { place: gate.place }
    }

    /// Consume the closed evidence to open the same gate.
    pub fn open(&self, gate: GateClosed<'id, Path>) -> GateOpen<'id, Path> {
        GateOpen { place: gate.place }
    }
}

impl<Path: PoolPath> GateOpen<'static, Path> {
    /// Move the same gate state across its backend's private body boundary.
    pub fn lend<'p, S>(
        self,
        loan: &oer_protocols_radio_port::BodyLoan<'p, Path::Identity, S>,
    ) -> GateOpen<'p, Path> {
        GateOpen {
            place: GatePlace {
                slot: loan.lend_slot(self.place.slot),
            },
        }
    }
}

impl<'p, Path: PoolPath> GateOpen<'p, Path> {
    /// Move the same gate state across its backend's private body boundary.
    pub fn into_owned<S>(
        self,
        loan: &oer_protocols_radio_port::BodyLoan<'p, Path::Identity, S>,
    ) -> GateOpen<'static, Path> {
        GateOpen {
            place: GatePlace {
                slot: loan.reclaim_slot(self.place.slot),
            },
        }
    }
}

impl<Path: PoolPath> GateClosed<'static, Path> {
    /// Move the same gate state across its backend's private body boundary.
    pub fn lend<'p, S>(
        self,
        loan: &oer_protocols_radio_port::BodyLoan<'p, Path::Identity, S>,
    ) -> GateClosed<'p, Path> {
        GateClosed {
            place: GatePlace {
                slot: loan.lend_slot(self.place.slot),
            },
        }
    }
}

impl<'p, Path: PoolPath> GateClosed<'p, Path> {
    /// Move the same gate state across its backend's private body boundary.
    pub fn into_owned<S>(
        self,
        loan: &oer_protocols_radio_port::BodyLoan<'p, Path::Identity, S>,
    ) -> GateClosed<'static, Path> {
        GateClosed {
            place: GatePlace {
                slot: loan.reclaim_slot(self.place.slot),
            },
        }
    }
}

/// A terminal: the end of one operation, carrying every owner it took.
/// Received units and TBTTs are not terminals.
pub enum LowerMacTerminal<'id, P: PortTypes<'id>> {
    /// The terminal preserves the submitted shape and all of its owners.
    TxCompleted(TxCompleted<'id, P>),
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
    /// A pairwise installation is effective for TX and RX; the installed
    /// owner is returned exactly once.
    PairwiseKeyInstalled(InstalledPairwise<'id, P>),
    /// A retained installation is selected without writing material or
    /// resetting counters. It is the sole RX-active installation of its ID.
    GroupKeySelected {
        /// The installed owner, returned exactly once.
        key: <P::StationKeys as KeyTypes<'id>>::GroupKey,
        /// Exclusive admission for the next GTK mutation.
        permission: <P::StationKeys as KeyTypes<'id>>::GroupPermission,
    },
}

/// A transmission's end. A single MPDU's terminal retains its closed
/// operation owner: the higher layer consumes `Completed::into_parts` to
/// recover context, ticket and the complete MPDU, and queues cannot
/// exchange their components before that terminal boundary. A data arena's
/// end is not a terminal: [`DataPort::service_tx`](crate::DataPort::service_tx)
/// returns it.
pub enum TxCompleted<'id, P: PortTypes<'id>> {
    /// Exactly one submitted MPDU, including its original body owner.
    Single(SingleCompleted<'id, P>),
    /// The access point's beacon, however it ended: its single MPDU's
    /// terminal and the beacon permission its admission took.
    Beacon {
        /// The beacon's MPDU, as [`Self::Single`] carries one.
        completed: SingleCompleted<'id, P>,
        /// The installation's one beacon permission.
        permission: crate::BeaconPermission<'id, P::BeaconPath>,
    },
}

/// A backend's single terminal, with its original accepted owner.
pub type SingleCompleted<'id, P> = crate::Completed<
    <P as PortTypes<'id>>::Context,
    Slot<'id, <P as PortTypes<'id>>::TicketKind, <P as PortTypes<'id>>::TicketPath>,
    <P as PortTypes<'id>>::TxMpdu,
    PortSingleOutcome<'id, P>,
>;
/// A receive Block Ack slot of the phase types `P`.
pub type PortBlockAckSlot<'id, P> =
    <<P as PortTypes<'id>>::BlockAckPool as crate::association::RxBlockAckPool>::Slot<
        'id,
        <P as PortTypes<'id>>::BlockAckPath,
    >;
/// The outcome of a single MPDU in the phase types `P`.
pub type PortSingleOutcome<'id, P> =
    crate::SingleOutcome<<P as PortTypes<'id>>::AckEvidence, <P as PortTypes<'id>>::AirDenied>;

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
        spp: oer_protocols_ieee80211_wire::security::Spp,
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
    /// Equal identities name the same installation; no two installations of
    /// one port installation share one.
    type Identity: PartialEq;
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
pub enum InstalledGroup<'id, P: PortTypes<'id>> {
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

/// The pairwise installation, of its role. Only the driver creates it,
/// after the hardware write took effect.
pub enum InstalledPairwise<'id, P: PortTypes<'id>> {
    /// The station's pairwise installation.
    Station(<P::StationKeys as KeyTypes<'id>>::PairwiseKey),
    /// The access point's pairwise installation.
    AccessPoint(<P::AccessPointKeys as KeyTypes<'id>>::PairwiseKey),
}

/// A removed key's slot, of its role.
pub enum Removed<'id, P: PortTypes<'id>> {
    /// The station's.
    Station(RemovedKey<'id, P::StationKeys>),
    /// The access point's.
    AccessPoint(RemovedKey<'id, P::AccessPointKeys>),
}

/// The keys of a role in the phase types `P`.
pub trait RoleKeys<'id, P: PortTypes<'id>>: InterfaceRole + Sized {
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
    /// Preserve role when publishing an actual pairwise installation terminal.
    fn installed_pairwise(
        key: <Self::Keys as KeyTypes<'id>>::PairwiseKey,
    ) -> InstalledPairwise<'id, P>;
}

impl<'id, P: PortTypes<'id>> RoleKeys<'id, P> for Station {
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
    fn installed_pairwise(
        key: <Self::Keys as KeyTypes<'id>>::PairwiseKey,
    ) -> InstalledPairwise<'id, P> {
        InstalledPairwise::Station(key)
    }
}

impl<'id, P: PortTypes<'id>> RoleKeys<'id, P> for AccessPoint {
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
    fn installed_pairwise(
        key: <Self::Keys as KeyTypes<'id>>::PairwiseKey,
    ) -> InstalledPairwise<'id, P> {
        InstalledPairwise::AccessPoint(key)
    }
}

/// The types a backend's data port carries in its owners and terminals.
pub trait PortTypes<'id>: Sized {
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
    /// Nominal authority of this backend's beacon component
    /// ([`BeaconPool`](crate::BeaconPool)).
    type BeaconPath: PoolPath;
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
    /// An external MPDU body. A `Sent` MPDU retains its body internally
    /// for a retry; renewing that MPDU returns the body with its memory.
    type TxBody: crate::TxBody;
    /// The kind of the transmit tickets.
    type TicketKind;
    /// What the backend observed of a single MPDU's solicited response (an
    /// ACK, or the BlockAck to a BlockAckReq): a policy that needs the
    /// response's SNR requires [`AckSnr`](crate::AckSnr) of it.
    type AckEvidence: Copy + core::fmt::Debug + Eq;
    /// What the backend observed of a data arena's ACK or BlockAck: a
    /// policy that needs its SNR requires [`BlockAckSnr`](crate::BlockAckSnr)
    /// of it.
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
    /// Memory a network fills with one data frame in place, which a data
    /// arena's slot holds from its intake until it returns
    /// ([`DataPort::arena`](crate::DataPort::arena)). It names no installation: it outlives
    /// one and crosses execution contexts.
    type DataMemory: Send + 'static;
    /// The memory of one received MPDU, which goes back through
    /// [`DataPort::recycle`](crate::DataPort::recycle).
    type RxBuffer: crate::RxBuffer;
    /// A received buffer lent to a network owner
    /// ([`DataPort::lend`](crate::DataPort::lend)).
    type LentRx: crate::LentRxBuffer;
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

/// One attempt awaiting preparation: its ticket, its MPDU in a
/// buffer, its context, body, protection, key, power and the operation it asks the air for.
/// The frame's kind sets its response and
/// its retry plan ([`RetryPlan`]) or one rate; its access category picks
/// its queue, whose contention the backend draws.
pub struct Attempt<'id, T, M, K, X, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    /// The original operation's context, returned with its terminal.
    pub context: X,
    /// The attempt's place in flight.
    pub ticket: Slot<'id, T, Path>,
    /// Fresh memory with its checked description and body, or the original
    /// returned MPDU together with this attempt's port-bound rates.
    pub source: M,
    /// The access category the attempt contends as.
    pub access_category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory,
    /// The control exchange that protects its first transmission; a
    /// retry plan's stages name their own.
    pub protection: Protection,
    /// Its transmit power.
    pub power: SupportedPower<K>,
    /// What the attempt is, for the radio it shares the air with.
    pub coex_operation: TxOperation,
}

/// A sealed operation retaining its body and preparation loans until submission.
/// Only its concrete backend's private loan can release the complete owner.
pub struct AdmittedAttempt<
    'id,
    T,
    M: PreparedTx<'id>,
    K,
    X,
    Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>,
> {
    parts:
        AdmittedTxParts<M::OwnedContext, Slot<'static, T, Path>, M::OwnedPayload, M::Execution, K>,
    proof: core::marker::PhantomData<fn(&'id (), M, X) -> (&'id (), M, X)>,
}

/// Whole backend custody and its execution metadata after the frontend loan.
pub struct AdmittedTxParts<C, T, M, E, K> {
    /// Indivisible accepted operation.
    pub operation: crate::InFlight<C, T, M>,
    /// Metadata containing no independently replaceable MPDU.
    pub execution: E,
    /// Selected access category.
    pub access_category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory,
    /// Selected protection.
    pub protection: Protection,
    /// Selected transmit power.
    pub power: SupportedPower<K>,
    /// Selected coexistence operation.
    pub coex_operation: TxOperation,
}

impl<'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath> AdmittedAttempt<'id, T, M, K, X, Path> {
    /// Inspect the sealed owner without extracting or replacing any part.
    pub fn operation(
        &self,
    ) -> &crate::InFlight<M::OwnedContext, Slot<'static, T, Path>, M::OwnedPayload> {
        &self.parts.operation
    }

    /// Transfer the whole accepted owner through the backend's private frontend.
    pub fn into_parts(
        self,
        _: &oer_protocols_radio_port::BodyLoan<'id, Path::Identity, M::Backend>,
    ) -> AdmittedTxParts<M::OwnedContext, Slot<'static, T, Path>, M::OwnedPayload, M::Execution, K>
    {
        self.parts
    }
}

impl<'id, T, M, K, X, Path: PoolPath> Attempt<'id, T, M, K, X, Path>
where
    M: PreparedTx<'id>,
    M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X>,
{
    /// Lower the body loans before sealing the whole operation for backend custody.
    pub fn admit(self) -> AdmittedAttempt<'id, T, M, K, X, Path> {
        let crate::AdmissionOwners {
            context,
            ticket,
            payload,
            execution,
        } = self
            .source
            .into_admission(self.context, self.ticket, crate::Admission::new());
        AdmittedAttempt {
            parts: AdmittedTxParts {
                operation: crate::InFlight::admit(context, ticket, payload),
                execution,
                access_category: self.access_category,
                protection: self.protection,
                power: self.power,
                coex_operation: self.coex_operation,
            },
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
/// use oer_protocols_ieee80211_contracts_port::*;
/// fn group<'k, 'id: 'k, T: PortTypes<'id>>(
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
    ClassFrame<'p, C, K>,
    <T as PortTypes<'id>>::TxBody,
    <R as RoleKeys<'id, T>>::TxKey<'k>,
    RetransmissionPlan<'p, K>,
>;

/// Class-specific source whose portable layout has already been checked.
pub type CheckedPoolSource<'k, 'p, 'id, T, C, K, R> = crate::CheckedTxSource<
    <T as TxBufferPool<'id, C>>::Buffer,
    <T as TxBufferPool<'id, C>>::Sent,
    ClassFrame<'p, C, K>,
    <T as PortTypes<'id>>::TxBody,
    <R as RoleKeys<'id, T>>::TxKey<'k>,
    RetransmissionPlan<'p, K>,
>;

/// Preparation returns a port-bound MPDU or its unchanged class-specific source.
pub type PoolPreparation<'k, 'p, 'id, T, C, K, R> = Result<
    <T as PortTypes<'id>>::Prepared<'k, 'p, R, K>,
    PreparationRefused<PoolSource<'k, 'p, 'id, T, C, K, R>>,
>;

/// A class's fresh allocation or its retained published MPDU.
pub type PoolMpdu<'id, T, C> =
    ReturnedMpdu<<T as TxBufferPool<'id, C>>::Buffer, <T as TxBufferPool<'id, C>>::Sent>;

/// A backend's closed preparation proof. Cancellation recovers its owners
/// without creating a TX terminal; submission consumes the proof itself.
pub trait PreparedTx<'id>: Sized {
    /// The backend's resource family.
    type Types: PortTypes<'id>;
    /// The backend's tag: only the holder of its private loan
    /// ([`BodyLoan`](oer_protocols_radio_port::BodyLoan) of this tag) can
    /// consume this sealed admission.
    type Backend;
    /// Opaque context selected independently of the body's loan.
    type OwnedContext;
    /// Complete owned payload stored by P2.
    type OwnedPayload;
    /// Metadata needed by the backend to execute this MPDU.
    type Execution;
    /// Lower body loans before the operation is sealed. The invariant
    /// admission authority is constructed only by `Attempt::admit`.
    fn into_admission(
        self,
        context: <Self::Types as PortTypes<'id>>::Context,
        ticket: Slot<
            'id,
            <Self::Types as PortTypes<'id>>::TicketKind,
            <Self::Types as PortTypes<'id>>::TicketPath,
        >,
        admission: crate::Admission<Self>,
    ) -> crate::AdmissionOwners<
        Self::OwnedContext,
        Slot<
            'static,
            <Self::Types as PortTypes<'id>>::TicketKind,
            <Self::Types as PortTypes<'id>>::TicketPath,
        >,
        Self::OwnedPayload,
        Self::Execution,
    >;
    /// Recover the buffer and any external body. A `Sent` MPDU keeps its
    /// body nested until renewal.
    fn into_parts(
        self,
    ) -> (
        <Self::Types as PortTypes<'id>>::AnyTxBuffer,
        Option<<Self::Types as PortTypes<'id>>::TxBody>,
    );
}

/// An installed TX key and the PN allocated by its core for one fresh MPDU.
/// Retransmissions retain the encoded header rather than requesting another.
pub struct NumberedKey<Key> {
    /// The phase's borrowed hardware key authority.
    pub key: Key,
    /// Caller-allocated, checked 48-bit packet number.
    pub packet_number: oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber,
}

/// The MPDU supplied to a submission. Fresh memory travels with its
/// `ClassFrame` and body; a `Sent` MPDU already owns that description and
/// body, and receives only the new attempt's port-bound rate plan.
pub enum TxSource<B, N, F, O, Key, Plan> {
    /// A new MPDU, numbered at its first protected transmission.
    Fresh {
        /// Caller-owned memory, writable only before admission.
        buffer: B,
        /// The checked, port-bound frame description.
        frame: F,
        /// External octets at the end of the MPDU, retained until its return.
        body: Option<O>,
        /// The role's TX protection; the encoded Protected bit is derived from it.
        protection: TxProtection<NumberedKey<Key>>,
    },
    /// The same published MPDU again: keep its SN and PN; it carries Retry
    /// once an earlier transmission may have reached a receiver.
    Retransmission {
        /// The returned MPDU owns its description, payload and body.
        mpdu: N,
        /// Only the transmission parameters change, not the MPDU.
        plan: Plan,
    },
}

/// The port-bound rates of a new attempt of a `Sent` MPDU. The backend
/// checks that its response kind matches the original MPDU's kind.
#[derive(Debug)]
pub enum RetransmissionPlan<'p, K> {
    /// An individually acknowledged MPDU.
    Individual(RetryPlan<PortRate<'p, K>>),
    /// An individually addressed MPDU with no ACK.
    IndividualNoAck(crate::SupportedNoAck<'p, K>),
    /// A group-addressed MPDU.
    Group(PortRate<'p, K>),
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
    /// The backend source has no authority for this body resource loan.
    MissingBodyLoan,
    /// A retry names a different key installation, even in the same slot.
    RetransmissionKey,
    /// The new rate plan requires a different response kind.
    RetransmissionKind,
}

/// A class of transmit buffers: memory of one size, which the composition
/// sets.
pub trait TxBufferClass {
    /// The longest MPDU a buffer of the class holds, without its FCS.
    const CAPACITY: usize;
}

/// The phase types of a backend that keeps a pool of the buffer class `C`.
pub trait TxBufferPool<'id, C: TxBufferClass>: PortTypes<'id> {
    /// A buffer of the class: its memory, which the body holds and writes
    /// while it is not submitted. The backend owns its type and mints it
    /// only in the install's resources.
    type Buffer: TxBufferMemory;

    /// Prepare a source while its buffer class is concrete. The resulting
    /// closed value retains phase, install and role until `submit`
    /// consumes it. A refusal returns the original typed source.
    fn prepare<'k, 'p, R: RoleKeys<'id, Self>, K: Ieee80211Capabilities>(
        source: CheckedPoolSource<'k, 'p, 'id, Self, C, K, R>,
    ) -> PoolPreparation<'k, 'p, 'id, Self, C, K, R>
    where
        'id: 'k;

    /// The buffer, as any class's.
    fn erase(buffer: Self::Buffer) -> Self::AnyTxBuffer;

    /// A published MPDU, protected or open. It owns the immutable encoded
    /// header, payload, checked description and external body. A protected
    /// MPDU also retains its PN and original key-install identity.
    /// A withdrawal returns Fresh only if never published; otherwise Sent.
    /// Retransmission preserves SN, PN and payload; it carries Retry once an
    /// earlier transmission may have reached a receiver.
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

    /// The form the buffer of `mpdu` comes back in, read by borrow:
    /// [`TxBufferForm::Fresh`] when [`Self::class`] takes it,
    /// [`TxBufferForm::Sent`] when [`Self::sent`] does, `None` when it is of
    /// another class. Nothing changes and no owner moves.
    fn class_of(mpdu: &Self::TxMpdu) -> Option<TxBufferForm>;

    /// The form of the buffer a single MPDU's terminal returns, read by
    /// borrow as [`Self::class_of`] reads it, before the terminal is taken
    /// apart.
    fn completed_class(completed: &SingleCompleted<'id, Self>) -> Option<TxBufferForm> {
        Self::class_of(completed.payload())
    }
}

/// The longest MSDU payload a network's transmit memory carries: an
/// Ethernet payload, at most 1500 octets (IEEE Std 802.3-2022, 3.2.7).
pub const NETWORK_MSDU_PAYLOAD: usize = 1500;

/// The longest data MPDU written in place in front of a
/// [`NETWORK_MSDU_PAYLOAD`] payload: a QoS Data header, the CCMP header and
/// LLC/SNAP, without the MIC and FCS the transmitter appends.
pub const NETWORK_MPDU_LEN: usize =
    oer_protocols_ieee80211_wire::data::IEEE80211_QOS_DATA_HEADER_LEN
        + oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN
        + oer_protocols_ieee80211_wire::data::LLC_SNAP_HEADER_LEN
        + NETWORK_MSDU_PAYLOAD;

/// The frame region a backend's network memory holds ([`TxNetworkPool`]).
pub trait NetworkCapacity {
    /// The longest MPDU the frame region holds, without its MIC and FCS.
    const NETWORK_CAPACITY: usize;
}

/// The class of the phase types `T`'s network transmit memory as one single
/// MPDU's buffer ([`TxNetworkPool`]): its frame region, from the MPDU's
/// first octet, holds [`NetworkCapacity::NETWORK_CAPACITY`] octets. A
/// caller that carries the longest MSDU checks at compile time that it is
/// at least [`NETWORK_MPDU_LEN`].
pub struct NetworkFrames<T>(core::marker::PhantomData<fn() -> T>);

impl<T: NetworkCapacity> TxBufferClass for NetworkFrames<T> {
    const CAPACITY: usize = T::NETWORK_CAPACITY;
}

/// A fresh buffer of the phase types `T` over a network's transmit memory.
pub type NetworkTxBuffer<'id, T> = <T as TxBufferPool<'id, NetworkFrames<T>>>::Buffer;

/// The phase types of a backend whose single-MPDU path also sends from a
/// network's transmit memory ([`PortTypes::DataMemory`]): the same memory a
/// data arena takes, as a buffer of the class [`NetworkFrames`]. The
/// network wrote one MSDU's Ethernet frame into it; the caller writes the
/// MPDU in place from the frame region's first octet
/// ([`TxBufferMemory::bytes_mut`]), prepares it through
/// [`TxBufferPool::prepare`] like any buffer of a class, and the terminal
/// returns the same memory, fresh or `Sent`, through
/// [`TxBufferPool::class`] and [`TxBufferPool::sent`]. A port mints the
/// buffer under its installation
/// ([`AccessPointPort::network_buffer`](crate::AccessPointPort::network_buffer));
/// [`Self::into_memory`] hands the memory back to the network.
pub trait TxNetworkPool<'id>: NetworkCapacity + TxBufferPool<'id, NetworkFrames<Self>> {
    /// The frame region of the network's `memory` before it is lent: the
    /// octets [`TxBufferMemory::bytes_mut`] of its buffer opens, which a
    /// caller reads to choose the MSDU's queue before anything is lent.
    /// `None` for memory that holds no frame region, which a backend's
    /// geometry excludes.
    fn frame_region(memory: &mut Self::DataMemory) -> Option<&mut [u8]>;

    /// The network's memory of a fresh buffer, whole: it no longer names an
    /// installation.
    ///
    /// # Errors
    ///
    /// The hardware still holds the memory (a completion never handed it
    /// back): the buffer comes back unchanged.
    fn into_memory(
        buffer: NetworkTxBuffer<'id, Self>,
    ) -> Result<Self::DataMemory, NetworkTxBuffer<'id, Self>>;
}

/// The form in which a buffer of a class comes back from its MPDU
/// ([`TxBufferPool::class_of`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxBufferForm {
    /// Fresh memory, never published.
    Fresh,
    /// A published MPDU, `Sent`.
    Sent,
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

impl<B: TxBufferMemory, S> ReturnedMpdu<B, S> {
    /// Release the old MPDU through its owner's renewal policy. A fresh
    /// allocation releases any previous admission age as well. The
    /// operation consumes either owner once and always returns the buffer;
    /// the renewal's result says whether its memory was the caller's.
    pub fn into_fresh(
        self,
        renew: impl FnOnce(S) -> B,
    ) -> (B, Result<(), BufferMemoryUnavailable>) {
        let mut buffer = match self {
            Self::Fresh(buffer) => buffer,
            Self::Sent(mpdu) => renew(mpdu),
        };
        let renewed = buffer.renew_msdu();
        (buffer, renewed)
    }
}

/// A buffer token whose memory is not the body's: its backend's hardware
/// owns it, or its place holds none. The token stays the body's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferMemoryUnavailable;

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
    /// Discard the previous MSDU's admission state before reusing its
    /// allocation for a new MSDU. Retry and re-formation never call this.
    ///
    /// # Errors
    ///
    /// The memory is not the body's; nothing changes.
    fn renew_msdu(&mut self) -> Result<(), BufferMemoryUnavailable>;
    /// The buffer's octets, as long as its class holds.
    ///
    /// # Errors
    ///
    /// The memory is not the body's.
    fn bytes_mut(&mut self) -> Result<&mut [u8], BufferMemoryUnavailable>;
}

/// A frame that a buffer of the class `C` fits; only [`ClassFrame::new`]
/// makes one.
pub struct ClassFrame<'p, C, K> {
    frame: TxFrame<'p, K>,
    class: core::marker::PhantomData<C>,
}

impl<'p, C: TxBufferClass, K: Ieee80211Capabilities> ClassFrame<'p, C, K> {
    /// `frame`, when a buffer of the class `C` fits it.
    ///
    /// # Errors
    ///
    /// The frame, when it is longer than the class holds.
    pub fn new(frame: TxFrame<'p, K>) -> Result<Self, TxFrame<'p, K>> {
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
    pub fn into_frame(self) -> TxFrame<'p, K> {
        self.frame
    }

    /// Borrow the checked description for preparation without consuming
    /// the original operation, so a refusal can return it intact.
    pub const fn frame(&self) -> &TxFrame<'p, K> {
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
impl<'p, C, K> ClassFrame<'p, C, K> {
    /// Add only the port-bound retry plan to an individually acknowledged MPDU.
    pub fn individual(
        frame: ClassSizedFrame<C, crate::AckHeader, K>,
        plan: RetryPlan<PortRate<'p, K>>,
    ) -> Self {
        Self {
            frame: TxFrame::Individual {
                frame: frame.frame,
                plan,
            },
            class: core::marker::PhantomData,
        }
    }
    /// Add only the port-bound rate to a group MPDU.
    pub fn group(
        frame: ClassSizedFrame<C, crate::HeaderSpec<crate::GroupAddress>, K>,
        rate: PortRate<'p, K>,
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
/// attempt of that queue and keeps the frame's sequence number. A transmission
/// without an ACK sets the Retry bit, which stays; a CTS timeout or a collision
/// sets nothing. An exhausted plan ends the attempt unacknowledged. A backend
/// whose hardware retries carries the plan out there.
#[derive(Debug)]
pub struct RetryPlan<R> {
    /// The first stage: a plan always has one.
    first: RetryStage<R>,
    /// The stages after the first, in order. The occupied cells form a
    /// prefix, which every operation keeps; the array's length is the bound.
    rest: [Option<RetryStage<R>>; RETRY_STAGES - 1],
}

impl<R: Copy> Clone for RetryStage<R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R: Copy> Copy for RetryStage<R> {}
impl<R: Copy> Clone for RetryPlan<R> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<R: Copy> Copy for RetryPlan<R> {}

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

    /// The plan past its first stage; `None` when that stage was its last.
    pub fn after_first(self) -> Option<Self> {
        let [second, third, fourth] = self.rest;
        Some(Self {
            first: second?,
            rest: [third, fourth, None],
        })
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
    /// # use oer_protocols_ieee80211_contracts_port::{Protection, RetryPlan, RetryStage};
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

    /// The plan with each stage protected by the exchange `protection`
    /// names for its rate; rates, counts and order unchanged.
    pub fn with_protection(self, mut protection: impl FnMut(&R) -> Protection) -> Self {
        let mut stage = |stage: RetryStage<R>| RetryStage {
            protection: protection(&stage.rate),
            ..stage
        };
        Self {
            first: stage(self.first),
            rest: self.rest.map(|cell| cell.map(&mut stage)),
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

/// One gate and its arbiter's unique construction authority.
///
/// The installation's mint builds this component once. Returning a gate
/// after a cycle does not mint another arbiter or another pair of lease places.
pub struct ArbiterPool;

impl oer_protocols_radio_port::PoolSet for ArbiterPool {
    type Pools<'id, Path: PoolPath> = (
        GateOpen<'id, Path>,
        GateIssuer<'id, Path>,
        ArbiterAuthority<'id, Path>,
    );

    fn build<'id, Path: PoolPath>(
        builder: oer_protocols_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        let (open, issuer) = builder.into_pools::<GatePool>();
        (
            open,
            issuer,
            ArbiterAuthority {
                brand: core::marker::PhantomData,
            },
        )
    }
}

/// The sole right to construct the arbiter of this installation's gate.
#[derive(Debug)]
#[must_use = "construct the arbiter exactly once with its initial gate"]
pub struct ArbiterAuthority<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    brand: core::marker::PhantomData<fn(&'id (), Path) -> (&'id (), Path)>,
}

impl<Path: PoolPath> ArbiterAuthority<'static, Path> {
    /// Lend the one construction authority without copying or reissuing it.
    pub fn lend<'p, S>(
        self,
        _: &oer_protocols_radio_port::BodyLoan<'p, Path::Identity, S>,
    ) -> ArbiterAuthority<'p, Path> {
        ArbiterAuthority {
            brand: core::marker::PhantomData,
        }
    }
}

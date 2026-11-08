//! The access-point role on the shared port.
//!
//! One [`DataPort`] is the installation's one physical owner: the MAC, the
//! receive ring, the interrupt owner, DMA and the transmit queues, the
//! station's data arenas, the gate and the start and stop. Each role reaches
//! it through its own typed entry points: the station through
//! [`DataPort::admit`] and [`DataPort::command`], the access point through
//! [`AccessPointPort`], beside them. No operation switches on a runtime role.
//! Both roles' attempts go to the same [`DataPort::submit`] and the same
//! queues, so a backend runs one implementation for both, and the terminals
//! of both come in the same [`PortSink`].
//!
//! The access point holds its own permissions, which the installation mints
//! once beside the station's: its configure permission
//! ([`ConfigurePermission`] of [`AccessPoint`]), its association, its
//! pairwise slots (one per association identifier), its group slots and group
//! permission ([`PortTypes::AccessPointKeys`]), and the one
//! [`BeaconPermission`]. Without them an access-point operation does not
//! compile, and none of them fits a station operation. A beacon permission
//! of an earlier installation of the port compiles and is refused
//! explicitly ([`AccessPointPort::current_beacon`]) before a beacon is
//! built.
//!
//! # Acceptance and terminals
//!
//! | Operation | Accepted at | Refused before | Ends with |
//! | --- | --- | --- | --- |
//! | a transmission ([`AccessPointPort::admit_access_point`]) | [`DataPort::submit`] | [`SubmitRefusal`](crate::SubmitRefusal), its withdrawn terminal in the sink | [`TxCompleted::Single`](crate::TxCompleted::Single) |
//! | a beacon ([`AccessPointPort::admit_beacon`]) | [`DataPort::submit`] | [`BeaconRefusal::StalePermission`] from [`AccessPointPort::current_beacon`] before the beacon is built; then as a transmission, its terminal returning the permission | [`TxCompleted::Beacon`](crate::TxCompleted::Beacon) |
//! | a key installation or removal, a configuration, a TSF control ([`AccessPointPort::access_point_command`]) | [`AccessPointProgress::Done`] | [`AccessPointProgress::Refused`] with the command | its terminal in the sink, or none for a TSF control |
//!
//! # Roles stop on their own
//!
//! The access point stops with its own commands: a configuration
//! [`AccessPointReceive::Off`], which ends its queued attempts (a queued
//! beacon included) withdrawn and waits for its published one, the removal
//! of each pairwise key and of its group key, and the TSF stop. None of them
//! withdraws, holds or changes the station's attempts, keys or
//! configuration, and an access-point command held at its barrier holds no
//! station command. A confirmed [`DataPort::poll_stop`] stops the shared
//! owner for both roles: every queued attempt ends withdrawn, the access
//! point's interface receives nothing and its TSF runs no more, and each
//! role's permissions stay with their holders.
//!
//! # Values a backend sources first
//!
//! Who writes a beacon's Timestamp field, and how the interface's TSF
//! restarts and stops, are hardware facts. A backend declares them in its
//! [`AccessPointServices`]; one that has not sourced them declares none, and
//! then no [`BeaconStamp`] or [`SupportedTsfControl`] exists for it, so no
//! beacon is published and no TSF control is sent.
//!
//! # Compile-fail tests
//!
//! A beacon is admitted with the one beacon permission, so a second beacon
//! is not admitted while the first is out:
//!
//! ```compile_fail,E0382
//! use oer_protocols_ieee80211_contracts_port::*;
//! use oer_protocols_radio_port::{PoolPath, Slot};
//! fn twice<'c, 'id, T, M: PreparedTx<'id>, N: PreparedTx<'id>, K, X, Path: PoolPath, Q: PoolPath>(
//!     permission: CurrentBeacon<'c, 'id, Q>,
//!     first: (X, Slot<'id, T, Path>, M, SupportedPower<K>, BeaconStamp<K>),
//!     second: (X, Slot<'id, T, Path>, N, SupportedPower<K>, BeaconStamp<K>),
//! ) where
//!     M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X>,
//!     N::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X>,
//! {
//!     let (context, ticket, source, power, stamp) = first;
//!     let _one = BeaconAttempt { context, ticket, source, power, coex_operation: TxOperation::Data, permission, stamp }.admit();
//!     let (context, ticket, source, power, stamp) = second;
//!     let _two = BeaconAttempt { context, ticket, source, power, coex_operation: TxOperation::Data, permission, stamp }.admit();
//! }
//! ```
//!
//! Its compiling twin admits one beacon with the permission:
//!
//! ```
//! use oer_protocols_ieee80211_contracts_port::*;
//! use oer_protocols_radio_port::{PoolPath, Slot};
//! fn once<'c, 'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath, Q: PoolPath>(
//!     permission: CurrentBeacon<'c, 'id, Q>,
//!     beacon: (X, Slot<'id, T, Path>, M, SupportedPower<K>, BeaconStamp<K>),
//! ) where
//!     M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X>,
//! {
//!     let (context, ticket, source, power, stamp) = beacon;
//!     let _one = BeaconAttempt { context, ticket, source, power, coex_operation: TxOperation::Data, permission, stamp }.admit();
//! }
//! ```
//!
//! A checked permission is lent for one borrow of its port, so it is not
//! kept past a reinstallation:
//!
//! ```compile_fail
//! use oer_protocols_ieee80211_contracts_port::*;
//! fn keep<'id, P: AccessPointPort<'id>>(
//!     port: &P,
//!     permission: BeaconPermission<'id, <P::Types as PortTypes<'id>>::BeaconPath>,
//! ) -> Option<CurrentBeacon<'static, 'id, <P::Types as PortTypes<'id>>::BeaconPath>> {
//!     port.current_beacon(permission).ok()
//! }
//! ```
//!
//! A beacon permission is not made by a caller:
//!
//! ```compile_fail,E0451
//! use oer_protocols_ieee80211_contracts_port::BeaconPermission;
//! use oer_protocols_radio_port::Slot;
//! fn forge<'id, K>(slot: Slot<'id, K>) -> BeaconPermission<'id> {
//!     BeaconPermission { slot, generation: 0 }
//! }
//! ```

use core::marker::PhantomData;

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_wire::{
    address::IndividualAddress, ccmp::GroupKeyId, qos::WmmAccessCategory,
};
use oer_protocols_radio_port::{BodyLoan, LocalId, PoolPath, Root, Slot};

use crate::{
    AccessPoint, AdmittedAttempt, AdmittedTxParts, CommandRefusal, CommandTypes,
    ConfigurePermission, DataPort, Ieee80211Capabilities, InFlight, KeyTypes, PortSink, PortTypes,
    PreparedTx, Protection, SupportedFilter, SupportedPower, TxOperation,
};

/// The access category of every beacon: a beacon contends as voice, the
/// first slice's policy taken from the archive's access point.
pub const BEACON_ACCESS_CATEGORY: WmmAccessCategory = WmmAccessCategory::Voice;

/// Who writes a beacon's Timestamp field (IEEE Std 802.11-2020 9.4.1.10).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BeaconTimestamp {
    /// The transmitter writes the interface's TSF into the field as the
    /// beacon goes out; what the caller wrote there is replaced.
    Transmitter,
    /// The field goes on the air as the caller wrote it.
    Caller,
}

/// What a backend serves of the access-point role beyond the shared port.
/// A value the backend has not sourced is absent, and what needs it is
/// refused: no checked value of it exists.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AccessPointServices {
    /// Who writes a beacon's Timestamp field, once the backend's sequence
    /// is sourced.
    pub beacon_timestamp: Option<BeaconTimestamp>,
    /// Whether the backend restarts and stops the access point's TSF by a
    /// sourced sequence.
    pub tsf_control: bool,
}

/// The capabilities of a backend that serves the access-point role.
pub trait AccessPointCapabilities: Ieee80211Capabilities {
    /// What it serves of the role.
    const ACCESS_POINT: AccessPointServices;
}

/// The Timestamp rule of a beacon on `K`: only a rule `K` declared has one.
pub struct BeaconStamp<K> {
    source: BeaconTimestamp,
    brand: PhantomData<fn() -> K>,
}

impl<K: AccessPointCapabilities> BeaconStamp<K> {
    /// The rule `source`, when `K` declares that it writes the field so.
    pub const fn new(source: BeaconTimestamp) -> Option<Self> {
        match K::ACCESS_POINT.beacon_timestamp {
            Some(declared) if declared as u8 == source as u8 => Some(Self {
                source,
                brand: PhantomData,
            }),
            _ => None,
        }
    }
}

impl<K> BeaconStamp<K> {
    /// Who writes the field.
    pub const fn source(&self) -> BeaconTimestamp {
        self.source
    }
}

impl<K> Clone for BeaconStamp<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for BeaconStamp<K> {}

impl<K> core::fmt::Debug for BeaconStamp<K> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("BeaconStamp")
            .field(&self.source)
            .finish()
    }
}

/// What a TSF control does to the access point's TSF.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TsfControl {
    /// Restart it from zero and run it.
    Restart,
    /// Stop it.
    Stop,
}

/// A TSF control `K` carries out by its sourced sequence.
pub struct SupportedTsfControl<K> {
    control: TsfControl,
    brand: PhantomData<fn() -> K>,
}

impl<K: AccessPointCapabilities> SupportedTsfControl<K> {
    /// `control`, when `K` declares its TSF control.
    pub const fn new(control: TsfControl) -> Option<Self> {
        if K::ACCESS_POINT.tsf_control {
            Some(Self {
                control,
                brand: PhantomData,
            })
        } else {
            None
        }
    }
}

impl<K> SupportedTsfControl<K> {
    /// The control.
    pub const fn get(&self) -> TsfControl {
        self.control
    }
}

impl<K> Clone for SupportedTsfControl<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for SupportedTsfControl<K> {}

impl<K> core::fmt::Debug for SupportedTsfControl<K> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("SupportedTsfControl")
            .field(&self.control)
            .finish()
    }
}

/// What the access point's interface receives. Its BSSID is its own
/// address ([`AccessPointPort::access_point_address`]).
pub enum AccessPointReceive<K> {
    /// Deliver no access-point frames.
    Off,
    /// Receive the BSS by these rules.
    Bss {
        /// The receive rules supported for the access point's interface.
        filter: SupportedFilter<K, AccessPoint>,
    },
}

impl<K> Clone for AccessPointReceive<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for AccessPointReceive<K> {}

impl<K> core::fmt::Debug for AccessPointReceive<K> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Off => formatter.write_str("Off"),
            Self::Bss { filter } => formatter
                .debug_struct("Bss")
                .field("filter", filter)
                .finish(),
        }
    }
}

enum BeaconKind {}

/// The beacon component of a backend's pool set: it mints the
/// installation's one [`BeaconGrant`], which the backend issues as the
/// installation's [`BeaconPermission`].
pub struct BeaconPool;

impl oer_protocols_radio_port::PoolSet for BeaconPool {
    type Pools<'id, Path: PoolPath> = BeaconGrant<'id, Path>;

    fn build<'id, Path: PoolPath>(
        builder: oer_protocols_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        let [slot] = builder.into_pools::<oer_protocols_radio_port::Slots<BeaconKind, 1>>();
        BeaconGrant { slot }
    }
}

/// The minted beacon place before its backend issues it to one installation.
#[must_use = "the backend issues the grant as its installation's beacon permission"]
pub struct BeaconGrant<'id, Path: PoolPath = Root<LocalId<'id>>> {
    slot: Slot<'id, BeaconKind, Path>,
}

impl<'id, Path: PoolPath> BeaconGrant<'id, Path> {
    /// Issue the grant as the permission of the installation `generation`:
    /// the backend that consumed the installation's mint holds the grant.
    pub fn issue(self, generation: u32) -> BeaconPermission<'id, Path> {
        BeaconPermission {
            slot: self.slot,
            generation,
        }
    }
}

/// The installation's one permission to have a beacon out: a beacon's
/// admission takes it and its terminal gives it back, so at most one beacon
/// is admitted, queued or on the air. It names the installation that issued
/// it; a backend refuses the permission of an earlier installation
/// ([`BeaconRefusal::StalePermission`]).
#[must_use = "the beacon permission returns only with its beacon's terminal"]
pub struct BeaconPermission<'id, Path: PoolPath = Root<LocalId<'id>>> {
    slot: Slot<'id, BeaconKind, Path>,
    generation: u32,
}

impl<Path: PoolPath> core::fmt::Debug for BeaconPermission<'_, Path> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BeaconPermission")
            .field("generation", &self.generation)
            .finish()
    }
}

impl<Path: PoolPath> BeaconPermission<'_, Path> {
    /// The generation of the installation that issued it.
    pub const fn generation(&self) -> u32 {
        self.generation
    }
}

impl<Path: PoolPath> BeaconPermission<'static, Path> {
    /// Move the same permission across its backend's private body boundary.
    pub fn lend<'p, S>(self, loan: &BodyLoan<'p, Path::Identity, S>) -> BeaconPermission<'p, Path> {
        BeaconPermission {
            slot: loan.lend_slot(self.slot),
            generation: self.generation,
        }
    }
}

impl<'p, Path: PoolPath> BeaconPermission<'p, Path> {
    /// Move the same permission across its backend's private body boundary.
    pub fn into_owned<S>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> BeaconPermission<'static, Path> {
        BeaconPermission {
            slot: loan.reclaim_slot(self.slot),
            generation: self.generation,
        }
    }
}

/// Why [`AccessPointPort::current_beacon`] refused a beacon permission:
/// before any beacon is built or admitted, the permission comes back.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BeaconRefusal {
    /// The permission was issued by an earlier installation of the port:
    /// final for this permission; the current installation's permission is
    /// with its holder or returns with its beacon's terminal.
    StalePermission {
        /// The permission's installation.
        permission: u32,
        /// The port's installation.
        installation: u32,
    },
}

/// A refused beacon permission, unchanged, and why.
#[derive(Debug)]
pub struct BeaconRefused<'id, Path: PoolPath> {
    /// Why.
    pub reason: BeaconRefusal,
    /// The permission.
    pub permission: BeaconPermission<'id, Path>,
}

/// A beacon permission the port `'p` found to be its current
/// installation's: it lives within that borrow of the port, so no
/// reinstallation comes between the check and the beacon's admission.
pub struct CurrentBeacon<'p, 'id, Path: PoolPath> {
    permission: BeaconPermission<'id, Path>,
    port: PhantomData<&'p ()>,
}

impl<'id, Path: PoolPath> CurrentBeacon<'_, 'id, Path> {
    /// The permission back, unused.
    pub fn into_permission(self) -> BeaconPermission<'id, Path> {
        self.permission
    }
}

impl<Path: PoolPath> core::fmt::Debug for CurrentBeacon<'_, '_, Path> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("CurrentBeacon")
            .field(&self.permission)
            .finish()
    }
}

/// One beacon awaiting admission: an access-point attempt of the frame the
/// caller prepared as its Beacon, with the beacon permission and the
/// Timestamp rule. Its access category is [`BEACON_ACCESS_CATEGORY`] and its
/// first transmission goes unprotected.
pub struct BeaconAttempt<
    'c,
    'id,
    T,
    M,
    K,
    X,
    Path: PoolPath = Root<LocalId<'id>>,
    BeaconPath: PoolPath = Root<LocalId<'id>>,
> {
    /// The operation's context, returned with its terminal.
    pub context: X,
    /// The attempt's place in flight.
    pub ticket: Slot<'id, T, Path>,
    /// The prepared beacon frame.
    pub source: M,
    /// Its transmit power.
    pub power: SupportedPower<K>,
    /// What the attempt is, for the radio it shares the air with.
    pub coex_operation: TxOperation,
    /// The installation's one beacon permission, checked by its port.
    pub permission: CurrentBeacon<'c, 'id, BeaconPath>,
    /// Who writes its Timestamp field.
    pub stamp: BeaconStamp<K>,
}

impl<'id, T, M, K, X, Path: PoolPath, BeaconPath: PoolPath>
    BeaconAttempt<'_, 'id, T, M, K, X, Path, BeaconPath>
where
    M: PreparedTx<'id>,
    M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X>,
{
    /// Seal the beacon as one attempt on the beacon's access category,
    /// keeping the permission beside it.
    pub fn admit(self) -> AdmittedBeacon<'id, T, M, K, X, Path, BeaconPath> {
        let Self {
            context,
            ticket,
            source,
            power,
            coex_operation,
            permission,
            stamp,
        } = self;
        AdmittedBeacon {
            attempt: crate::Attempt {
                context,
                ticket,
                source,
                access_category: BEACON_ACCESS_CATEGORY,
                protection: Protection::None,
                power,
                coex_operation,
            }
            .admit(),
            permission: permission.permission,
            stamp,
        }
    }
}

/// A sealed beacon: its attempt, the beacon permission and its Timestamp
/// rule. Only its backend's private loan releases them.
pub struct AdmittedBeacon<
    'id,
    T,
    M: PreparedTx<'id>,
    K,
    X,
    Path: PoolPath = Root<LocalId<'id>>,
    BeaconPath: PoolPath = Root<LocalId<'id>>,
> {
    attempt: AdmittedAttempt<'id, T, M, K, X, Path>,
    permission: BeaconPermission<'id, BeaconPath>,
    stamp: BeaconStamp<K>,
}

/// What a backend takes from a sealed beacon.
pub struct BeaconParts<'id, C, T, P, E, K, BeaconPath: PoolPath> {
    /// The beacon's attempt.
    pub attempt: AdmittedTxParts<C, T, P, E, K>,
    /// The permission, which its terminal returns.
    pub permission: BeaconPermission<'id, BeaconPath>,
    /// Who writes its Timestamp field.
    pub stamp: BeaconStamp<K>,
}

impl<'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath, BeaconPath: PoolPath>
    AdmittedBeacon<'id, T, M, K, X, Path, BeaconPath>
{
    /// Inspect the sealed operation without extracting any part.
    pub fn operation(&self) -> &InFlight<M::OwnedContext, Slot<'static, T, Path>, M::OwnedPayload> {
        self.attempt.operation()
    }

    /// Who writes its Timestamp field.
    pub const fn stamp(&self) -> BeaconStamp<K> {
        self.stamp
    }

    /// Transfer the whole beacon through the backend's private frontend.
    pub fn into_parts(
        self,
        loan: &BodyLoan<'id, Path::Identity, M::Backend>,
    ) -> BeaconParts<
        'id,
        M::OwnedContext,
        Slot<'static, T, Path>,
        M::OwnedPayload,
        M::Execution,
        K,
        BeaconPath,
    > {
        BeaconParts {
            attempt: self.attempt.into_parts(loan),
            permission: self.permission,
            stamp: self.stamp,
        }
    }
}

/// The types of a port `P`.
type Types<'id, P> = <P as DataPort<'id>>::Types;

/// A sealed single MPDU of the access point's interface on the port `P`.
pub type AccessPointSubmission<'k, 'p, 'id, P> = AdmittedAttempt<
    'id,
    <Types<'id, P> as PortTypes<'id>>::TicketKind,
    <Types<'id, P> as PortTypes<'id>>::Prepared<'k, 'p, AccessPoint, <P as DataPort<'id>>::Caps>,
    <P as DataPort<'id>>::Caps,
    <Types<'id, P> as PortTypes<'id>>::Context,
    <Types<'id, P> as PortTypes<'id>>::TicketPath,
>;

/// A sealed beacon of the access point on the port `P`.
pub type BeaconSubmission<'k, 'p, 'id, P> = AdmittedBeacon<
    'id,
    <Types<'id, P> as PortTypes<'id>>::TicketKind,
    <Types<'id, P> as PortTypes<'id>>::Prepared<'k, 'p, AccessPoint, <P as DataPort<'id>>::Caps>,
    <P as DataPort<'id>>::Caps,
    <Types<'id, P> as PortTypes<'id>>::Context,
    <Types<'id, P> as PortTypes<'id>>::TicketPath,
    <Types<'id, P> as PortTypes<'id>>::BeaconPath,
>;

/// The access point's keys of the phase types `T`.
type AccessPointKeys<'id, T> = <T as PortTypes<'id>>::AccessPointKeys;

/// The phase types of a backend that runs [`AccessPointCommand`]s: the
/// payloads only its issuer makes.
pub trait AccessPointTypes<'id>: CommandTypes<'id> {
    /// An accepted pairwise installation of the access point: its vacant
    /// slot, peer, key material and the identity its key will carry.
    type AccessPointPairwiseInstall;
    /// An accepted group installation of the access point: its vacant slot,
    /// the group permission, its key ID and material.
    type AccessPointGroupInstall;
}

/// One operation of the access point on the shared port.
///
/// The port runs the access point's commands in the order it sent them,
/// apart from the station's: one held at its barrier comes back
/// [`AccessPointProgress::Held`] and runs again on a later call, before any
/// later command of the access point; one the port cannot run in its state
/// comes back refused, with every owner unchanged.
pub enum AccessPointCommand<'id, T: AccessPointTypes<'id>, K: Ieee80211Capabilities> {
    /// Install a peer's pairwise key; its terminal
    /// ([`LowerMacTerminal::PairwiseKeyInstalled`](crate::LowerMacTerminal::PairwiseKeyInstalled))
    /// returns the installed key.
    InstallPairwise(T::AccessPointPairwiseInstall),
    /// Install the group key; its terminal is the receive boundary of the
    /// new installation
    /// ([`LowerMacTerminal::GroupKeyInstalled`](crate::LowerMacTerminal::GroupKeyInstalled)).
    InstallGroup(T::AccessPointGroupInstall),
    /// Remove a peer's pairwise key once no admitted attempt uses it.
    RemovePairwise {
        /// The association owner the removal returns.
        association: <AccessPointKeys<'id, T> as KeyTypes<'id>>::Association,
        /// The installed key.
        key: <AccessPointKeys<'id, T> as KeyTypes<'id>>::PairwiseKey,
    },
    /// Remove the group key once no admitted attempt uses it.
    RemoveGroup {
        /// The access point's group permission.
        permission: <AccessPointKeys<'id, T> as KeyTypes<'id>>::GroupPermission,
        /// The installed key.
        key: <AccessPointKeys<'id, T> as KeyTypes<'id>>::GroupKey,
    },
    /// Set what the access point receives once none of its frames is on the
    /// air: its queued attempts end withdrawn, a beacon's with its
    /// permission.
    Configure {
        /// The access point's unique configure permission, which the
        /// terminal returns.
        permission: ConfigurePermission<
            'id,
            AccessPoint,
            T::AccessPointCommandPath,
            T::AccessPointConfigureContext,
        >,
        /// What it receives.
        receive: AccessPointReceive<K>,
    },
    /// Restart or stop the access point's TSF; it has no terminal.
    Tsf(SupportedTsfControl<K>),
}

impl<'id, T: AccessPointTypes<'id>, K: Ieee80211Capabilities> AccessPointCommand<'id, T, K> {
    /// The command's name, for a refusal's or a barrier's observation.
    pub const fn kind(&self) -> AccessPointCommandKind {
        match self {
            Self::InstallPairwise(_) => AccessPointCommandKind::InstallPairwise,
            Self::InstallGroup(_) => AccessPointCommandKind::InstallGroup,
            Self::RemovePairwise { .. } => AccessPointCommandKind::RemovePairwise,
            Self::RemoveGroup { .. } => AccessPointCommandKind::RemoveGroup,
            Self::Configure { .. } => AccessPointCommandKind::Configure,
            Self::Tsf(_) => AccessPointCommandKind::Tsf,
        }
    }
}

/// The kind of an [`AccessPointCommand`], without its owners.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AccessPointCommandKind {
    /// [`AccessPointCommand::InstallPairwise`].
    InstallPairwise,
    /// [`AccessPointCommand::InstallGroup`].
    InstallGroup,
    /// [`AccessPointCommand::RemovePairwise`].
    RemovePairwise,
    /// [`AccessPointCommand::RemoveGroup`].
    RemoveGroup,
    /// [`AccessPointCommand::Configure`].
    Configure,
    /// [`AccessPointCommand::Tsf`].
    Tsf,
}

/// How far [`AccessPointPort::access_point_command`] took one command.
pub enum AccessPointProgress<'id, T: AccessPointTypes<'id>, K: Ieee80211Capabilities> {
    /// The command ran; its terminal, if it has one, is in the sink.
    Done,
    /// The command waits at its barrier; the caller runs it again, before
    /// any later command of the access point, on a later call.
    Held(AccessPointCommand<'id, T, K>),
    /// The port cannot run the command in its state; nothing changed.
    Refused(AccessPointCommand<'id, T, K>, CommandRefusal),
}

/// An access-point pairwise installation command and the identity its
/// installed key will carry.
pub type AccessPointPairwiseIssue<'id, T, K> = (
    AccessPointCommand<'id, T, K>,
    <AccessPointKeys<'id, T> as KeyTypes<'id>>::Identity,
);

/// The access point's issuer of the commands only the backend makes: it
/// mints each installation's identity and payload. The backend creates it
/// with the installation, beside the station's issuer.
pub trait AccessPointCommands<'id> {
    /// The backend's phase types.
    type Types: AccessPointTypes<'id>;
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;

    /// Install `key` for frames with `peer` in the access point's vacant
    /// pairwise `slot`. Returns the command and the identity its installed
    /// key will carry.
    fn install_access_point_pairwise(
        &mut self,
        association: &<AccessPointKeys<'id, Self::Types> as KeyTypes<'id>>::Association,
        slot: <AccessPointKeys<'id, Self::Types> as KeyTypes<'id>>::PairwiseSlot,
        peer: IndividualAddress,
        key: Ccmp128Key,
    ) -> AccessPointPairwiseIssue<'id, Self::Types, Self::Caps>;

    /// Install the access point's group key in its vacant `slot`, consuming
    /// the group permission until the installation's terminal.
    fn install_access_point_group(
        &mut self,
        association: &<AccessPointKeys<'id, Self::Types> as KeyTypes<'id>>::Association,
        permission: <AccessPointKeys<'id, Self::Types> as KeyTypes<'id>>::GroupPermission,
        slot: <AccessPointKeys<'id, Self::Types> as KeyTypes<'id>>::GroupSlot,
        key_id: GroupKeyId,
        key: Ccmp128Key,
    ) -> AccessPointCommand<'id, Self::Types, Self::Caps>;
}

/// The access-point role of a shared port: its entry points beside the
/// station's, over the same physical owner, queues and sink.
pub trait AccessPointPort<'id>:
    DataPort<'id, Types: AccessPointTypes<'id>, Caps: AccessPointCapabilities>
{
    /// The access point's own address, its BSSID.
    fn access_point_address(&self) -> IndividualAddress;

    /// The generation of the port's current installation, which issued its
    /// current beacon permission.
    fn installation_generation(&self) -> u32;

    /// Check that `permission` is the current installation's, lending it for
    /// this borrow of the port. Only this check makes a [`CurrentBeacon`].
    ///
    /// # Errors
    ///
    /// The permission, unchanged, when an earlier installation issued it.
    fn current_beacon(
        &self,
        permission: BeaconPermission<'id, <Self::Types as PortTypes<'id>>::BeaconPath>,
    ) -> Result<
        CurrentBeacon<'_, 'id, <Self::Types as PortTypes<'id>>::BeaconPath>,
        BeaconRefused<'id, <Self::Types as PortTypes<'id>>::BeaconPath>,
    > {
        let installation = self.installation_generation();
        if permission.generation == installation {
            Ok(CurrentBeacon {
                permission,
                port: PhantomData,
            })
        } else {
            Err(BeaconRefused {
                reason: BeaconRefusal::StalePermission {
                    permission: permission.generation,
                    installation,
                },
                permission,
            })
        }
    }

    /// Accept a sealed access-point attempt, dropping its borrow of the
    /// port; [`DataPort::submit`] queues it as the access point's.
    fn admit_access_point<'k, 'p>(
        &self,
        attempt: AccessPointSubmission<'k, 'p, 'id, Self>,
    ) -> Self::Admitted;

    /// Accept a sealed beacon with its current permission;
    /// [`DataPort::submit`] queues it as the access point's, and every end
    /// of it, a refused submission's included, is a
    /// [`TxCompleted::Beacon`](crate::TxCompleted::Beacon) returning the
    /// permission.
    fn admit_beacon<'k, 'p>(&self, beacon: BeaconSubmission<'k, 'p, 'id, Self>) -> Self::Admitted;

    /// Run one access-point command, or hold it at its barrier.
    ///
    /// # Errors
    ///
    /// The backend's contract fault; the command's owners stay with it.
    fn access_point_command(
        &mut self,
        command: AccessPointCommand<'id, Self::Types, Self::Caps>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<AccessPointProgress<'id, Self::Types, Self::Caps>, Self::Fault>;
}

#[cfg(test)]
mod tests;

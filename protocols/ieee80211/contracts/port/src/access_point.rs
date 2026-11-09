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
//! compile, and none of them fits a station operation.
//!
//! Each of these tokens names the installation that issued it
//! ([`IssuedSlot`](crate::IssuedSlot)). After a reinstallation of the same
//! identity a token of the earlier installation still compiles; the port
//! refuses it explicitly, with its owners, at the operation's acceptance
//! point: [`DataPort::submit`] for a beacon and for an attempt under an
//! access-point key, [`AccessPointPort::access_point_command`] for every
//! command. The station's operations refuse them the same way
//! ([`CommandRefusal::StalePermission`](crate::CommandRefusal::StalePermission),
//! [`SubmitRefusal::StalePermission`](crate::SubmitRefusal::StalePermission)).
//!
//! # Acceptance and terminals
//!
//! | Operation | Accepted at | Refused before | Ends with |
//! | --- | --- | --- | --- |
//! | a transmission ([`AccessPointPort::admit_access_point`]) | [`DataPort::submit`] | [`SubmitRefusal`](crate::SubmitRefusal) (a key of an earlier installation: `StalePermission`), its withdrawn terminal in the sink | [`TxCompleted::Single`](crate::TxCompleted::Single) |
//! | a beacon ([`AccessPointPort::admit_beacon`]) | [`DataPort::submit`] | the same (a permission of an earlier installation: `StalePermission`), its terminal returning the permission | [`TxCompleted::Beacon`](crate::TxCompleted::Beacon); `Expired` when not published by its deadline |
//! | a key installation or removal, a configuration with its TSF control ([`AccessPointPort::access_point_command`]) | [`AccessPointProgress::Done`] | [`AccessPointProgress::Refused`] with the command ([`AccessPointRefusal`]) | its terminal in the sink |
//!
//! # Roles stop on their own
//!
//! The access point stops with its own commands: a configuration
//! [`AccessPointReceive::Off`], which ends its queued attempts (a queued
//! beacon included) withdrawn and waits for its published one, the removal
//! of each pairwise key and of its group key, and a configuration that stops
//! its TSF. None of them withdraws, holds or changes the station's attempts,
//! keys or configuration, and an access-point command held at its barrier
//! holds no station command. A confirmed [`DataPort::poll_stop`] stops the
//! shared owner for both roles: every queued attempt ends withdrawn, the
//! access point's interface receives nothing and its TSF runs no more, and
//! each role's permissions stay with their holders.
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
//! use oer_portable_time::Instant;
//! fn twice<'id, T, M: PreparedTx<'id>, N: PreparedTx<'id>, K, X, Path: PoolPath, Q: PoolPath>(
//!     permission: BeaconPermission<'id, Q>,
//!     first: (X, Slot<'id, T, Path>, M, SupportedPower<K>, BeaconStamp<K>, Instant),
//!     second: (X, Slot<'id, T, Path>, N, SupportedPower<K>, BeaconStamp<K>, Instant),
//! ) where
//!     M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X>,
//!     N::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X>,
//! {
//!     let (context, ticket, source, power, stamp, deadline) = first;
//!     let _one = BeaconAttempt { context, ticket, source, power, permission, stamp, deadline }.admit();
//!     let (context, ticket, source, power, stamp, deadline) = second;
//!     let _two = BeaconAttempt { context, ticket, source, power, permission, stamp, deadline }.admit();
//! }
//! ```
//!
//! Its compiling twin admits one beacon with the permission:
//!
//! ```
//! use oer_protocols_ieee80211_contracts_port::*;
//! use oer_protocols_radio_port::{PoolPath, Slot};
//! use oer_portable_time::Instant;
//! fn once<'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath, Q: PoolPath>(
//!     permission: BeaconPermission<'id, Q>,
//!     beacon: (X, Slot<'id, T, Path>, M, SupportedPower<K>, BeaconStamp<K>, Instant),
//! ) where
//!     M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X>,
//! {
//!     let (context, ticket, source, power, stamp, deadline) = beacon;
//!     let _one = BeaconAttempt { context, ticket, source, power, permission, stamp, deadline }.admit();
//! }
//! ```
//!
//! A beacon permission is not made by a caller:
//!
//! ```compile_fail,E0451
//! use oer_protocols_ieee80211_contracts_port::{BeaconPermission, IssuedSlot};
//! fn forge<'id, K>(slot: IssuedSlot<'id, K>) -> BeaconPermission<'id> {
//!     BeaconPermission { slot }
//! }
//! ```

use core::marker::PhantomData;

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use oer_protocols_ieee80211_wire::{
    address::IndividualAddress, ccmp::GroupKeyId, qos::WmmAccessCategory,
};
use oer_protocols_radio_port::{BodyLoan, LocalId, PoolPath, Root, Slot};

use crate::issued::{IssuedSlot, IssuedSlots, SlotGrant};
use crate::{
    AccessPoint, AdmittedAttempt, AdmittedTxParts, CommandTypes, ConfigurePermission, DataPort,
    Ieee80211Capabilities, InFlight, KeyTypes, PortSink, PortTypes, PreparedTx, Protection,
    SupportedFilter, SupportedPower, TxOperation,
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

/// The beacon component of a backend's pool set: its grant issues the
/// installation's one [`BeaconPermission`].
pub struct BeaconPool;

impl oer_protocols_radio_port::PoolSet for BeaconPool {
    type Pools<'id, Path: PoolPath> = BeaconGrant<'id, Path>;

    fn build<'id, Path: PoolPath>(
        builder: oer_protocols_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        BeaconGrant {
            slot: builder.into_pools::<IssuedSlots<BeaconKind, 1>>(),
        }
    }
}

/// The beacon permission a mint made, before its backend issues it.
#[must_use = "the backend issues the grant as its installation's beacon permission"]
pub struct BeaconGrant<'id, Path: PoolPath = Root<LocalId<'id>>> {
    slot: SlotGrant<'id, BeaconKind, 1, Path>,
}

impl<'id, Path: PoolPath> BeaconGrant<'id, Path> {
    /// Issue the permission of the installation `generation`.
    pub fn issue(self, generation: u32) -> BeaconPermission<'id, Path> {
        let [slot] = self.slot.issue(generation);
        BeaconPermission { slot }
    }
}

/// The installation's one permission to have a beacon out: a beacon's
/// admission takes it and its terminal gives it back, so at most one beacon
/// is admitted, queued or on the air. It names the installation that issued
/// it; [`DataPort::submit`] refuses the permission of an earlier
/// installation ([`SubmitRefusal::StalePermission`](crate::SubmitRefusal::StalePermission)).
#[must_use = "the beacon permission returns only with its beacon's terminal"]
pub struct BeaconPermission<'id, Path: PoolPath = Root<LocalId<'id>>> {
    slot: IssuedSlot<'id, BeaconKind, Path>,
}

impl<Path: PoolPath> core::fmt::Debug for BeaconPermission<'_, Path> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("BeaconPermission")
            .field(&self.slot)
            .finish()
    }
}

impl<Path: PoolPath> BeaconPermission<'_, Path> {
    /// The generation of the installation that issued it.
    pub const fn generation(&self) -> u32 {
        self.slot.generation()
    }
}

impl<Path: PoolPath> BeaconPermission<'static, Path> {
    /// Move the same permission across its backend's private body boundary.
    pub fn lend<'p, S>(self, loan: &BodyLoan<'p, Path::Identity, S>) -> BeaconPermission<'p, Path> {
        BeaconPermission {
            slot: self.slot.lend(loan),
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
            slot: self.slot.into_owned(loan),
        }
    }
}

/// Why [`AccessPointPort::access_point_command`] refused a command; nothing
/// changed and the command comes back with every owner.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AccessPointRefusal {
    /// The radio is not running.
    NotEnabled,
    /// A permission, key slot or association of the command was issued by
    /// an earlier installation of the port: final for that token.
    StalePermission {
        /// The token's installation.
        issued: u32,
        /// The port's installation.
        installation: u32,
    },
    /// The command waited at its barrier past the backend's command bound
    /// and did not run: what its barrier began is undone, and the command
    /// comes back with every owner, as
    /// [`CommandRefusal::Expired`](crate::CommandRefusal::Expired).
    Expired,
    /// The backend's access point has no hardware key entry for the group
    /// key id the installation names: final for the command, which comes
    /// back with every owner; the caller issues the installation under
    /// another id.
    UnsupportedGroupKeyId,
}

/// One beacon awaiting admission: an access-point attempt of the frame the
/// caller prepared as its Beacon, with the beacon permission, the Timestamp
/// rule and the instant it stays fresh until. Its access category is
/// [`BEACON_ACCESS_CATEGORY`], its coexistence operation
/// [`TxOperation::Beacon`] and its transmission goes unprotected.
///
/// The caller submits it for its target TBTT and names the next TBTT as its
/// `deadline`: a beacon the port has not published by then ends
/// [`TxOutcome::Expired`](crate::TxOutcome::Expired), its permission and
/// every other owner returned, and nothing of it went on the air. A beacon
/// whose end is unknown ([`TxOutcome::Indeterminate`](crate::TxOutcome::Indeterminate))
/// is not sent again: the caller builds the next TBTT's beacon instead.
pub struct BeaconAttempt<
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
    /// The installation's one beacon permission.
    pub permission: BeaconPermission<'id, BeaconPath>,
    /// Who writes its Timestamp field.
    pub stamp: BeaconStamp<K>,
    /// The port's instant it must be published by: the next TBTT.
    pub deadline: Instant,
}

impl<'id, T, M, K, X, Path: PoolPath, BeaconPath: PoolPath>
    BeaconAttempt<'id, T, M, K, X, Path, BeaconPath>
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
            permission,
            stamp,
            deadline,
        } = self;
        AdmittedBeacon {
            attempt: crate::Attempt {
                context,
                ticket,
                source,
                access_category: BEACON_ACCESS_CATEGORY,
                protection: Protection::None,
                power,
                coex_operation: TxOperation::Beacon,
            }
            .admit(),
            permission,
            stamp,
            deadline,
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
    deadline: Instant,
}

/// What a backend takes from a sealed beacon.
pub struct BeaconParts<'id, C, T, P, E, K, BeaconPath: PoolPath> {
    /// The beacon's attempt.
    pub attempt: AdmittedTxParts<C, T, P, E, K>,
    /// The permission, which its terminal returns.
    pub permission: BeaconPermission<'id, BeaconPath>,
    /// Who writes its Timestamp field.
    pub stamp: BeaconStamp<K>,
    /// The port's instant it must be published by.
    pub deadline: Instant,
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

    /// The port's instant it must be published by.
    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Transfer the whole beacon through the backend's private frontend.
    pub fn into_parts(
        self,
        loan: &BodyLoan<'id, Path::Identity, M::Backend>,
    ) -> SealedBeaconParts<'id, T, M, K, Path, BeaconPath> {
        BeaconParts {
            attempt: self.attempt.into_parts(loan),
            permission: self.permission,
            stamp: self.stamp,
            deadline: self.deadline,
        }
    }
}

/// What a backend takes from a sealed beacon of the preparation `M`.
pub type SealedBeaconParts<'id, T, M, K, Path, BeaconPath> = BeaconParts<
    'id,
    <M as PreparedTx<'id>>::OwnedContext,
    Slot<'static, T, Path>,
    <M as PreparedTx<'id>>::OwnedPayload,
    <M as PreparedTx<'id>>::Execution,
    K,
    BeaconPath,
>;

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
        /// A restart or stop of its TSF, after the receive rules took
        /// effect.
        tsf: Option<SupportedTsfControl<K>>,
    },
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
}

/// A failed [`AccessPointPort::access_point_command`] of the port `P`.
pub type AccessPointCommandFailed<'id, P> = crate::CommandFailed<
    AccessPointCommand<'id, <P as DataPort<'id>>::Types, <P as DataPort<'id>>::Caps>,
    <P as DataPort<'id>>::Fault,
>;

/// How far [`AccessPointPort::access_point_command`] took one command.
pub enum AccessPointProgress<'id, T: AccessPointTypes<'id>, K: Ieee80211Capabilities> {
    /// The command ran; its terminal, if it has one, is in the sink.
    Done,
    /// The command waits at its barrier; the caller runs it again, before
    /// any later command of the access point, on a later call.
    Held(AccessPointCommand<'id, T, K>),
    /// The port cannot run the command; nothing changed.
    Refused(AccessPointCommand<'id, T, K>, AccessPointRefusal),
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

    /// Accept a sealed access-point attempt, dropping its borrow of the
    /// port; [`DataPort::submit`] queues it as the access point's.
    fn admit_access_point<'k, 'p>(
        &self,
        attempt: AccessPointSubmission<'k, 'p, 'id, Self>,
    ) -> Self::Admitted;

    /// Accept a sealed beacon with its permission; [`DataPort::submit`]
    /// queues it as the access point's, or refuses it, its permission an
    /// earlier installation's, as
    /// [`SubmitRefusal::StalePermission`](crate::SubmitRefusal::StalePermission).
    /// Every end of it, a refused submission's included, is a
    /// [`TxCompleted::Beacon`](crate::TxCompleted::Beacon) returning the
    /// permission.
    fn admit_beacon<'k, 'p>(&self, beacon: BeaconSubmission<'k, 'p, 'id, Self>) -> Self::Admitted;

    /// Run one access-point command, or hold it at its barrier.
    ///
    /// # Errors
    ///
    /// The backend's contract fault, with the command when the port did
    /// not take its owners ([`CommandFailed`](crate::CommandFailed)).
    fn access_point_command(
        &mut self,
        command: AccessPointCommand<'id, Self::Types, Self::Caps>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<
        AccessPointProgress<'id, Self::Types, Self::Caps>,
        AccessPointCommandFailed<'id, Self>,
    >;
}

#[cfg(test)]
mod tests;

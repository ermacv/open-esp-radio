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
use oer_protocols_radio_port::{BodyLoan, LocalId, PoolPath, Root, Slot, Slots};

use crate::{AccessPoint, Ieee80211Capabilities, SupportedFilter};

/// The access category of every beacon: a beacon contends as voice, the
/// first slice's policy taken from the archive's access point.
pub const BEACON_ACCESS_CATEGORY: WmmAccessCategory = WmmAccessCategory::Voice;

/// Who writes a beacon's Timestamp field (IEEE Std 802.11-2020 9.4.1.10).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BeaconTimestamp {
    /// The transmitter writes the interface's TSF into the field as the
    /// beacon goes out; what the caller wrote there is replaced on the air.
    Transmitter,
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

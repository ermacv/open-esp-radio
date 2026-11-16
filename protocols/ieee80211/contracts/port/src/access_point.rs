//! The access point's values: what its MAC does with a beacon's timestamp
//! and its TSF, and the receive rules of its interface. Its calls are the
//! data path's, with its own places; their list is fixed with the access
//! point's controller.

use core::marker::PhantomData;

use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

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

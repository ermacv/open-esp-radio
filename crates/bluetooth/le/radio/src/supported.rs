//! Values checked against a backend's capabilities when they are built:
//! [`Supported`].

use core::marker::PhantomData;

use crate::{
    AcceptListChange, AdvertisingConfiguration, AdvertisingEvent, AdvertisingReception,
    ConnectionConfiguration, ConnectionEvent, ConnectionEventTiming, DataPdu, LeCapabilities,
    LeInstant, LeRadioCapabilities, LeWindow, OutsideEpoch, RadioDuration, RadioTiming,
    ScanFilterPolicy, ScanType, ScanWindow, ScannerConfiguration, TestReceive, TestTransmit,
    TxPower,
};

/// Header type of `ADV_IND`.
const ADV_IND: u8 = 0x0;
/// Header type of `ADV_DIRECT_IND`.
const ADV_DIRECT_IND: u8 = 0x1;
/// Header type of `ADV_NONCONN_IND`.
const ADV_NONCONN_IND: u8 = 0x2;
/// Header type of `ADV_SCAN_IND`.
const ADV_SCAN_IND: u8 = 0x6;
/// The header bits a legacy advertising PDU keeps clear: the RFU bit.
const RESERVED_HEADER_BITS: u8 = 0x10;
/// The highest connection event scheduling priority.
const MAX_PRIORITY: u8 = 15;
/// The longest LE Test packet payload.
pub(crate) const TEST_PAYLOAD_MAX: usize = 255;

/// Values checked against a backend's capabilities when they are built.
///
/// A Controller builds every value it submits as a [`Supported`] of its
/// capabilities marker `K`: the check runs once, against `K::CAPS`, and
/// the backend refuses nothing the capabilities exclude. A timed value is
/// also placed: each of its reservations comes from the one reservation
/// function both the Controller and the backend use
/// ([`RadioTiming::reservation`]) and ends within the capabilities'
/// horizon of the instant it was placed at, so the backend never meets a
/// window it cannot schedule.
///
/// It is neither `Clone`, `Copy` nor `Default` and has no
/// public constructor but [`Supported::new`] and [`Supported::placed`]:
///
/// ```compile_fail,E0451
/// # use core::marker::PhantomData;
/// # use oer_bluetooth_radio::{NoCapabilities, Supported, TxPower};
/// let forged: Supported<NoCapabilities, TxPower> =
///     Supported { value: TxPower::from_dbm(0), caps: PhantomData };
/// ```
///
/// ```compile_fail,E0599
/// # use oer_bluetooth_radio::{Supported, TxPower, LeCapabilities};
/// fn copy<K: LeCapabilities>(power: &Supported<K, TxPower>) -> Supported<K, TxPower> {
///     power.clone()
/// }
/// ```
///
/// ```compile_fail,E0277
/// # use oer_bluetooth_radio::{NoCapabilities, Supported, TxPower};
/// let _: Supported<NoCapabilities, TxPower> = Default::default();
/// ```
///
/// Nor is a value that occupies the air built without its placement:
///
/// ```compile_fail,E0599
/// # use oer_bluetooth_radio::{LeCapabilities, ScanWindow, Supported};
/// fn unplaced<K: LeCapabilities>(window: ScanWindow) -> Supported<K, ScanWindow> {
///     Supported::new(window).unwrap()
/// }
/// ```
///
/// Its compiling twin builds the value through the check, and a timed one
/// placed:
///
/// ```
/// # use oer_bluetooth_radio::{LeCapabilities, Supported, TxPower};
/// fn build<K: LeCapabilities>(dbm: i8) -> Option<Supported<K, TxPower>> {
///     Supported::new(TxPower::from_dbm(dbm)).ok()
/// }
/// # use oer_bluetooth_radio::{LeInstant, ScanWindow};
/// fn place<K: LeCapabilities>(window: ScanWindow, now: LeInstant) -> Option<Supported<K, ScanWindow>> {
///     Supported::placed(window, now).ok()
/// }
/// ```
pub struct Supported<K: LeCapabilities, T> {
    value: T,
    caps: PhantomData<fn() -> K>,
}

impl<K: LeCapabilities, T: core::fmt::Debug> core::fmt::Debug for Supported<K, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.value.fmt(f)
    }
}

impl<K: LeCapabilities, T: PartialEq> PartialEq for Supported<K, T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<K: LeCapabilities, T: Eq> Eq for Supported<K, T> {}

impl<K: LeCapabilities, T: Untimed> Supported<K, T> {
    /// `value`, when `K::CAPS` serve it. A value that occupies the air is
    /// built only placed ([`Self::placed`]).
    ///
    /// # Errors
    ///
    /// [`Unplaced::Unsupported`]: the capabilities exclude the value.
    pub fn new(value: T) -> Result<Self, Unplaced> {
        if !value.supported(&K::CAPS) {
            return Err(Unplaced::Unsupported);
        }
        Ok(Self {
            value,
            caps: PhantomData,
        })
    }
}

impl<K: LeCapabilities, T: Timed> Supported<K, T> {
    /// `value` placed at `now`: the capabilities serve it, and each of its
    /// reservations lies in the radio epoch and ends within
    /// `K::CAPS.max_ahead` of `now`.
    ///
    /// # Errors
    ///
    /// Which of those does not hold.
    pub fn placed(value: T, now: LeInstant) -> Result<Self, Unplaced> {
        if !value.supported(&K::CAPS) {
            return Err(Unplaced::Unsupported);
        }
        for air in value.air_windows(&K::CAPS.timing)?.into_iter().flatten() {
            let reservation = K::CAPS.timing.reservation(air)?;
            let horizon = reservation
                .end()
                .checked_duration_since(now)
                .ok_or(Unplaced::BeyondHorizon)?;
            if horizon > K::CAPS.max_ahead {
                return Err(Unplaced::BeyondHorizon);
            }
        }
        Ok(Self {
            value,
            caps: PhantomData,
        })
    }
}

impl<K: LeCapabilities, T> Supported<K, T> {
    /// The checked value.
    pub const fn get(&self) -> &T {
        &self.value
    }
}

/// Why a value cannot be built as [`Supported`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Unplaced {
    /// The capabilities exclude it.
    Unsupported,
    /// A reservation ends past the capabilities' horizon, or before the
    /// instant it was placed at.
    BeyondHorizon,
    /// A reservation lies outside the radio epoch.
    OutsideEpoch,
}

impl From<OutsideEpoch> for Unplaced {
    fn from(OutsideEpoch: OutsideEpoch) -> Self {
        Self::OutsideEpoch
    }
}

/// A value a backend's capabilities serve or exclude.
pub trait Capability {
    /// Whether `caps` serve this value.
    fn supported(&self, caps: &LeRadioCapabilities) -> bool;
}

/// A value that occupies no air, built with [`Supported::new`].
pub trait Untimed: Capability {}

/// A value that occupies the air, built only placed with
/// [`Supported::placed`]: what it reserves.
pub trait Timed: Capability {
    /// Its air windows, each of which the backend reserves from
    /// [`RadioTiming::reservation`]; at most three, for an advertising
    /// event's channels.
    ///
    /// # Errors
    ///
    /// A window lies outside the radio epoch.
    fn air_windows(&self, timing: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced>;
}

impl Untimed for TxPower {}

impl Capability for TxPower {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        self.dbm() >= caps.min_tx_power_dbm
    }
}

impl Untimed for AdvertisingConfiguration<'_> {}

impl Capability for AdvertisingConfiguration<'_> {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        let header = self.pdu.header();
        let kind = match (header & 0x0f, self.reception) {
            (ADV_NONCONN_IND, AdvertisingReception::None) => caps.legacy_advertising.nonconnectable,
            (
                ADV_IND | ADV_SCAN_IND,
                AdvertisingReception::ScanResponse(_) | AdvertisingReception::Report,
            )
            | (ADV_DIRECT_IND, AdvertisingReception::Report) => caps.legacy_advertising.responsive,
            _ => false,
        };
        kind && header & RESERVED_HEADER_BITS == 0
            && caps.phys.contains(self.phy)
            && self.tx_power.supported(caps)
    }
}

impl Capability for AdvertisingEvent {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        caps.legacy_advertising.any() && self.channel_spacing > caps.timing.preparation_lead
    }
}

impl Timed for AdvertisingEvent {
    fn air_windows(&self, timing: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced> {
        let mut windows = [None; 3];
        let span = self
            .channel_spacing
            .checked_sub(timing.preparation_lead)
            .ok_or(Unplaced::Unsupported)?;
        for (position, window) in windows.iter_mut().enumerate() {
            let Some((_, anchor)) = self.channel_anchor(position)? else {
                break;
            };
            *window = Some(LeWindow::new(anchor, span).map_err(|_| Unplaced::OutsideEpoch)?);
        }
        Ok(windows)
    }
}

impl Untimed for ScannerConfiguration {}

impl Capability for ScannerConfiguration {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        let scan_type = match self.scan_type {
            ScanType::Passive => caps.passive_scanning,
            ScanType::Active => caps.active_scanning,
        };
        let filter = match self.filter_policy {
            ScanFilterPolicy::AcceptAll => true,
            ScanFilterPolicy::AcceptListOnly => caps.filter_accept_list,
        };
        scan_type && filter && caps.phys.contains(self.phy) && self.tx_power.supported(caps)
    }
}

impl Capability for ScanWindow {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        caps.scanning()
    }
}

impl Timed for ScanWindow {
    fn air_windows(&self, _: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced> {
        Ok([Some(self.window), None, None])
    }
}

impl Untimed for ConnectionConfiguration {}

impl Capability for ConnectionConfiguration {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        caps.peripheral_connection.is_some()
            && caps.phys.contains(self.phy)
            && self.tx_power.supported(caps)
    }
}

impl Capability for ConnectionEvent {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        let Some(connection) = caps.peripheral_connection else {
            return false;
        };
        let wait = match self.timing {
            ConnectionEventTiming::First {
                transmit_window,
                timing_guard,
            } => {
                transmit_window > RadioDuration::ZERO
                    && timing_guard
                        .checked_mul(2)
                        .and_then(|guards| guards.checked_add(transmit_window))
                        .is_some_and(|wait| wait <= connection.max_first_receive_wait)
            }
            ConnectionEventTiming::Recurring { receive_wait, .. } => {
                receive_wait <= connection.max_receive_wait
            }
        };
        // The event's span lasts at most its interval and the widening:
        // within the horizon, as every duration a backend counts.
        let widening = match self.timing {
            ConnectionEventTiming::First { timing_guard, .. } => timing_guard,
            ConnectionEventTiming::Recurring { widening, .. } => widening,
        };
        let span = self.interval > RadioDuration::ZERO
            && self
                .interval
                .checked_add(widening)
                .is_some_and(|span| span <= caps.max_ahead);
        wait && span && self.priority <= MAX_PRIORITY
    }
}

impl Timed for ConnectionEvent {
    fn air_windows(&self, _: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced> {
        Ok([Some(self.window), None, None])
    }
}

impl Untimed for DataPdu<'_> {}

impl Capability for DataPdu<'_> {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        caps.peripheral_connection.is_some_and(|connection| {
            self.payload().len() <= usize::from(connection.max_data_payload)
        })
    }
}

impl Capability for TestTransmit<'_> {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        self.payload.len() <= TEST_PAYLOAD_MAX
            && caps.direct_test_mode
            && caps.test_phys.contains(self.phy.phy())
            && self.tx_power.supported(caps)
    }
}

impl Timed for TestTransmit<'_> {
    fn air_windows(&self, _: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced> {
        Ok([Some(self.window), None, None])
    }
}

impl Capability for TestReceive {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        caps.direct_test_mode
            && caps.test_phys.contains(self.phy.phy())
            && self.tx_power.supported(caps)
    }
}

impl Timed for TestReceive {
    fn air_windows(&self, _: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced> {
        Ok([Some(self.window), None, None])
    }
}

impl Untimed for AcceptListChange {}

impl Capability for AcceptListChange {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        caps.filter_accept_list
    }
}

#[cfg(test)]
mod tests;

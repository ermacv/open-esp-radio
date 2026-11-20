//! Values checked against a backend's capabilities when they are built:
//! [`Supported`].

use core::marker::PhantomData;

use crate::{
    AdvertisingData, AdvertisingEvent, AdvertisingFilterPolicy, AdvertisingReception,
    AdvertisingSet, ConnectionEvent, ConnectionEventTiming, ConnectionSetup, LeCapabilities,
    LeConnectionCapabilities, LeInstant, LeWindow, OutsideEpoch, RadioDuration, RadioTiming,
    ScanFilterPolicy, ScanType, ScanWindow, ScannerSetup, TestWindow, TxPower, dtm::DtmTest,
};
use oer_protocols_bluetooth_le_wire::advertising_header::{
    AdvertisingPduType, LegacyAdvertisingPduPrefix,
};

/// Values checked against a backend's capabilities when they are built.
///
/// A Controller builds every value it submits as a [`Supported`] of its
/// capabilities marker `K`: the check runs once, against `K::CAPS`, and
/// the backend refuses nothing the capabilities exclude. What the
/// capabilities decide alone (a role they do not serve) is a constant
/// check of the value's kind: a value of a role `K` does not serve does not
/// build. A timed value is
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
/// # use oer_protocols_bluetooth_le_contracts_port::{NoCapabilities, Supported, TxPower};
/// let forged: Supported<NoCapabilities, TxPower> =
///     Supported { value: todo!(), caps: PhantomData };
/// ```
///
/// ```compile_fail,E0599
/// # use oer_protocols_bluetooth_le_contracts_port::{Supported, TxPower, LeCapabilities};
/// fn copy<K: LeCapabilities>(power: &Supported<K, TxPower>) -> Supported<K, TxPower> {
///     power.clone()
/// }
/// ```
///
/// ```compile_fail,E0277
/// # use oer_protocols_bluetooth_le_contracts_port::{NoCapabilities, Supported, TxPower};
/// let _: Supported<NoCapabilities, TxPower> = Default::default();
/// ```
///
/// Nor is a value that occupies the air built without its placement:
///
/// ```compile_fail,E0599
/// # use oer_protocols_bluetooth_le_contracts_port::{LeCapabilities, ScanWindow, Supported};
/// fn unplaced<K: LeCapabilities>(window: ScanWindow) -> Supported<K, ScanWindow> {
///     Supported::new(window).unwrap()
/// }
/// ```
///
/// Its compiling twin builds the value through the check, and a timed one
/// placed:
///
/// ```
/// # use oer_protocols_bluetooth_le_contracts_port::{LeCapabilities, Supported, TxPower};
/// # use oer_protocols_bluetooth_le_contracts_port::LeTxCeiling;
/// fn build<K: LeCapabilities>(ceiling: LeTxCeiling, dbm: i8) -> Option<Supported<K, TxPower>> {
///     Supported::new(TxPower::within(ceiling, dbm).ok()?).ok()
/// }
/// # use oer_protocols_bluetooth_le_contracts_port::{LeInstant, ScanWindow};
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

impl<K: LeCapabilities, T: Untimed<K>> Supported<K, T> {
    /// `value`, when `K::CAPS` serve it. A value that occupies the air is
    /// built only placed ([`Self::placed`]).
    ///
    /// # Errors
    ///
    /// [`Unplaced::Unsupported`]: the capabilities exclude the value.
    pub fn new(value: T) -> Result<Self, Unplaced> {
        if !value.supported() {
            return Err(Unplaced::Unsupported);
        }
        Ok(Self {
            value,
            caps: PhantomData,
        })
    }
}

impl<K: LeCapabilities, T: Timed<K>> Supported<K, T> {
    /// `value` placed at `now`: the capabilities serve it, and each of its
    /// reservations lies in the radio epoch and ends within
    /// `K::CAPS.max_ahead` of `now`.
    ///
    /// # Errors
    ///
    /// Which of those does not hold.
    pub fn placed(value: T, now: LeInstant) -> Result<Self, Unplaced> {
        if !value.supported() {
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

/// A value the capabilities `K` serve or exclude.
pub trait Capability<K: LeCapabilities> {
    /// Whether `K` serve this value.
    fn supported(&self) -> bool;
}

/// A value that occupies no air, built with [`Supported::new`].
pub trait Untimed<K: LeCapabilities>: Capability<K> {}

/// A value that occupies the air, built only placed with
/// [`Supported::placed`]: what it reserves.
pub trait Timed<K: LeCapabilities>: Capability<K> {
    /// Its air windows, each of which the backend reserves from
    /// [`RadioTiming::reservation`]; at most three, for an advertising
    /// event's channels.
    ///
    /// # Errors
    ///
    /// A window lies outside the radio epoch.
    fn air_windows(&self, timing: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced>;
}

/// The connections of the capabilities `K`.
struct Connections<K>(PhantomData<fn() -> K>);

impl<K: LeCapabilities> Connections<K> {
    /// `K`'s connections, or a failed build where `K` serves none.
    const SERVED: LeConnectionCapabilities = match K::CAPS.peripheral_connection {
        Some(connection) => connection,
        None => panic!("a connection's value needs capabilities that serve connections"),
    };
}

impl<K: LeCapabilities> Untimed<K> for TxPower {}

impl<K: LeCapabilities> Capability<K> for TxPower {
    fn supported(&self) -> bool {
        self.dbm() >= K::CAPS.min_tx_power_dbm
    }
}

impl<K: LeCapabilities> Untimed<K> for AdvertisingData<'_> {}

impl<K: LeCapabilities> Capability<K> for AdvertisingData<'_> {
    fn supported(&self) -> bool {
        const {
            if !K::CAPS.legacy_advertising.any() {
                panic!("advertising needs capabilities that serve it");
            }
        };
        let caps = K::CAPS;
        let Ok(advertisement) = LegacyAdvertisingPduPrefix::decode(self.pdu.bytes()) else {
            return false;
        };
        let header = advertisement.header;
        let kind = match (header.pdu_type(), self.reception) {
            (AdvertisingPduType::AdvNonconnInd, AdvertisingReception::None) => {
                caps.legacy_advertising.nonconnectable
            }
            (
                AdvertisingPduType::AdvInd | AdvertisingPduType::AdvScanInd,
                AdvertisingReception::ScanResponse(_) | AdvertisingReception::Report,
            )
            | (AdvertisingPduType::AdvDirectInd, AdvertisingReception::Report) => {
                caps.legacy_advertising.responsive
            }
            _ => false,
        };
        kind && !header.reserved()
    }
}

impl<K: LeCapabilities> Untimed<K> for AdvertisingSet<'_> {}

impl<K: LeCapabilities> Capability<K> for AdvertisingSet<'_> {
    fn supported(&self) -> bool {
        let filter = match self.filter_policy {
            AdvertisingFilterPolicy::AcceptAll => true,
            AdvertisingFilterPolicy::ScanAcceptList
            | AdvertisingFilterPolicy::ConnectAcceptList
            | AdvertisingFilterPolicy::BothAcceptList => K::CAPS.filter_accept_list,
        };
        Capability::<K>::supported(&self.data)
            && filter
            && K::CAPS.phys.contains(self.phy)
            && Capability::<K>::supported(&self.tx_power)
    }
}

impl<K: LeCapabilities> Capability<K> for AdvertisingEvent<K> {
    fn supported(&self) -> bool {
        const {
            if !K::CAPS.legacy_advertising.any() {
                panic!("advertising needs capabilities that serve it");
            }
        };
        true
    }
}

impl<K: LeCapabilities> Timed<K> for AdvertisingEvent<K> {
    fn air_windows(&self, _: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced> {
        let mut windows = [None; 3];
        let span = self.channel_spacing.listening().get();
        for (position, window) in windows.iter_mut().enumerate() {
            let Some((_, anchor)) = self.channel_anchor(position)? else {
                break;
            };
            *window = Some(LeWindow::new(anchor, span).map_err(|_| Unplaced::OutsideEpoch)?);
        }
        Ok(windows)
    }
}

impl<K: LeCapabilities> Untimed<K> for ScannerSetup {}

impl<K: LeCapabilities> Capability<K> for ScannerSetup {
    fn supported(&self) -> bool {
        const {
            if !K::CAPS.scanning() {
                panic!("a scanner needs capabilities that serve scanning");
            }
        };
        let caps = K::CAPS;
        let scan_type = match self.scan_type {
            ScanType::Passive => caps.passive_scanning,
            ScanType::Active => caps.active_scanning,
        };
        let filter = match self.filter_policy {
            ScanFilterPolicy::AcceptAll => true,
            ScanFilterPolicy::AcceptListOnly => caps.filter_accept_list,
        };
        scan_type
            && filter
            && caps.phys.contains(self.phy)
            && Capability::<K>::supported(&self.tx_power)
    }
}

impl<K: LeCapabilities> Capability<K> for ScanWindow {
    fn supported(&self) -> bool {
        const {
            if !K::CAPS.scanning() {
                panic!("a scan window needs capabilities that serve scanning");
            }
        };
        true
    }
}

impl<K: LeCapabilities> Timed<K> for ScanWindow {
    fn air_windows(&self, _: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced> {
        Ok([Some(self.window), None, None])
    }
}

impl<K: LeCapabilities> Untimed<K> for ConnectionSetup {}

impl<K: LeCapabilities> Capability<K> for ConnectionSetup {
    fn supported(&self) -> bool {
        const { Connections::<K>::SERVED };
        K::CAPS.phys.contains(self.phy) && Capability::<K>::supported(&self.tx_power)
    }
}

impl<K: LeCapabilities> Capability<K> for ConnectionEvent {
    fn supported(&self) -> bool {
        let connection = const { Connections::<K>::SERVED };
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
                .is_some_and(|span| span <= K::CAPS.max_ahead);
        wait && span
    }
}

impl<K: LeCapabilities> Timed<K> for ConnectionEvent {
    fn air_windows(&self, _: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced> {
        Ok([Some(self.window), None, None])
    }
}

impl<K: LeCapabilities> Untimed<K> for DtmTest {}

impl<K: LeCapabilities> Capability<K> for DtmTest {
    fn supported(&self) -> bool {
        const {
            if !K::CAPS.direct_test_mode {
                panic!("a test needs capabilities that serve Direct Test Mode");
            }
        };
        let caps = K::CAPS;
        match *self {
            DtmTest::Transmit { phy, power, .. } => {
                caps.test_phys.contains(phy.phy()) && Capability::<K>::supported(&power)
            }
            DtmTest::Receive { phy, .. } => caps.test_phys.contains(phy.phy()),
        }
    }
}

impl<K: LeCapabilities> Capability<K> for TestWindow {
    fn supported(&self) -> bool {
        const {
            if !K::CAPS.direct_test_mode {
                panic!("a test window needs capabilities that serve Direct Test Mode");
            }
        };
        true
    }
}

impl<K: LeCapabilities> Timed<K> for TestWindow {
    fn air_windows(&self, _: &RadioTiming) -> Result<[Option<LeWindow>; 3], Unplaced> {
        Ok([Some(self.window), None, None])
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests;

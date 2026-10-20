//! The station's power-management state as its access point sees it: the
//! first decision of `modem_sleep`'s portable decision half, which its split
//! takes over without a second copy.
//!
//! A station tells its access point whether it sleeps with the PM bit of a
//! Null. A Null's retries are its retry plan in the driver
//! ([`NULL_TRANSMISSIONS`]); one the plan could not deliver leaves the two
//! sides apart. The decision here closes the gap the one way the station
//! can observe it: awake, it sent PM=0 that never arrived, so the access
//! point may still buffer its frames, and a beacon whose TIM shows the
//! station's association identifier says it does: the station sends PM=0
//! again. The beacon is read with
//! [`parse_sta_beacon`](oer_ieee80211_wire::station_beacon::parse_sta_beacon).

use core::num::NonZeroU8;

use oer_ieee80211_wire::station_beacon::StaTimObservation;
use oer_ieee80211_wire::station_power_save::StaPowerManagement;

/// The transmissions a Null's retry plan makes: the vendor's limit of
/// Null retries, plus the first.
pub const NULL_TRANSMISSIONS: NonZeroU8 = {
    #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
    const fn checked() -> NonZeroU8 {
        match NonZeroU8::MIN.checked_add(crate::modem_sleep::NULL_RETRY_LIMIT) {
            Some(count) => count,
            None => panic!("a Null's retry limit plus its first transmission must fit"),
        }
    }
    checked()
};

/// What the Null's completed retry plan established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NullOutcome {
    /// The access point acknowledged the Null.
    Delivered,
    /// Delivery was not confirmed, including a local preparation refusal.
    Unconfirmed,
}

/// What the station knows of its access point's view of its power state.
///
/// A Null completion carries an explicit outcome rather than a boolean:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_sta::power_save::PowerSaveView;
/// use oer_ieee80211_wire::station_power_save::StaPowerManagement;
/// fn sent(view: &mut PowerSaveView) {
///     view.null_sent(StaPowerManagement::Active, false);
/// }
/// ```
///
/// Confirmed and unconfirmed transitions retain the access point's view:
///
/// ```
/// use oer_ieee80211_sta::power_save::{NullOutcome, PowerSaveView};
/// use oer_ieee80211_wire::station_power_save::StaPowerManagement;
/// let mut view = PowerSaveView::Awake;
/// view.null_sent(StaPowerManagement::PowerSave, NullOutcome::Delivered);
/// view.null_sent(StaPowerManagement::Active, NullOutcome::Unconfirmed);
/// assert_eq!(view, PowerSaveView::AwakeUnconfirmed);
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PowerSaveView {
    /// Both ends know that the station is awake.
    #[default]
    Awake,
    /// The station sleeps; a PM=1 Null may have reached the access point.
    Asleep,
    /// The access point may think the station sleeps: a PM=1 Null may have
    /// arrived, and the awake station has no acknowledgement of PM=0 since.
    AwakeUnconfirmed,
}

/// What the station does about its access point's view.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerSaveDecision {
    /// Nothing.
    Keep,
    /// Send a Null with PM=0 again: the access point buffers frames for an
    /// awake station.
    SendActive,
}

impl PowerSaveView {
    /// The station sent a Null with `power_management`, which its retry
    /// plan delivered or not.
    pub fn null_sent(&mut self, power_management: StaPowerManagement, outcome: NullOutcome) {
        *self = match (power_management, outcome, *self) {
            (StaPowerManagement::PowerSave, _, _) => {
                // Even an unacknowledged PM=1 may have reached the access
                // point: its ACK alone may have been lost.
                Self::Asleep
            }
            (StaPowerManagement::Active, NullOutcome::Delivered, _)
            | (StaPowerManagement::Active, _, Self::Awake) => Self::Awake,
            (StaPowerManagement::Active, NullOutcome::Unconfirmed, _) => Self::AwakeUnconfirmed,
        };
    }

    /// A beacon of the station's BSS with `tim`: what the station does.
    pub fn beacon(&self, tim: Option<StaTimObservation>) -> PowerSaveDecision {
        match tim {
            Some(tim) if *self == Self::AwakeUnconfirmed && tim.unicast_buffered => {
                PowerSaveDecision::SendActive
            }
            _ => PowerSaveDecision::Keep,
        }
    }

    /// Whether the station sleeps.
    pub const fn asleep(&self) -> bool {
        matches!(self, Self::Asleep)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tim(unicast_buffered: bool) -> Option<StaTimObservation> {
        Some(StaTimObservation {
            dtim_count: 0,
            dtim_period: 1,
            unicast_buffered,
            group_buffered: false,
        })
    }

    #[test]
    fn an_undelivered_active_null_is_sent_again_when_the_tim_shows_our_frames() {
        let mut view = PowerSaveView::Awake;
        view.null_sent(StaPowerManagement::PowerSave, NullOutcome::Delivered);
        view.null_sent(StaPowerManagement::Active, NullOutcome::Unconfirmed);
        // The access point buffers for the station: send PM=0 again.
        assert_eq!(view.beacon(tim(true)), PowerSaveDecision::SendActive);
        // A TIM without the station's bit says nothing.
        assert_eq!(view.beacon(tim(false)), PowerSaveDecision::Keep);
        // Delivered at last: the two sides agree.
        view.null_sent(StaPowerManagement::Active, NullOutcome::Delivered);
        assert_eq!(view.beacon(tim(true)), PowerSaveDecision::Keep);
    }

    #[test]
    fn a_sleeping_station_leaves_its_buffered_frames_to_its_wake() {
        let mut view = PowerSaveView::Awake;
        view.null_sent(StaPowerManagement::PowerSave, NullOutcome::Unconfirmed);
        assert!(view.asleep());
        assert_eq!(view.beacon(tim(true)), PowerSaveDecision::Keep);
    }

    #[test]
    fn an_unconfirmed_active_null_does_not_invent_a_prior_sleep() {
        let mut view = PowerSaveView::Awake;
        view.null_sent(StaPowerManagement::Active, NullOutcome::Unconfirmed);
        assert_eq!(view, PowerSaveView::Awake);
        assert_eq!(view.beacon(tim(true)), PowerSaveDecision::Keep);
    }
}

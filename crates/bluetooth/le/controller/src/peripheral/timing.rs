//! Connection event geometry over the backend's connection allowances.
//!
//! The Link Layer anchors every event on the central's first packet. Until a
//! packet has been received the anchor is the start of a transmit window of
//! uncertain position, which recurs at every interval. Clock drift widens the
//! receive window by the sum of both sleep-clock accuracies over the time since
//! the last received anchor (Core Vol 6 Part B 4.2.4); the backend adds its
//! jitter and guards as [`ConnectionAllowances`] describes.

use oer_bluetooth_radio::LeRadio;
use oer_bluetooth_radio::{
    ConnectionAllowances, ConnectionEventTiming, LeInstant, LeWindow, RadioDuration,
};

use crate::{EpochExhausted, PlanningCalculation as C, PlanningOperation, PlanningRole};

/// The anchor phase of a connection between events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Phase {
    /// The last anchor a received packet established; drift accumulates from
    /// here.
    pub(crate) reference: LeInstant,
    /// From the reference to the nominal anchor of the last event.
    pub(crate) since_reference: RadioDuration<LeRadio>,
    /// Width of the transmit window still uncertain at the anchor, zero once a
    /// packet fixed the anchor.
    pub(crate) transmit_window: RadioDuration<LeRadio>,
}

impl Phase {
    /// The phase of an anchor a received packet just established, or of the
    /// first anchor with its transmit window.
    pub(crate) const fn at(anchor: LeInstant, transmit_window: RadioDuration<LeRadio>) -> Self {
        Self {
            reference: anchor,
            since_reference: RadioDuration::from_micros(0),
            transmit_window,
        }
    }
}

/// One planned connection event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Plan {
    pub(crate) anchor: LeInstant,
    /// From the phase's reference to `anchor`.
    pub(crate) since_reference: RadioDuration<LeRadio>,
    pub(crate) window: LeWindow,
    pub(crate) timing: ConnectionEventTiming,
    pub(crate) transmit_window: RadioDuration<LeRadio>,
}

/// The first event, anchored at the start of its transmit window.
///
/// # Errors
///
/// Its window lies outside the radio epoch.
// CAPABILITY: bluetooth-first-peripheral-connection-window
pub(crate) fn first(
    anchor: LeInstant,
    transmit_window: RadioDuration<LeRadio>,
    allowances: ConnectionAllowances,
) -> Result<Plan, EpochExhausted> {
    // A span past `u64::MAX` cannot lie in the epoch either.
    let outside = error(C::WindowGeometry);
    let lead = allowances
        .boundary_guard
        .checked_add(allowances.first_event_guard)
        .ok_or(outside)?;
    let start = anchor.checked_sub(lead.get()).ok_or(outside)?;
    let duration = lead
        .checked_add(transmit_window)
        .and_then(|span| span.checked_add(allowances.first_event_length))
        .ok_or(outside)?;
    Ok(Plan {
        anchor,
        since_reference: RadioDuration::from_micros(0),
        window: LeWindow::nonempty(start, duration).ok_or(outside)?,
        timing: ConnectionEventTiming::First {
            transmit_window,
            timing_guard: allowances.first_event_guard,
        },
        transmit_window,
    })
}

/// A later event `since_reference` after the phase's reference, widened by
/// the drift accumulated since then, with an uncertain transmit window.
///
/// # Errors
///
/// Its anchor or window lies outside the radio epoch.
// CAPABILITY: bluetooth-recurring-peripheral-events, bluetooth-sleep-clock-accuracy-window-widening
pub(crate) fn recurring(
    reference: LeInstant,
    since_reference: RadioDuration<LeRadio>,
    transmit_window: RadioDuration<LeRadio>,
    peer_sleep_clock_ppm: u16,
    allowances: ConnectionAllowances,
) -> Result<Plan, EpochExhausted> {
    let anchor = reference
        .checked_add(since_reference)
        .ok_or(error(C::Recurrence))?;
    let widening = widening(since_reference, peer_sleep_clock_ppm, allowances)?;
    // A span past `u64::MAX` cannot lie in the epoch either.
    let outside = error(C::WindowGeometry);
    let lead = allowances
        .boundary_guard
        .checked_add(allowances.receive_guard)
        .and_then(|lead| lead.checked_add(widening))
        .ok_or(outside)?;
    let start = anchor.checked_sub(lead.get()).ok_or(outside)?;
    let duration = lead
        .checked_add(widening)
        .and_then(|span| span.checked_add(transmit_window))
        .and_then(|span| span.checked_add(allowances.event_length))
        .ok_or(outside)?;
    let receive_wait = widening
        .checked_mul(2)
        .and_then(|span| span.checked_add(allowances.receive_guard))
        .and_then(|span| span.checked_add(transmit_window))
        .and_then(|span| span.checked_add(allowances.receive_tail))
        .ok_or(outside)?;
    Ok(Plan {
        anchor,
        since_reference,
        window: LeWindow::nonempty(start, duration).ok_or(outside)?,
        timing: ConnectionEventTiming::Recurring {
            receive_wait,
            widening,
        },
        transmit_window,
    })
}

/// Whole milliseconds times the combined accuracy, truncated as the vendor
/// computes it, then jitter. The u64 span times two u16 ppm coefficients fits
/// u128; do not reject representable final drift through intermediate overflow.
fn widening(
    elapsed: RadioDuration<LeRadio>,
    peer_ppm: u16,
    allowances: ConnectionAllowances,
) -> Result<RadioDuration<LeRadio>, EpochExhausted> {
    let ppm = u128::from(peer_ppm) + u128::from(allowances.local_sleep_clock_ppm);
    let drift = (u128::from(elapsed.as_micros()) / 1_000) * ppm / 1_000;
    // A widening past `u64::MAX` cannot lie in the epoch.
    u64::try_from(drift)
        .ok()
        .and_then(|drift| RadioDuration::from_micros(drift).checked_add(allowances.widening_jitter))
        .ok_or(error(C::Widening))
}

fn error(calculation: C) -> EpochExhausted {
    EpochExhausted::at(
        PlanningRole::Peripheral,
        PlanningOperation::Event,
        calculation,
    )
}

#[cfg(test)]
mod tests {
    use oer_bluetooth_radio::{
        ConnectionAllowances, ConnectionEventTiming, LeInstant, RadioDuration,
    };

    use super::{first, recurring};

    const S31: ConnectionAllowances = ConnectionAllowances {
        local_sleep_clock_ppm: 500,
        widening_jitter: RadioDuration::from_micros(63),
        receive_guard: RadioDuration::from_micros(10),
        receive_tail: RadioDuration::from_micros(2),
        boundary_guard: oer_bluetooth_radio::NonZeroRadioDuration::from_micros(
            core::num::NonZeroU64::MIN,
        ),
        first_event_guard: RadioDuration::from_micros(16),
        event_length: RadioDuration::from_micros(5_047),
        first_event_length: RadioDuration::from_micros(5_155),
    };

    #[test]
    fn the_first_event_listens_across_its_transmit_window() {
        let plan = first(
            LeInstant::from_micros(100_000),
            RadioDuration::from_micros(2_500),
            S31,
        )
        .unwrap();
        assert_eq!(plan.window.start().as_micros(), 100_000 - 17);
        assert_eq!(plan.window.end().as_micros(), 100_000 + 2_500 + 5_155);
        assert_eq!(
            plan.timing,
            ConnectionEventTiming::First {
                transmit_window: RadioDuration::from_micros(2_500),
                timing_guard: RadioDuration::from_micros(16),
            }
        );
    }

    #[test]
    fn recurring_events_widen_with_elapsed_time_and_both_clocks() {
        let reference = LeInstant::from_micros(100_000);
        // 50 ms at 500 + 50 ppm: floor(50 * 550 / 1000) = 27 us, plus 63.
        let since = RadioDuration::from_micros(50_000);
        let plan = recurring(reference, since, RadioDuration::from_micros(0), 50, S31).unwrap();
        assert_eq!(plan.anchor.as_micros(), 150_000);
        let widening: u64 = 27 + 63;
        assert_eq!(plan.window.start().as_micros(), 150_000 - 10 - widening - 1);
        assert_eq!(plan.window.end().as_micros(), 150_000 + widening + 5_047);
        assert_eq!(
            plan.timing,
            ConnectionEventTiming::Recurring {
                receive_wait: RadioDuration::from_micros(10 + 2 * widening + 2),
                widening: RadioDuration::from_micros(widening),
            }
        );
        // An uncertain transmit window stays part of every listening.
        let uncertain =
            recurring(reference, since, RadioDuration::from_micros(1_250), 50, S31).unwrap();
        assert_eq!(
            uncertain.timing,
            ConnectionEventTiming::Recurring {
                receive_wait: RadioDuration::from_micros(10 + 2 * widening + 1_250 + 2),
                widening: RadioDuration::from_micros(widening),
            }
        );
        assert_eq!(
            uncertain.window.duration().as_micros(),
            plan.window.duration().as_micros() + 1_250
        );
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use crate::PlanningCalculation;

    const ALLOWANCES: ConnectionAllowances = ConnectionAllowances {
        local_sleep_clock_ppm: 500,
        widening_jitter: RadioDuration::from_micros(63),
        receive_guard: RadioDuration::from_micros(10),
        receive_tail: RadioDuration::from_micros(2),
        boundary_guard: oer_bluetooth_radio::NonZeroRadioDuration::from_micros(
            core::num::NonZeroU64::MIN,
        ),
        first_event_guard: RadioDuration::from_micros(16),
        event_length: RadioDuration::from_micros(5_047),
        first_event_length: RadioDuration::from_micros(5_155),
    };

    #[test]
    fn widening_accepts_large_spans_without_intermediate_narrowing() {
        let elapsed = RadioDuration::from_micros(u64::MAX);
        let wide = widening(elapsed, u16::MAX, ALLOWANCES).unwrap();
        let expected = (u128::from(u64::MAX) / 1_000) * (u128::from(u16::MAX) + 500) / 1_000 + 63;
        assert_eq!(u128::from(wide.as_micros()), expected);
        assert!(wide.as_micros() > u64::from(u32::MAX));
        // A widening past `u64::MAX` cannot lie in the epoch.
        let overflow = ConnectionAllowances {
            widening_jitter: elapsed,
            ..ALLOWANCES
        };
        assert!(matches!(
            widening(RadioDuration::from_micros(1_000_000), 50, overflow),
            Err(EpochExhausted {
                calculation: PlanningCalculation::Widening,
                ..
            })
        ));
    }

    #[test]
    fn first_and_recurring_geometry_end_at_both_epoch_edges() {
        let zero = RadioDuration::from_micros(0);
        // The first window starts 17 us before its anchor.
        assert!(first(LeInstant::from_micros(17), zero, ALLOWANCES).is_ok());
        assert!(matches!(
            first(LeInstant::from_micros(16), zero, ALLOWANCES),
            Err(EpochExhausted {
                calculation: PlanningCalculation::WindowGeometry,
                ..
            })
        ));
        assert_eq!(
            first(LeInstant::from_micros(u64::MAX - 5_155), zero, ALLOWANCES)
                .unwrap()
                .window
                .end(),
            LeInstant::from_micros(u64::MAX)
        );
        assert!(first(LeInstant::from_micros(u64::MAX - 5_154), zero, ALLOWANCES).is_err());
        // An anchor past the epoch's end.
        assert!(matches!(
            recurring(
                LeInstant::from_micros(u64::MAX - 100),
                RadioDuration::from_micros(101),
                zero,
                50,
                ALLOWANCES
            ),
            Err(EpochExhausted {
                calculation: PlanningCalculation::Recurrence,
                ..
            })
        ));
    }
}

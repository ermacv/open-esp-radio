//! Placement of radio events without overlapping reservations.
//!
//! Every event reserves its air window plus the radio's preparation lead
//! before it. A role proposes the earliest start it accepts and the latest
//! start it still accepts; the arbiter moves the start past every busy
//! reservation. A proposal that cannot start by its latest start is not
//! placed, and the role skips that event.

use oer_bluetooth_radio::LeRadio;
use oer_bluetooth_radio::{LeInstant, LeWindow, NonZeroRadioDuration, OutsideEpoch, RadioTiming};

/// One event a role wants to schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Proposal {
    pub(crate) earliest: LeInstant,
    pub(crate) latest: LeInstant,
    pub(crate) duration: NonZeroRadioDuration<LeRadio>,
}

/// The earliest start in `[earliest, latest]` whose reservation
/// `[start - lead, start + duration)` overlaps no busy reservation; `busy`
/// holds each role's reservations as one group.
///
/// # Errors
///
/// A reservation the placement needs lies outside the radio epoch.
pub(crate) fn place(
    proposal: Proposal,
    timing: RadioTiming,
    busy: &[&[Option<LeWindow>]],
) -> Result<Option<LeInstant>, OutsideEpoch> {
    let mut start = proposal.earliest;
    if start > proposal.latest {
        return Ok(None);
    }
    let windows = || busy.iter().copied().flatten().flatten();
    // Each pass either settles or moves past one busy reservation.
    for _ in 0..=windows().count() {
        let reserved = reservation(start, proposal.duration, timing)?;
        match windows()
            .filter(|window| window.overlaps(reserved))
            .map(|window| window.end())
            .max()
        {
            None => return Ok(Some(start)),
            Some(end) => {
                // A conflict already beyond the permitted placement range is
                // ordinary no-work; no future anchor is required in that case.
                let Some(latest_reservation_start) =
                    proposal.latest.checked_sub(timing.preparation_lead)
                else {
                    return Ok(None);
                };
                if end > latest_reservation_start {
                    return Ok(None);
                }
                start = end
                    .checked_add(timing.preparation_lead)
                    .ok_or(OutsideEpoch)?;
            }
        }
    }
    Ok(None)
}

/// The reservation of an air window starting at `start`.
///
/// # Errors
///
/// The window or its reservation lies outside the radio epoch.
pub(crate) fn reservation(
    start: LeInstant,
    duration: NonZeroRadioDuration<LeRadio>,
    timing: RadioTiming,
) -> Result<LeWindow, OutsideEpoch> {
    let window = LeWindow::nonempty(start, duration).ok_or(OutsideEpoch)?;
    timing.reservation(window)
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU64;

    use oer_bluetooth_radio::{
        LeInstant, LeWindow, NonZeroRadioDuration, OutsideEpoch, RadioDuration, RadioTiming,
    };

    use super::{Proposal, place, reservation};

    const TIMING: RadioTiming = RadioTiming {
        preparation_lead: RadioDuration::from_micros(100),
        admission_guard: RadioDuration::from_micros(0),
        connection: oer_bluetooth_radio::ConnectionAllowances {
            local_sleep_clock_ppm: 0,
            widening_jitter: RadioDuration::from_micros(0),
            receive_guard: RadioDuration::from_micros(0),
            receive_tail: RadioDuration::from_micros(0),
            boundary_guard: NonZeroRadioDuration::from_micros(NonZeroU64::MIN),
            first_event_guard: RadioDuration::from_micros(0),
            event_length: RadioDuration::from_micros(0),
            first_event_length: RadioDuration::from_micros(0),
        },
    };

    fn proposal(earliest: u64, latest: u64, duration: u64) -> Proposal {
        Proposal {
            earliest: LeInstant::from_micros(earliest),
            latest: LeInstant::from_micros(latest),
            duration: micros(duration),
        }
    }

    fn micros(micros: u64) -> NonZeroRadioDuration<oer_bluetooth_radio::LeRadio> {
        NonZeroRadioDuration::from_micros(NonZeroU64::new(micros).unwrap())
    }

    fn busy(start: u64, duration: u64) -> Option<LeWindow> {
        reservation(LeInstant::from_micros(start), micros(duration), TIMING).ok()
    }

    #[test]
    fn a_free_timeline_keeps_the_earliest_start() {
        assert_eq!(
            place(
                proposal(1_000, 2_000, 500),
                TIMING,
                &[&[None, busy(5_000, 100)]]
            ),
            Ok(Some(LeInstant::from_micros(1_000)))
        );
    }

    #[test]
    fn a_start_moves_past_every_busy_reservation_including_the_lead() {
        // Busy 1_000..1_500 (reservation 900..1_500); then 1_700..1_800.
        let busy = [busy(1_000, 500), busy(1_700, 100)];
        assert_eq!(
            place(proposal(1_000, 5_000, 200), TIMING, &[&busy]),
            Ok(Some(LeInstant::from_micros(1_900)))
        );
        assert_eq!(
            place(proposal(1_000, 1_800, 200), TIMING, &[&busy]),
            Ok(None)
        );
    }
    #[test]
    fn a_conflict_is_no_work_but_geometry_outside_the_epoch_is_an_error() {
        assert_eq!(place(proposal(1, 2, 1), TIMING, &[]), Err(OutsideEpoch));
        assert_eq!(
            place(proposal(u64::MAX - 1, u64::MAX - 1, 2), TIMING, &[]),
            Err(OutsideEpoch)
        );
        assert_eq!(
            place(proposal(1_000, 1_200, 100), TIMING, &[&[busy(1_000, 500)]]),
            Ok(None)
        );
    }
}

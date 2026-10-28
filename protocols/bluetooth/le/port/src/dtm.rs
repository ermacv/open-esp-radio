//! Shared Direct Test Mode window geometry (Core Vol 6 Part F 4.1.6).
//!
//! The backend progresses an admitted test autonomously. This value-only
//! calculation preserves the transmitter grid and receiver window contract.

use crate::{
    LeInstant, LeRadio, LeWindow, NonZeroRadioDuration, OutsideEpoch, RadioDuration, RadioTiming,
    TestPhy,
};
use core::num::NonZeroU64;
use oer_bluetooth_le_wire::{TestChannel, dtm::DtmPayloadPattern};

/// Length of one receiver listening window.
pub const DTM_RECEIVE_WINDOW: NonZeroRadioDuration<LeRadio> =
    NonZeroRadioDuration::from_micros(NonZeroU64::new(5_000).unwrap());

/// Slack between planning the first event of a test and the earliest instant
/// the backend admits, covering the time until the request reaches it.
pub const DTM_PLANNING_SLACK: RadioDuration<LeRadio> = RadioDuration::from_micros(500);

/// Slack of a later transmitter packet, planned as soon as the previous one
/// ended. It covers the request's way to the backend so that a grid slot the
/// request cannot reach in time is skipped at planning rather than refused,
/// which would cost a retry and a further interval.
pub const DTM_RECURRING_TRANSMIT_SLACK: RadioDuration<LeRadio> = RadioDuration::from_micros(100);

/// One test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmTest {
    /// Transmit test packets.
    Transmit {
        /// RF channel.
        channel: TestChannel,
        /// PHY.
        phy: TestPhy,
        /// Payload pattern.
        pattern: DtmPayloadPattern,
        /// Payload length in bytes.
        length: u8,
    },
    /// Receive and count test packets.
    Receive {
        /// RF channel.
        channel: TestChannel,
        /// PHY.
        phy: TestPhy,
    },
}

/// Calculation that prevented a required DTM continuation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmCalculation {
    /// The earliest anchor admitted by preparation and guard.
    Admission,
    /// First-packet or receive-window planning slack.
    InitialAnchor,
    /// The next point on the previous transmitter's interval grid.
    NextAnchor,
    /// The earliest recurring transmitter point reachable now.
    ReachableAnchor,
    /// The duration of skipped grid slots.
    SlotAlignment,
    /// The air window of a test event.
    AirWindow,
    /// Its preparation reservation.
    Reservation,
}

/// A required DTM continuation lies outside the radio epoch. The caller retains its test and previous window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmEpochExhausted {
    /// The calculation whose result left the epoch.
    pub calculation: DtmCalculation,
}

/// Failure of a detached DTM window plan, with no state mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmWindowError {
    /// The selected transmitter PHY has no implemented airtime calculation.
    UnsupportedPhy,
    /// A checked time calculation left this radio epoch.
    OutsideEpoch(DtmEpochExhausted),
}

/// Plan one physical DTM air window without requiring TX authority for RX.
/// Backends use the same timing and grid contract.
/// Failure changes no payload or previous-window state.
pub fn next_window(
    test: DtmTest,
    now: LeInstant,
    timing: RadioTiming,
    previous: Option<LeWindow>,
) -> Result<LeWindow, DtmWindowError> {
    let admitted =
        admitted_anchor(now, timing).ok_or(DtmWindowError::OutsideEpoch(DtmEpochExhausted {
            calculation: DtmCalculation::Admission,
        }))?;
    let geometry = WindowGeometry::new(test).ok_or(DtmWindowError::UnsupportedPhy)?;
    plan_window(geometry, admitted, timing, previous).map_err(DtmWindowError::OutsideEpoch)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowGeometry {
    Transmit {
        air: NonZeroRadioDuration<LeRadio>,
        interval: RadioDuration<LeRadio>,
    },
    Receive,
}

impl WindowGeometry {
    fn new(test: DtmTest) -> Option<Self> {
        match test {
            DtmTest::Transmit { phy, length, .. } => Some(Self::Transmit {
                air: transmit_air_time(phy, length)?,
                // Evaluating I(L) over the bounded u8 length gives these
                // exact grid intervals; coded TX is refused above.
                interval: RadioDuration::from_micros(match (phy, length) {
                    (TestPhy::Le1M, 0..=37) | (TestPhy::Le2M, 0..=83) => 625,
                    (TestPhy::Le1M, 38..=115) | (TestPhy::Le2M, 84..=239) => 1_250,
                    (TestPhy::Le1M, 116..=193) | (TestPhy::Le2M, 240..=255) => 1_875,
                    (TestPhy::Le1M, 194..=255) => 2_500,
                    (TestPhy::LeCodedS8 | TestPhy::LeCodedS2, _) => return None,
                }),
            }),
            DtmTest::Receive { .. } => Some(Self::Receive),
        }
    }
}

fn plan_window(
    geometry: WindowGeometry,
    admitted: LeInstant,
    timing: RadioTiming,
    previous: Option<LeWindow>,
) -> Result<LeWindow, DtmEpochExhausted> {
    use DtmCalculation as C;
    let at = |calculation| DtmEpochExhausted { calculation };
    let window = match geometry {
        WindowGeometry::Transmit { air, interval } => {
            let anchor = match previous {
                None => admitted
                    .checked_add(DTM_PLANNING_SLACK)
                    .ok_or(at(C::InitialAnchor))?,
                Some(last) => {
                    let first = last
                        .start()
                        .checked_add(interval)
                        .ok_or(at(C::NextAnchor))?;
                    let reachable = admitted
                        .checked_add(DTM_RECURRING_TRANSMIT_SLACK)
                        .ok_or(at(C::ReachableAnchor))?;
                    // A grid point the request still reaches is the next
                    // one; otherwise the first grid point after it.
                    match reachable.checked_duration_since(first) {
                        None => first,
                        Some(late) => {
                            let slots = ceil_div(late.as_micros(), interval.as_micros())
                                .ok_or(at(C::SlotAlignment))?;
                            interval
                                .checked_mul(slots)
                                .and_then(|skipped| first.checked_add(skipped))
                                .ok_or(at(C::SlotAlignment))?
                        }
                    }
                }
            };
            LeWindow::nonempty(anchor, air).ok_or(at(C::AirWindow))?
        }
        WindowGeometry::Receive => {
            let earliest = admitted
                .checked_add(DTM_PLANNING_SLACK)
                .ok_or(at(C::InitialAnchor))?;
            let anchor = previous.map_or(earliest, |last| last.end().max(earliest));
            LeWindow::nonempty(anchor, DTM_RECEIVE_WINDOW).ok_or(at(C::AirWindow))?
        }
    };
    timing
        .reservation(window)
        .map_err(|OutsideEpoch| at(C::Reservation))?;
    Ok(window)
}

/// The earliest anchor the backend admits at `now`.
fn admitted_anchor(now: LeInstant, timing: RadioTiming) -> Option<LeInstant> {
    now.checked_add(timing.preparation_lead)?
        .checked_add(timing.admission_guard)
}

/// Air time of one LE Test packet: preamble, access address, header,
/// payload and CRC; `None` for a PHY without a transmitter airtime.
fn transmit_air_time(phy: TestPhy, length: u8) -> Option<NonZeroRadioDuration<LeRadio>> {
    // The packet overhead, and the airtime of one payload byte.
    let (overhead, byte) = match phy {
        TestPhy::Le1M => (const { NonZeroU64::new((1 + 4 + 2 + 3) * 8).unwrap() }, 8),
        TestPhy::Le2M => (const { NonZeroU64::new((2 + 4 + 2 + 3) * 4).unwrap() }, 4),
        TestPhy::LeCodedS8 | TestPhy::LeCodedS2 => return None,
    };
    // A u8 payload keeps the exact sum below 2,200 us.
    Some(NonZeroRadioDuration::from_micros(
        overhead.checked_add(u64::from(length).checked_mul(byte)?)?,
    ))
}

fn ceil_div(value: u64, divisor: u64) -> Option<u64> {
    value
        .checked_div(divisor)?
        .checked_add(u64::from(value.checked_rem(divisor)? != 0))
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;

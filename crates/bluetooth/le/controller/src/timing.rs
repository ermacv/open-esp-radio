//! Event geometry: connection events over the backend's connection
//! allowances, and legacy advertising channels after the vendor item
//! geometry.
//!
//! The Link Layer anchors every event on the central's first packet. Until a
//! packet has been received the anchor is the start of a transmit window of
//! uncertain position, which recurs at every interval. Clock drift widens the
//! receive window by the sum of both sleep-clock accuracies over the time since
//! the last received anchor (Core Vol 6 Part B 4.2.4); the backend adds its
//! jitter and guards as [`ConnectionAllowances`] describes.

use core::num::{NonZeroU32, NonZeroU64};

use oer_bluetooth_ll::connection::LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS;
use oer_bluetooth_radio::LeRadio;
use oer_bluetooth_radio::{
    ConnectionAllowances, ConnectionEventTiming, LeInstant, LeWindow, NonZeroRadioDuration,
    RadioDuration, RadioTiming,
};

use PlanningCalculation as C;

/// The logical owner whose continuation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningRole {
    /// Legacy advertising.
    Advertising,
    /// A peripheral connection.
    Peripheral,
}

/// The operation being prepared, before any new request is published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningOperation {
    /// One event.
    Event,
}

/// The failing calculation within that operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningCalculation {
    /// One advertising channel's air time plus the preparation lead.
    ChannelSpacing,
    /// A whole advertising event over its channels.
    EventDuration,
    /// The next connection anchor after the elapsed intervals.
    Recurrence,
    /// A connection event's receive window around its anchor.
    WindowGeometry,
    /// The window widening from the combined sleep-clock drift.
    Widening,
}

/// A required continuation of a schedule lies outside the radio epoch: no
/// later sample can bring it back.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EpochExhausted {
    /// The role whose continuation left the epoch.
    pub role: PlanningRole,
    /// The operation being prepared.
    pub operation: PlanningOperation,
    /// The calculation whose result left the epoch.
    pub calculation: PlanningCalculation,
}

impl EpochExhausted {
    /// The failure of `calculation` in `role`'s `operation`.
    pub const fn at(
        role: PlanningRole,
        operation: PlanningOperation,
        calculation: PlanningCalculation,
    ) -> Self {
        Self {
            role,
            operation,
            calculation,
        }
    }
}

/// The anchor phase of a connection between events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Phase {
    /// The last anchor a received packet established; drift accumulates from
    /// here.
    pub reference: LeInstant,
    /// From the reference to the nominal anchor of the last event.
    pub since_reference: RadioDuration<LeRadio>,
    /// Width of the transmit window still uncertain at the anchor, zero once a
    /// packet fixed the anchor.
    pub transmit_window: RadioDuration<LeRadio>,
}

impl Phase {
    /// The phase of an anchor a received packet just established, or of the
    /// first anchor with its transmit window.
    pub const fn at(anchor: LeInstant, transmit_window: RadioDuration<LeRadio>) -> Self {
        Self {
            reference: anchor,
            since_reference: RadioDuration::from_micros(0),
            transmit_window,
        }
    }
}

/// One planned connection event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Plan {
    pub anchor: LeInstant,
    /// From the phase's reference to `anchor`.
    pub since_reference: RadioDuration<LeRadio>,
    pub window: LeWindow,
    pub timing: ConnectionEventTiming,
    pub transmit_window: RadioDuration<LeRadio>,
}

/// The first event, anchored at the start of its transmit window.
///
/// # Errors
///
/// Its window lies outside the radio epoch.
pub fn first(
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
pub fn recurring(
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

/// Inter-frame space.
const T_IFS_MICROS: u32 = 150;
/// LE 1M air time of `SCAN_REQ`.
const SCAN_REQ_AIR_MICROS: u32 = (1 + 4 + 2 + 12 + 3) * 8;
/// The vendor's response-capable scheduler tail after `ADV_IND`.
const RESPONSE_CAPABLE_TAIL_MICROS: u32 = 4;

/// The length of a legacy advertising-channel PDU, header included: from
/// the two header bytes alone up to the 37 payload bytes of a legacy PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyPduLength(u8);

impl LegacyPduLength {
    /// The shortest legacy PDU: its header only.
    pub const MIN: Self = Self(2);
    /// The longest legacy PDU: its header and 37 payload bytes.
    pub const MAX: Self = Self(39);

    /// `length` bytes, or `None` outside `2..=39`.
    pub const fn new(length: usize) -> Option<Self> {
        if length >= Self::MIN.0 as usize && length <= Self::MAX.0 as usize {
            Some(Self(length as u8))
        } else {
            None
        }
    }

    /// The length in bytes.
    pub const fn get(self) -> usize {
        self.0 as usize
    }
}

/// What one legacy advertising channel awaits after its PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvertisingResponse {
    /// Nothing: a non-connectable, non-scannable set.
    None,
    /// A scan request answered by a scan response of `length` bytes, header
    /// included, or, for a `connectable` set, a connection indication,
    /// whichever exchange is longer.
    ScanResponse {
        /// The scan response PDU's length, header included.
        length: LegacyPduLength,
        /// The set also accepts a connection indication.
        connectable: bool,
    },
    /// A connection indication from the directed target only.
    Directed,
}

/// LE 1M air time of a legacy PDU of `length` bytes, header included.
pub const fn legacy_air_micros(length: LegacyPduLength) -> NonZeroU32 {
    // Preamble, access address, header and CRC.
    const OVERHEAD: NonZeroU32 = NonZeroU32::new((1 + 4 + 2 + 3) * 8).unwrap();
    // The length holds the header and at most 37 payload bytes: it never
    // underflows or saturates.
    OVERHEAD.saturating_add((length.0 as u32 - 2) * 8)
}

/// Air time of one advertising channel's `pdu_length`-byte packet and the
/// responses it awaits: a non-scannable channel reserves the PDU's air time;
/// a response-capable channel also covers the vendor's four-microsecond
/// response-capable tail and the longest exchange that follows.
pub fn advertising_channel_air(
    pdu_length: LegacyPduLength,
    response: AdvertisingResponse,
) -> NonZeroRadioDuration<LeRadio> {
    // `LegacyPduLength` bounds both lengths to 39 bytes, so every sum below
    // stays under 10,000 us: none saturates.
    let air = legacy_air_micros(pdu_length);
    let responses = match response {
        AdvertisingResponse::None => 0,
        AdvertisingResponse::Directed => {
            RESPONSE_CAPABLE_TAIL_MICROS + T_IFS_MICROS + LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS
        }
        AdvertisingResponse::ScanResponse {
            length,
            connectable,
        } => {
            let scan = SCAN_REQ_AIR_MICROS + T_IFS_MICROS + legacy_air_micros(length).get();
            let exchange = if connectable {
                scan.max(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS)
            } else {
                scan
            };
            RESPONSE_CAPABLE_TAIL_MICROS + T_IFS_MICROS + exchange
        }
    };
    NonZeroRadioDuration::from_micros(NonZeroU64::from(air.saturating_add(responses)))
}

/// The time between consecutive channels of an advertising event: one
/// channel's air time and responses, plus the preparation lead.
///
/// # Errors
///
/// The spacing lies outside the radio epoch.
pub fn advertising_channel_spacing(
    channel_air: NonZeroRadioDuration<LeRadio>,
    timing: RadioTiming,
) -> Result<RadioDuration<LeRadio>, EpochExhausted> {
    // A spacing past `u64::MAX` cannot lie in the epoch.
    channel_air
        .get()
        .checked_add(timing.preparation_lead)
        .ok_or(advertising_error(C::ChannelSpacing))
}

/// The whole event over `channels` channels, at least one: every channel
/// but the last one takes a spacing; the last one needs no lead after it.
///
/// # Errors
///
/// The duration lies outside the radio epoch.
pub fn advertising_event_duration(
    channel_air: NonZeroRadioDuration<LeRadio>,
    channels: NonZeroU64,
    timing: RadioTiming,
) -> Result<NonZeroRadioDuration<LeRadio>, EpochExhausted> {
    let spacing = advertising_channel_spacing(channel_air, timing)?;
    spacing
        .checked_mul(channels.get() - 1)
        .and_then(|prefix| channel_air.checked_add(prefix))
        .ok_or(advertising_error(C::EventDuration))
}

fn advertising_error(calculation: C) -> EpochExhausted {
    EpochExhausted::at(
        PlanningRole::Advertising,
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

#[cfg(test)]
mod advertising_tests {
    use super::*;

    const TIMING: RadioTiming = RadioTiming {
        preparation_lead: RadioDuration::from_micros(137),
        admission_guard: RadioDuration::from_micros(0),
        connection: ConnectionAllowances {
            local_sleep_clock_ppm: 500,
            widening_jitter: RadioDuration::from_micros(0),
            receive_guard: RadioDuration::from_micros(0),
            receive_tail: RadioDuration::from_micros(0),
            boundary_guard: NonZeroRadioDuration::from_micros(NonZeroU64::MIN),
            first_event_guard: RadioDuration::from_micros(0),
            event_length: RadioDuration::from_micros(0),
            first_event_length: RadioDuration::from_micros(0),
        },
    };

    fn length(bytes: usize) -> LegacyPduLength {
        LegacyPduLength::new(bytes).unwrap()
    }

    #[test]
    fn a_legacy_pdu_length_holds_its_header_and_at_most_37_payload_bytes() {
        assert_eq!(LegacyPduLength::new(0), None);
        assert_eq!(LegacyPduLength::new(1), None);
        assert_eq!(LegacyPduLength::new(2), Some(LegacyPduLength::MIN));
        assert_eq!(LegacyPduLength::new(39), Some(LegacyPduLength::MAX));
        assert_eq!(LegacyPduLength::new(40), None);
        assert_eq!(LegacyPduLength::new(usize::MAX), None);
        assert_eq!(
            legacy_air_micros(LegacyPduLength::MIN).get(),
            (1 + 4 + 2 + 3) * 8
        );
    }

    #[test]
    fn a_legacy_pdu_takes_eight_microseconds_per_octet_on_le_1m() {
        // Preamble, access address and CRC around a 39-byte PDU.
        assert_eq!(
            legacy_air_micros(LegacyPduLength::MAX).get(),
            (1 + 4 + 39 + 3) * 8
        );
    }

    #[test]
    fn a_response_capable_channel_covers_its_longest_exchange() {
        let plain = advertising_channel_air(LegacyPduLength::MAX, AdvertisingResponse::None);
        assert_eq!(plain.get().as_micros(), 376);
        let scannable = advertising_channel_air(
            LegacyPduLength::MAX,
            AdvertisingResponse::ScanResponse {
                length: LegacyPduLength::MAX,
                connectable: false,
            },
        );
        assert_eq!(
            scannable.get().as_micros(),
            376 + 4 + 150 + u64::from(SCAN_REQ_AIR_MICROS) + 150 + 376
        );
        let directed = advertising_channel_air(length(14), AdvertisingResponse::Directed);
        assert_eq!(
            directed.get().as_micros(),
            u64::from(legacy_air_micros(length(14)).get())
                + 4
                + 150
                + u64::from(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS)
        );
    }

    #[test]
    fn the_last_channel_needs_no_lead_after_it() {
        let air = advertising_channel_air(LegacyPduLength::MAX, AdvertisingResponse::None);
        assert_eq!(
            advertising_channel_spacing(air, TIMING),
            Ok(RadioDuration::from_micros(376 + 137))
        );
        assert_eq!(
            advertising_event_duration(air, NonZeroU64::new(3).unwrap(), TIMING)
                .map(|duration| duration.get()),
            Ok(RadioDuration::from_micros(2 * (376 + 137) + 376))
        );
        // One channel takes only its own air time: no spacing, no lead.
        assert_eq!(
            advertising_event_duration(air, NonZeroU64::MIN, TIMING).map(|duration| duration.get()),
            Ok(RadioDuration::from_micros(376))
        );
    }
}

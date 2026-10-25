//! The S31 connection-event policy and Direct Test Mode notes recovered from
//! the vendor BLE Controller. No radio role in this repository applies them.
//!
//! Direct Test Mode, as the vendor runs it:
//! - the test event bodies stop the scheduler
//!   (`r_sym_bt_74l62ZLsZuXg67pPHSd7`), publish the item as the list-zero
//!   head and run it; a test event never enters a running list;
//! - the event bodies disable the route on every test event without CTE
//!   (`r_sym_ble_YpJTETFGhduIAMkkBKjc`); it stays disabled until Test End
//!   reinitializes the PHY;
//! - Test End reinitializes the PHY, which routes the channel again
//!   (`r_ble_phy_init`);
//! - the vendor ends a test by stopping the scheduler rather than cancelling
//!   its running event (`sym_dtm_NsbldBIeGraE2wg0AVy7`).

use core::num::NonZeroU64;

use oer_bluetooth_le_radio::{ConnectionAllowances, LeRadio, NonZeroRadioDuration, RadioDuration};

// Source-owned S31 connection-event policy. The provenance of each term is
// the recurring-event section of
// docs/vendor/esp32s31/bluetooth-peripheral-connection.md:
// the 16-us widening jitter is the Controller configuration's default
// `ble_ll_jitter_usecs`, the 10-us receive guard is the private-options
// default, the 2-us receive tail and the 1-us boundary are CPU-time ticks, and
// 1,074 us is the minimum LE 1M event duration. A recurring event ends
// 1,074 us after its widened anchor less the preparation lead; the first
// event ends 1,074 us plus the boundary after its transmit window, with the
// 16-us first-event uncertainty on each side.
const WIDENING_JITTER_MICROS: u32 = 16;
const RECEIVE_GUARD_MICROS: u32 = 10;
const RECEIVE_TAIL_MICROS: u32 = 2;
const BOUNDARY_GUARD_MICROS: u32 = 1;
const FIRST_EVENT_GUARD_MICROS: u32 = 16;
/// SOURCE: pinned `libble_app.a[ble_28.o]` table
/// `sym_ble_Qw8LKJo0HxALvv5KCN4X`, which `r_ble_ll_conn_get_min_dura_required`
/// (`r_sym_ble_IXD9YD54AdqppMHisVZw`), `ble_ll_conn_created`
/// (`r_sym_ble_62UX9ux9YcuUoIf4gPAp`) and the move body
/// (`r_sym_ble_JqbtnypGd2wyQym9wmK4`) index by PHY; LE 1M is its first entry.
pub const LE_1M_EVENT_MICROS: u32 = 1_074;

/// SOURCE: pinned `libble_app.a[ble_3.o]` connection-event setup
/// `r_sym_ble_rsCCyH2B22gdYkN4LOOJ`, which installs the link-state event span
/// from the connection interval: the whole interval up to 7,499 us, otherwise
/// the interval less 2,000 us plus the peripheral's window widening. Its
/// third branch, for an exchange of maximum-length packets that does not fit
/// the interval less 2,150 us, cannot occur on LE 1M above 7,499 us.
const WHOLE_INTERVAL_SPAN_MICROS: u32 = 7_499;
const LONG_INTERVAL_MARGIN_MICROS: u32 = 2_000;

/// The Controller span of one connection event in microseconds.
pub fn connection_event_span(
    interval: RadioDuration<LeRadio>,
    widening: RadioDuration<LeRadio>,
) -> RadioDuration<LeRadio> {
    if interval <= RadioDuration::from_micros(u64::from(WHOLE_INTERVAL_SPAN_MICROS)) {
        return interval;
    }
    interval
        .checked_sub(RadioDuration::from_micros(u64::from(
            LONG_INTERVAL_MARGIN_MICROS,
        )))
        .and_then(|span| span.checked_add(widening))
        .expect("a long interval exceeds its margin and a widening fits the epoch")
}

/// The connection allowances of a scheduler profile whose preparation lead
/// is `preparation_lead_micros`, or `None` when the lead leaves no event
/// time: it must lie below [`LE_1M_EVENT_MICROS`].
pub const fn connection_allowances(
    preparation_lead_micros: u32,
    local_sleep_clock_ppm: u16,
) -> Option<ConnectionAllowances> {
    let Some(event_micros) = LE_1M_EVENT_MICROS.checked_sub(preparation_lead_micros) else {
        return None;
    };
    if event_micros == 0 {
        return None;
    }
    Some(ConnectionAllowances {
        local_sleep_clock_ppm,
        widening_jitter: RadioDuration::from_micros(WIDENING_JITTER_MICROS as u64),
        receive_guard: RadioDuration::from_micros(RECEIVE_GUARD_MICROS as u64),
        receive_tail: RadioDuration::from_micros(RECEIVE_TAIL_MICROS as u64),
        boundary_guard: NonZeroRadioDuration::from_micros(
            const {
                NonZeroU64::new(BOUNDARY_GUARD_MICROS as u64)
                    .expect("the boundary guard is nonzero")
            },
        ),
        first_event_guard: RadioDuration::from_micros(FIRST_EVENT_GUARD_MICROS as u64),
        event_length: RadioDuration::from_micros(event_micros as u64),
        first_event_length: RadioDuration::from_micros(
            (LE_1M_EVENT_MICROS + BOUNDARY_GUARD_MICROS) as u64,
        ),
    })
}

#[cfg(test)]
mod tests;

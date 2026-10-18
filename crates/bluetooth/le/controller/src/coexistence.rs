//! Coexistence urgency of the roles' events.
//!
//! The vendor Controller raises the coexistence priority of some events so
//! that a radio sharing the antenna yields to them; the backend turns the
//! level into its arbiter's priority. The rules here decide only the level.

use oer_bluetooth_radio::LeRadio;
use oer_bluetooth_radio::{CoexistenceLevel, RadioDuration};

/// Advertising events between raised events for an advertising interval.
///
/// SOURCE(esp32s31): pinned `libble_app.a` `coexAdv.c.o_1.o`
/// `r_sym_coexAdv_VG7vTjgdtb9y55QZyPP0` (the `r_ble_lll_adv_coex_dpc_update_on_adv_start`
/// role in `oer-symbol-lineage`) over the legacy table
/// `sym_coexAdv_H8ZFnQsVsYzrsw6Rv2Wu` = `04 09 0b 03 02 01 28 00 50 00`: an
/// interval of at most 40 units of 625 us raises every third event, at most
/// 80 units every second one and a longer interval every event. The start
/// also raises the level until the first update.
pub(crate) const fn advertising_period(interval: RadioDuration<LeRadio>) -> u16 {
    let units = interval.as_micros() / 625;
    if units <= 40 {
        3
    } else if units <= 80 {
        2
    } else {
        1
    }
}

/// The level of the advertising event planned after `ended` events of the
/// set ended.
///
/// SOURCE(esp32s31): pinned `libble_app.a` `coexAdv.c.o_1.o`
/// `r_sym_coexAdv_A5DXuOYpsXb125NuYN3d` counts one update per ended event
/// and raises the level when the count is a multiple of the period;
/// `ble_2.o` `r_sym_ble_2eLkvpKuoaxT83Mj406p` schedules the next event
/// before that update, so an event carries the level of the count before
/// the previous event ended, and the first two carry the start level.
// CAPABILITY: coex-coexistence-policy-and-scheduler-ble-advertising-dynamic-priority
pub(crate) const fn advertising_level(period: u16, ended: u16) -> CoexistenceLevel {
    if ended <= 1 || (ended - 1).is_multiple_of(period) {
        CoexistenceLevel::Elevated
    } else {
        CoexistenceLevel::Baseline
    }
}

/// Connection events that keep the new connection's raised level.
const CONNECTION_OPENING_EVENTS: u16 = 6;

/// The level of a peripheral connection event.
///
/// SOURCE(esp32s31): pinned `libble_app.a` `coexConn.c.o_1.o`
/// `r_sym_coexConn_zaMnJdgVnBd25ru35IkB` (the
/// `r_ble_lll_conn_coex_dpc_update_on_event_scheduled` role) over the table
/// `sym_coexConn_1jwNCSpbwgr6k9BEu0Eg` = `04 09 0b 04 09 0b 08 06 03 14 02 28
/// 01 50`. The first six events of a connection are raised; the vendor
/// raises a central's to the highest level after repeated failed events, a
/// peripheral's never. Later, the missed events allowed since the event of
/// the last valid reception follow the interval in 1.25 ms units doubled: 3
/// up to 20, 2 up to 40, 1 up to 80 and none beyond. More missed events, or
/// a local control procedure (`r_sym_coexConn_FwW1rUBEXrFbsmQm3dvU` when
/// one starts), raise the level. `r_sym_coexConn_8zwo11wSByltviHFU8eN`
/// counts receptions from event zero when the connection is created.
// CAPABILITY: coex-coexistence-policy-and-scheduler-ble-connection-dynamic-priority
pub(crate) const fn peripheral_connection_level(
    event_counter: u16,
    last_reception: Option<u16>,
    interval_micros: u32,
    local_procedure: bool,
) -> CoexistenceLevel {
    if event_counter < CONNECTION_OPENING_EVENTS {
        return CoexistenceLevel::Elevated;
    }
    let doubled = (interval_micros / 1_250 * 2) as u16;
    let allowed = if doubled <= 20 {
        3
    } else if doubled <= 40 {
        2
    } else if doubled <= 80 {
        1
    } else {
        0
    };
    let base = match last_reception {
        Some(event) => event,
        None => 0,
    };
    if event_counter.wrapping_sub(base) > allowed || local_procedure {
        CoexistenceLevel::Elevated
    } else {
        CoexistenceLevel::Baseline
    }
}

#[cfg(test)]
mod tests;

//! Coexistence priorities of the radio's events.
//!
//! Alone on the antenna, every event requests it at the highest priority;
//! this equal standalone policy is a product choice. When another radio
//! shares the antenna, the radio applies the vendor BLE Controller's dynamic
//! priority control: fixed lanes per role plus an event lane chosen by the
//! Controller core's [`CoexistenceLevel`].

use oer_bluetooth_radio::CoexistenceLevel;
use oer_esp32s31_bluetooth_memory::{
    AdvertisingCoexistencePriorities, ConnectionCoexistencePriorities,
    LegacyScanCoexistencePriorities, PeripheralConnectionCoexistenceProtection,
    SchedulerItemCoexistencePriority,
};

/// How the radio shares the antenna.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CoexistenceProfile {
    /// No other radio uses the antenna.
    #[default]
    Standalone,
    /// Another radio shares the antenna through the coexistence arbiter.
    Shared,
}

/// The standalone lane of every event.
const STANDALONE: u8 = 15;

/// The legacy advertising event lane at each level.
///
/// SOURCE: pinned `libble_app.a` `coexAdv.c.o_1.o`
/// `r_sym_coexAdv_sLW7oGzvK2Nq64ivPWK2` (the
/// `r_ble_lll_adv_coex_dpc_process_pri` role) writes lane zero from
/// `r_sym_coexAdv_OyYkQW0rKTbKcPp0FJNt`, which indexes the level into the
/// first three bytes of the legacy table `sym_coexAdv_H8ZFnQsVsYzrsw6Rv2Wu`
/// = `04 09 0b ...`, installed with dynamic control enabled by
/// `coexStack.c.o_1.o` `brk_sym_coexStack_hzrw4DdhxAw5xAlGFEwI` when the
/// stack initializes.
const LEGACY_ADVERTISING_EVENT: [u8; 3] = [4, 9, 11];

/// The legacy advertising lanes before the event lane.
///
/// SOURCE: pinned `libble_app.a` `coexAdv.c.o_1.o`
/// `r_sym_coexAdv_pNmEzY32xPoYRR8VuoXP` (the `r_ble_lll_adv_coex_pti_init`
/// role): for a legacy set, lane zero takes byte 0 of the default table
/// `sym_coexAdv_FwE5ZScbIgFmY4AsgYl9` = `04 04 04`, lane one zero, and lanes
/// two and three the two bytes of `sym_coexAdv_6kaAR6zMjkyNvhAbQULQ` =
/// `0d 0d`.
const LEGACY_ADVERTISING_LANES: [u8; 4] = [4, 0, 13, 13];

/// The passive scan window's lanes.
///
/// SOURCE: pinned `libble_app.a` `coexScan.c.o_1.o`. The scan install
/// `r_sym_coexScan_FGUQNnreeyQiPkw2qTYu`, called when the stack initializes
/// with dynamic control enabled, points the dynamic table at
/// `sym_coexScan_ptmWrd4ifytw4hAMsVqa` = `04 04 04 04 04 04 0b 0b 0b 0b 0d
/// 0d ...` and copies `sym_coexScan_8YO4r7UIOrnwAQvzje9T` = `04 04 04 04 04
/// 0d` and `sym_coexScan_RNzbZGAqNFVVa0WpBKWj` = `0d 0d` into bytes 7..15 of
/// the default table `sym_coexCommonDpc_RBSOWgxUegzIb4MrpZ6q`.
/// `r_sym_coexScan_s7w1EV32meG8f6y0sPBq` (the
/// `r_ble_lll_ext_scan_coex_pti_init` role) gives a passive scanner, scan
/// type 0 as HCI LE Set Scan Parameters stores it, lanes zero and one from
/// default bytes 7 and 10 and clears lanes two and three.
/// `r_sym_ble_M0sTWGzdUqAUyXoK849F` (`r_ble_lll_scan_restart`) then calls
/// `r_sym_coexScan_wNFqQvVjWMhmRY8ZGk4o` (`r_ble_lll_ext_scan_coex_dpc_process`)
/// for each window: on a primary channel lane zero takes dynamic byte 0 or
/// 1 and lane one byte 6 or 7, the column chosen by the scan's time-based
/// state. Both columns of the pinned table are equal, so the lanes do not
/// depend on that state.
const PASSIVE_SCAN_LANES: [u8; 4] = [4, 11, 0, 0];

/// The connection event lane at each level.
///
/// SOURCE: pinned `libble_app.a` `coexConn.c.o_1.o`
/// `r_sym_coexConn_sQLo226BUyucqr2y4gOW` (the
/// `r_ble_lll_conn_coex_dpc_process` role) writes lane zero from row one,
/// bytes 3..6, of `sym_coexConn_1jwNCSpbwgr6k9BEu0Eg` = `04 09 0b 04 09 0b
/// ...` at the level; the peripheral path `r_sym_ble_tPr7egUaNHmqfcieCA5O`
/// passes row one.
const CONNECTION_EVENT: [u8; 3] = [4, 9, 11];

/// The connection's base lane.
///
/// SOURCE: pinned `libble_app.a` `coexConn.c.o_1.o`
/// `r_sym_coexConn_BqYvJSQvD3IUJJyL5jD8` (the `r_ble_lll_conn_coex_pti_init`
/// role) writes both lanes from the default table
/// `sym_coexConn_JIeRoeXKfSVWkakjpLwz` = `04 04 08`, byte 0 or 1 by role;
/// the event lane is replaced for every event.
const CONNECTION_BASE: u8 = 4;

/// The connection's coexistence protection in 625 us slots.
///
/// SOURCE: pinned `libble_app.a` `coexConn.c.o_1.o`
/// `r_sym_coexConn_H75YiVgeEk0LdscxQrpJ` (the
/// `r_ble_lll_conn_coex_protect_time_update` role) reads byte 6 of the
/// connection table (8), converts it to 256 us units rounding up and bounded
/// to 1..=63, and writes it to link-state bits 19:14 with bits 21:20 set.
const CONNECTION_PROTECTION_SLOTS: u32 = 8;

const fn lane(value: u8) -> SchedulerItemCoexistencePriority {
    match SchedulerItemCoexistencePriority::new(value) {
        Some(lane) => lane,
        None => panic!("a reviewed lane fits five bits"),
    }
}

const fn level_index(level: CoexistenceLevel) -> usize {
    match level {
        CoexistenceLevel::Baseline => 0,
        CoexistenceLevel::Elevated => 1,
        CoexistenceLevel::Critical => 2,
    }
}

/// The lanes of one legacy advertising event.
// CAPABILITY: bluetooth-hardware-pti-coexistence
pub(crate) const fn advertising_priorities(
    profile: CoexistenceProfile,
    level: CoexistenceLevel,
) -> AdvertisingCoexistencePriorities {
    match profile {
        CoexistenceProfile::Standalone => AdvertisingCoexistencePriorities {
            lanes: [lane(STANDALONE); 4],
        },
        CoexistenceProfile::Shared => {
            let [_, one, two, three] = LEGACY_ADVERTISING_LANES;
            AdvertisingCoexistencePriorities {
                lanes: [
                    lane(LEGACY_ADVERTISING_EVENT[level_index(level)]),
                    lane(one),
                    lane(two),
                    lane(three),
                ],
            }
        }
    }
}

/// The lanes of one passive scan window.
pub(crate) const fn passive_scan_priorities(
    profile: CoexistenceProfile,
) -> LegacyScanCoexistencePriorities {
    let lanes = match profile {
        CoexistenceProfile::Standalone => [STANDALONE; 4],
        CoexistenceProfile::Shared => PASSIVE_SCAN_LANES,
    };
    LegacyScanCoexistencePriorities {
        lanes: [
            lane(lanes[0]),
            lane(lanes[1]),
            lane(lanes[2]),
            lane(lanes[3]),
        ],
    }
}

/// The lanes of one connection event.
pub(crate) const fn connection_priorities(
    profile: CoexistenceProfile,
    level: CoexistenceLevel,
) -> ConnectionCoexistencePriorities {
    match profile {
        CoexistenceProfile::Standalone => ConnectionCoexistencePriorities {
            event: lane(STANDALONE),
            base: lane(STANDALONE),
        },
        CoexistenceProfile::Shared => ConnectionCoexistencePriorities {
            event: lane(CONNECTION_EVENT[level_index(level)]),
            base: lane(CONNECTION_BASE),
        },
    }
}

/// The coexistence protection of a connection opened under `profile`.
pub(crate) const fn connection_protection(
    profile: CoexistenceProfile,
) -> Option<PeripheralConnectionCoexistenceProtection> {
    match profile {
        CoexistenceProfile::Standalone => None,
        CoexistenceProfile::Shared => {
            let units = (CONNECTION_PROTECTION_SLOTS * 625).div_ceil(256);
            let units = if units == 0 {
                1
            } else if units > 63 {
                63
            } else {
                units
            };
            PeripheralConnectionCoexistenceProtection::new(units as u8)
        }
    }
}

#[cfg(test)]
mod tests;

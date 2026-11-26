#![no_std]
use oer_protocols_ieee80211_contracts_port::{AccessPoint, InterfaceLayout, Layout, Station};
// A layout has at most one interface of a role: two stations, or two
// access points, do not build.
pub const STATION_AND_ACCESS_POINT: () =
    <(InterfaceLayout<Station, 8, 1, 2, 4>, InterfaceLayout<AccessPoint, 17, 4, 2, 0>) as Layout>::CHECKED;
#[cfg(ui_reject)]
pub const TWO_STATIONS: () =
    <(InterfaceLayout<Station, 8, 1, 2, 4>, InterfaceLayout<Station, 2, 1, 0, 0>) as Layout>::CHECKED; // ui-error@outside: E0080: evaluation panicked: a layout has at most one interface of a role

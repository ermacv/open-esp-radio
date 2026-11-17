#![no_std]
use oer_protocols_ieee80211_contracts_port::{DATA_SINGLES, DataLayout};
// A layout's data places are none (a station's) or one a transmit queue
// (an access point's): another number does not build.
pub const STATION: () = DataLayout::<8, 2, 4, 0>::CHECKED;
pub const ACCESS_POINT: () = DataLayout::<17, 2, 4, DATA_SINGLES>::CHECKED;
#[cfg(ui_reject)]
pub const THREE: () = DataLayout::<17, 2, 4, 3>::CHECKED; // ui-error@outside: E0080: evaluation panicked: a layout has no data place or one a transmit queue

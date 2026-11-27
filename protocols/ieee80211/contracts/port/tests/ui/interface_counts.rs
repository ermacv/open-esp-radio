#![no_std]
use oer_protocols_ieee80211_contracts_port::{InterfaceLayout, Station};
// An interface's places are counted in a byte.
pub const FITS: () = InterfaceLayout::<Station, 255, 255, 255>::CHECKED;
#[cfg(ui_reject)]
pub const PAIRWISE: () = InterfaceLayout::<Station, 256, 0, 0>::CHECKED; // ui-error@outside: E0080: evaluation panicked: an interface's places are counted in a byte
#[cfg(ui_reject)]
pub const GROUPS: () = InterfaceLayout::<Station, 1, 256, 0>::CHECKED;
#[cfg(ui_reject)]
pub const AGREEMENTS: () = InterfaceLayout::<Station, 1, 0, 256>::CHECKED;

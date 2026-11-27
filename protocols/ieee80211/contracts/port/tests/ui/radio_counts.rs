#![no_std]
use oer_protocols_ieee80211_contracts_port::{
    First, InterfaceLayout, Layout, RadioLayout, Second, Station,
};
// A long chain has one to a frame set's 64 descriptors.
type One = (InterfaceLayout<Station, 1, 0, 0>,);
pub const FITS: () = <RadioLayout<(First, Second), 64, One> as Layout>::CHECKED;
pub const SMALLEST: () = <RadioLayout<(First,), 1, One> as Layout>::CHECKED;
pub const NONE: () = <RadioLayout<(), 1, One> as Layout>::CHECKED;
#[cfg(ui_reject)]
pub const NO_DESCRIPTOR: () = <RadioLayout<(First,), 0, One> as Layout>::CHECKED; // ui-error@outside: E0080: evaluation panicked: a long chain has one to 64 descriptors
#[cfg(ui_reject)]
pub const TOO_LONG: () = <RadioLayout<(First, Second), 65, One> as Layout>::CHECKED;

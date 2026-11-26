#![no_std]
use oer_protocols_ieee80211_contracts_port::{InterfaceLayout, Layout, RadioLayout, Station};
// A chain has one to a frame set's 64 descriptors, and an installation's
// chains, the long ones and a chain of one a queue, are numbered in a byte.
type One = (InterfaceLayout<Station, 1, 1, 0, 0>,);
pub const FITS: () = <RadioLayout<252, 64, One> as Layout>::CHECKED;
pub const SMALLEST: () = <RadioLayout<0, 1, One> as Layout>::CHECKED;
#[cfg(ui_reject)]
pub const NO_DESCRIPTOR: () = <RadioLayout<2, 0, One> as Layout>::CHECKED; // ui-error@outside: E0080: evaluation panicked: a chain has one to 64 descriptors, and an installation's chains, the long ones and one a queue, are counted in a byte
#[cfg(ui_reject)]
pub const TOO_LONG: () = <RadioLayout<2, 65, One> as Layout>::CHECKED;
#[cfg(ui_reject)]
pub const TOO_MANY: () = <RadioLayout<253, 32, One> as Layout>::CHECKED;

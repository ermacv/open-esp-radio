#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{Install, NoCapabilities, PortPools};
use oer_protocols_radio_port::Mint;
pub fn none<'id>(mint: Mint<'id, PortPools<NoCapabilities, 0, 0, 0, 2, 0, 0, 0>>) -> Install<'id, 0, 0, 0, 2, 0, 0> {
    mint.into_pools()
}
#[cfg(ui_reject)]
pub const REFUSED: () = PortPools::<NoCapabilities, 0, 0, 0, 2, 0, 0, 1>::GEOMETRY; // ui-error@outside: E0080: evaluation panicked: a test session with capabilities that serve no Direct Test Mode

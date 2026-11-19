#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{
    Install, LeCapabilities, LeLimits, LeRadioCapabilities, LegacyAdvertisingTypes, PortPools,
};
use oer_protocols_radio_port::Mint;
pub enum Caps {}
impl LeCapabilities for Caps {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        legacy_advertising: LegacyAdvertisingTypes { nonconnectable: true, responsive: false },
        limits: LeLimits { advertising_sets: 1, ..LeLimits::NONE },
        ..LeRadioCapabilities::NONE
    };
}
pub fn one<'id>(mint: Mint<'id, PortPools<Caps, 1, 0, 0, 2, 0, 0, 0>>) -> Install<'id, 1, 0, 0, 2, 0, 0> {
    mint.into_pools()
}
#[cfg(ui_reject)]
pub const REFUSED: () = PortPools::<Caps, 2, 0, 0, 2, 0, 0, 0>::GEOMETRY; // ui-error@outside: E0080: evaluation panicked: more advertising sets than the capabilities hold

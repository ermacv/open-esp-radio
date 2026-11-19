#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{
    Install, LeCapabilities, LeConnectionCapabilities, LeLimits, LeRadioCapabilities,
    LinkAcknowledgement, PortPools, RadioDuration,
};
use oer_protocols_radio_port::Mint;
pub enum Caps {}
impl LeCapabilities for Caps {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        peripheral_connection: Some(LeConnectionCapabilities {
            max_data_payload: 27,
            link_acknowledgement: LinkAcknowledgement::Hardware,
            max_first_receive_wait: RadioDuration::from_micros(0),
            max_receive_wait: RadioDuration::from_micros(0),
        }),
        limits: LeLimits { connections: 1, connection_events: 2, transmit_places: 1, ..LeLimits::NONE },
        ..LeRadioCapabilities::NONE
    };
}
pub fn two<'id>(mint: Mint<'id, PortPools<Caps, 0, 0, 1, 2, 1, 0, 0>>) -> Install<'id, 0, 0, 1, 2, 1, 0> {
    mint.into_pools()
}
#[cfg(ui_reject)]
pub const REFUSED: () = PortPools::<Caps, 0, 0, 1, 1, 1, 0, 0>::GEOMETRY; // ui-error@outside: E0080: evaluation panicked: a connection has at least two event places: the event out and the next

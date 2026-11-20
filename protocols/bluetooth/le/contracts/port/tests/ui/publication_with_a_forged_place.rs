#![no_std]
use oer_protocols_bluetooth_le_contracts_port::{AdvEvent, AdvertisingEvent, LePort, Supported};
pub fn publish<'id, P: LePort<'id, 1, 0, 1, 2, 1, 2, 0>>(
    port: &mut P,
    place: AdvEvent<'id>,
    event: &Supported<P::Caps, AdvertisingEvent<P::Caps>>,
) {
    #[cfg(ui_reject)]
    let place = AdvEvent { brand: core::marker::PhantomData, instance: 0 }; // ui-error: E0451
    let _ = port.publish_advertising(place, event);
}

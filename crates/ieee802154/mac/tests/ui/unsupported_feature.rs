#![no_std]
use oer_ieee802154::owner::{Has, Ieee802154Caps, Promiscuous};
use oer_ieee802154::{Ieee802154Capabilities, RadioCapabilities};
pub struct Capabilities;
impl Ieee802154Caps for Capabilities {
    const CAPS: Ieee802154Capabilities = Ieee802154Capabilities {
        #[cfg(ui_reject)]
        operations: RadioCapabilities::NONE,
        #[cfg(not(ui_reject))]
        operations: RadioCapabilities::PROMISCUOUS,
        interfaces: 1,
        transmit_power: None,
        cca_threshold: None,
        receive_sensitivity_dbm: -100,
        csl: None,
        enhanced_ack_header_ies: 0,
    };
}
pub fn feature() -> Has<Capabilities, Promiscuous> {
    Has::from_const() // ui-error: E0080
}

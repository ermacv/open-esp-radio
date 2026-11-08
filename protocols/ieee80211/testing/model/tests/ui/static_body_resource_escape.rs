#![no_std]
extern crate alloc;
use alloc::boxed::Box;
use core::any::Any;
use oer_protocols_ieee80211_testing_model::{ModelBackend, ModelResources};
use oer_protocols_ieee80211_contracts_port::TxBufferMemory;
use oer_protocols_radio_port::StaticId;

pub enum Declaration {}
type Id = StaticId<Declaration>;
type Backend = ModelBackend<Id>;

// An installation's tokens carry its brand `'p` even under a static
// identity: none escapes into an owner without that brand.
pub fn escape<'p>(mut resources: ModelResources<'p, Backend>) -> Box<dyn Any> {
    let mut buffer = resources.shared.small.pop().expect("one model resource");
    #[cfg(ui_reject)]
    return Box::new(buffer); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    Box::new(buffer.bytes_mut().map(|bytes| bytes.len()))
}

#![no_std]
extern crate alloc;
use alloc::boxed::Box;
use core::any::Any;
use oer_ieee80211_model::{KeyIdentity, ModelRxBuffer};
use oer_ieee80211_port::RxBuffer;
use oer_radio_port::StaticId;

pub enum Declaration {}
type Id = StaticId<Declaration>;
pub fn rx<'p>(frame: ModelRxBuffer<'p, Id>) -> Box<dyn Any> {
    #[cfg(ui_reject)]
    return Box::new(frame); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    Box::new(frame.bytes().len())
}
pub fn key<'p>(identity: KeyIdentity<'p, Id>) -> Box<dyn Any> {
    #[cfg(ui_reject)]
    return Box::new(identity.clone()); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    Box::new(core::mem::size_of_val(&identity))
}

#![no_std]
extern crate alloc;
use alloc::boxed::Box;
use core::any::Any;
use oer_ieee80211_core_roles_sta::{epoch::EpochContext, machine::StaTxContext};
use oer_radio_port::{LoanedContext, PortContext, Root, StaticId};

pub enum Declaration {}
type Id = StaticId<Declaration>;
type Path = Root<Id>;

pub fn epoch<'p>(context: PortContext<'p, Id, EpochContext<Path, ()>>) -> Box<dyn Any> {
    let (epoch, ()) = context.into_parts();
    #[cfg(ui_reject)]
    return Box::new(epoch); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    Box::new(core::mem::size_of_val(&epoch))
}

pub fn station<'p>(context: StaTxContext<'p, Path>) -> Box<dyn Any> {
    let (epoch, _) = context.enclose().into_parts();
    #[cfg(ui_reject)]
    return Box::new(epoch); // ui-error: lifetime may not live long enough
    #[cfg(not(ui_reject))]
    Box::new(core::mem::size_of_val(&epoch))
}

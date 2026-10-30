#![no_std]
extern crate alloc;
use alloc::boxed::Box;
use core::any::Any;
use oer_ieee80211_testing_model::{ModelBackend, ModelResources};
use oer_ieee80211_contracts_port::TxBufferMemory;
use oer_radio_port::{Disabled, Ended, Owner, PortBody, StaticId};

pub enum Declaration {}
type Id = StaticId<Declaration>;
type Backend = ModelBackend<Id>;
pub struct Body;

impl PortBody<Id, Backend> for Body {
    type Output = Box<dyn Any>;
    async fn run<'a, 'p>(self, owner: Owner<'a, 'p, Id, Backend, Disabled>, mut resources: ModelResources<'p, Backend>) -> Ended<Id, Backend, Self::Output>
    where Backend: 'p, Id: 'p, 'p: 'a {
        let mut buffer = resources.small.pop().expect("one model body resource");
        #[cfg(ui_reject)]
        let output: Box<dyn Any> = Box::new(buffer); // ui-error: lifetime may not live long enough
        #[cfg(not(ui_reject))]
        let output: Box<dyn Any> = Box::new(buffer.bytes_mut().len());
        Ended::uninstall(owner, (), output)
    }
}

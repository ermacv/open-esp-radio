#![no_std]
use oer_radio_port::{Disabled, Ended, Owner, PortBackend, PortBody, Slot};

pub struct Body;
impl<Id: oer_radio_port::SlotIdentity + 'static, B> PortBody<Id, B> for Body
where
    B: 'static + for<'id> PortBackend<Id, Resources<'id> = Slot<'id, (), oer_radio_port::Root<Id>>, Uninstall = ()>,
{
    #[cfg(ui_reject)]
    type Output = Slot<'static, (), oer_radio_port::Root<Id>>;
    #[cfg(not(ui_reject))]
    type Output = ();

    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Id, B, Disabled>,
        slot: Slot<'id, (), oer_radio_port::Root<Id>>,
    ) -> Ended<Id, B, Self::Output> where B: 'id, Id: 'id, 'id: 'p {
        #[cfg(ui_reject)]
        {
            Ended::uninstall(owner, (), slot) // ui-error: lifetime may not live long enough
        }
        #[cfg(not(ui_reject))]
        {
            drop(slot);
            Ended::uninstall(owner, (), ())
        }
    }
}

#![no_std]
use oer_radio_port::{Disabled, Ended, Owner, PortBackend, PortBody, Slot};

pub struct Body;
impl<B> PortBody<B> for Body
where
    B: for<'id> PortBackend<Resources<'id> = Slot<'id, ()>, Uninstall = ()>,
{
    #[cfg(ui_reject)]
    type Output = Slot<'static, ()>;
    #[cfg(not(ui_reject))]
    type Output = ();

    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, B, Disabled>,
        slot: Slot<'id, ()>,
    ) -> Ended<'id, B, Self::Output> {
        #[cfg(ui_reject)]
        {
            Ended::uninstall(owner, (), slot) // ui-error: null lifetime may not live long enough
        }
        #[cfg(not(ui_reject))]
        {
            drop(slot);
            Ended::uninstall(owner, (), ())
        }
    }
}

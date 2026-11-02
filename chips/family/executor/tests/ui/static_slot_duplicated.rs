#![no_std]
#![forbid(unsafe_code)]
use oer_radio_port::{
    Abandon, Quarantine, SlotIdentity, SlotOwner, SlotRetire, SlotStopped,
};

pub struct Owner;
impl<Id: SlotIdentity> SlotOwner<Id> for Owner {
    type Vacant = ();
    type Pools = ();
    type Queues = ();
    const VACANT_QUEUES: Self::Queues = ();
    type Frontend<'s> = () where Self: 's, Id: 's;
    type Invocation<'s> = () where Self: 's, Id: 's;
    fn prepare_queues(&mut self, _: &mut Self::Queues, mint: oer_radio_port::Mint<'static, (), Id>) { mint.into_pools(); }
    fn split<'s>(_: &'s mut (), _: oer_radio_port::SlotWakes<'s, Id>) -> Result<((), ()), oer_radio_port::SlotInitError>
    where Self: 's, Id: 's { Ok(((), ()))
    }

    async fn run<'s>(&mut self, _: &mut Self::Invocation<'s>, request: SlotRetire<'_, Id, Self>) -> SlotStopped<Id, Self> where Self: 's, Id: 's {
        request.requested().await
    }

    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>)  where Self: 's, Id: 's {}
}

oer_espressif_executor::declare_port_slot!(first: crate::Owner);
oer_espressif_executor::declare_port_slot!(second: crate::Owner);

pub fn slots() {
    let first = first::take_static().expect("one take");
    #[cfg(ui_reject)]
    let second = first;
    #[cfg(not(ui_reject))]
    let second = second::take_static().expect("another declaration");
    let _ = first.register(Owner); // ui-error: E0382
    let _ = second.register(Owner);
}

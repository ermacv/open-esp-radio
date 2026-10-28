#![no_std]
extern crate alloc;
use alloc::boxed::Box;
use core::any::Any;
use oer_radio_port::{
    Abandon, BodyLoan, Mint, Quarantine, Root, Slot, SlotBody, SlotBodyOwner,
    SlotFrontend, SlotOwner, SlotRetire, SlotStopped, SlotWakes, Slots, StaticId,
};

pub enum Declaration {}
pub type Id = StaticId<Declaration>;
pub struct Owner;
type Token<'p> = Slot<'p, (), Root<Id>>;

impl SlotOwner<Id> for Owner {
    type Vacant = ();
    type Pools = Slots<(), 1>;
    type Queues = Option<Token<'static>>;
    const VACANT_QUEUES: Self::Queues = None;
    type Frontend<'s> = &'s mut Option<Token<'static>>;
    type Invocation<'s> = ();
    fn prepare_queues(&mut self, queues: &mut Self::Queues, mint: Mint<'static, Self::Pools, Id>) {
        let [token] = mint.into_pools();
        *queues = Some(token);
    }
    fn split<'s>(queues: &'s mut Self::Queues, _: SlotWakes<'s, Id>) -> Result<(Self::Frontend<'s>, ()), oer_radio_port::SlotInitError>
    where Self: 's, Id: 's {
        Ok((queues, ()))
    }
    async fn run<'s>(&mut self, _: &mut (), retire: SlotRetire<'_, Id, Self>) -> SlotStopped<Id, Self>
    where Self: 's, Id: 's {
        retire.requested().await
    }

    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>)  where Self: 's, Id: 's {}
}

impl SlotBodyOwner<Id> for Owner {
    type Body<'p> = Token<'p>;
    fn lend<'p, 's>(
        frontend: &'p mut Self::Frontend<'s>, loan: BodyLoan<'p, Id, Self>,
    ) -> Result<Self::Body<'p>, oer_radio_port::SlotInitError>
    where Self: 's, Id: 's, 's: 'p {
        Ok(loan.lend_slot(frontend.take().expect("one body resource")))
    }
}

pub struct Body;
impl SlotBody<Id, Owner> for Body {
    type Output = Box<dyn Any>;
    async fn run<'p>(self, resource: Token<'p>) -> Self::Output
    where Owner: 'p, Id: 'p {
        #[cfg(ui_reject)]
        return Box::new(resource); // ui-error: lifetime may not live long enough
        #[cfg(not(ui_reject))]
        Box::new(resource.index())
    }
}

pub async fn body(frontend: &mut SlotFrontend<'_, Id, Owner>) -> Result<Box<dyn Any>, oer_radio_port::SlotInitError> {
    frontend.run_body(Body).await
}

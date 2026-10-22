extern crate std;

use oer_radio_port::{Abandon, Quarantine, SlotIdentity, SlotOwner, SlotRetire, SlotStopped};

struct Owner;

impl<Id: SlotIdentity> SlotOwner<Id> for Owner {
    type Vacant = ();
    type Pools = ();
    type Queues = ();
    const VACANT_QUEUES: Self::Queues = ();
    type Frontend<'s>
        = ()
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = ()
    where
        Self: 's,
        Id: 's;
    fn prepare_queues(
        &mut self,
        _: &mut Self::Queues,
        mint: oer_radio_port::Mint<'static, (), Id>,
    ) {
        mint.into_pools();
    }
    fn split<'s>(_: &'s mut (), _: oer_radio_port::SlotWakes<'s, Id>) -> ((), ())
    where
        Self: 's,
        Id: 's,
    {
        ((), ())
    }

    async fn run<'s>(
        &mut self,
        _: &mut Self::Invocation<'s>,
        request: SlotRetire<'_, Id, Self>,
    ) -> SlotStopped<Id, Self>
    where
        Self: 's,
        Id: 's,
    {
        request.requested().await
    }
    fn abandon<'s>(&mut self, _: Option<&mut Self::Invocation<'s>>, request: Quarantine) -> Abandon
    where
        Self: 's,
        Id: 's,
    {
        Abandon::Unconfirmed(request)
    }
    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>)
    where
        Self: 's,
        Id: 's,
    {
    }
}

// This crate expands its own placement macro; consumers do not need this
// allowance, because the unsafe operation belongs to the exported macro.
#[allow(
    unsafe_code,
    reason = "the local macro invocation claims its private static"
)]
mod declarations {
    crate::declare_port_slot!(pub first: super::super::Owner);
    crate::declare_port_slot!(pub second: super::super::Owner);
}

#[test]
fn each_nominal_static_declaration_can_be_taken_exactly_once() {
    let first = declarations::first::take_static().expect("first take");
    let second = declarations::second::take_static().expect("independent declaration");
    assert!(declarations::first::take_static().is_none());
    assert!(declarations::second::take_static().is_none());
    let (frontend, registration) = first.register(Owner).ok().expect("fresh backing");
    drop(frontend);
    drop(registration);
    assert!(declarations::first::take_static().is_none());
    let mut second = super::StaticPortSlot::new(second);
    let (frontend, registration) = second.register(Owner).ok().expect("fresh backing");
    assert!(second.register(Owner).is_err());
    let join = frontend.finish();
    let mut registration = std::boxed::Box::pin(registration.run());
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    let core::task::Poll::Ready(Some(completion)) =
        core::future::Future::poll(registration.as_mut(), &mut cx)
    else {
        panic!("ordinary P2 retirement completes");
    };
    drop(registration);
    completion.unregistered();
    let mut join = std::boxed::Box::pin(join.wait());
    let core::task::Poll::Ready(Ok(joined)) = core::future::Future::poll(join.as_mut(), &mut cx)
    else {
        panic!("unregistered P2 publishes Joined");
    };
    drop(join);
    assert!(declarations::second::take_static().is_none());
    let (frontend, registration) = second
        .reinstall(joined, |()| Owner)
        .ok()
        .expect("Joined permits reuse of the same backing");
    drop(frontend);
    drop(registration);
    assert!(declarations::second::take_static().is_none());
}

//! Initial reference-fixture worlds and their generative local slots.
use super::*;
use oer_radio_port::{LocalId, PortBody, PortEnd, PortSlot, with_local_slot};

struct World<'scope, S: ModelSlotOwner<LocalId<'scope>>> {
    slot: Option<PortSlot<LocalId<'scope>, S>>,
}
impl<'scope, S: ModelSlotOwner<LocalId<'scope>>> Drop for World<'scope, S> {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            slot.end_model_world();
        }
    }
}

pub(super) struct Fixture<C = ModelCaps, X = ()> {
    air: Rc<ModelAir>,
    channel: Channel,
    addresses: [IndividualAddress; 2],
    types: PhantomData<fn() -> (C, X)>,
}
impl<C: Ieee80211Capabilities, X: ModelContext> Fixture<C, X> {
    pub(super) fn new(
        channel: Channel,
        addresses: [IndividualAddress; 2],
        clock: Rc<dyn Clock>,
    ) -> (Self, Rc<ModelAir>) {
        let air = Rc::new(ModelAir::new(clock));
        (
            Self {
                air: air.clone(),
                channel,
                addresses,
                types: PhantomData,
            },
            air,
        )
    }
}

pub(super) enum Report<T> {
    Joined(T),
    Stopped(T),
    Abandoned,
}

pub(super) async fn run_model<C, X, P, Out>(world: Fixture<C, X>, body: P) -> Report<Out>
where
    C: Ieee80211Capabilities,
    X: ModelContext,
    for<'scope> P: PortBody<LocalId<'scope>, ModelBackend<LocalId<'scope>, C, X>, Output = Out>,
{
    inspect_model(world, body, |_| {}).await
}

pub(super) async fn inspect_model<C, X, P, Out>(
    fixture: Fixture<C, X>,
    body: P,
    inspect: impl FnOnce(&Report<Out>),
) -> Report<Out>
where
    C: Ieee80211Capabilities,
    X: ModelContext,
    for<'scope> P: PortBody<LocalId<'scope>, ModelBackend<LocalId<'scope>, C, X>, Output = Out>,
{
    with_local_slot(async |claim| {
        let mut world = World {
            slot: Some(claim.place()),
        };
        #[expect(
            clippy::expect_used,
            reason = "a freshly created fixture owns its one model slot"
        )]
        let slot = world.slot.as_mut().expect("fresh model world");
        let backend = ModelBackend::from_air(fixture.channel, fixture.addresses, fixture.air);
        #[expect(
            clippy::expect_used,
            reason = "a fresh generative slot is unregistered"
        )]
        let end = oer_radio_port::LocalExecution
            .scope(
                (slot).register(backend).ok().expect("fresh fixture slot"),
                async |frontend| oer_radio_port::run_port(frontend, body).await,
            )
            .await;
        let report = match end {
            PortEnd::Uninstalled { joined, output } => {
                drop(joined);
                Report::Joined(output)
            }
            PortEnd::FailStopped { outcome, output } => {
                assert!(matches!(
                    outcome.outcome(),
                    oer_radio_port::Abandon::Stopped(_)
                ));
                drop(outcome);
                Report::Stopped(output)
            }
            PortEnd::Abandoned { outcome, .. } => {
                drop(outcome);
                Report::Abandoned
            }
        };
        inspect(&report);
        drop(world);
        report
    })
    .await
}

macro_rules! plain_context {
    ($ty:ty) => {
        impl<Id: SlotIdentity> oer_radio_port::ContextProjection<Id> for $ty {
            type View<'a, 'p>
                = &'a Self
            where
                Self: 'p,
                Id: 'p,
                'p: 'a;
            type Parts<'p>
                = Self
            where
                Self: 'p,
                Id: 'p;
            fn view<'a, 'p>(&'a self, _: oer_radio_port::ContextLoan<'p, Id>) -> Self::View<'a, 'p>
            where
                Self: 'p,
                Id: 'p,
                'p: 'a,
            {
                self
            }
            fn into_parts<'p>(self, _: oer_radio_port::ContextLoan<'p, Id>) -> Self::Parts<'p>
            where
                Self: 'p,
                Id: 'p,
            {
                self
            }
        }
        impl<'p, Id: SlotIdentity + 'p> LoanedContext<'p, Id> for $ty {
            type Owned = Self;
            fn enclose(self) -> oer_radio_port::PortContext<'p, Id, Self> {
                oer_radio_port::PortContext::new(self)
            }
            fn restore(context: oer_radio_port::PortContext<'p, Id, Self>) -> Self {
                context.into_parts()
            }
        }
    };
}

pub(super) use plain_context;

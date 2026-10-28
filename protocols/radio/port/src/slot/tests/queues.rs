use super::*;
use crate::{BodyLoan, Mint, Root, Slot, SlotBody, SlotBodyOwner, SlotWakes, Slots};
use oer_memory::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, LocalSpscReceiver, LocalSpscSender,
};

enum Kind {}
type Token<Id> = Slot<'static, Kind, Root<Id>>;

struct Payload<Id: SlotIdentity> {
    token: Token<Id>,
    context: Rc<Cell<usize>>,
}

enum Command<Id: SlotIdentity> {
    Echo(Payload<Id>),
    Return(Payload<Id>),
}

struct Queues<Id: SlotIdentity> {
    commands: AffineSpscQueue<Command<Id>, 2>,
    events: AffineSpscQueue<Payload<Id>, 1>,
    initial: Option<Token<Id>>,
}

struct Frontend<'s, Id: SlotIdentity> {
    commands: AffineSpscSender<'s, Command<Id>, 2>,
    events: AffineSpscReceiver<'s, Payload<Id>, 1>,
    token: Option<Token<Id>>,
    wakes: SlotWakes<'s, Id>,
}

struct Invocation<'s, Id: SlotIdentity> {
    commands: AffineSpscReceiver<'s, Command<Id>, 2>,
    events: AffineSpscSender<'s, Payload<Id>, 1>,
    wakes: SlotWakes<'s, Id>,
}

struct QueueOwner<'m, Id: SlotIdentity> {
    model: &'m Model,
    returned: Option<Payload<Id>>,
}

struct BodyViews<'p, Id: SlotIdentity, S: SlotOwner<Id>> {
    commands: LocalSpscSender<'p, Command<Id>, 2>,
    events: LocalSpscReceiver<'p, Payload<Id>, 1>,
    token: Option<Slot<'p, Kind, Root<Id>>>,
    loan: BodyLoan<'p, Id, S>,
    wakes: SlotWakes<'p, Id>,
}

struct BodyPayload<'p, Id: SlotIdentity, S: SlotOwner<Id>> {
    token: Option<Slot<'p, Kind, Root<Id>>>,
    context: Rc<Cell<usize>>,
    returns: LocalSpscSender<'p, Command<Id>, 2>,
    loan: BodyLoan<'p, Id, S>,
    wakes: SlotWakes<'p, Id>,
}

impl<Id: SlotIdentity, S: SlotOwner<Id>> BodyPayload<'_, Id, S> {
    fn echo(mut self) {
        let token = self.token.take().expect("one body owner");
        assert!(
            self.returns
                .try_send(Command::Echo(Payload {
                    token: self.loan.reclaim_slot(token),
                    context: self.context.clone(),
                }))
                .is_ok()
        );
        self.wakes.wake_p2();
    }
}

impl<Id: SlotIdentity, S: SlotOwner<Id>> Drop for BodyPayload<'_, Id, S> {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            assert!(
                self.returns
                    .try_send(Command::Return(Payload {
                        token: self.loan.reclaim_slot(token),
                        context: self.context.clone(),
                    }))
                    .is_ok()
            );
            self.wakes.wake_p2();
        }
    }
}

impl<'p, Id: SlotIdentity, S: SlotOwner<Id>> BodyViews<'p, Id, S> {
    fn initial(&mut self, context: Rc<Cell<usize>>) -> BodyPayload<'p, Id, S> {
        BodyPayload {
            token: self.token.take(),
            context,
            returns: self.commands,
            loan: self.loan,
            wakes: self.wakes,
        }
    }

    async fn receive(&mut self) -> BodyPayload<'p, Id, S> {
        let payload = poll_fn(|cx| {
            self.wakes.register_frontend(cx);
            self.events.try_receive().map_or(Poll::Pending, Poll::Ready)
        })
        .await;
        BodyPayload {
            token: Some(self.loan.lend_slot(payload.token)),
            context: payload.context,
            returns: self.commands,
            loan: self.loan,
            wakes: self.wakes,
        }
    }
}

impl<Id: SlotIdentity> SlotBodyOwner<Id> for QueueOwner<'_, Id> {
    type Body<'p>
        = BodyViews<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p;

    fn lend<'p, 's>(
        frontend: &'p mut Self::Frontend<'s>,
        loan: BodyLoan<'p, Id, Self>,
    ) -> Result<Self::Body<'p>, crate::SlotInitError>
    where
        Self: 's,
        Id: 's,
        's: 'p,
    {
        Ok(BodyViews {
            commands: frontend.commands.borrow_local(),
            events: frontend.events.borrow_local(),
            token: frontend.token.take().map(|token| loan.lend_slot(token)),
            loan,
            wakes: frontend.wakes,
        })
    }
}

struct EchoBody(Rc<Cell<usize>>);

struct HoldingBody(Rc<Cell<usize>>);

struct RequestBody {
    context: Rc<Cell<usize>>,
    returns: bool,
}

impl<'m, Id: SlotIdentity> SlotBody<Id, QueueOwner<'m, Id>> for RequestBody {
    type Output = &'static str;

    async fn run<'p>(self, mut views: BodyViews<'p, Id, QueueOwner<'m, Id>>) -> Self::Output
    where
        QueueOwner<'m, Id>: 'p,
        Id: 'p,
    {
        let _payload = views.initial(self.context);
        views.wakes.signals.quarantine();
        if !self.returns {
            pending::<()>().await;
        }
        "requested"
    }
}

impl<'m, Id: SlotIdentity> SlotBody<Id, QueueOwner<'m, Id>> for HoldingBody {
    type Output = ();

    async fn run<'p>(self, mut views: BodyViews<'p, Id, QueueOwner<'m, Id>>)
    where
        QueueOwner<'m, Id>: 'p,
        Id: 'p,
    {
        let _payload = views.initial(self.0);
        pending::<()>().await;
    }
}

impl<'m, Id: SlotIdentity> SlotBody<Id, QueueOwner<'m, Id>> for EchoBody {
    type Output = usize;

    async fn run<'p>(self, mut views: BodyViews<'p, Id, QueueOwner<'m, Id>>) -> usize
    where
        QueueOwner<'m, Id>: 'p,
        Id: 'p,
    {
        views.initial(self.0.clone()).echo();
        let returned = views.receive().await;
        assert!(Rc::ptr_eq(&self.0, &returned.context));
        assert_eq!(
            returned
                .token
                .as_ref()
                .expect("returned body capacity")
                .index(),
            0
        );
        let value = returned.context.get();
        // Drop publishes the entire owned return, without a P1 reference.
        drop(returned);
        value
    }
}

#[test]
fn fresh_body_loan_returns_owned_payloads_without_leaving_a_body_reference_in_p2() {
    let model = model(true);
    let context = Rc::new(Cell::new(1));
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let owner = QueueOwner {
            model: &model,
            returned: None,
        };
        let outer = crate::LocalExecution.scope(
            (&mut slot)
                .register(owner)
                .ok()
                .expect("fresh registration"),
            async |mut frontend| {
                let result = frontend.run_body(EchoBody(context.clone())).await;
                assert!(frontend.finish().wait().await.is_ok());
                result
            },
        );
        let mut outer = Box::pin(outer);
        assert!(poll(outer.as_mut()).is_pending());
        assert_eq!(poll(outer.as_mut()), Poll::Ready(Ok(7)));
        assert_eq!(model.finishes.get(), 1);
        assert_eq!(model.drops.get(), 1);
        assert_eq!(Rc::strong_count(&context), 1);
    }));
}

#[test]
fn cancelling_a_borrowed_body_resource_only_queues_its_whole_owned_return_on_p1() {
    let model = model(false);
    let context = Rc::new(Cell::new(123));
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let owner = QueueOwner {
            model: &model,
            returned: None,
        };
        let outer = crate::LocalExecution.scope(
            (&mut slot)
                .register(owner)
                .ok()
                .expect("synchronous registration"),
            async |mut frontend| {
                model.executor.set(Executor::P1);
                let mut body = Box::pin(frontend.run_body(HoldingBody(context.clone())));
                assert!(poll(body.as_mut()).is_pending());
                drop(body);
                assert_eq!(context.get(), 123, "P1 published, without cleanup");
                assert_eq!(model.abandons.get(), 0);
                drop(frontend);
                assert_eq!(model.drops.get(), 0);
                model.executor.set(Executor::P2);
            },
        );
        finish(outer);
        assert_eq!(model.starts.get(), 0);
        assert_eq!(model.abandons.get(), 1);
        assert_eq!(context.get(), 0, "P2 erased the returned payload");
        assert_eq!(model.drops.get(), 0);
        assert_eq!(Rc::strong_count(&context), 2, "P2 retains the owner");
    }));
}

#[test]
fn supervisor_cancels_an_unrelated_body_wait_but_keeps_a_returned_request() {
    for returns in [false, true] {
        let model = model(false);
        let context = Rc::new(Cell::new(123));
        finish(with_local_slot(async |claim| {
            let mut slot = claim.place();
            let owner = QueueOwner {
                model: &model,
                returned: None,
            };
            let outer = crate::LocalExecution.scope(
                (&mut slot)
                    .register(owner)
                    .ok()
                    .expect("registered P2 before body"),
                async |mut frontend| {
                    model.executor.set(Executor::P1);
                    let result = frontend
                        .run_body(RequestBody {
                            context: context.clone(),
                            returns,
                        })
                        .await;
                    assert_eq!(context.get(), 123, "body only publishes its return");
                    assert_eq!(model.abandons.get(), 0);
                    drop(frontend);
                    model.executor.set(Executor::P2);
                    result
                },
            );
            let expected = if returns {
                Ok("requested")
            } else {
                Err(SlotInitError::Quarantined)
            };
            assert_eq!(finish(outer), expected);
            assert_eq!(
                model.starts.get(),
                0,
                "observe quarantine before driver creation"
            );
            assert_eq!(model.abandons.get(), 1);
            assert_eq!(context.get(), 0);
            assert_eq!(model.drops.get(), 0);
            slot.end_model_world();
            assert_eq!(model.drops.get(), 1);
        }));
        assert_eq!(Rc::strong_count(&context), 1);
    }
}

impl<Id: SlotIdentity> SlotOwner<Id> for QueueOwner<'_, Id> {
    type Vacant = ();
    type Pools = Slots<Kind, 1>;
    type Queues = Queues<Id>;
    const VACANT_QUEUES: Self::Queues = Queues {
        commands: AffineSpscQueue::new(),
        events: AffineSpscQueue::new(),
        initial: None,
    };
    type Frontend<'s>
        = Frontend<'s, Id>
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = Invocation<'s, Id>
    where
        Self: 's,
        Id: 's;

    fn prepare_queues(&mut self, queues: &mut Self::Queues, mint: Mint<'static, Self::Pools, Id>) {
        let [token] = mint.into_pools();
        queues.initial = Some(token);
    }

    fn split<'s>(
        queues: &'s mut Self::Queues,
        wakes: SlotWakes<'s, Id>,
    ) -> Result<(Self::Frontend<'s>, Self::Invocation<'s>), crate::SlotInitError>
    where
        Self: 's,
        Id: 's,
    {
        let (commands_tx, commands_rx) = queues.commands.split();
        let (events_tx, events_rx) = queues.events.split();
        Ok((
            Frontend {
                commands: commands_tx,
                events: events_rx,
                token: queues.initial.take(),
                wakes,
            },
            Invocation {
                commands: commands_rx,
                events: events_tx,
                wakes,
            },
        ))
    }

    async fn run<'s>(
        &mut self,
        invocation: &mut Self::Invocation<'s>,
        retire: SlotRetire<'_, Id, Self>,
    ) -> SlotStopped<Id, Self>
    where
        Self: 's,
        Id: 's,
    {
        assert_eq!(self.model.executor.get(), Executor::P2);
        self.model.starts.set(self.model.starts.get() + 1);
        poll_fn(|_| {
            if let Ok(command) = invocation.commands.try_receive() {
                match command {
                    Command::Echo(payload) => {
                        payload.context.set(7);
                        assert!(invocation.events.try_send(payload).is_ok());
                        invocation.wakes.wake_frontend();
                    }
                    Command::Return(payload) => {
                        assert!(self.returned.replace(payload).is_none());
                    }
                }
            }
            if retire.is_requested() && self.returned.is_some() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        retire.requested().await
    }

    fn abandon<'s>(
        &mut self,
        invocation: Option<&mut Self::Invocation<'s>>,
        authority: Quarantine,
    ) -> Abandon
    where
        Self: 's,
        Id: 's,
    {
        assert_eq!(self.model.executor.get(), Executor::P2);
        self.model.abandons.set(self.model.abandons.get() + 1);
        if let Some(invocation) = invocation {
            while let Ok(Command::Echo(payload) | Command::Return(payload)) =
                invocation.commands.try_receive()
            {
                payload.context.set(0);
                assert!(self.returned.replace(payload).is_none());
            }
        }
        Abandon::Unconfirmed(authority)
    }

    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>)
    where
        Self: 's,
        Id: 's,
    {
        assert_eq!(self.model.executor.get(), Executor::P2);
        assert_eq!(
            self.returned
                .as_ref()
                .expect("returned whole owner")
                .token
                .index(),
            0
        );
        self.model.finishes.set(self.model.finishes.get() + 1);
    }
}

impl<Id: SlotIdentity> Drop for QueueOwner<'_, Id> {
    fn drop(&mut self) {
        assert_eq!(self.model.executor.get(), Executor::P2);
        self.model.drops.set(self.model.drops.get() + 1);
    }
}

impl<Id: SlotIdentity> ModelSlotOwner<Id> for QueueOwner<'_, Id> {
    fn end_world(&mut self, queues: Option<&mut Queues<Id>>) {
        assert_eq!(self.model.executor.get(), Executor::P2);
        if let Some(queues) = queues {
            let (_, commands) = queues.commands.split();
            while let Ok(Command::Echo(payload) | Command::Return(payload)) = commands.try_receive()
            {
                payload.context.set(0);
            }
            let (_, events) = queues.events.split();
            while let Ok(payload) = events.try_receive() {
                payload.context.set(0);
            }
        }
        if let Some(payload) = self.returned.take() {
            payload.context.set(0);
        }
    }

    fn end_vacant_world(_: &mut ()) {}
}

#[test]
fn separate_queue_backing_transfers_whole_owned_tokens_and_non_send_contexts() {
    let model = model(true);
    let context = Rc::new(Cell::new(1));
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let owner = QueueOwner {
            model: &model,
            returned: None,
        };
        let outer = crate::LocalExecution.scope(
            (&mut slot).register(owner).ok().expect("fresh slot"),
            async |mut frontend| {
                model.executor.set(Executor::P1);
                {
                    let ports = frontend.ports();
                    let token = ports.token.take().expect("single owned capacity");
                    assert!(
                        ports
                            .commands
                            .try_send(Command::Echo(Payload {
                                token,
                                context: context.clone()
                            }))
                            .is_ok()
                    );
                    ports.wakes.wake_p2();
                }
                model.executor.set(Executor::P2);
                let payload = poll_fn(|cx| {
                    model.executor.set(Executor::P1);
                    let ports = frontend.ports();
                    ports.wakes.register_frontend(cx);
                    let result = ports
                        .events
                        .try_receive()
                        .map_or(Poll::Pending, Poll::Ready);
                    model.executor.set(Executor::P2);
                    result
                })
                .await;
                assert!(Rc::ptr_eq(&context, &payload.context));
                assert_eq!(context.get(), 7);
                assert_eq!(payload.token.index(), 0);
                model.executor.set(Executor::P1);
                let ports = frontend.ports();
                assert!(ports.commands.try_send(Command::Return(payload)).is_ok());
                ports.wakes.wake_p2();
                let join = frontend.finish();
                model.executor.set(Executor::P2);
                join.wait().await.is_ok()
            },
        );
        let mut outer = Box::pin(outer);
        assert!(poll(outer.as_mut()).is_pending());
        assert_eq!(poll(outer.as_mut()), Poll::Ready(true));
        assert_eq!(model.finishes.get(), 1);
        assert_eq!(model.drops.get(), 1);
    }));
}

#[test]
fn cancellation_before_first_driver_poll_preserves_queue_endpoints_for_p2_cleanup() {
    let model = model(false);
    let context = Rc::new(Cell::new(123));
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let owner = QueueOwner {
            model: &model,
            returned: None,
        };
        let outer = crate::LocalExecution.scope(
            (&mut slot)
                .register(owner)
                .ok()
                .expect("registered before frontend construction"),
            async |mut frontend| {
                model.executor.set(Executor::P1);
                let ports = frontend.ports();
                let token = ports.token.take().expect("single capacity");
                assert!(
                    ports
                        .commands
                        .try_send(Command::Echo(Payload {
                            token,
                            context: context.clone()
                        }))
                        .is_ok()
                );
                drop(frontend);
                assert_eq!(context.get(), 123);
                assert_eq!(model.abandons.get(), 0);
                model.executor.set(Executor::P2);
            },
        );
        finish(outer);
        assert_eq!(model.starts.get(), 0);
        assert_eq!(model.abandons.get(), 1);
        assert_eq!(context.get(), 0);
        assert_eq!(model.drops.get(), 0);
        assert_eq!(
            Rc::strong_count(&context),
            2,
            "queued owner is retained, never dropped on P1"
        );
        slot.end_model_world();
        assert_eq!(model.drops.get(), 1);
        assert_eq!(Rc::strong_count(&context), 1, "model world ended on P2");
    }));
}

struct EmptyBody;
impl<'m, Id: SlotIdentity> SlotBody<Id, QueueOwner<'m, Id>> for EmptyBody {
    type Output = ();
    async fn run<'p>(self, _: BodyViews<'p, Id, QueueOwner<'m, Id>>)
    where
        QueueOwner<'m, Id>: 'p,
        Id: 'p,
    {
    }
}

#[test]
fn a_registration_never_lends_its_initial_body_resources_twice() {
    let model = model(true);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let owner = QueueOwner {
            model: &model,
            returned: None,
        };
        let outer = crate::LocalExecution.scope(
            (&mut slot)
                .register(owner)
                .ok()
                .expect("fresh registration"),
            async |mut frontend| {
                assert_eq!(frontend.run_body(EmptyBody).await, Ok(()));
                assert_eq!(
                    frontend.run_body(EmptyBody).await,
                    Err(SlotInitError::BodyAlreadyIssued)
                );
                assert!(frontend.abandon().wait().await.is_err());
            },
        );
        finish(outer);
        assert_eq!(model.abandons.get(), 1);
        assert_eq!(model.finishes.get(), 0);
        slot.end_model_world();
    }));
}

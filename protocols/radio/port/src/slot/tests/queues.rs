use super::*;
use crate::{Mint, Root, Slot, SlotWakes, Slots};
use oer_memory::{AffineSpscQueue, AffineSpscReceiver, AffineSpscSender};

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

impl<Id: SlotIdentity> SlotOwner<Id> for QueueOwner<'_, Id> {
    type Pools = Slots<Kind, 1>;
    type Queues = Queues<Id>;
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

    fn queues(&mut self, mint: Mint<'static, Self::Pools, Id>) -> Self::Queues {
        let [token] = mint.into_pools();
        Queues {
            commands: AffineSpscQueue::new(),
            events: AffineSpscQueue::new(),
            initial: Some(token),
        }
    }

    fn split<'s>(
        queues: &'s mut Self::Queues,
        wakes: SlotWakes<'s, Id>,
    ) -> (Self::Frontend<'s>, Self::Invocation<'s>)
    where
        Self: 's,
        Id: 's,
    {
        let (commands_tx, commands_rx) = queues.commands.split();
        let (events_tx, events_rx) = queues.events.split();
        (
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
        )
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

    fn finish(self) {
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
        let outer = crate::LocalExecution
            .scope(&mut slot, owner, async |mut frontend| {
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
            })
            .ok()
            .expect("fresh slot");
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
        let outer = crate::LocalExecution
            .scope(&mut slot, owner, async |mut frontend| {
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
            })
            .ok()
            .expect("registered before frontend construction");
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
    }));
}

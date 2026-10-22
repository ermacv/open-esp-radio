//! Durable queue ownership and P1-only borrowed views.
use super::{Backend, Environment, Fault, ModelCaps, Stimulus};
use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    future::poll_fn,
    task::{Poll, Waker},
};
use oer_bluetooth_le_port::{
    backend::{OwnedCommand, OwnedRequest, OwnedSession},
    *,
};
use oer_radio_port::{
    BodyLoan, Child, Correlation, Permit, PortContext, PortPhase, Root, Seat, SlotIdentity,
};
use oer_time::Instant;

pub(crate) const CAPACITY: usize = 2;
pub(crate) const SEATS: usize = 3;
type Path<Id> = Root<Id>;
type Capacity<Id> = Permit<'static, CommandCapacity, Child<Path<Id>, 0>>;
use super::capacity::Pool;
type Retained<Id> = (OwnedCommand<Path<Id>, ModelCaps>, Capacity<Id>);
pub(crate) type Work<Id, X> = OwnedRequest<Path<Id>, X, ModelCaps>;

pub(crate) enum SessionOwner<Id: SlotIdentity> {
    Idle(OwnedSession<Path<Id>, Idle>),
    Running(OwnedSession<Path<Id>, Running>),
    Quarantined,
}
pub(crate) struct OwnedEvent<Id: SlotIdentity, X> {
    pub admitted: Admitted,
    pub at: Instant,
    pub result: EventResult,
    pub cause: Cause,
    pub count: Option<PacketCount>,
    pub session: SessionOwner<Id>,
    pub context: X,
    pub capacity: Option<Capacity<Id>>,
}
impl<Id: SlotIdentity, X> OwnedEvent<Id, X> {
    pub fn lend<'p>(self, loan: &BodyLoan<'p, Id, Backend<Id, X>>) -> Terminal<'p, Path<Id>, X> {
        let session = match self.session {
            SessionOwner::Idle(s) => ReturnedSession::Idle(s.lend(loan)),
            SessionOwner::Running(s) => ReturnedSession::Running(s.lend(loan)),
            SessionOwner::Quarantined => ReturnedSession::Quarantined,
        };
        Terminal {
            admitted: self.admitted,
            admitted_at: self.at,
            result: self.result,
            cause: self.cause,
            completion: Completion::DirectTest {
                count: self.count,
                session,
            },
            context: PortContext::from_owned(self.context, loan),
        }
    }
    pub fn retain<'p>(
        t: Terminal<'p, Path<Id>, X>,
        loan: &BodyLoan<'p, Id, Backend<Id, X>>,
    ) -> Self {
        let Completion::DirectTest { session, count } = t.completion;
        let session = match session {
            ReturnedSession::Idle(s) => SessionOwner::Idle(OwnedSession::retain(s, loan)),
            ReturnedSession::Running(s) => SessionOwner::Running(OwnedSession::retain(s, loan)),
            ReturnedSession::Quarantined => SessionOwner::Quarantined,
        };
        Self {
            admitted: t.admitted,
            at: t.admitted_at,
            result: t.result,
            cause: t.cause,
            count,
            session,
            context: t.context.into_owned(loan),
            capacity: None,
        }
    }
}
pub(crate) struct Shared<Id: SlotIdentity, X> {
    pub env: Rc<Environment>,
    pub radio: usize,
    pub epoch: u64,
    pub pool: Pool<Id>,
    pub pending: RefCell<VecDeque<(Work<Id, X>, Instant)>>,
    pub events: RefCell<VecDeque<OwnedEvent<Id, X>>>,
    pub quarantine: RefCell<Vec<Retained<Id>>>,
    pub outstanding: RefCell<Vec<LeOperationId>>,
    pub poisoned_requests: RefCell<Vec<Work<Id, X>>>,
    pub enabled: Cell<bool>,
    pub draining: Cell<bool>,
    pub end: Cell<Option<EventEnd<Fault>>>,
    pub fault: Cell<Option<Fault>>,
    pub body: RefCell<Option<Waker>>,
    pub driver: RefCell<Option<Waker>>,
}
impl<Id: SlotIdentity, X> Shared<Id, X> {
    pub fn wake_driver(&self) {
        if let Some(w) = self.driver.borrow().as_ref() {
            w.wake_by_ref();
        }
    }
    pub fn wake_body(&self) {
        if let Some(w) = self.body.borrow().as_ref() {
            w.wake_by_ref();
        }
    }
    pub fn poison(&self, fault: Fault) {
        let cause = self.fault.get().unwrap_or(fault);
        self.fault.set(Some(cause));
        // An exposed final end remains stable. Pending or queued terminals
        // prevent Drained from being exposed, so undelivered owners can still
        // escalate that unobserved end to accounting quarantine.
        if self.end.get().is_none()
            || !self.events.borrow().is_empty()
            || !self.pending.borrow().is_empty()
        {
            self.end.set(Some(EventEnd::Quarantined { cause }));
        }
        self.enabled.set(false);
        self.wake_body();
        self.wake_driver();
    }
    #[allow(
        clippy::expect_used,
        reason = "every admitted work carries exactly one capacity bundle"
    )]
    pub fn serve(&self) {
        if self.fault.get().is_some() && !self.draining.get() {
            return;
        }
        let Some((work, at)) = self.pending.borrow_mut().pop_front() else {
            if self.draining.replace(false) {
                self.env.stop(self.radio);
                self.enabled.set(false);
                self.end.set(Some(EventEnd::Drained {
                    cause: self.fault.get().unwrap_or(Fault::Disabled),
                }));
                self.wake_body();
            }
            return;
        };
        let input = if self.draining.get() {
            None
        } else {
            self.env.stimulus(self.radio)
        };
        if input == Some(Stimulus::StaleEpoch) && self.env.next_epoch(self.radio).is_err() {
            self.poison(Fault::EpochExhausted);
        }
        let cause = if self.fault.get() == Some(Fault::EpochExhausted) {
            Cause::Fault
        } else if work.command.epoch() != self.env.epoch(self.radio) {
            Cause::StaleEpoch
        } else if self.draining.get() {
            Cause::Disabled
        } else {
            match input {
                Some(Stimulus::AirDenied) => Cause::AirDenied,
                Some(Stimulus::Withdrawn) => Cause::Withdrawn,
                Some(Stimulus::Indeterminate | Stimulus::DrainedFault) => Cause::Fault,
                None | Some(Stimulus::StaleEpoch) => Cause::Complete,
            }
        };
        let result = match cause {
            Cause::Complete => EventResult::Executed { anchor: None },
            Cause::Withdrawn => EventResult::Aborted,
            Cause::Fault => {
                if input == Some(Stimulus::Indeterminate) {
                    EventResult::Indeterminate
                } else {
                    EventResult::NotExecuted
                }
            }
            Cause::Disabled | Cause::StaleEpoch | Cause::AirDenied => EventResult::NotExecuted,
        };
        let Work {
            id,
            capacity,
            command,
            context,
        } = work;
        let admitted = Admitted {
            id,
            epoch: command.epoch(),
        };
        let mut count = None;
        let mut capacity = Some(capacity);
        let session = match result {
            EventResult::Indeterminate => {
                // Keep both the actual submitted lease and capacity, not identities.
                self.quarantine
                    .borrow_mut()
                    .push((command, capacity.take().expect("admitted command capacity")));
                self.poison(Fault::Injected);
                SessionOwner::Quarantined
            }
            EventResult::Executed { .. } => match command {
                OwnedCommand::Transmit(s, config) => {
                    self.env.transmit(self.radio, *config.get());
                    SessionOwner::Running(s.transition())
                }
                OwnedCommand::Receive(s, config) => {
                    self.env.receive(self.radio, *config.get());
                    SessionOwner::Running(s.transition())
                }
                OwnedCommand::End(s) => {
                    count = Some(self.env.stop(self.radio));
                    SessionOwner::Idle(s.transition())
                }
            },
            EventResult::Aborted => {
                self.env.stop(self.radio);
                match command {
                    OwnedCommand::Transmit(s, _) | OwnedCommand::Receive(s, _) => {
                        SessionOwner::Idle(s)
                    }
                    OwnedCommand::End(s) => SessionOwner::Idle(s.transition()),
                }
            }
            EventResult::NotExecuted => match command {
                OwnedCommand::Transmit(s, _) | OwnedCommand::Receive(s, _) => SessionOwner::Idle(s),
                OwnedCommand::End(s) => SessionOwner::Running(s),
            },
        };
        if input == Some(Stimulus::DrainedFault) {
            self.fault.set(Some(Fault::Injected));
            self.draining.set(true);
        }
        self.events.borrow_mut().push_back(OwnedEvent {
            admitted,
            at,
            result,
            cause,
            count,
            session,
            context,
            capacity,
        });
        self.wake_body();
        self.wake_driver();
    }
    pub fn delivered(&self, id: LeOperationId, capacity: Option<Capacity<Id>>) {
        if let Some(capacity) = capacity {
            self.pool.release(capacity);
        }
        self.outstanding.borrow_mut().retain(|held| *held != id);
    }
    pub fn pop(&self) -> Option<OwnedEvent<Id, X>> {
        let mut event = self.events.borrow_mut().pop_front()?;
        self.delivered(event.admitted.id, event.capacity.take());
        Some(event)
    }
}
/// Separate queues and initial resources retained by the slot through quarantine.
pub struct Queues<Id: SlotIdentity, X> {
    pub(crate) shared: Shared<Id, X>,
    pub(crate) seats: Option<[Seat<'static, CommandCapacity, Child<Path<Id>, 0>>; SEATS]>,
    pub(crate) idle: Option<OwnedSession<Path<Id>, Idle>>,
}
/// One running P1 phase. It borrows queues, never the backend or environment's
/// driver operations; the loan conversion authority is private.
pub struct ModelPhase<'a, 'p, Id: SlotIdentity, X> {
    pub(crate) view: &'a super::View<'p, Id, X>,
}
impl<'p, Id: SlotIdentity, X> LePhase<'p, Path<Id>, X> for ModelPhase<'_, 'p, Id, X> {
    type Caps = ModelCaps;
    async fn take<'a>(
        &'a self,
        seat: &'a mut Seat<'p, CommandCapacity, Child<Path<Id>, 0>>,
    ) -> Permit<'p, CommandCapacity, Child<Path<Id>, 0>>
    where
        'p: 'a,
    {
        // Borrowing the typed seat keeps its unique index occupied on P1.
        let capacity = self.view.shared.pool.take(seat.index()).await;
        self.view.loan.lend_permit(capacity)
    }
    fn release(&self, capacity: Permit<'p, CommandCapacity, Child<Path<Id>, 0>>) {
        self.view
            .shared
            .pool
            .release(self.view.loan.reclaim_permit(capacity));
    }
    fn submit(
        &self,
        request: Request<'p, Path<Id>, X, ModelCaps>,
    ) -> PortResult<Admitted, Refused<Request<'p, Path<Id>, X, ModelCaps>>, Fault> {
        let q = self.view.shared;
        if let Some(fault) = q.fault.get() {
            let owned = request.retain(&self.view.loan);
            // Poison consumes custody into durable backing even before admission.
            q.poisoned_requests.borrow_mut().push(owned);
            return Err(oer_radio_port::Poisoned { cause: fault });
        }
        let refusal = if request.id.is_backend_reserved() {
            Some(Refusal::ReservedId)
        } else if request.epoch() != q.epoch || q.epoch != q.env.epoch(q.radio) {
            Some(Refusal::StaleEpoch)
        } else if q.outstanding.borrow().contains(&request.id) {
            Some(Refusal::DuplicateId)
        } else if !q.enabled.get() {
            Some(Refusal::Unavailable)
        } else {
            None
        };
        if let Some(reason) = refusal {
            return Ok(Err(Refused { reason, request }));
        }
        let at = q.env.now();
        let receipt = Admitted {
            id: request.id,
            epoch: request.epoch(),
        };
        q.outstanding.borrow_mut().push(request.id);
        q.pending
            .borrow_mut()
            .push_back((request.retain(&self.view.loan), at));
        q.wake_driver();
        Ok(Ok(receipt))
    }
}
impl<'p, Id: SlotIdentity, X> PortPhase for ModelPhase<'_, 'p, Id, X> {
    type Event = Event<'p, Path<Id>, X>;
    type Fault = Fault;
    async fn next_event(&mut self) -> Result<Result<Self::Event, EventsLost>, EventEnd<Fault>> {
        let q = self.view.shared;
        let _wait = BodyWait(q);
        poll_fn(|cx| {
            *q.body.borrow_mut() = Some(cx.waker().clone());
            if let Some(event) = q.pop() {
                return Poll::Ready(Ok(Ok(Event::Terminal(event.lend(&self.view.loan)))));
            }
            if let Some(end) = q.end.get() {
                Poll::Ready(Err(end))
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

pub(crate) struct BodyWait<'a, Id: SlotIdentity, X>(pub &'a Shared<Id, X>);
impl<Id: SlotIdentity, X> Drop for BodyWait<'_, Id, X> {
    fn drop(&mut self) {
        self.0.body.borrow_mut().take();
    }
}

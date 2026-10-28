//! Durable queue ownership and P1-only borrowed views.
use super::{
    Backend, Environment, Fault, ModelCaps, Stimulus,
    air::Radio,
    storage::{Local, WakerSlot},
};
use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use core::{cell::Cell, future::poll_fn, task::Poll};
use oer_bluetooth_le_port::{
    backend::{OwnedCommand, OwnedRequest, OwnedSession},
    *,
};
use oer_radio_port::{
    BodyLoan, Child, Correlation, FifoPermitPool, Permit, PermitPool, PortContext, PortPhase, Root,
    Seat, SlotIdentity,
};
use oer_time::Instant;

pub(crate) const CAPACITY: usize = 2;
pub(crate) const SEATS: usize = 3;
type Path<Id> = Root<Id>;
type Capacity<Id> = Permit<'static, CommandCapacity, Child<Path<Id>, 0>>;
/// The FIFO pool of command capacity and its line of body seats.
pub(crate) type Pool<Id> =
    FifoPermitPool<'static, CommandCapacity, CAPACITY, SEATS, Child<Path<Id>, 0>>;
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
pub(crate) struct Attachment {
    pub env: Rc<Environment>,
    pub radio: Rc<Radio>,
}
pub(crate) struct Shared<Id: SlotIdentity, X> {
    pub attachment: Option<Attachment>,
    pub epoch: u64,
    /// Present once the queues are prepared; a body view borrows it.
    pub pool: Option<Pool<Id>>,
    pub pending: Local<VecDeque<(Work<Id, X>, Instant)>>,
    pub events: Local<VecDeque<OwnedEvent<Id, X>>>,
    pub quarantine: Local<Vec<Retained<Id>>>,
    pub outstanding: Local<Vec<LeOperationId>>,
    pub poisoned_requests: Local<Vec<Work<Id, X>>>,
    pub enabled: Cell<bool>,
    pub draining: Cell<bool>,
    pub end: Cell<Option<EventEnd<Fault>>>,
    pub fault: Cell<Option<Fault>>,
    pub body: WakerSlot,
    pub driver: WakerSlot,
}
impl<Id: SlotIdentity, X> Shared<Id, X> {
    pub const fn vacant() -> Self {
        Self {
            attachment: None,
            epoch: 0,
            pool: None,
            pending: Local::new(VecDeque::new()),
            events: Local::new(VecDeque::new()),
            quarantine: Local::new(Vec::new()),
            outstanding: Local::new(Vec::new()),
            poisoned_requests: Local::new(Vec::new()),
            enabled: Cell::new(false),
            draining: Cell::new(false),
            end: Cell::new(Some(EventEnd::Quarantined {
                cause: Fault::UnpreparedQueues,
            })),
            fault: Cell::new(Some(Fault::UnpreparedQueues)),
            body: WakerSlot::new(),
            driver: WakerSlot::new(),
        }
    }
    pub fn attachment(&self) -> Result<&Attachment, Fault> {
        self.attachment.as_ref().ok_or(Fault::UnpreparedQueues)
    }
    pub fn lend_initial<'p>(
        &self,
        initial: &mut Option<Initial<Id>>,
        loan: &BodyLoan<'p, Id, Backend<Id, X>>,
    ) -> Result<super::Resources<'p, Id>, oer_radio_port::SlotInitError> {
        if self.attachment.is_none() {
            return Err(oer_radio_port::SlotInitError::QueuesUnprepared);
        }
        Initial::lend(initial, loan)
    }
    pub fn wake_driver(&self) {
        self.driver.wake();
    }
    pub fn wake_body(&self) {
        self.body.wake();
    }
    pub fn poison(&self, fault: Fault) {
        let cause = self.fault.get().unwrap_or(fault);
        self.fault.set(Some(cause));
        // An exposed final end remains stable. Pending or queued terminals
        // prevent Drained from being exposed, so undelivered owners can still
        // escalate that unobserved end to accounting quarantine.
        if self.end.get().is_none()
            || !self.events.with(|events| events.is_empty())
            || !self.pending.with(|pending| pending.is_empty())
        {
            self.end.set(Some(EventEnd::Quarantined { cause }));
        }
        self.enabled.set(false);
        self.wake_body();
        self.wake_driver();
    }
    pub fn serve(&self) {
        if self.fault.get().is_some() && !self.draining.get() {
            return;
        }
        let attachment = match self.attachment() {
            Ok(attachment) => attachment,
            Err(fault) => {
                self.poison(fault);
                return;
            }
        };
        let Some((work, at)) = self.pending.with(VecDeque::pop_front) else {
            if self.draining.replace(false) {
                attachment.env.stop(&attachment.radio);
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
            attachment.env.stimulus(&attachment.radio)
        };
        if input == Some(Stimulus::StaleEpoch)
            && attachment.env.next_epoch(&attachment.radio).is_err()
        {
            self.poison(Fault::EpochExhausted);
        }
        let cause = if self.fault.get() == Some(Fault::EpochExhausted) {
            Cause::Fault
        } else if work.command.epoch() != attachment.env.epoch(&attachment.radio) {
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
        let (session, capacity) = match result {
            EventResult::Indeterminate => {
                self.quarantine
                    .with(|quarantine| quarantine.push((command, capacity)));
                self.poison(Fault::Injected);
                (SessionOwner::Quarantined, None)
            }
            EventResult::Executed { .. } => {
                let session = match command {
                    OwnedCommand::Transmit(s, config) => {
                        attachment.env.transmit(&attachment.radio, *config.get());
                        SessionOwner::Running(s.transition())
                    }
                    OwnedCommand::Receive(s, config) => {
                        attachment.env.receive(&attachment.radio, *config.get());
                        SessionOwner::Running(s.transition())
                    }
                    OwnedCommand::End(s) => {
                        count = Some(attachment.env.stop(&attachment.radio));
                        SessionOwner::Idle(s.transition())
                    }
                };
                (session, Some(capacity))
            }
            EventResult::Aborted => {
                attachment.env.stop(&attachment.radio);
                let session = match command {
                    OwnedCommand::Transmit(s, _) | OwnedCommand::Receive(s, _) => {
                        SessionOwner::Idle(s)
                    }
                    OwnedCommand::End(s) => SessionOwner::Idle(s.transition()),
                };
                (session, Some(capacity))
            }
            EventResult::NotExecuted => {
                let session = match command {
                    OwnedCommand::Transmit(s, _) | OwnedCommand::Receive(s, _) => {
                        SessionOwner::Idle(s)
                    }
                    OwnedCommand::End(s) => SessionOwner::Running(s),
                };
                (session, Some(capacity))
            }
        };
        if input == Some(Stimulus::DrainedFault) {
            self.fault.set(Some(Fault::Injected));
            self.draining.set(true);
        }
        self.events.with(|events| {
            events.push_back(OwnedEvent {
                admitted,
                at,
                result,
                cause,
                count,
                session,
                context,
                capacity,
            })
        });
        self.wake_body();
        self.wake_driver();
    }
    pub fn delivered(&self, pool: &Pool<Id>, id: LeOperationId, capacity: Option<Capacity<Id>>) {
        if let Some(capacity) = capacity {
            pool.release(capacity);
        }
        self.outstanding
            .with(|outstanding| outstanding.retain(|held| *held != id));
    }
    pub fn pop(&self, pool: &Pool<Id>) -> Option<OwnedEvent<Id, X>> {
        let mut event = self.events.with(VecDeque::pop_front)?;
        self.delivered(pool, event.admitted.id, event.capacity.take());
        Some(event)
    }
}
/// Separate queues and initial resources retained by the slot through quarantine.
pub struct Queues<Id: SlotIdentity, X> {
    pub(crate) shared: Shared<Id, X>,
    pub(crate) initial: Option<Initial<Id>>,
}
pub(crate) struct Initial<Id: SlotIdentity> {
    pub seats: [Seat<'static, CommandCapacity, Child<Path<Id>, 0>>; SEATS],
    pub idle: OwnedSession<Path<Id>, Idle>,
}
impl<Id: SlotIdentity> Initial<Id> {
    pub fn lend<'p, X>(
        initial: &mut Option<Self>,
        loan: &BodyLoan<'p, Id, Backend<Id, X>>,
    ) -> Result<super::Resources<'p, Id>, oer_radio_port::SlotInitError> {
        let initial = initial
            .take()
            .ok_or(oer_radio_port::SlotInitError::MissingResource(
                "BLE initial resources",
            ))?;
        Ok(super::Resources {
            seats: initial.seats.map(|seat| loan.lend_seat(seat)),
            session: initial.idle.lend(loan),
        })
    }
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
        self.view.pool.take_loaned(seat, self.view.loan).await
    }
    fn release(&self, capacity: Permit<'p, CommandCapacity, Child<Path<Id>, 0>>) {
        self.view
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
            q.poisoned_requests.with(|requests| requests.push(owned));
            return Err(oer_radio_port::Poisoned { cause: fault });
        }
        let attachment = match q.attachment() {
            Ok(attachment) => attachment,
            Err(fault) => {
                q.poisoned_requests
                    .with(|requests| requests.push(request.retain(&self.view.loan)));
                q.poison(fault);
                return Err(oer_radio_port::Poisoned { cause: fault });
            }
        };
        let refusal = if request.id.is_backend_reserved() {
            Some(Refusal::ReservedId)
        } else if request.epoch() != q.epoch || q.epoch != attachment.env.epoch(&attachment.radio) {
            Some(Refusal::StaleEpoch)
        } else if q
            .outstanding
            .with(|outstanding| outstanding.contains(&request.id))
        {
            Some(Refusal::DuplicateId)
        } else if !q.enabled.get() {
            Some(Refusal::Unavailable)
        } else {
            None
        };
        if let Some(reason) = refusal {
            return Ok(Err(Refused { reason, request }));
        }
        let at = attachment.env.now();
        let receipt = Admitted {
            id: request.id,
            epoch: request.epoch(),
        };
        q.outstanding
            .with(|outstanding| outstanding.push(request.id));
        q.pending
            .with(|pending| pending.push_back((request.retain(&self.view.loan), at)));
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
            q.body.register(cx.waker());
            if let Some(event) = q.pop(self.view.pool) {
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
        self.0.body.clear();
    }
}

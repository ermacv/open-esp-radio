//! Complete terminal owners retained across queueing and sink rejection.
use super::*;

pub(super) enum SessionOwner<Id: SlotIdentity> {
    Idle(OwnedSession<Path<Id>, Idle>),
    Running(OwnedSession<Path<Id>, Running>),
    Quarantined,
}
pub(super) struct OwnedTerminal<Id: SlotIdentity, X> {
    pub admitted: Admitted,
    pub at: Instant,
    pub result: EventResult,
    pub cause: Cause,
    pub session: SessionOwner<Id>,
    pub count: Option<PacketCount>,
    pub context: X,
    pub credit: Option<Credit<Id>>,
}
impl<Id: SlotIdentity, X> OwnedTerminal<Id, X> {
    pub fn lend<'p, D: Device>(
        self,
        loan: &BodyLoan<'p, Id, Backend<Id, D, X>>,
    ) -> Terminal<'p, Path<Id>, X> {
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
                session,
                count: self.count,
            },
            context: PortContext::from_owned(self.context, loan),
        }
    }
    pub fn retain<'p, D: Device>(
        t: Terminal<'p, Path<Id>, X>,
        loan: &BodyLoan<'p, Id, Backend<Id, D, X>>,
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
            session,
            count,
            context: t.context.into_owned(loan),
            credit: None,
        }
    }
}
impl<Id: SlotIdentity, D: Device, X> Backend<Id, D, X> {
    pub(super) async fn serve(&mut self, draining: bool) -> Result<(), Fault> {
        let device = self
            .device
            .as_mut()
            .expect("registered backend retains its device");
        let (request, _) = self
            .current
            .as_ref()
            .expect("current work is durable before serving");
        let stale = request.command.epoch() != device.epoch();
        let mut count = None;
        let outcome = if stale {
            Ok(Started::NotExecuted(Cause::StaleEpoch))
        } else if draining {
            Ok(Started::NotExecuted(Cause::Disabled))
        } else {
            match &request.command {
                OwnedCommand::Transmit(_, config) => {
                    device.start(Start::Transmit(*config.get())).await
                }
                OwnedCommand::Receive(_, config) => {
                    device.start(Start::Receive(*config.get())).await
                }
                OwnedCommand::End(_) => device.end().await.map(|n| {
                    count = Some(n);
                    Started::Executed
                }),
            }
        };
        // Cancellation before here leaves the exact whole request in current.
        let (
            OwnedRequest {
                id,
                capacity,
                command,
                context,
            },
            at,
        ) = self.current.take().expect("one served work owner");
        let admitted = Admitted {
            id,
            epoch: command.epoch(),
        };
        let result = match outcome {
            Ok(Started::Executed) => EventResult::Executed { anchor: None },
            Ok(Started::NotExecuted(_)) => EventResult::NotExecuted,
            Ok(Started::Aborted) => EventResult::Aborted,
            Err(_) => EventResult::Indeterminate,
        };
        let cause = match outcome {
            Ok(Started::Executed) => Cause::Complete,
            Ok(Started::NotExecuted(c)) => c,
            Ok(Started::Aborted) => Cause::Withdrawn,
            Err(_) => Cause::Fault,
        };
        let (session, credit) = match (command, outcome) {
            (OwnedCommand::Transmit(s, _) | OwnedCommand::Receive(s, _), Ok(Started::Executed)) => {
                (owned::SessionOwner::Running(s.transition()), Some(capacity))
            }
            (OwnedCommand::Transmit(s, _) | OwnedCommand::Receive(s, _), Ok(_)) => {
                (owned::SessionOwner::Idle(s), Some(capacity))
            }
            (OwnedCommand::End(s), Ok(Started::Executed)) => {
                (owned::SessionOwner::Idle(s.transition()), Some(capacity))
            }
            (OwnedCommand::End(s), Ok(_)) => (owned::SessionOwner::Running(s), Some(capacity)),
            (command, Err(_)) => {
                self.quarantine = Some((command, capacity));
                (owned::SessionOwner::Quarantined, None)
            }
        };
        self.terminal = Some(OwnedTerminal {
            admitted,
            at,
            result,
            cause,
            session,
            count,
            context,
            credit,
        });
        outcome.map(|_| ())
    }
}

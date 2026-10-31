//! One-shot results of the station's control requests.
//!
//! Each command kind has one permission. An admitted request holds it until
//! the service delivers that request's terminal; the [`Operation`] returned
//! to the caller only observes the terminal. Dropping the observer detaches
//! it: the request continues and its terminal still returns the permission,
//! so the next request of that kind waits for it instead of posting a
//! duplicate. Requests carry an internal correlation, so a late terminal
//! belongs only to its own request.

use super::super::ServiceOutput;
use super::super::contract::{Command, CommandOutcome, CommandRefusal};
use core::{
    cell::Cell,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use oer_sync::PriorityWaker;

/// The actual terminal of one admitted control request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationEnd {
    /// The station completed the request.
    Completed(CommandOutcome),
    /// The station refused the request in its current state.
    Refused(CommandRefusal),
    /// Another request of this kind, such as a reconnect attempt, held the
    /// station's credit; this request was not started.
    CreditHeld,
    /// The service ended without this request's terminal.
    ServiceEnded,
}

/// Correlations of operations; the reconnect policy and raw producers keep
/// values below this range.
pub(super) const FIRST_CORRELATION: u64 = 1 << 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Permission {
    /// No request of this kind is admitted.
    Free,
    /// The request holds the permission; `observed` while its operation lives.
    Held { correlation: u64, observed: bool },
    /// The terminal arrived and waits for its observer.
    Ended { correlation: u64, end: OperationEnd },
}

/// One command kind's permission and its two waiters: the observer of the
/// admitted request, and the control waiting to admit the next one.
pub(super) struct Kind {
    permission: Cell<Permission>,
    observer: PriorityWaker,
    admission: PriorityWaker,
}

impl Kind {
    const fn new() -> Self {
        Self {
            permission: Cell::new(Permission::Free),
            observer: PriorityWaker::new(),
            admission: PriorityWaker::new(),
        }
    }
    fn release(&self) {
        self.permission.set(Permission::Free);
        self.admission.wake();
    }
}

/// The permissions of the three command kinds.
pub(super) struct Operations {
    connect: Kind,
    disconnect: Kind,
    scan: Kind,
    next: Cell<u64>,
}

impl Operations {
    pub(super) const fn new() -> Self {
        Self {
            connect: Kind::new(),
            disconnect: Kind::new(),
            scan: Kind::new(),
            next: Cell::new(FIRST_CORRELATION),
        }
    }
    pub(super) fn reset(&self) {
        for kind in [&self.connect, &self.disconnect, &self.scan] {
            kind.release();
        }
    }
    pub(super) fn kind(&self, command: Command) -> &Kind {
        match command {
            Command::Connect => &self.connect,
            Command::Disconnect => &self.disconnect,
            Command::BackgroundScan => &self.scan,
        }
    }
    /// Ready once the kind's permission returned; the next correlation.
    pub(super) fn poll_free(&self, command: Command, cx: &mut Context<'_>) -> Poll<u64> {
        let kind = self.kind(command);
        kind.admission.register(cx.waker());
        if kind.permission.get() != Permission::Free {
            return Poll::Pending;
        }
        let correlation = self.next.get();
        // Three kinds admit one request each between terminals; the range
        // above the policy's correlations is never exhausted, and wrapping
        // re-enters it rather than the reserved values.
        self.next
            .set(correlation.checked_add(1).unwrap_or(FIRST_CORRELATION));
        Poll::Ready(correlation)
    }
    /// The request was admitted to the service: it now holds the permission.
    pub(super) fn admitted(&self, command: Command, correlation: u64) {
        self.kind(command).permission.set(Permission::Held {
            correlation,
            observed: true,
        });
    }
    /// Record a request's terminal; another producer's terminal is ignored.
    pub(super) fn observe<K>(&self, output: &ServiceOutput<K>) {
        let (request, end) = match output {
            ServiceOutput::CommandCompleted(terminal) => {
                (terminal.request, OperationEnd::Completed(terminal.outcome))
            }
            ServiceOutput::CommandRefused(refused) => {
                (refused.request, OperationEnd::Refused(refused.reason))
            }
            ServiceOutput::CommandInFlight(request) => (*request, OperationEnd::CreditHeld),
            _ => return,
        };
        let kind = self.kind(request.command());
        match kind.permission.get() {
            Permission::Held {
                correlation,
                observed: true,
            } if correlation == request.correlation() => {
                kind.permission.set(Permission::Ended { correlation, end });
                kind.observer.wake();
            }
            Permission::Held {
                correlation,
                observed: false,
            } if correlation == request.correlation() => kind.release(),
            _ => {}
        }
    }
    /// Wake every waiter after the service ended; each reads its own cause.
    pub(super) fn service_ended(&self) {
        for kind in [&self.connect, &self.disconnect, &self.scan] {
            kind.observer.wake();
            kind.admission.wake();
        }
    }
}

/// The observer of one admitted control request; it resolves exactly once
/// to that request's terminal. Dropping it detaches the observer without
/// returning the permission, which the terminal returns.
#[must_use = "dropping the operation detaches its observer; the request continues"]
pub struct Operation<'c> {
    state: &'c super::State,
    command: Command,
    correlation: u64,
    resolved: bool,
}

impl<'c> Operation<'c> {
    pub(super) const fn new(state: &'c super::State, command: Command, correlation: u64) -> Self {
        Self {
            state,
            command,
            correlation,
            resolved: false,
        }
    }
    /// The request this operation observes.
    pub const fn command(&self) -> Command {
        self.command
    }
}

impl Future for Operation<'_> {
    type Output = OperationEnd;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<OperationEnd> {
        if self.resolved {
            // A resolved operation never takes a later request's terminal.
            return Poll::Pending;
        }
        let kind = self.state.operations.kind(self.command);
        kind.observer.register(cx.waker());
        let end = match kind.permission.get() {
            Permission::Ended { correlation, end } if correlation == self.correlation => end,
            Permission::Held { correlation, .. }
                if correlation == self.correlation && self.state.ended() =>
            {
                OperationEnd::ServiceEnded
            }
            _ => return Poll::Pending,
        };
        kind.release();
        self.resolved = true;
        Poll::Ready(end)
    }
}

impl Drop for Operation<'_> {
    fn drop(&mut self) {
        if self.resolved {
            return;
        }
        let kind = self.state.operations.kind(self.command);
        match kind.permission.get() {
            Permission::Held {
                correlation,
                observed: true,
            } if correlation == self.correlation => {
                kind.permission.set(Permission::Held {
                    correlation,
                    observed: false,
                });
                // An ended service delivers no later terminal.
                if self.state.ended() {
                    kind.release();
                }
            }
            Permission::Ended { correlation, .. } if correlation == self.correlation => {
                kind.release()
            }
            _ => {}
        }
    }
}

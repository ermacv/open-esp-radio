//! One-shot results of the station's control requests.
//!
//! Each command kind has one permission, the only admission of station
//! requests: an [`Operation`]'s request and the reconnect policy's request
//! both take it, and a request holds it until the service delivers that
//! request's terminal. The [`Operation`] returned to the caller only
//! observes the terminal. Dropping the observer detaches it: the request
//! continues and its terminal still returns the permission, so the next
//! request of that kind waits for it instead of posting a duplicate.
//! Requests carry an internal correlation, so a late terminal belongs only
//! to its own request.
//!
//! A new control epoch frees every permission. The previous epoch's result
//! that an operation still observes, its terminal or the service's end,
//! moves to the kind's retired record, where that operation reads it.

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
/// admitted request, and the control waiting to admit the next one; and the
/// previous epoch's result its operation has not read.
pub(super) struct Kind {
    permission: Cell<Permission>,
    retired: Cell<Option<(u64, OperationEnd)>>,
    observer: PriorityWaker,
    admission: PriorityWaker,
}

impl Kind {
    const fn new() -> Self {
        Self {
            permission: Cell::new(Permission::Free),
            retired: Cell::new(None),
            observer: PriorityWaker::new(),
            admission: PriorityWaker::new(),
        }
    }
    /// The result an observing operation still has to read when its epoch
    /// ends: its terminal, or the end of the service that never delivered it.
    fn unread(&self) -> Option<(u64, OperationEnd)> {
        match self.permission.get() {
            Permission::Held {
                correlation,
                observed: true,
            } => Some((correlation, OperationEnd::ServiceEnded)),
            Permission::Ended { correlation, end } => Some((correlation, end)),
            _ => None,
        }
    }
}

/// A new control epoch was refused: an operation of `0`'s kind from an
/// earlier epoch still holds the kind's retired result unread, and this
/// epoch's operation of that kind also ends unread.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnreadOperation(pub Command);

/// The permissions of the three command kinds.
pub(super) struct Operations {
    connect: Kind,
    disconnect: Kind,
    scan: Kind,
    next: Cell<u64>,
    /// The runner's reconnect policy waits here for a permission.
    released: PriorityWaker,
}

impl Operations {
    pub(super) const fn new() -> Self {
        Self {
            connect: Kind::new(),
            disconnect: Kind::new(),
            scan: Kind::new(),
            next: Cell::new(FIRST_CORRELATION),
            released: PriorityWaker::new(),
        }
    }
    fn kinds(&self) -> [(Command, &Kind); 3] {
        [
            (Command::Connect, &self.connect),
            (Command::Disconnect, &self.disconnect),
            (Command::BackgroundScan, &self.scan),
        ]
    }
    fn release(&self, kind: &Kind) {
        kind.permission.set(Permission::Free);
        kind.admission.wake();
        self.released.wake();
    }
    /// Whether a new epoch can begin without overwriting a retired result
    /// an earlier epoch's operation has not read.
    pub(super) fn check_epoch(&self) -> Result<(), UnreadOperation> {
        for (command, kind) in self.kinds() {
            if kind.unread().is_some() && kind.retired.get().is_some() {
                return Err(UnreadOperation(command));
            }
        }
        Ok(())
    }
    /// Begin a new epoch, after [`Self::check_epoch`]: every permission is
    /// free, and an unread result of the ended epoch is retired for its
    /// operation.
    pub(super) fn begin_epoch(&self) {
        for (_, kind) in self.kinds() {
            if let Some(unread) = kind.unread() {
                kind.retired.set(Some(unread));
            }
            self.release(kind);
            kind.observer.wake();
        }
    }
    /// Whether a request of `command` can be admitted now.
    pub(super) fn is_free(&self, command: Command) -> bool {
        self.kind(command).permission.get() == Permission::Free
    }
    /// The reconnect policy's request takes the kind's permission, with no
    /// observer; `false` while another request holds it.
    pub(super) fn admit_unobserved(&self, command: Command, correlation: u64) -> bool {
        let kind = self.kind(command);
        if kind.permission.get() != Permission::Free {
            return false;
        }
        kind.permission.set(Permission::Held {
            correlation,
            observed: false,
        });
        true
    }
    /// Wake `cx` when any permission returns.
    pub(super) fn register_released(&self, cx: &Context<'_>) {
        self.released.register(cx.waker());
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
            } if correlation == request.correlation() => self.release(kind),
            _ => {}
        }
    }
    /// Wake every waiter after the service ended; each reads its own cause.
    pub(super) fn service_ended(&self) {
        for (_, kind) in self.kinds() {
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
    /// The internal correlation of the observed request.
    #[cfg(test)]
    pub(crate) const fn correlation(&self) -> u64 {
        self.correlation
    }
}

impl Future for Operation<'_> {
    type Output = OperationEnd;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<OperationEnd> {
        if self.resolved {
            // A resolved operation never takes a later request's terminal.
            return Poll::Pending;
        }
        let operations = &self.state.operations;
        let kind = operations.kind(self.command);
        kind.observer.register(cx.waker());
        if let Some((correlation, end)) = kind.retired.get()
            && correlation == self.correlation
        {
            // The epoch of this operation ended; its result was retired.
            kind.retired.set(None);
            self.resolved = true;
            return Poll::Ready(end);
        }
        let end = match kind.permission.get() {
            Permission::Ended { correlation, end } if correlation == self.correlation => end,
            Permission::Held { correlation, .. }
                if correlation == self.correlation && self.state.ended() =>
            {
                OperationEnd::ServiceEnded
            }
            _ => return Poll::Pending,
        };
        operations.release(kind);
        self.resolved = true;
        Poll::Ready(end)
    }
}

impl Drop for Operation<'_> {
    fn drop(&mut self) {
        if self.resolved {
            return;
        }
        let operations = &self.state.operations;
        let kind = operations.kind(self.command);
        if kind
            .retired
            .get()
            .is_some_and(|(correlation, _)| correlation == self.correlation)
        {
            kind.retired.set(None);
            return;
        }
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
                    operations.release(kind);
                }
            }
            Permission::Ended { correlation, .. } if correlation == self.correlation => {
                operations.release(kind)
            }
            _ => {}
        }
    }
}

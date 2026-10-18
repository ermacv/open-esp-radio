//! Explicit user confirmation with cancellation and stale-reply protection.
//!
//! The application and its console share this object on one executor. It never
//! owns HCI or confirms pairing itself. The UI must compare both peers' numbers;
//! possession of a challenge is not evidence that a human performed comparison.
//! Console transports must additionally bind replies to the current boot.

use core::cell::Cell;
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, signal::Signal};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Challenge {
    pub id: u64,
    /// Display with six digits, including leading zeroes.
    pub number: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmationError {
    Busy,
    InvalidNumber,
    Exhausted,
    StaleOrDuplicate,
}

/// Caller-owned exchange, retained across connection/Host changes in one boot.
pub struct NumericComparison {
    sequence: Cell<u64>,
    pending: Cell<Option<Challenge>>,
    answered: Cell<bool>,
    response: Signal<NoopRawMutex, bool>,
    changed: Signal<NoopRawMutex, ()>,
}

impl NumericComparison {
    pub const fn new() -> Self {
        Self {
            sequence: Cell::new(0),
            pending: Cell::new(None),
            answered: Cell::new(false),
            response: Signal::new(),
            changed: Signal::new(),
        }
    }

    /// Start a prompt. The returned unique lease owns its lifetime.
    ///
    /// Drop the lease on disconnect, SMP failure/timeout, or application
    /// cancellation. No reply from this prompt can authorize the next prompt.
    pub fn begin(&self, number: u32) -> Result<Confirmation<'_>, ConfirmationError> {
        if self.pending.get().is_some() {
            return Err(ConfirmationError::Busy);
        }
        if number > 999_999 {
            return Err(ConfirmationError::InvalidNumber);
        }
        let id = self
            .sequence
            .get()
            .checked_add(1)
            .ok_or(ConfirmationError::Exhausted)?;
        self.sequence.set(id);
        self.response.reset();
        self.answered.set(false);
        self.pending.set(Some(Challenge { id, number }));
        self.changed.signal(());
        Ok(Confirmation { owner: self })
    }

    pub fn pending(&self) -> Option<Challenge> {
        self.pending.get().filter(|_| !self.answered.get())
    }

    /// Cancellation-safe UI wake; re-read `pending()` after waking.
    pub async fn wait_changed(&self) {
        self.changed.wait().await
    }

    /// Submit an explicit decision for the exact displayed challenge once.
    pub fn respond(&self, challenge: Challenge, accept: bool) -> Result<(), ConfirmationError> {
        if self.pending() != Some(challenge) {
            return Err(ConfirmationError::StaleOrDuplicate);
        }
        self.answered.set(true);
        self.response.signal(accept);
        self.changed.signal(());
        Ok(())
    }
}

impl Default for NumericComparison {
    fn default() -> Self {
        Self::new()
    }
}

/// Non-cloneable prompt owner. Dropping it revokes even an already queued reply.
#[must_use = "dropping the prompt revokes its response"]
pub struct Confirmation<'a> {
    owner: &'a NumericComparison,
}

impl Confirmation<'_> {
    /// Wait without blocking Host/hardware progress. Cancellation of this wait
    /// alone retains the prompt; dropping the lease cancels it completely.
    pub async fn wait(&mut self) -> bool {
        self.owner.response.wait().await
    }
}

impl Drop for Confirmation<'_> {
    fn drop(&mut self) {
        self.owner.pending.set(None);
        self.owner.answered.set(false);
        self.owner.response.reset();
        self.owner.changed.signal(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };

    #[test]
    fn no_auto_confirmation_and_rejection_is_delivered_once() {
        let exchange = NumericComparison::new();
        let mut lease = exchange.begin(123).unwrap();
        let challenge = exchange.pending().unwrap();
        assert!(
            pin!(lease.wait())
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        exchange.respond(challenge, false).unwrap();
        assert_eq!(
            exchange.respond(challenge, true),
            Err(ConfirmationError::StaleOrDuplicate)
        );
        assert_eq!(
            pin!(lease.wait()).poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(false)
        );
    }

    #[test]
    fn cancelled_queued_acceptance_cannot_confirm_reused_number() {
        let exchange = NumericComparison::new();
        let lease = exchange.begin(123).unwrap();
        let old = exchange.pending().unwrap();
        exchange.respond(old, true).unwrap();
        drop(lease);
        let mut next = exchange.begin(123).unwrap();
        let current = exchange.pending().unwrap();
        assert_ne!(old.id, current.id);
        assert_eq!(
            exchange.respond(old, true),
            Err(ConfirmationError::StaleOrDuplicate)
        );
        assert!(
            pin!(next.wait())
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        exchange.respond(current, true).unwrap();
        assert_eq!(
            pin!(next.wait()).poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(true)
        );
    }

    #[test]
    fn simultaneous_prompt_wrong_number_and_identifier_wrap_are_rejected() {
        let exchange = NumericComparison::new();
        assert!(matches!(
            exchange.begin(1_000_000),
            Err(ConfirmationError::InvalidNumber)
        ));
        let lease = exchange.begin(0).unwrap();
        assert!(matches!(exchange.begin(1), Err(ConfirmationError::Busy)));
        let challenge = exchange.pending().unwrap();
        assert_eq!(
            exchange.respond(
                Challenge {
                    number: 1,
                    ..challenge
                },
                true
            ),
            Err(ConfirmationError::StaleOrDuplicate)
        );
        drop(lease);
        exchange.sequence.set(u64::MAX);
        assert!(matches!(
            exchange.begin(1),
            Err(ConfirmationError::Exhausted)
        ));
        assert_eq!(exchange.pending(), None);
    }
}

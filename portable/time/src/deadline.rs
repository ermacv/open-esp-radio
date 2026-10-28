#![forbid(unsafe_code)]

//! Finite deadlines as values, independent of any peripheral or reset owner.
//!
//! The caller supplies each time observation. This policy records one
//! obligation, rejects renewal and reports expired or stale completion;
//! it owns no clock, hardware or callback. The platform owner decides how
//! to enforce the deadline and retains its own affine hardware lease.

use crate::{Duration, Instant};

/// Explicit caller-selected timeout. There is deliberately no default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeadlineBudget(core::num::NonZeroU32);

impl DeadlineBudget {
    /// Microseconds from starting, including the caller's restoration work.
    pub const fn from_micros(micros: core::num::NonZeroU32) -> Self {
        Self(micros)
    }

    pub const fn as_micros(self) -> u32 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlineError {
    AlreadyActive,
    TimelineExhausted,
    StaleCompletion,
    Expired,
}

/// At most one outstanding deadline, identified by a generation that only
/// its own completion can retire, and only before its deadline.
#[derive(Debug, Default)]
pub struct Controller {
    generation: u64,
    active: Option<(u64, Instant)>,
}

impl Controller {
    pub const fn new() -> Self {
        Self {
            generation: 0,
            active: None,
        }
    }

    /// Start once at `now` and return the obligation's generation. An
    /// outstanding obligation or exhausted timeline leaves state unchanged.
    pub fn start(&mut self, now: Instant, budget: DeadlineBudget) -> Result<u64, DeadlineError> {
        if self.active.is_some() {
            return Err(DeadlineError::AlreadyActive);
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(DeadlineError::TimelineExhausted)?;
        let deadline = now
            .checked_add(Duration::from_micros(u64::from(budget.as_micros())))
            .ok_or(DeadlineError::TimelineExhausted)?;
        self.generation = generation;
        self.active = Some((generation, deadline));
        Ok(generation)
    }

    /// The outstanding deadline, including after an expired completion.
    pub const fn deadline(&self) -> Option<Instant> {
        match self.active {
            Some((_, deadline)) => Some(deadline),
            None => None,
        }
    }

    /// Complete `generation` at `now`. Equality or lateness reports
    /// [`DeadlineError::Expired`] and retains the original obligation.
    pub fn complete(&mut self, now: Instant, generation: u64) -> Result<(), DeadlineError> {
        let (active, deadline) = self.active.ok_or(DeadlineError::StaleCompletion)?;
        if generation != active {
            return Err(DeadlineError::StaleCompletion);
        }
        if now >= deadline {
            return Err(DeadlineError::Expired);
        }
        self.active = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: u64) -> Instant {
        Instant::from_micros(micros)
    }

    fn budget() -> DeadlineBudget {
        DeadlineBudget::from_micros(core::num::NonZeroU32::new(100).unwrap())
    }

    #[test]
    fn an_abandoned_obligation_cannot_be_renewed() {
        let mut c = Controller::new();
        let _abandoned = c.start(at(0), budget()).unwrap();
        assert_eq!(c.start(at(20), budget()), Err(DeadlineError::AlreadyActive));
        assert_eq!(c.deadline(), Some(at(100)));
    }

    #[test]
    fn restoration_must_finish_before_the_original_deadline() {
        let mut c = Controller::new();
        let ticket = c.start(at(0), budget()).unwrap();
        assert_eq!(c.start(at(20), budget()), Err(DeadlineError::AlreadyActive));
        assert_eq!(c.complete(at(100), ticket), Err(DeadlineError::Expired));
        assert_eq!(c.deadline(), Some(at(100)));
    }

    #[test]
    fn timely_completion_retires_only_its_obligation() {
        let mut c = Controller::default();
        assert_eq!(c.deadline(), None);
        let rejected = c.start(at(0), budget()).unwrap();
        c.complete(at(1), rejected).unwrap();
        assert_eq!(c.deadline(), None);
        let next = c.start(at(1), budget()).unwrap();
        assert_ne!(next, rejected);
        assert_eq!(c.deadline(), Some(at(101)));
        c.complete(at(100), next).unwrap();
        assert_eq!(c.deadline(), None);
    }

    #[test]
    fn equality_and_late_completion_retain_the_deadline() {
        for now in [100, 101] {
            let mut c = Controller::new();
            let ticket = c.start(at(0), budget()).unwrap();
            assert_eq!(c.complete(at(now), ticket), Err(DeadlineError::Expired));
            assert_eq!(c.deadline(), Some(at(100)));
        }
    }

    #[test]
    fn old_completion_cannot_retire_a_new_obligation() {
        let mut c = Controller::new();
        assert_eq!(c.complete(at(0), 0), Err(DeadlineError::StaleCompletion));
        let first = c.start(at(0), budget()).unwrap();
        c.complete(at(99), first).unwrap();
        let second = c.start(at(99), budget()).unwrap();
        assert_eq!(
            c.complete(at(99), first),
            Err(DeadlineError::StaleCompletion)
        );
        assert_eq!(c.deadline(), Some(at(199)));
        c.complete(at(100), second).unwrap();
    }

    #[test]
    fn time_overflow_leaves_the_policy_idle_and_does_not_consume_a_generation() {
        let mut c = Controller::new();
        assert_eq!(
            c.start(at(u64::MAX), budget()),
            Err(DeadlineError::TimelineExhausted)
        );
        assert_eq!(c.deadline(), None);
        assert_eq!(c.start(at(0), budget()), Ok(1));
    }

    #[test]
    fn generation_exhaustion_leaves_the_policy_idle() {
        let mut c = Controller {
            generation: u64::MAX,
            active: None,
        };
        assert_eq!(
            c.start(at(0), budget()),
            Err(DeadlineError::TimelineExhausted)
        );
        assert_eq!(c.deadline(), None);
        assert_eq!(c.generation, u64::MAX);
    }
}

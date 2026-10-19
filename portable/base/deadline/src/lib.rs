#![no_std]
#![forbid(unsafe_code)]

//! Finite system-deadline protection, independent of any peripheral client
//! and of the chip.
//!
//! No periodic feed, implicit renewal, or Drop-based disarm exists. A caller
//! completes a lease only after its own physical postconditions hold. A chip
//! adapter binds a [`Controller`] to its watchdog timer through [`Backend`];
//! the controller decides when the timer may be armed and disarmed.

/// Explicit caller-selected hardware timeout. There is deliberately no default.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeadlineBudget(core::num::NonZeroU32);

impl DeadlineBudget {
    /// Microseconds from arming, including the caller's restoration work.
    pub const fn from_micros(micros: core::num::NonZeroU32) -> Self {
        Self(micros)
    }

    pub const fn as_micros(self) -> u32 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeadlineError {
    AlreadyArmed,
    TimelineExhausted,
    StaleCompletion,
    Expired,
}

/// The watchdog timer a chip adapter drives.
pub trait Backend {
    /// Microseconds on the timeline the deadline is measured on.
    fn now_micros(&self) -> u64;
    /// Program the timer to reset the chip `micros` from now and start it.
    fn arm(&mut self, micros: u32);
    /// Stop the timer.
    fn disarm(&mut self);
}

/// At most one armed obligation at a time, each identified by a generation
/// that only its own lease can complete, and only before its deadline.
pub struct Controller<B> {
    backend: B,
    generation: u64,
    armed: Option<(u64, u64)>,
}

impl<B: Backend> Controller<B> {
    pub const fn new(backend: B) -> Self {
        Self {
            backend,
            generation: 0,
            armed: None,
        }
    }

    /// Arm once and return the obligation's generation. An existing
    /// obligation is rejected without changing its timer.
    pub fn arm(&mut self, budget: DeadlineBudget) -> Result<u64, DeadlineError> {
        if self.armed.is_some() {
            return Err(DeadlineError::AlreadyArmed);
        }
        let generation = self
            .generation
            .checked_add(1)
            .ok_or(DeadlineError::TimelineExhausted)?;
        let deadline = self
            .backend
            .now_micros()
            .checked_add(u64::from(budget.as_micros()))
            .ok_or(DeadlineError::TimelineExhausted)?;
        self.generation = generation;
        self.armed = Some((generation, deadline));
        self.backend.arm(budget.as_micros());
        Ok(generation)
    }

    /// Disarm the obligation `generation` before its deadline. Equality or
    /// lateness rejects completion and leaves the hardware armed.
    pub fn complete(&mut self, generation: u64) -> Result<(), DeadlineError> {
        let (active, deadline) = self.armed.ok_or(DeadlineError::StaleCompletion)?;
        if generation != active {
            return Err(DeadlineError::StaleCompletion);
        }
        if self.backend.now_micros() >= deadline {
            return Err(DeadlineError::Expired);
        }
        self.backend.disarm();
        self.armed = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Model {
        now: u64,
        armed: bool,
        arms: u32,
        disarms: u32,
    }
    impl Backend for Model {
        fn now_micros(&self) -> u64 {
            self.now
        }
        fn arm(&mut self, _: u32) {
            self.armed = true;
            self.arms += 1;
        }
        fn disarm(&mut self) {
            self.armed = false;
            self.disarms += 1;
        }
    }
    fn budget() -> DeadlineBudget {
        DeadlineBudget::from_micros(core::num::NonZeroU32::new(100).unwrap())
    }
    #[test]
    fn abandoned_operation_cannot_be_renewed() {
        let mut c = Controller::new(Model::default());
        let _abandoned = c.arm(budget()).unwrap();
        c.backend.now = 20;
        assert_eq!(c.arm(budget()), Err(DeadlineError::AlreadyArmed));
        assert_eq!(c.backend.arms, 1);
        assert!(c.backend.armed);
        assert_eq!(c.backend.disarms, 0);
    }

    #[test]
    fn restoration_must_finish_before_original_not_refreshed_deadline() {
        let mut c = Controller::new(Model::default());
        let ticket = c.arm(budget()).unwrap();
        // PHY returned at 20, but the client has not completed restoration.
        c.backend.now = 20;
        assert!(c.backend.armed);
        assert_eq!(c.arm(budget()), Err(DeadlineError::AlreadyArmed));
        c.backend.now = 100;
        assert_eq!(c.complete(ticket), Err(DeadlineError::Expired));
        assert_eq!(c.backend.arms, 1);
        assert_eq!(c.backend.disarms, 0);
    }

    #[test]
    fn safe_rejection_can_complete_and_does_not_poison_next_transaction() {
        let mut c = Controller::new(Model::default());
        let rejected = c.arm(budget()).unwrap();
        c.backend.now = 1;
        c.complete(rejected).unwrap();
        assert!(!c.backend.armed);
        let next = c.arm(budget()).unwrap();
        assert_ne!(next, rejected);
        c.backend.now = 100;
        c.complete(next).unwrap();
        assert!(!c.backend.armed);
    }
    #[test]
    fn equality_and_late_completion_never_disable_reset() {
        for now in [100, 101] {
            let mut c = Controller::new(Model::default());
            let ticket = c.arm(budget()).unwrap();
            c.backend.now = now;
            assert_eq!(c.complete(ticket), Err(DeadlineError::Expired));
            assert!(c.backend.armed);
            assert_eq!(c.backend.disarms, 0);
        }
    }
    #[test]
    fn old_completion_cannot_disarm_a_new_obligation() {
        let mut c = Controller::new(Model::default());
        let first = c.arm(budget()).unwrap();
        c.backend.now = 99;
        c.complete(first).unwrap();
        let second = c.arm(budget()).unwrap();
        assert_eq!(c.complete(first), Err(DeadlineError::StaleCompletion));
        assert!(c.backend.armed);
        c.complete(second).unwrap();
        assert_eq!(c.backend.disarms, 2);
    }
    #[test]
    fn timeline_overflow_rejects_before_hardware_programming() {
        let mut c = Controller::new(Model {
            now: u64::MAX,
            ..Model::default()
        });
        assert_eq!(c.arm(budget()), Err(DeadlineError::TimelineExhausted));
        assert_eq!(c.backend.arms, 0);
    }
}

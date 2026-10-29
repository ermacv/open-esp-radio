//! Current-boot terminal election and retained fault access. A claim never
//! releases before reset; reading on a later boot does not claim a terminal.
use core::sync::atomic::{AtomicBool, Ordering};
use oer_diagnostics::fault::{Error, Record};
use oer_memory::retained::{RetainedCell, RetainedGuard};

pub(crate) struct Election {
    claimed: AtomicBool,
    reset_started: AtomicBool,
}
impl Default for Election {
    fn default() -> Self {
        Self::new()
    }
}
impl Election {
    pub const fn new() -> Self {
        Self {
            claimed: AtomicBool::new(false),
            reset_started: AtomicBool::new(false),
        }
    }
    fn begin_reset(&self) -> bool {
        self.reset_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    pub fn claim(&self) -> Result<Claim<'_>, Reentered> {
        self.claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Claim { _election: self })
            .map_err(|_| Reentered)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Reentered;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitError {
    Held,
    Record(Error),
}
pub struct Claim<'a> {
    _election: &'a Election,
}

impl Claim<'_> {
    /// Several observation slots may belong to one terminal (panic plus its
    /// image hook). Each uses this same format, and each stays held until reset.
    pub fn commit(&self, cell: &RetainedCell<Record>, record: Record) -> Result<(), CommitError> {
        let mut slot = cell.try_lock().ok_or(CommitError::Held)?;
        slot.commit(record).map_err(CommitError::Record)?;
        core::mem::forget(slot);
        Ok(())
    }
}

pub fn peek(cell: &RetainedCell<Record>) -> Option<RetainedGuard<'_, Record>> {
    cell.try_lock()
}

crate::placed_static! {
    static ELECTION: Election = Election::new(), in critical_data "terminal_election";
}
pub(crate) fn claim() -> Result<Claim<'static>, Reentered> {
    ELECTION.claim()
}

pub(crate) fn begin_reset() -> bool {
    ELECTION.begin_reset()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reset_attempt_is_held_even_when_reset_faults_or_returns() {
        let boot = Election::new();
        assert!(boot.begin_reset());
        assert!(!boot.begin_reset());
        assert!(!boot.begin_reset());
        assert!(Election::new().begin_reset());
    }
    #[test]
    fn reentry_is_visible_and_dropping_claim_does_not_release_it() {
        let boot = Election::new();
        {
            let _first = boot.claim().unwrap();
            assert!(matches!(boot.claim(), Err(Reentered)));
        }
        assert!(matches!(boot.claim(), Err(Reentered)));
        assert!(Election::new().claim().is_ok());
    }
}

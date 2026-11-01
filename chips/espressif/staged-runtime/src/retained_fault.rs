//! Current-boot terminal election and retained fault access. A claim never
//! releases before reset; reading on a later boot does not claim a terminal.
//!
//! Every claim draws the next [`Terminal`] number from a retained counter, so
//! the records one terminal commits in several slots name it: a record left
//! by an earlier terminal, whose registers may be the same, carries an
//! earlier number.
use core::num::NonZeroU32;
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
    /// Claim this boot's terminal and number it from `numbers`, the last
    /// terminal's number. Only a claim locks `numbers`; a claim that finds it
    /// held numbers no terminal.
    pub fn claim(&self, numbers: &RetainedCell<u32>) -> Result<Claim<'_>, Reentered> {
        self.claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Reentered)?;
        let terminal = numbers.try_lock().map(|mut last| {
            let next = Terminal::after(*last);
            *last = next.get();
            next
        });
        Ok(Claim {
            _election: self,
            terminal,
        })
    }
}

/// One terminal's number, which every record it commits may copy to name
/// it. Never zero, so that a record's zero word names no terminal. Two
/// terminals share a number only after `u32::MAX - 1` others drew from the
/// same retained counter in between.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Terminal(NonZeroU32);

impl Terminal {
    /// The number after `last`, skipping zero.
    fn after(last: u32) -> Self {
        Self(NonZeroU32::new(last.wrapping_add(1)).unwrap_or(NonZeroU32::MIN))
    }

    /// The terminal a record's word names; `None` for zero.
    pub fn from_word(word: u32) -> Option<Self> {
        NonZeroU32::new(word).map(Self)
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }

    /// A record's word for `terminal`: zero when there is none.
    pub fn word(terminal: Option<Self>) -> u32 {
        terminal.map_or(0, Self::get)
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
    terminal: Option<Terminal>,
}

impl Claim<'_> {
    /// This terminal's number; `None` when the claim found the counter held.
    pub fn terminal(&self) -> Option<Terminal> {
        self.terminal
    }

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
crate::retained_static! {
    /// The last terminal's number; any value after a loss of power.
    static TERMINALS: u32;
}
pub(crate) fn claim() -> Result<Claim<'static>, Reentered> {
    ELECTION.claim(&TERMINALS)
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
    crate::retained_static! {
        static NUMBERS: u32;
    }
    crate::retained_static! {
        static WRAPPING: u32;
    }
    crate::retained_static! {
        static HELD: u32;
    }
    #[test]
    fn reentry_is_visible_and_dropping_claim_does_not_release_it() {
        let boot = Election::new();
        {
            let _first = boot.claim(&NUMBERS).unwrap();
            assert!(matches!(boot.claim(&NUMBERS), Err(Reentered)));
        }
        assert!(matches!(boot.claim(&NUMBERS), Err(Reentered)));
        assert!(Election::new().claim(&NUMBERS).is_ok());
    }
    #[test]
    fn every_boot_s_terminal_has_the_next_nonzero_number() {
        let numbers = |cell: &RetainedCell<u32>| {
            [(); 3].map(|()| Terminal::word(Election::new().claim(cell).unwrap().terminal()))
        };
        *NUMBERS.try_lock().unwrap() = 41;
        // Each boot elects anew; the retained counter numbers its terminal.
        assert_eq!(numbers(&NUMBERS), [42, 43, 44]);
        *WRAPPING.try_lock().unwrap() = u32::MAX - 1;
        assert_eq!(numbers(&WRAPPING), [u32::MAX, 1, 2]);
        assert_eq!(Terminal::from_word(0), None);
        let held = HELD.try_lock().unwrap();
        assert_eq!(Election::new().claim(&HELD).unwrap().terminal(), None);
        drop(held);
    }
}

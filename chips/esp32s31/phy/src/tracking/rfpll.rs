//! Measured RFPLL capacitor correction from the current ESP32-S31 archive.
//!
//! Reference: `phy_rfpll_cap_init_cal_track` and `phy_rfpll_cap_correct_track`
//! of the pinned esp-phy-lib `libphy.a`
//! (`chips/esp32s31/vendor/artifacts.toml`).
//! This finite child is the production RFPLL capacitor correction. It does
//! not enable automatic tracking or grant access to RF. The target-only
//! `crate::target_port::rfpll` executes the
//! search, memory child and current frequency-control envelope through typed
//! HAL operations when the caller already holds exclusive PHY access. The
//! outer RFPLL action selects this transaction; registered policy keeps that
//! action disabled pending physical qualification.
//! See the [module contract](rfpll/README.md) for that execution boundary.

pub mod search;
pub mod thermal;

/// Value-only terminal observation. This is not an RF access or lock proof.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observation {
    pub request: thermal::Request,
    pub outcome: thermal::Outcome,
    /// Age from sensor acquisition start at RFPLL entry, including sensor waits.
    /// None means the acquisition or observation clock was unavailable/invalid.
    pub sample_age_micros: Option<u64>,
}

use crate::analog::frequency::{
    PhyFrequencyCapCorrection, PhyFrequencyCapMemoryAction, PhyFrequencyCapMemoryCompletion,
    PhyFrequencyCapMemoryOutcome, PhyFrequencyCapMemoryRequest, PhyFrequencyCapMemoryTransition,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    Search(search::Action),
    Memory(PhyFrequencyCapMemoryAction),
    Complete(Outcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    Search(search::Completion),
    Memory(PhyFrequencyCapMemoryCompletion),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Outcome {
    pub search: search::Outcome,
    /// No update is issued for a zero measured delta.
    pub memory: Option<PhyFrequencyCapMemoryOutcome>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    WrongCompletion,
    AlreadyComplete,
}

#[derive(Debug, Eq, PartialEq)]
enum Step {
    Search(search::Search),
    Memory {
        search: search::Outcome,
        transition: PhyFrequencyCapMemoryTransition,
    },
    Complete(Outcome),
}

/// Owns the search and, when needed, the complete frequency-memory update.
/// The caller retains exclusive PHY access until the channel index is restored.
#[derive(Debug, Eq, PartialEq)]
pub struct Correction {
    current_channel: u16,
    step: Step,
}

impl Correction {
    pub const fn new(current_channel: u16) -> Self {
        Self {
            current_channel,
            step: Step::Search(search::Search::new()),
        }
    }

    #[inline(always)]
    pub const fn action(&self) -> Action {
        match &self.step {
            Step::Search(search) => Action::Search(search.action()),
            Step::Memory { transition, .. } => Action::Memory(transition.action()),
            Step::Complete(outcome) => Action::Complete(*outcome),
        }
    }

    #[inline(always)]
    pub fn advance(&mut self, completion: Completion) -> Result<(), Error> {
        match (&mut self.step, completion) {
            (Step::Search(search), Completion::Search(completion)) => {
                search
                    .advance(completion)
                    .map_err(|_| Error::WrongCompletion)?;
                if let search::Action::Complete(search) = search.action() {
                    self.step = if search.delta() == 0 {
                        Step::Complete(Outcome {
                            search,
                            memory: None,
                        })
                    } else {
                        Step::Memory {
                            search,
                            transition: PhyFrequencyCapMemoryTransition::new(
                                PhyFrequencyCapMemoryRequest {
                                    correction: PhyFrequencyCapCorrection::from_delta(
                                        search.delta(),
                                    ),
                                    current_channel: self.current_channel,
                                },
                            ),
                        }
                    };
                }
            }
            (Step::Memory { search, transition }, Completion::Memory(completion)) => {
                transition
                    .advance(completion)
                    .map_err(|_| Error::WrongCompletion)?;
                if let PhyFrequencyCapMemoryAction::Complete(memory) = transition.action() {
                    self.step = Step::Complete(Outcome {
                        search: *search,
                        memory: Some(memory),
                    });
                }
            }
            (Step::Complete(_), _) => return Err(Error::AlreadyComplete),
            _ => return Err(Error::WrongCompletion),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

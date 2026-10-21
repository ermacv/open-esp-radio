//! RFPLL checkpoints retain the physical envelope owned by the caller.
use super::{Failure, Step, io::Io};
use crate::{
    analog::frequency::PhyFrequencyCapMemoryAction as Memory,
    executor::wait::PhyShortDelay,
    tracking::rfpll::{Action, Completion, search, thermal},
};

pub(crate) enum Cursor<'a> {
    #[cfg(any(test, oer_probe))]
    Search(&'a mut search::Search),
    #[cfg(any(test, oer_probe))]
    Correction(&'a mut crate::tracking::rfpll::Correction),
    Thermal(&'a mut thermal::Transition),
}
impl Cursor<'_> {
    fn action(&self) -> Option<Action> {
        match self {
            #[cfg(any(test, oer_probe))]
            Self::Search(search) => match search.action() {
                search::Action::Complete(_) => None,
                action => Some(Action::Search(action)),
            },
            #[cfg(any(test, oer_probe))]
            Self::Correction(correction) => match correction.action() {
                Action::Complete(_) => None,
                action => Some(action),
            },
            Self::Thermal(thermal) => match thermal.action() {
                thermal::Action::Correct(action) => Some(action),
                _ => None,
            },
        }
    }
    fn advance(&mut self, completion: Completion) -> Result<(), Failure> {
        match self {
            #[cfg(any(test, oer_probe))]
            Self::Search(search) => {
                let Completion::Search(completion) = completion else {
                    return Err(Failure::Binding);
                };
                search.advance(completion).map_err(|_| Failure::Binding)
            }
            #[cfg(any(test, oer_probe))]
            Self::Correction(correction) => {
                correction.advance(completion).map_err(|_| Failure::Binding)
            }
            Self::Thermal(thermal) => thermal
                .advance(thermal::Completion::Correction(completion))
                .map_err(|_| Failure::Binding),
        }
    }

    /// One candidate (write, minimum settle, status), one complete memory
    /// entry (read and write), or one setup/final/restoration operation.
    /// No continuation can suspend between the halves of a memory entry.
    pub(super) fn advance_one<D: PhyShortDelay, C: oer_time::Clock>(
        &mut self,
        io: &mut Io<'_, '_>,
        clock: &C,
    ) -> Result<Step, Failure> {
        let Some(action) = self.action() else {
            return Ok(Step::Complete);
        };
        self.advance(io.rfpll::<D, _>(clock, action)?)?;
        match action {
            Action::Search(search::Action::WriteCap(_)) => {
                if matches!(
                    self.action(),
                    Some(Action::Search(search::Action::DelayMicros(_)))
                ) {
                    let settle = self.action().ok_or(Failure::Binding)?;
                    self.advance(io.rfpll::<D, _>(clock, settle)?)?;
                    let status = self.action().ok_or(Failure::Binding)?;
                    self.advance(io.rfpll::<D, _>(clock, status)?)?;
                }
            }
            Action::Memory(Memory::ReadMemory { .. }) => {
                let write = self.action().ok_or(Failure::Binding)?;
                self.advance(io.rfpll::<D, _>(clock, write)?)?;
            }
            _ => {}
        }
        Ok(Step::Runnable)
    }
}

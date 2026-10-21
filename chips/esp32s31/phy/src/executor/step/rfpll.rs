//! RFPLL checkpoints retain the physical envelope owned by the caller.
use super::{Failure, Step, io::Io};
use crate::{
    calibration::cold::{PhyColdI2cAction, PhyColdI2cObservation, PhyColdI2cTransaction},
    executor::wait::PhyShortDelay,
    tracking::rfpll::Correction,
};
use oer_time::{Clock, Duration, Instant};

pub(crate) enum Cursor<'a> {
    #[cfg(any(test, oer_probe))]
    Search(&'a mut crate::tracking::rfpll::search::Search),
    Correction(&'a mut Correction),
}
impl Cursor<'_> {
    /// One counted group, with one hardware dispatch and one model advance
    /// site; LLVM must not expand it into a branch per completed checkpoint.
    #[inline(never)]
    pub(super) fn advance_one<D: PhyShortDelay, C: Clock>(
        &mut self,
        io: &mut Io<'_, '_>,
        clock: &C,
    ) -> Result<Step, Failure> {
        let count = match self {
            #[cfg(any(test, oer_probe))]
            Self::Search(search) => search.checkpoint_len(),
            Self::Correction(correction) => correction.checkpoint_len(),
        };
        if count == 0 {
            return Ok(Step::Complete);
        }
        for _ in 0..count {
            let action = match self {
                #[cfg(any(test, oer_probe))]
                Self::Search(search) => crate::tracking::rfpll::Action::Search(search.action()),
                Self::Correction(correction) => correction.action(),
            };
            let completion = io.rfpll::<D, _>(clock, action)?;
            match self {
                #[cfg(any(test, oer_probe))]
                Self::Search(search) => {
                    let crate::tracking::rfpll::Completion::Search(completion) = completion else {
                        return Err(Failure::Binding);
                    };
                    search.advance(completion).map_err(|_| Failure::Binding)?;
                }
                Self::Correction(correction) => correction
                    .advance(completion)
                    .map_err(|_| Failure::Binding)?,
            }
        }
        Ok(Step::Runnable)
    }
}

/// Cold RFPLL retains its count limit and first/repeated one-microsecond
/// observations. It uses the same start/sample atom as T7 and direct RFPLL;
/// scheduler pauses are not charged to the direct path's twenty-us settle.
pub(super) struct ColdI2c<'a> {
    transaction: &'a mut PhyColdI2cTransaction,
    pub(super) remaining: u16,
    waited: bool,
}
impl<'a> ColdI2c<'a> {
    pub(super) fn new(transaction: &'a mut PhyColdI2cTransaction) -> Self {
        Self {
            transaction,
            remaining: crate::HARDWARE_EDGE_LIMIT,
            waited: false,
        }
    }
    #[inline(never)]
    pub(super) fn advance_one(
        &mut self,
        io: &mut Io<'_, '_>,
        now: Instant,
    ) -> Result<Step, Failure> {
        if self.remaining == 0 {
            return Err(Failure::TimedOut);
        }
        let action = self.transaction.action();
        let start = match action {
            PhyColdI2cAction::Complete(_) => return Ok(Step::Complete),
            PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. } => true,
            _ => false,
        };
        if !start && !self.waited {
            self.waited = true;
        } else {
            self.remaining -= 1;
            self.waited = false;
            if io.i2c(self.transaction, start)? == PhyColdI2cObservation::EdgeConsumed {
                return Ok(Step::Runnable);
            }
            self.waited = true;
        }
        Ok(Step::WaitAt(
            now.checked_add(Duration::from_micros(1))
                .ok_or(Failure::DeadlineOverflow)?,
        ))
    }
}

/// Direct and cold callers share Io::i2c for command publication, status
/// consumption, RMW advancement and errors; only their wait policies differ.
#[inline(never)]
pub(super) fn direct_i2c<C: Clock + ?Sized>(
    io: &mut Io<'_, '_>,
    clock: &C,
    mut binding: crate::analog::rfpll::RfpllFrequencyI2cBinding,
) -> Result<crate::analog::rfpll::RfpllFrequencyCompletion, Failure> {
    let transaction = binding.transaction_mut();
    let complete = crate::executor::wait::i2c::complete(
        clock,
        transaction,
        PhyColdI2cTransaction::action,
        |transaction, action| {
            let start = matches!(
                action,
                PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. }
            );
            io.i2c(transaction, start).map(|_| ())
        },
    )?;
    if !complete {
        return Err(Failure::TimedOut);
    }
    binding.into_completion().map_err(|_| Failure::Binding)
}

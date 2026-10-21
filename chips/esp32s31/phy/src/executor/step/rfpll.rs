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
    #[inline(always)]
    pub(super) fn action(&self) -> crate::tracking::rfpll::Action {
        match self {
            #[cfg(any(test, oer_probe))]
            Self::Search(search) => crate::tracking::rfpll::Action::Search(search.action()),
            Self::Correction(correction) => correction.action(),
        }
    }

    #[inline(always)]
    pub(super) fn advance(
        &mut self,
        completion: crate::tracking::rfpll::Completion,
    ) -> Result<(), Failure> {
        match self {
            #[cfg(any(test, oer_probe))]
            Self::Search(search) => {
                let crate::tracking::rfpll::Completion::Search(completion) = completion else {
                    return Err(Failure::Binding);
                };
                search.advance(completion).map_err(|_| Failure::Binding)
            }
            Self::Correction(correction) => {
                correction.advance(completion).map_err(|_| Failure::Binding)
            }
        }
    }
}

/// Partial helper data and the identity-bound masked transaction survive
/// command checkpoints under the caller's exclusive analog owner.
pub(super) struct Run<'a> {
    pub(super) cursor: Cursor<'a>,
    pub(super) transaction: Option<PhyColdI2cTransaction>,
    pub(super) phase: u8,
    pub(super) low: u8,
}
impl<'a> Run<'a> {
    pub(super) fn new(cursor: Cursor<'a>) -> Self {
        Self {
            cursor,
            transaction: None,
            phase: 0,
            low: 0,
        }
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

/// One bus command, including its immediate readiness observations. Stop at
/// the RMW read edge, preserving the typed transaction for the next poll.
#[inline(never)]
pub(super) fn command<C: Clock>(
    io: &mut Io<'_, '_>,
    clock: &C,
    transaction: &mut PhyColdI2cTransaction,
    kind: crate::tracking::observation::RfpllCommand,
    observed: bool,
) -> Result<crate::tracking::observation::RfpllCommandSample, Failure> {
    use crate::tracking::observation::RfpllCommandSample;
    let started = observed.then(|| clock.now());
    let mut published_at = None;
    let mut sample = RfpllCommandSample {
        command: kind,
        publication_micros: 0,
        readiness_micros: 0,
        invalid: false,
    };
    let complete = crate::executor::wait::poll::bounded(clock, || {
        let start = matches!(
            transaction.action(),
            PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. }
        );
        let consumed = io.i2c(transaction, start)? == PhyColdI2cObservation::EdgeConsumed;
        if start && consumed && observed {
            sample = RfpllCommandSample::elapsed(kind, started, clock);
            published_at = Some(clock.now());
        }
        Ok(consumed && !start)
    })?;
    if let Some(at) = published_at {
        let readiness = RfpllCommandSample::elapsed(kind, Some(at), clock);
        sample.readiness_micros = readiness.publication_micros;
        sample.invalid |= readiness.invalid;
    }
    if !complete {
        return Err(Failure::TimedOut);
    }
    Ok(sample)
}

/// Direct and cold callers share Io::i2c for command publication, status
/// consumption, RMW advancement and errors; only their wait policies differ.
#[inline(never)]
pub(super) fn direct_i2c<C: Clock + ?Sized>(
    io: &mut Io<'_, '_>,
    clock: &C,
    transaction: &mut PhyColdI2cTransaction,
) -> Result<(), Failure> {
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
    Ok(())
}

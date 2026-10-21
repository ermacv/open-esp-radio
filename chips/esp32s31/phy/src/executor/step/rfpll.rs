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

/// One shared direct observation policy. Runtime helper commands stop at
/// their completion edge; synchronous direct callers finish the transaction.
#[inline(never)]
pub(super) fn direct_i2c<C: Clock + ?Sized>(
    io: &mut Io<'_, '_>,
    clock: &C,
    transaction: &mut PhyColdI2cTransaction,
    command: Option<crate::tracking::observation::RfpllCommand>,
    observed: bool,
) -> Result<crate::tracking::observation::RfpllCommandSample, Failure> {
    use crate::tracking::observation::{RfpllCommand, RfpllCommandSample};
    let kind = command.unwrap_or(RfpllCommand::InitialLow);
    let started = if observed {
        clock.now()
    } else {
        Instant::EPOCH
    };
    let mut published = started;
    let complete = crate::executor::wait::poll::bounded(clock, || {
        let next = transaction.action();
        if matches!(next, PhyColdI2cAction::Complete(_)) {
            return Ok(true);
        }
        let start = matches!(
            next,
            PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. }
        );
        let consumed = io.i2c(transaction, start)? == PhyColdI2cObservation::EdgeConsumed;
        if consumed && start && observed {
            published = clock.now();
        }
        Ok(if command.is_some() {
            consumed && !start
        } else {
            matches!(transaction.action(), PhyColdI2cAction::Complete(_))
        })
    })?;
    if !complete {
        return Err(Failure::TimedOut);
    }
    Ok(if observed {
        RfpllCommandSample::intervals(kind, started, published, clock.now())
    } else {
        RfpllCommandSample {
            command: kind,
            publication_micros: 0,
            readiness_micros: 0,
            invalid: false,
        }
    })
}

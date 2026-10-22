//! RFPLL checkpoints retain the physical envelope owned by the caller.
use super::{Failure, io::Io};
use crate::{
    calibration::cold::{PhyColdI2cAction, PhyColdI2cObservation, PhyColdI2cTransaction},
    tracking::rfpll::Correction,
};
use oer_time::{Clock, Instant};

/// Outline the platform clock conversion once without changing its identity,
/// read count or time semantics in the direct observation loop.
pub(crate) struct OutlinedClock<'a, C: ?Sized>(pub(crate) &'a C);
#[inline(never)]
pub(super) fn clock_now<C: Clock + ?Sized>(clock: &C) -> Instant {
    clock.now()
}
impl<C: Clock + ?Sized> Clock for OutlinedClock<'_, C> {
    #[inline(always)]
    fn now(&self) -> Instant {
        clock_now(self.0)
    }
}

pub(crate) enum Cursor<'a> {
    #[cfg(any(test, oer_probe))]
    Search(&'a mut crate::tracking::rfpll::search::Search),
    Correction(&'a mut Correction),
}
impl Cursor<'_> {
    #[inline(never)]
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
    let clock = &OutlinedClock(clock);
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
        // Publication and its first readiness observation are one attempt.
        // The common settle deadline is unchanged: busy still expires, but
        // an already-ready write cannot be lost immediately after starting it.
        let completed = if consumed && start {
            io.i2c(transaction, false)? == PhyColdI2cObservation::EdgeConsumed
        } else {
            consumed
        };
        Ok(if command.is_some() {
            completed
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

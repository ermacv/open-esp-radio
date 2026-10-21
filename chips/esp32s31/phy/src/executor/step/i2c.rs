//! Shared counted I2C policy for cold RFPLL and Wi-Fi temperature tracking.
use super::{Failure, Step, io::Io};
use crate::calibration::cold::{PhyColdI2cAction, PhyColdI2cObservation, PhyColdI2cTransaction};
use oer_time::{Duration, Instant};

/// Retain the original attempt limit and first/repeated one-us observations.
/// Scheduler pauses do not consume a direct path's twenty-us settle budget.
pub(super) struct Counted<'a> {
    transaction: &'a mut PhyColdI2cTransaction,
    pub(super) remaining: u16,
    waited: bool,
}
impl<'a> Counted<'a> {
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
        let start = match self.transaction.action() {
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

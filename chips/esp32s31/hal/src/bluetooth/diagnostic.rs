//! Bounded retry of scheduler diagnostic-pair samples.
//!
//! The multiplexed diagnostic value crosses from the MAC clock domain, so the
//! PAC reads it twice per attempt and yields a sample only when both complete
//! reads agree. The vendor repeats the pair until it agrees, without a bound.
//! Here one sample makes at most the caller's attempt budget and reports
//! [`BluetoothDiagnosticUnsettled`] when the budget runs out; every caller
//! handles that as a scheduler fault.

/// Nonzero number of diagnostic-pair attempts permitted for one sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDiagnosticReadBudget(u32);

impl BluetoothDiagnosticReadBudget {
    /// Reject a zero-attempt budget, which could never take a sample.
    pub const fn new(attempts: u32) -> Option<Self> {
        if attempts == 0 {
            None
        } else {
            Some(Self(attempts))
        }
    }

    /// Return the exact maximum number of attempts.
    pub const fn attempts(self) -> u32 {
        self.0
    }
}

/// The two diagnostic reads of one sample never agreed within the budget.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDiagnosticUnsettled;

/// Repeat one PAC diagnostic attempt until it yields a sample or the budget
/// runs out.
pub(crate) fn settle<T>(
    budget: BluetoothDiagnosticReadBudget,
    mut attempt: impl FnMut() -> Option<T>,
) -> Result<T, BluetoothDiagnosticUnsettled> {
    for _ in 0..budget.attempts() {
        if let Some(sample) = attempt() {
            return Ok(sample);
        }
    }
    Err(BluetoothDiagnosticUnsettled)
}

#[cfg(test)]
mod tests;

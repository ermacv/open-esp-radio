//! Time-bounded retry of scheduler diagnostic-pair samples.
//!
//! The multiplexed diagnostic value crosses from the MAC clock domain, so the
//! PAC reads it twice per attempt and yields a sample only when both complete
//! reads agree. The vendor repeats the pair until it agrees, without a bound.
//! Here one sample is a named settle: it repeats the pair until it agrees or
//! the settle's limit, at most [`BluetoothDiagnosticSettleLimit::MAX`], has
//! passed on the caller's clock, and reports [`BluetoothDiagnosticUnsettled`]
//! then; every caller handles that as a scheduler fault.

use oer_time::{Clock, Duration};

/// The longest a diagnostic sample may settle: a named busy-wait of known
/// length, at most [`Self::MAX`], that counts against its context's step
/// bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDiagnosticSettleLimit(Duration);

impl BluetoothDiagnosticSettleLimit {
    /// The longest busy-wait a driver may make.
    pub const MAX: Duration = Duration::from_micros(20);

    /// Reject a zero limit, and one beyond [`Self::MAX`].
    pub const fn new(limit: Duration) -> Option<Self> {
        if limit.as_micros() == 0 || limit.as_micros() > Self::MAX.as_micros() {
            None
        } else {
            Some(Self(limit))
        }
    }

    /// The limit.
    pub const fn limit(self) -> Duration {
        self.0
    }
}

/// One diagnostic settle: its limit and the clock that measures it.
#[derive(Clone, Copy)]
pub struct BluetoothDiagnosticSettle<'clock> {
    limit: BluetoothDiagnosticSettleLimit,
    clock: &'clock dyn Clock,
}

impl<'clock> BluetoothDiagnosticSettle<'clock> {
    /// Settle within `limit` as `clock` measures it.
    pub const fn new(limit: BluetoothDiagnosticSettleLimit, clock: &'clock dyn Clock) -> Self {
        Self { limit, clock }
    }

    /// The settle's limit.
    pub const fn limit(&self) -> BluetoothDiagnosticSettleLimit {
        self.limit
    }
}

impl core::fmt::Debug for BluetoothDiagnosticSettle<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("BluetoothDiagnosticSettle")
            .field("limit", &self.limit)
            .finish_non_exhaustive()
    }
}

/// The two diagnostic reads of one sample never agreed within the settle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothDiagnosticUnsettled;

/// Repeat one PAC diagnostic attempt until it yields a sample or the
/// settle's limit has passed; the first attempt always runs.
pub(crate) fn settle<T>(
    settle: BluetoothDiagnosticSettle<'_>,
    mut attempt: impl FnMut() -> Option<T>,
) -> Result<T, BluetoothDiagnosticUnsettled> {
    let start = settle.clock.now();
    loop {
        if let Some(sample) = attempt() {
            return Ok(sample);
        }
        if settle.clock.now().saturating_duration_since(start) >= settle.limit.limit() {
            return Err(BluetoothDiagnosticUnsettled);
        }
    }
}

#[cfg(test)]
mod tests;

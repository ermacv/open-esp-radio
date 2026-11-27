//! Radio clock errors.

/// Why the radio clock could not be read; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClockError {
    /// The backend could not take a reading now (a latch failed, the clock
    /// belongs to a replaced radio start); a later call may succeed.
    Unavailable,
    /// The radio epoch has no later instant: the clock cannot advance, and
    /// no later call succeeds. The port still settles the work it admitted.
    EpochExhausted,
}

//! Correlation identities and radio clock errors.

use core::ops::RangeInclusive;

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

/// A caller-chosen correlation identity of one port's submissions.
///
/// Each port keeps its own identity type (a Wi-Fi `TxId`, an
/// IEEE 802.15.4 `RequestId`), so the identity of
/// one port cannot complete another's work; all of them are 32-bit values
/// with the same [`BACKEND_RESERVED`] range, which callers never choose and
/// a backend may use for work it submits on its own.
pub trait Correlation: Copy + Eq {
    /// The identity with this raw value.
    fn from_raw(raw: u32) -> Self;

    /// The raw value.
    fn raw(self) -> u32;

    /// Whether the identity lies in the backend's range.
    fn is_backend_reserved(self) -> bool {
        BACKEND_RESERVED.contains(&self.raw())
    }
}

/// Raw identities a caller never chooses: a backend may name its own work
/// with them, and a caller's allocator skips them.
pub const BACKEND_RESERVED: RangeInclusive<u32> = 0xFFFF_FF00..=u32::MAX;

/// A caller's allocator of correlation identities: consecutive values that
/// wrap before [`BACKEND_RESERVED`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CorrelationIds {
    next: u32,
}

impl Default for CorrelationIds {
    fn default() -> Self {
        Self::new()
    }
}

impl CorrelationIds {
    /// Identities from zero.
    pub const fn new() -> Self {
        Self::starting_at(0)
    }

    /// Identities from `first`; a reserved `first` starts at zero.
    pub const fn starting_at(first: u32) -> Self {
        Self {
            next: if first >= *BACKEND_RESERVED.start() {
                0
            } else {
                first
            },
        }
    }

    /// The next identity.
    #[allow(
        clippy::should_implement_trait,
        reason = "the allocator never ends and yields any identity type"
    )]
    pub fn next<I: Correlation>(&mut self) -> I {
        let id = self.next;
        self.next = match id.checked_add(1) {
            Some(next) if next < *BACKEND_RESERVED.start() => next,
            _ => 0,
        };
        I::from_raw(id)
    }
}

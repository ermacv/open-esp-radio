//! Values a trace build's run takes from the host, so that a comparison of
//! a constant's values needs one image: the same code, in the same place.
//! Without `cfg(oer_trace)` [`value`] is its default and the store is gone.

/// A constant a run may set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Knob {
    /// The received units after which a lent unit's return wakes the
    /// ring's task.
    ReturnQuietUnits = 0,
    /// The fewest frames of one pass that show a burst.
    RecycledBurstUnits = 1,
}

/// The knobs a run names, set or [`UNSET`].
pub const KNOBS: usize = 4;
/// A knob that keeps its default.
pub const UNSET: u32 = u32::MAX;

#[cfg(oer_trace)]
static VALUES: [core::sync::atomic::AtomicU32; KNOBS] =
    [const { core::sync::atomic::AtomicU32::new(UNSET) }; KNOBS];

/// `knob`'s value, `default` unless the run set it.
#[cfg(oer_trace)]
#[inline]
pub fn value(knob: Knob, default: u32) -> u32 {
    match VALUES
        .get(knob as usize)
        .map(|value| value.load(core::sync::atomic::Ordering::Relaxed))
    {
        Some(UNSET) | None => default,
        Some(value) => value,
    }
}

/// `knob`'s value: its default, without a trace build.
#[cfg(not(oer_trace))]
#[inline(always)]
pub const fn value(_knob: Knob, default: u32) -> u32 {
    default
}

/// The run's values, each [`UNSET`] or the knob's.
#[cfg(oer_trace)]
pub fn set(values: [u32; KNOBS]) {
    for (knob, value) in VALUES.iter().zip(values) {
        knob.store(value, core::sync::atomic::Ordering::Relaxed);
    }
}

/// Nothing to set without a trace build.
#[cfg(not(oer_trace))]
pub fn set(_values: [u32; KNOBS]) {}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;

    #[test]
    fn a_knob_nobody_set_keeps_its_default() {
        assert_eq!(value(Knob::RecycledBurstUnits, 7), 7);
    }
}

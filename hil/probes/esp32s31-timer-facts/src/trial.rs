//! A trial's arithmetic, apart from the hardware: the expected cycle of a
//! target counted from the reference alarm, a cycle against another as a
//! signed offset, and the alarms a window saw.

/// The boot's rate: `cycles` of the hart between two alarms `ticks` apart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Rate {
    pub(crate) ticks: u32,
    pub(crate) cycles: u32,
}

impl Rate {
    /// The cycles of `ticks` at this rate, rounded down; `u32::MAX` past it.
    pub(crate) fn cycles_of(self, ticks: u64) -> u32 {
        let cycles = u128::from(ticks)
            .saturating_mul(u128::from(self.cycles))
            .checked_div(u128::from(self.ticks))
            .unwrap_or(u128::MAX);
        u32::try_from(cycles).unwrap_or(u32::MAX)
    }

    /// The cycles of `quarters` quarter ticks, rounded down.
    pub(crate) fn cycles_of_quarters(self, quarters: u16) -> u32 {
        let cycles = u128::from(quarters)
            .saturating_mul(u128::from(self.cycles))
            .checked_div(u128::from(self.ticks).saturating_mul(4))
            .unwrap_or(u128::MAX);
        u32::try_from(cycles).unwrap_or(u32::MAX)
    }

    /// The cycle `ticks` after the reference alarm seen at `reference`.
    pub(crate) fn expected(self, reference: u32, ticks: u64) -> u32 {
        reference.wrapping_add(self.cycles_of(ticks))
    }
}

/// `cycle` less `from`, as the signed offset of two samples of one
/// wrapping 32-bit counter less than 2^31 cycles apart.
pub(crate) const fn offset(cycle: u32, from: u32) -> i32 {
    cycle.wrapping_sub(from).cast_signed()
}

/// Whether `now` lies before `until` on the wrapping counter.
pub(crate) const fn before(now: u32, until: u32) -> bool {
    offset(now, until) < 0
}

/// The alarms one window saw: their count and the cycles of the first two.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Seen {
    pub(crate) count: u8,
    pub(crate) first: Option<u32>,
    pub(crate) second: Option<u32>,
}

impl Seen {
    pub(crate) const NONE: Self = Self {
        count: 0,
        first: None,
        second: None,
    };

    /// The window with one more alarm, seen at `cycle`.
    #[must_use]
    pub(crate) const fn with(self, cycle: u32) -> Self {
        Self {
            count: self.count.saturating_add(1),
            first: match self.first {
                Some(first) => Some(first),
                None => Some(cycle),
            },
            second: match (self.first, self.second) {
                (Some(_), None) => Some(cycle),
                (_, second) => second,
            },
        }
    }

    /// The tally's bin: no alarm, one, more than one.
    pub(crate) const fn bin(self) -> usize {
        match self.count {
            0 => 0,
            1 => 1,
            _ => 2,
        }
    }
}

/// How a window's first two alarms lie against the `expected` cycle:
/// whether one came more than `tolerance` cycles early, and the deviation
/// of the first within `tolerance` of it.
pub(crate) fn classify(seen: Seen, expected: u32, tolerance: u32) -> (bool, Option<i32>) {
    let tolerance = i64::from(tolerance);
    let deviations = [seen.first, seen.second]
        .into_iter()
        .flatten()
        .map(|cycle| offset(cycle, expected));
    let mut early = false;
    let mut on_time = None;
    for deviation in deviations {
        let wide = i64::from(deviation);
        if wide < tolerance.saturating_neg() {
            early = true;
        } else if wide <= tolerance && on_time.is_none() {
            on_time = Some(deviation);
        }
    }
    (early, on_time)
}

#[cfg(test)]
mod tests;

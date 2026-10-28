//! The transmit power and CCA threshold a backend accepts, published as
//! capabilities and checked on every setting and request that names them.

/// Why a transmit-power level set cannot be published.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransmitPowerLevelsError {
    /// The set has no level.
    Empty,
    /// A level is lower than its predecessor.
    Descending {
        /// Index of the lower level.
        index: usize,
    },
}

/// The transmit power levels a backend radiates, in dBm: non-empty and
/// non-decreasing.
///
/// A requested power selects the greatest level not above it, so the radio
/// never radiates more than asked; a request below the lowest or above the
/// highest level is refused.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TransmitPowerLevels {
    levels_dbm: &'static [i8],
}

impl TransmitPowerLevels {
    /// Validate a level set.
    pub const fn new(levels_dbm: &'static [i8]) -> Result<Self, TransmitPowerLevelsError> {
        let Some((&first, mut rest)) = levels_dbm.split_first() else {
            return Err(TransmitPowerLevelsError::Empty);
        };
        let mut previous = first;
        while let Some((&level, tail)) = rest.split_first() {
            if level < previous {
                // rest is a suffix of levels_dbm, so this difference is exact.
                let index = levels_dbm.len().saturating_sub(rest.len());
                return Err(TransmitPowerLevelsError::Descending { index });
            }
            previous = level;
            rest = tail;
        }
        Ok(Self { levels_dbm })
    }

    /// The levels, lowest first.
    pub const fn levels_dbm(self) -> &'static [i8] {
        self.levels_dbm
    }

    /// The lowest level.
    pub const fn lowest_dbm(self) -> i8 {
        // Min's identity is MAX; construction proves at least one level.
        let mut lowest = i8::MAX;
        let mut rest = self.levels_dbm;
        while let Some((&level, tail)) = rest.split_first() {
            if level < lowest {
                lowest = level;
            }
            rest = tail;
        }
        lowest
    }

    /// The highest level.
    pub const fn highest_dbm(self) -> i8 {
        // Max's identity is MIN; construction proves at least one level.
        let mut highest = i8::MIN;
        let mut rest = self.levels_dbm;
        while let Some((&level, tail)) = rest.split_first() {
            if level > highest {
                highest = level;
            }
            rest = tail;
        }
        highest
    }

    /// The level a request of `requested_dbm` radiates: the greatest level
    /// not above it.
    pub const fn select(self, requested_dbm: i8) -> Result<i8, TransmitPowerError> {
        if requested_dbm < self.lowest_dbm() || requested_dbm > self.highest_dbm() {
            return Err(TransmitPowerError::OutOfRange {
                requested_dbm,
                lowest_dbm: self.lowest_dbm(),
                highest_dbm: self.highest_dbm(),
            });
        }
        let mut selected = self.lowest_dbm();
        let mut rest = self.levels_dbm;
        while let Some((&level, tail)) = rest.split_first() {
            if level > requested_dbm {
                break;
            }
            selected = level;
            rest = tail;
        }
        Ok(selected)
    }
}

/// The transmit power a backend accepts.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TransmitPowerCapability {
    /// The levels it radiates.
    pub levels: TransmitPowerLevels,
    /// The power every channel starts at, in dBm.
    pub reset_dbm: i8,
}

impl TransmitPowerCapability {
    /// The level `requested_dbm` radiates under an optional capability.
    pub const fn check(
        capability: Option<Self>,
        requested_dbm: i8,
    ) -> Result<i8, TransmitPowerError> {
        match capability {
            Some(capability) => capability.levels.select(requested_dbm),
            None => Err(TransmitPowerError::Unsupported),
        }
    }
}

/// Why a transmit power was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TransmitPowerError {
    /// The backend publishes no transmit-power capability.
    Unsupported,
    /// The power lies outside the backend's levels.
    OutOfRange {
        /// The rejected power in dBm.
        requested_dbm: i8,
        /// The lowest level in dBm.
        lowest_dbm: i8,
        /// The highest level in dBm.
        highest_dbm: i8,
    },
}

/// The clear-channel assessment energy threshold a backend accepts, in dBm.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CcaThresholdCapability {
    /// The lowest threshold.
    pub lowest_dbm: i8,
    /// The highest threshold.
    pub highest_dbm: i8,
    /// The threshold the radio starts with.
    pub reset_dbm: i8,
}

impl CcaThresholdCapability {
    /// Check `threshold_dbm` against an optional capability.
    pub const fn check(
        capability: Option<Self>,
        threshold_dbm: i8,
    ) -> Result<(), CcaThresholdError> {
        match capability {
            Some(capability)
                if threshold_dbm >= capability.lowest_dbm
                    && threshold_dbm <= capability.highest_dbm =>
            {
                Ok(())
            }
            Some(capability) => Err(CcaThresholdError::OutOfRange {
                requested_dbm: threshold_dbm,
                lowest_dbm: capability.lowest_dbm,
                highest_dbm: capability.highest_dbm,
            }),
            None => Err(CcaThresholdError::Unsupported),
        }
    }
}

/// Why a CCA threshold was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CcaThresholdError {
    /// The backend publishes no CCA threshold capability.
    Unsupported,
    /// The threshold lies outside the backend's range.
    OutOfRange {
        /// The rejected threshold in dBm.
        requested_dbm: i8,
        /// The lowest threshold in dBm.
        lowest_dbm: i8,
        /// The highest threshold in dBm.
        highest_dbm: i8,
    },
}

#[cfg(test)]
mod tests;

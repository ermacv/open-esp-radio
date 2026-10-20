//! Direct I²C completion within the existing observation settle.

use crate::calibration::cold::PhyColdI2cAction;
use oer_time::Clock;

/// Accept the terminal state in the same attempt that consumes its hardware
/// edge. A software-only terminal dispatch must not lose an already observed
/// completion to the elapsed-time check between attempts.
///
/// SOURCE(esp32s31): complete rev0 ROM `phy_chip_i2c_readReg_org` and
/// `phy_chip_i2c_writeReg` return the result once the busy observation clears;
/// there is no further hardware wait after that successful observation.
#[inline]
pub(crate) fn complete<S, E>(
    clock: &dyn Clock,
    state: &mut S,
    action: impl Fn(&S) -> PhyColdI2cAction,
    mut advance: impl FnMut(&mut S, PhyColdI2cAction) -> Result<(), E>,
) -> Result<bool, E> {
    super::poll::bounded(clock, || {
        let next = action(state);
        if matches!(next, PhyColdI2cAction::Complete(_)) {
            return Ok(true);
        }
        advance(state, next)?;
        Ok(matches!(action(state), PhyColdI2cAction::Complete(_)))
    })
}

#[cfg(test)]
mod tests;

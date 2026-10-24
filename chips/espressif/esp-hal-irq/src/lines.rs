//! The startup check of the CPU lines the image's interrupt table binds:
//! each bound level's line is enabled, hardware-vectored, level-triggered
//! and holds that level, and each bound source is silent or routed to its
//! level's line.
//!
//! Until the table writes the routes and the lines itself, esp-hal's
//! vectoring gives level `n` the CLIC line `15 + n` (`line_of`) with the
//! control byte `0x1f | (n - 1) << 5` (`control_of`), and esp-hal's
//! routes map a source at level `n` to that line. This module holds the
//! arithmetic and the verdicts; the chip code reads the words.
//!
//! SOURCE: esp-hal 1f646c5 `interrupt/riscv.rs` (`init_vectoring`,
//! `PRIORITY_TO_INTERRUPT`: vector `n - 1` is line `16 + n - 1`) and
//! `interrupt/riscv/clic.rs` (`prio_to_bits`); ESP-IDF 4d59230d
//! `soc/<chip>/include/soc/clic_reg.h` (`NLBITS` 3, `CLIC_EXT_INTR_NUM_OFFSET`
//! 16).

/// The first CPU line of the external interrupts; lines below it are the
/// core-local ones.
pub const FIRST_EXTERNAL_LINE: u8 = 16;

/// The levels a vectored line can hold: three level bits.
pub const LEVELS: u8 = 8;

/// The line esp-hal's vectoring gives `level` (1 to [`LEVELS`]).
#[must_use]
pub const fn line_of(level: u8) -> u8 {
    FIRST_EXTERNAL_LINE + level - 1
}

/// The control byte of a line at `level` (1 to [`LEVELS`]): the level in
/// the three level bits, every lower bit one.
#[must_use]
pub const fn control_of(level: u8) -> u8 {
    0x1f | ((level - 1) << 5)
}

/// The value of a line's trigger field for a positive level-triggered line.
pub const LEVEL_TRIGGERED: u8 = 0;

/// One CLIC line as its word holds it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LineState {
    /// The interrupt enable.
    pub enabled: bool,
    /// Hardware vectoring.
    pub vectored: bool,
    /// The trigger field: [`LEVEL_TRIGGERED`] or an edge or negative trigger.
    pub trigger: u8,
    /// The control byte.
    pub control: u8,
}

/// How a bound source's line disagrees with the table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LineError {
    /// The table names a level no vectored line holds.
    NoSuchLevel { level: u8 },
    /// The source's route word is not one the matrix block declares, or it
    /// names no CPU line.
    NoRouteWord,
    /// The source is routed to `line`, not to its level's `expected` line.
    WrongLine { line: u8, expected: u8 },
    /// The level's line is disabled.
    Disabled { line: u8 },
    /// The level's line is not hardware-vectored.
    NotVectored { line: u8 },
    /// The level's line is edge- or negative-triggered.
    NotLevelTriggered { line: u8, trigger: u8 },
    /// The level's line holds `control`, not its level's `expected` byte.
    WrongControl { line: u8, control: u8, expected: u8 },
}

/// Check that `level`'s line, in `state`, is enabled, vectored, positive
/// level-triggered and holds `level`.
///
/// # Errors
///
/// The first disagreement.
pub fn check_line(level: u8, state: LineState) -> Result<(), LineError> {
    if level == 0 || level > LEVELS {
        return Err(LineError::NoSuchLevel { level });
    }
    let line = line_of(level);
    let expected = control_of(level);
    if !state.enabled {
        Err(LineError::Disabled { line })
    } else if !state.vectored {
        Err(LineError::NotVectored { line })
    } else if state.trigger != LEVEL_TRIGGERED {
        Err(LineError::NotLevelTriggered {
            line,
            trigger: state.trigger,
        })
    } else if state.control != expected {
        Err(LineError::WrongControl {
            line,
            control: state.control,
            expected,
        })
    } else {
        Ok(())
    }
}

/// Check that a source bound at `level` is silent (`routed` zero) or routed
/// to its level's line.
///
/// # Errors
///
/// [`LineError::WrongLine`] or [`LineError::NoSuchLevel`].
pub fn check_route(level: u8, routed: u8) -> Result<(), LineError> {
    if level == 0 || level > LEVELS {
        return Err(LineError::NoSuchLevel { level });
    }
    let expected = line_of(level);
    if routed == 0 || routed == expected {
        Ok(())
    } else {
        Err(LineError::WrongLine {
            line: routed,
            expected,
        })
    }
}

/// Check a source bound at `level`: its level's line, in `state`
/// ([`check_line`]), and its route, `routed` the line its route word names
/// ([`check_route`]) or `None` when the matrix declares no word for it.
///
/// # Errors
///
/// The first disagreement.
pub fn check_bound(level: u8, state: LineState, routed: Option<u8>) -> Result<(), LineError> {
    check_line(level, state)?;
    check_route(level, routed.ok_or(LineError::NoRouteWord)?)
}

/// The level of `routed`, the line a source's route word names: `None` for
/// a silent source (line zero) and for a line whose control byte, which
/// `control` reads, holds no level or cannot be read.
pub fn routed_level(routed: u8, control: impl FnOnce(u8) -> Option<u8>) -> Option<u8> {
    if routed == 0 {
        return None;
    }
    level_of_control(control(routed)?)
}

/// The level a line's control byte holds, when it holds one.
#[must_use]
pub const fn level_of_control(control: u8) -> Option<u8> {
    if control & 0x1f == 0x1f {
        Some((control >> 5) + 1)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;

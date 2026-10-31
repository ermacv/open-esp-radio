//! What the stand knows of a board's ROM and of its ways back, apart from
//! the hardware that takes them: the reset paths, the recovery steps a
//! board's rungs take, and the ROM's reset lines.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub use crate::Result;

/// A way the stand resets a board on request (`cargo stand board reset
/// --via`, a soak's paths).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResetPath {
    /// RTS on the chip's own USB Serial/JTAG port.
    Rts,
    /// The CPU reset through the chip's JTAG.
    Jtag,
    /// A power cycle of the board's hub port.
    Power,
    /// A power cycle, then a reset into the ROM's download mode as soon as
    /// the board's USB returns.
    Download,
}

/// A step of a board's recovery, as the board journal records it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecoveryStep {
    /// A pulse on RTS of the chip's own USB Serial/JTAG port.
    RtsReset,
    /// Its switchable hub port was powered off and on.
    PowerCycle,
    /// Its switchable hub port was off and was powered on.
    PowerOn,
    /// A system reset through the chip's builtin USB-JTAG (OpenOCD `reset
    /// run`), which clears low-power state an RTS reset leaves.
    JtagReset,
    /// Its hub port's power cycled and, as soon as its USB returned, a reset
    /// into the ROM's download mode through its USB Serial/JTAG.
    DownloadEntry,
    /// Its chip's recovery image (`boot-smoke`) flashed, and it answered.
    Reflash,
}

/// One way to reset a board.
pub trait Rung {
    fn step(&self) -> RecoveryStep;
    /// Reset the board; the console its reset read, when the reset read it.
    fn reset(&self) -> Result<Option<String>>;
}

/// The ROM's last `rst:… boot:…` line in `console`.
pub fn reset_line(console: &str) -> Option<String> {
    console
        .lines()
        .map(str::trim)
        .rfind(|line| is_reset_line(line))
        .map(str::to_owned)
}

/// Whether `line` is a ROM reset line: a ROM that answers, booting from
/// flash or waiting for a download.
pub fn is_reset_line(line: &str) -> bool {
    line.starts_with("rst:") && line.contains("boot:")
}

#[cfg(test)]
mod tests;

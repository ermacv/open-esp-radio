//! What the stand knows of a board's ROM and of its ways back, apart from
//! the hardware that takes them: the reset paths and recovery steps, the
//! ladder [`climb`] takes over a board's rungs, and the ROM's reset lines.
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

/// One rung the ladder tried.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LadderStep {
    pub step: RecoveryStep,
    /// The ROM's reset line after the rung, or why the rung failed.
    pub outcome: std::result::Result<Option<String>, String>,
    /// Where the ROM's banner says core 0 was when the reset hit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved_pc: Option<u32>,
    /// Whether the firmware answered after the rung.
    pub cleared: bool,
}

/// Where the ladder ended.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "end", rename_all = "kebab-case")]
pub enum LadderEnd {
    /// The firmware answered after the last step.
    Cleared,
    /// The firmware never answered, but the ROM did, booting from flash or
    /// waiting for a download: the stand can load firmware into the board.
    Loadable { reset_line: String },
    /// Neither the firmware nor the ROM answered: a person is needed.
    Silent,
}

/// What the ladder did.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Ladder {
    pub steps: Vec<LadderStep>,
    pub end: LadderEnd,
}

/// Climb `rungs`, then the download entry when the board has one, stopping
/// at the first step after which `answers` sees the firmware answer.
/// `console` reads the board's console after a rung that read none itself.
pub fn climb(
    rungs: &[&dyn Rung],
    download_entry: Option<&dyn Fn() -> Result<String>>,
    console: &dyn Fn() -> String,
    answers: &mut dyn FnMut() -> bool,
) -> Ladder {
    let mut steps = Vec::new();
    for rung in rungs {
        let outcome = rung
            .reset()
            .map(|banner| banner.unwrap_or_else(console))
            .map_err(|error| error.to_string());
        let cleared = outcome.is_ok() && answers();
        steps.push(LadderStep {
            step: rung.step(),
            saved_pc: outcome.as_deref().ok().and_then(saved_pc),
            outcome: outcome.map(|banner| reset_line(&banner)),
            cleared,
        });
        if cleared {
            return Ladder {
                steps,
                end: LadderEnd::Cleared,
            };
        }
    }
    if let Some(entry) = download_entry {
        let outcome = entry().map_err(|error| error.to_string());
        steps.push(LadderStep {
            step: RecoveryStep::DownloadEntry,
            saved_pc: outcome.as_deref().ok().and_then(saved_pc),
            outcome: outcome.map(|banner| reset_line(&banner)),
            cleared: false,
        });
    }
    // The download entry's line, when it reached the ROM, else the latest
    // line of a ROM that answered.
    let end = steps
        .iter()
        .rev()
        .find_map(|step| step.outcome.as_ref().ok().cloned().flatten())
        .map_or(LadderEnd::Silent, |reset_line| LadderEnd::Loadable {
            reset_line,
        });
    Ladder { steps, end }
}

/// The program counter the ROM reports core 0 was at when the reset hit
/// (`Core0 Saved PC:0x...`), the last one printed.
pub fn saved_pc(banner: &str) -> Option<u32> {
    banner.lines().rev().find_map(|line| {
        let value = line.split_once("Core0 Saved PC:")?.1.trim();
        u32::from_str_radix(value.strip_prefix("0x")?, 16).ok()
    })
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

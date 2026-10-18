//! A board's resets: the USB Serial/JTAG openers and reset sequences, and
//! [`climb`], the one ladder of a board's ways back that every recovery and
//! `cargo stand board reset` take.
//!
//! The rungs are the board's stand-file `reset` steps in their order: an RTS
//! pulse on its USB Serial/JTAG port, a system reset through its builtin
//! USB-JTAG with OpenOCD, and its hub port's power. After them, a board that
//! resets by power can be put into the ROM's download mode: its port is
//! powered off and on and, as soon as its USB returns, reset into download
//! through the USB Serial/JTAG, before a flashed image that switches the
//! USB Serial/JTAG off runs. A ROM in download mode serves the USB
//! Serial/JTAG, so the stand can load firmware into the board.

use std::{path::Path, thread, time::Duration};

use serde::{Deserialize, Serialize};

use crate::port::{Lines, Port, Settings};

/// How long RTS holds the chip in reset when resetting into the application.
const APPLICATION_RESET: Duration = Duration::from_millis(200);

/// The console at `port`, opened without a reset: RTS is released before
/// DTR, so the lines never pass through the reset-with-boot-strap state
/// ([`Lines::Released`]).
///
/// This and [`reset_into_application`] are the only ways stand tools open a
/// board's console, for one command or for a long interactive session alike.
pub fn open_without_reset(port: &Path) -> std::io::Result<Port> {
    Port::open(port, Settings::CONSOLE)
}

/// Reset the board at `port` into its flashed application and return its
/// open console: RTS pulses the chip's reset while DTR keeps the boot strap
/// released. `espflash`'s own reset after connecting leaves some chips in
/// their ROM download mode.
pub fn reset_into_application(port: &Path) -> std::io::Result<Port> {
    let mut serial = Port::open(port, Settings::CONSOLE.lines(Lines::Kept))?;
    serial.set_dtr(false)?;
    serial.set_rts(true)?;
    thread::sleep(APPLICATION_RESET);
    serial.set_rts(false)?;
    Ok(serial)
}

/// Reset the board at `port` into its ROM's download mode through its USB
/// Serial/JTAG and return its open console: espflash's `usb-reset` into
/// download, DTR holding the boot strap across RTS's edge. The ROM serves the
/// USB Serial/JTAG in that mode, so the stand can load firmware even when the
/// flashed image switches the USB Serial/JTAG off once it runs.
pub fn reset_into_download(port: &Path) -> std::io::Result<Port> {
    let mut serial = Port::open(port, Settings::CONSOLE.lines(Lines::Kept))?;
    download_sequence(
        |step| apply(&mut serial, step),
        || thread::sleep(Duration::from_millis(100)),
    )?;
    Ok(serial)
}

fn apply(serial: &mut Port, step: Step) -> std::io::Result<()> {
    match step {
        Step::Dtr(level) => serial.set_dtr(level),
        Step::Rts(level) => serial.set_rts(level),
        Step::ClearInput => serial.clear_input(),
    }
}

#[derive(Clone, Copy)]
enum Step {
    Dtr(bool),
    Rts(bool),
    ClearInput,
}

/// Reset the chip behind a USB-Serial/JTAG port and discard what the old
/// boot sent, so the next read starts with the new boot.
pub fn reset_usb_serial_jtag(serial: &mut Port) -> std::io::Result<()> {
    sequence(
        |step| apply(serial, step),
        || thread::sleep(Duration::from_millis(100)),
    )
}

fn sequence<E>(
    mut apply: impl FnMut(Step) -> std::result::Result<(), E>,
    mut settle: impl FnMut(),
) -> std::result::Result<(), E> {
    // espflash's USB-Serial/JTAG reset sequence, with old-boot input drained
    // just before the reset. The chip restarts on RTS's rising edge, not on
    // its release: it boots while RTS is still asserted, and an
    // application has sent its hello before the release, so input drained
    // after the edge would discard the new boot's first words.
    settle();
    apply(Step::Dtr(false))?;
    settle();
    apply(Step::ClearInput)?;
    apply(Step::Rts(true))?;
    apply(Step::Dtr(false))?;
    apply(Step::Rts(true))?;
    settle();
    apply(Step::Rts(false))
}

/// espflash's USB Serial/JTAG reset into download mode: the boot strap
/// (DTR) held while RTS resets the chip, then both released.
fn download_sequence<E>(
    mut apply: impl FnMut(Step) -> std::result::Result<(), E>,
    mut settle: impl FnMut(),
) -> std::result::Result<(), E> {
    apply(Step::Rts(false))?;
    apply(Step::Dtr(false))?;
    settle();
    apply(Step::Dtr(true))?;
    apply(Step::Rts(false))?;
    settle();
    apply(Step::Rts(true))?;
    apply(Step::Dtr(false))?;
    apply(Step::Rts(true))?;
    settle();
    apply(Step::Dtr(false))?;
    apply(Step::Rts(false))
}

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
    fn reset(&self) -> crate::Result<Option<String>>;
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
    download_entry: Option<&dyn Fn() -> crate::Result<String>>,
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

/// The ROM's last reset line in `console`.
pub fn reset_line(console: &str) -> Option<String> {
    crate::console::reset_line(console).map(str::to_owned)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod ladder_tests;

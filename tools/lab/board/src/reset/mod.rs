//! A board's resets: the USB Serial/JTAG openers and reset sequences, the
//! rungs of the board's reset ladder.
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

#[cfg(test)]
mod tests;

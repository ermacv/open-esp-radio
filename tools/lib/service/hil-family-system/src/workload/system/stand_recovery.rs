//! The stand's recovery of a board whose image switched its USB Serial/JTAG
//! off its pads: the board leaves USB, its RTS and JTAG resets cannot reach
//! it, and only its hub port's power brings it back. Two phases, each from
//! a board that left USB on request:
//!
//! 1. the reset ladder from the stand file climbs until the image answers,
//!    which only the power rung can make it do;
//! 2. the download entry powers the board off and on and resets it into its
//!    ROM's download mode, where a flash finds it; an RTS reset then boots
//!    the image again.
use crate::Result;
use oer_devices::reset;
use oer_devices::reset::{Ladder, LadderEnd, RecoveryStep};
use oer_hil_workload::context::Context;
use serde::Serialize;
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// How long the board may take to leave USB after its acknowledgement.
const LEAVE: Duration = Duration::from_secs(5);
/// How long the console is read after a rung that read none itself.
const BANNER: Duration = Duration::from_secs(2);
/// How long the image may take to answer after a reset.
const ANSWER: Duration = Duration::from_secs(15);
/// How long the board's port may take to return after a reset.
const RETURN: Duration = Duration::from_secs(15);
/// How long udev may take to hand a returned port to the stand's group.
const PORT_ACCESS: Duration = Duration::from_secs(1);

#[derive(Serialize)]
struct Evidence {
    /// How long the board took to leave USB, per phase.
    left_usb_millis: [u64; 2],
    ladder: Ladder,
    ladder_millis: u64,
    /// The ROM's reset line after the download entry.
    download_reset_line: String,
    download_millis: u64,
}

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    let port = context.lab.dut.serial.clone();
    let board = context.dut_board()?;
    let answers = |name: &str| answers(&output.join(name), context);
    if !answers("before") {
        return Err("requires the diagnostic-usb-jtag-off image".into());
    }

    let first = leave_usb(&output.join("leave-1"), &port, context)?;
    let started = Instant::now();
    let mut attempt = 0;
    let rungs = board.rungs();
    let ladder = reset::climb(
        &rungs.iter().map(Box::as_ref).collect::<Vec<_>>(),
        None,
        &|| board.console(BANNER),
        &mut || {
            attempt += 1;
            answers(&format!("ladder-{attempt}"))
        },
    );
    let ladder_millis = millis(started.elapsed());
    cleared_by_power(&ladder)?;

    let second = leave_usb(&output.join("leave-2"), &port, context)?;
    if !board.has_power() {
        return Err(
            "the board does not reset by power: the stand file gives it no power step".into(),
        );
    }
    let started = Instant::now();
    let banner = board.download_entry()?;
    let download_millis = millis(started.elapsed());
    let download_reset_line = reset::reset_line(&banner).unwrap_or_default();
    waits_for_download(&download_reset_line)?;
    // Back to the image, through the USB Serial/JTAG the ROM drives.
    let rts = rungs
        .iter()
        .find(|rung| rung.step() == RecoveryStep::RtsReset)
        .ok_or("the board's ladder has no RTS reset to leave download mode")?;
    rts.reset()?;
    if !answers("after-download") {
        return Err("the image did not answer after the RTS reset out of download mode".into());
    }

    context.results.observe(
        "stand-recovery",
        &Evidence {
            left_usb_millis: [millis(first), millis(second)],
            ladder,
            ladder_millis,
            download_reset_line,
            download_millis,
        },
    );
    Ok(())
}

/// Ask the image to switch its USB off and wait until its port is gone;
/// how long it took.
fn leave_usb(output: &Path, port: &Path, context: &Context<'_>) -> Result<Duration> {
    context.with_capture(output, |capture| {
        // A request follows the boot's hello.
        capture.request_image_keys(ANSWER)?;
        capture.expect_detach()?;
        let oer_hil_protocol::system::UsbDisabled =
            capture.request(0, oer_hil_protocol::system::DisableUsb, ANSWER)?;
        Ok(())
    })?;
    let started = Instant::now();
    while port.exists() {
        if started.elapsed() > LEAVE {
            return Err(format!(
                "the board was still on USB {} s after its image switched USB off",
                LEAVE.as_secs()
            )
            .into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(started.elapsed())
}

/// Whether the image answers as the USB Serial/JTAG-off image, once its
/// port is back: udev recreates the board's link and hands it to the
/// stand's group a moment after the board returns to USB.
fn answers(output: &Path, context: &Context<'_>) -> bool {
    let port = &context.lab.dut.serial;
    let started = Instant::now();
    while !port.exists() {
        if started.elapsed() > RETURN {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(PORT_ACCESS);
    context
        .with_capture(output, |capture| {
            Ok(capture
                .request_image_keys(ANSWER)?
                .has::<oer_hil_protocol::system::DisableUsb>())
        })
        .unwrap_or(false)
}

/// A board off USB is out of reach of every rung but its power: the ladder
/// must end cleared, by the power cycle.
fn cleared_by_power(ladder: &Ladder) -> Result<()> {
    match (&ladder.end, ladder.steps.last()) {
        (LadderEnd::Cleared, Some(last)) if last.step == RecoveryStep::PowerCycle => Ok(()),
        _ => Err(format!(
            "the ladder did not bring the board back by its power: {:?}",
            ladder
        )
        .into()),
    }
}

/// Whether the ROM's reset line shows it waiting for a download.
fn waits_for_download(line: &str) -> Result<()> {
    if oer_devices::console::is_reset_line(line) && line.contains("DOWNLOAD") {
        Ok(())
    } else {
        Err(format!("the ROM does not wait for a download after the entry: `{line}`").into())
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_devices::reset::LadderStep;

    fn step(step: RecoveryStep, cleared: bool) -> LadderStep {
        LadderStep {
            step,
            outcome: Ok(None),
            saved_pc: None,
            cleared,
        }
    }

    #[test]
    fn only_the_power_rung_may_bring_back_a_board_off_usb() {
        let by_power = Ladder {
            steps: vec![
                step(RecoveryStep::RtsReset, false),
                step(RecoveryStep::PowerCycle, true),
            ],
            end: LadderEnd::Cleared,
        };
        assert!(cleared_by_power(&by_power).is_ok());
        let by_rts = Ladder {
            steps: vec![step(RecoveryStep::RtsReset, true)],
            end: LadderEnd::Cleared,
        };
        assert!(
            cleared_by_power(&by_rts).is_err(),
            "the board never left USB"
        );
        let silent = Ladder {
            steps: vec![step(RecoveryStep::PowerCycle, false)],
            end: LadderEnd::Silent,
        };
        assert!(cleared_by_power(&silent).is_err());
    }

    #[test]
    fn the_download_entry_ends_at_a_rom_waiting_for_a_download() {
        assert!(
            waits_for_download("rst:0x1 (POWERON),boot:0x6f (DOWNLOAD(USB/UART0/SPI))").is_ok()
        );
        assert!(waits_for_download("rst:0x1 (POWERON),boot:0x8 (SPI_FAST_FLASH_BOOT)").is_err());
        assert!(waits_for_download("").is_err());
    }
}

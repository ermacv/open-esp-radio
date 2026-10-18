//! Bringing back a device under test that stopped answering.
//!
//! When a failed repetition's target does not answer the post-mortem query,
//! or its bootloader resets in a loop, the runner climbs the board's ladder
//! ([`oer_devices::reset::climb`]): the resets of its stand-file `reset` ladder
//! in order, then, for a board that resets by power, the automatic entry into
//! its ROM's download mode. The first step after which the firmware answers
//! is journaled as a recovery; it counts as hardware-level when the port had
//! vanished or the ROM was waiting for a download, which firmware cannot
//! cause. Every step is kept in the repetition's `post-mortem/recovery.json`
//! or `reset-escalation.json`.
//!
//! A board is quarantined only when no step brings it back to a state in
//! which firmware can be loaded: its ROM stays silent. A ROM that answers,
//! booting from flash or waiting for a download, means the stand can reflash
//! the board, however bad its firmware; such a board is never quarantined.
//! A quarantined board serves nobody until a person resets or power-cycles
//! it. What the stand saw is kept in the repetition's `post-mortem/`, which
//! the quarantine names; a quarantine the arbiter does not record is an
//! error of the run, since the board would serve the next lease broken.
//!
//! The chip's recovery image is flashed only to a board whose ROM answered,
//! so a recovery image that does not answer is a fault of that image or the
//! checkout that built it, never a reason to quarantine the board.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use oer_devices::reset::{Ladder, LadderEnd, RecoveryStep};
use oer_stand_arbiter::{Arbiter, QuarantineTrigger};
use oer_stand_board::LeasedBoard;

use crate::post_mortem::{self, Finding};

/// What the ladder did.
pub enum Recovery {
    /// The board answered again after `step`.
    Recovered {
        step: RecoveryStep,
        hardware: bool,
        reset_line: Option<String>,
        /// Where core 0 was when the reset hit, from the ROM banner.
        core0: Option<String>,
        finding: Box<Finding>,
    },
    /// After `step` the ROM answered, but the firmware did not: a firmware
    /// or host fault. The stand can reflash the board, so it is not
    /// quarantined.
    BootedSilent {
        step: RecoveryStep,
        reset_line: String,
    },
    /// The board was quarantined for `trigger`.
    Quarantined {
        trigger: QuarantineTrigger,
        reason: String,
    },
}

impl Recovery {
    /// One sentence for the repetition's failure message.
    pub fn describe(&self) -> String {
        match self {
            Self::Recovered {
                step,
                hardware,
                reset_line,
                core0,
                ..
            } => format!(
                "the target did not answer; {step:?} brought it back ({} failure: {}){}",
                if *hardware {
                    "hardware"
                } else {
                    "firmware or unknown"
                },
                reset_line.as_deref().unwrap_or("no reset line"),
                core0
                    .as_deref()
                    .map(|core0| format!("; core 0 was at {core0}"))
                    .unwrap_or_default()
            ),
            Self::BootedSilent { step, reset_line } => format!(
                "the target did not answer; after {step:?} its ROM answered ({reset_line}) \
                 but the firmware did not: a firmware or host fault, not a board fault"
            ),
            Self::Quarantined { reason, .. } => format!("the board was quarantined: {reason}"),
        }
    }

    pub fn quarantined(&self) -> bool {
        matches!(self, Self::Quarantined { .. })
    }
}

/// Recover `board`, which did not answer after a failure in the
/// repetition whose output is `output`; `origin` names the run.
pub fn recover(
    board: &LeasedBoard,
    output: &Path,
    elf: Option<&[u8]>,
    origin: &str,
) -> crate::Result<Option<Recovery>> {
    // A cancelled run stops rather than judges the board it leaves.
    if !judges_board(oer_process::cancellation_requested()) {
        return Ok(None);
    }
    let arbiter = Arbiter::open()?;
    let mac = board.mac().to_string();
    let port = board.port();
    let evidence = output.join("post-mortem");
    let hardware =
        board.current_port(Duration::ZERO).is_none() || console_waits_for_download(output);
    let finding = std::cell::RefCell::new(None);
    let download_entry = board.download_entry_if_powered();
    let rungs = board.rungs();
    let ladder = oer_devices::reset::climb(
        &rungs.iter().map(Box::as_ref).collect::<Vec<_>>(),
        download_entry
            .as_ref()
            .map(|entry| entry as &dyn Fn() -> oer_devices::Result<String>),
        &|| board.console(BANNER_WATCH),
        &mut || {
            let found = post_mortem::inspect(port, board.access(), output, elf);
            let answered = found.is_some();
            *finding.borrow_mut() = found;
            answered
        },
    );
    let _ = std::fs::create_dir_all(&evidence);
    let _ = oer_durable::atomic_json(&evidence.join(RECOVERY_FILE), &ladder);
    let Some(last) = ladder.steps.last() else {
        return Ok(None);
    };
    let reset_line = last.outcome.clone().ok().flatten();
    match (ladder.end.clone(), finding.into_inner()) {
        (LadderEnd::Cleared, Some(finding)) => {
            let hardware = hardware
                || reset_line
                    .as_deref()
                    .is_some_and(|line| line.contains("DOWNLOAD"));
            // The earliest banner that named where core 0 was: the first
            // reset's, before later ones restart the core elsewhere.
            let core0 = ladder
                .steps
                .iter()
                .find_map(|step| step.saved_pc)
                .map(|address| {
                    let symbols = elf.and_then(|elf| oer_elf::dwarf::Symbolizer::new(elf).ok());
                    post_mortem::symbol(symbols.as_ref(), address)
                });
            let _ = arbiter.journal().record_by(
                String::from("stand"),
                Some(mac.clone()),
                oer_stand_journal::BoardEventKind::Recovered {
                    step: last.step,
                    hardware,
                    reset_line: reset_line.clone(),
                    origin: origin.to_owned(),
                },
            );
            Ok(Some(Recovery::Recovered {
                step: last.step,
                hardware,
                reset_line,
                core0,
                finding: Box::new(finding),
            }))
        }
        _ if !judges_board(oer_process::cancellation_requested()) => {
            eprintln!(
                "hil: the run was cancelled while the board was recovering; it is not \
                 quarantined"
            );
            Ok(None)
        }
        // A ROM that answers can be reflashed: only a silent one needs a person.
        (LadderEnd::Loadable { reset_line }, _) => Ok(Some(Recovery::BootedSilent {
            step: last.step,
            reset_line,
        })),
        _ => quarantine(
            &arbiter,
            &mac,
            QuarantineTrigger::Unreachable,
            format!(
                "neither its firmware nor its ROM answered after {}",
                describe_steps(&ladder)
            ),
            &evidence,
        )
        .map(Some),
    }
}

/// Journal that the board with `mac` answered its chip's recovery image
/// after the runner flashed it; failure is reported, never fatal.
pub fn record_reflash(mac: Option<String>, origin: String) {
    let recorded = Arbiter::open().and_then(|arbiter| {
        arbiter.journal().record_by(
            String::from("stand"),
            mac,
            oer_stand_journal::BoardEventKind::Recovered {
                step: RecoveryStep::Reflash,
                hardware: false,
                reset_line: None,
                origin,
            },
        )
    });
    if let Err(error) = recorded {
        eprintln!("hil-arbiter: cannot record the reflash: {error}");
    }
}

/// The file a recovery records its ladder in, in the repetition's
/// `post-mortem/`.
pub const RECOVERY_FILE: &str = "recovery.json";

/// The steps of `ladder`, as a list for a sentence.
fn describe_steps(ladder: &Ladder) -> String {
    ladder
        .steps
        .iter()
        .map(|step| format!("{:?}", step.step))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A bootloader that keeps resetting the same way: the ROM answers, but no
/// image starts, whatever the stand flashes.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BootLoop {
    /// The repeated ROM reset line.
    pub reset_line: String,
    /// How many consecutive resets of that kind the console shows.
    pub resets: usize,
}

/// The reset code of a ROM line, `rst:0x7` of `rst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x..`.
fn reset_code(line: &str) -> Option<&str> {
    line.split([' ', '(', ','])
        .next()
        .filter(|code| code.starts_with("rst:"))
}

/// The boot loop `console` ends in, if any: at least two consecutive ROM
/// reset lines with the same reset code at its end.
pub fn boot_loop(console: &str) -> Option<BootLoop> {
    let lines = console
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("rst:") && line.contains("boot:"))
        .collect::<Vec<_>>();
    let last = *lines.last()?;
    let code = reset_code(last)?;
    let resets = lines
        .iter()
        .rev()
        .take_while(|line| reset_code(line) == Some(code))
        .count();
    (resets >= 2).then(|| BootLoop {
        reset_line: last.to_owned(),
        resets,
    })
}

/// What the stand did about a boot loop, recorded as the repetition's
/// `reset-escalation.json`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ResetEscalation {
    pub boot_loop: BootLoop,
    pub ladder: Ladder,
}

/// The file a repetition records its reset escalation in.
pub const RESET_ESCALATION_FILE: &str = "reset-escalation.json";

/// How long the console is read for the ROM's line after a step that does
/// not return one itself.
const BANNER_WATCH: Duration = Duration::from_secs(2);

/// Climb past the RTS reset that did not clear `found`: the board's ladder
/// without its RTS rung, then its download entry, stopping at the first step
/// after which `boots` sees the image answer. A board whose ROM stays silent
/// after them all is quarantined for a person.
pub fn escalate_boot_loop(
    board: &LeasedBoard,
    found: BootLoop,
    output: &Path,
    origin: &str,
    mut boots: impl FnMut() -> bool,
) -> crate::Result<ResetEscalation> {
    let rungs = board.rungs();
    let download_entry = board.download_entry_if_powered();
    let ladder = oer_devices::reset::climb(
        &rungs
            .iter()
            .map(Box::as_ref)
            .filter(|rung| rung.step() != RecoveryStep::RtsReset)
            .collect::<Vec<_>>(),
        download_entry
            .as_ref()
            .map(|entry| entry as &dyn Fn() -> oer_devices::Result<String>),
        &|| board.console(BANNER_WATCH),
        &mut boots,
    );
    let escalation = ResetEscalation {
        boot_loop: found,
        ladder,
    };
    let _ = oer_durable::atomic_json(&output.join(RESET_ESCALATION_FILE), &escalation);
    let mac = board.mac();
    match (&escalation.ladder.end, escalation.ladder.steps.last()) {
        (LadderEnd::Cleared, Some(step)) => {
            // A recovery the journal misses changes no board's service.
            if let Ok(arbiter) = Arbiter::open() {
                let _ = arbiter.journal().record_by(
                    String::from("stand"),
                    Some(mac.to_string()),
                    oer_stand_journal::BoardEventKind::Recovered {
                        step: step.step,
                        hardware: true,
                        reset_line: step.outcome.clone().ok().flatten(),
                        origin: origin.to_owned(),
                    },
                );
            }
        }
        // A ROM that answers can be reflashed.
        (LadderEnd::Loadable { .. }, _) => {}
        _ => {
            let arbiter = Arbiter::open().map_err(|error| UnrecordedQuarantine {
                mac: mac.to_string(),
                cause: error.to_string(),
            })?;
            quarantine(
                &arbiter,
                mac,
                QuarantineTrigger::BootLoop,
                format!(
                    "its bootloader resets in a loop ({}) that {} did not clear, and its ROM stays silent",
                    escalation.boot_loop.reset_line,
                    describe_steps(&escalation.ladder)
                ),
                output,
            )?;
        }
    }
    Ok(escalation)
}

/// Whether an unanswered query after a reset judges the board. A cancelled
/// runner stops waiting for the answer early, which says nothing about it.
fn judges_board(cancelled: bool) -> bool {
    !cancelled
}

/// Set once this process quarantined its device under test: the run's
/// remaining repetitions then record the quarantine without touching it.
static QUARANTINED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Image classes whose firmware did not answer after its ROM booted it,
/// earlier in this process: the run's remaining repetitions of them cannot
/// pass, so they are recorded without holding the board.
static SILENT_IMAGES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Record that `image`'s firmware did not answer after a boot.
pub fn mark_image_silent(image: &str) {
    if let Ok(mut images) = SILENT_IMAGES.lock()
        && !images.iter().any(|known| known == image)
    {
        images.push(image.to_owned());
    }
}

/// Whether `image`'s firmware did not answer after a boot in this process.
pub fn image_silent(image: &str) -> bool {
    SILENT_IMAGES
        .lock()
        .is_ok_and(|images| images.iter().any(|known| known == image))
}

/// Whether this process quarantined its device under test.
pub fn device_quarantined() -> bool {
    QUARANTINED.load(std::sync::atomic::Ordering::Relaxed)
}

fn quarantine(
    arbiter: &Arbiter,
    mac: &str,
    trigger: QuarantineTrigger,
    reason: String,
    evidence: &Path,
) -> crate::Result<Recovery> {
    arbiter
        .quarantine(
            mac,
            trigger,
            reason.clone(),
            Some(evidence.display().to_string()),
        )
        .map_err(|error| UnrecordedQuarantine {
            mac: mac.to_owned(),
            cause: error.to_string(),
        })?;
    QUARANTINED.store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(Recovery::Quarantined { trigger, reason })
}

/// A board needs a person, but the arbiter did not record its quarantine:
/// the run must stop, since the board would otherwise serve its remaining
/// repetitions and the next lease broken.
#[derive(Debug)]
pub struct UnrecordedQuarantine {
    pub mac: String,
    pub cause: String,
}

impl std::fmt::Display for UnrecordedQuarantine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the board {} needs a person, but its quarantine was not recorded: {}",
            self.mac, self.cause
        )
    }
}

impl std::error::Error for UnrecordedQuarantine {}

/// Whether the repetition's console ended in the ROM waiting for a download.
fn console_waits_for_download(output: &Path) -> bool {
    let tail = |path: PathBuf| {
        std::fs::read(path).ok().map(|bytes| {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            text.lines()
                .rev()
                .take(20)
                .any(|line| line.contains("DOWNLOAD(") || line.contains("waiting for download"))
        })
    };
    tail(output.join("uart.log")).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_board_whose_rom_stays_silent_is_quarantined() {
        // A ROM that answers can be reflashed, whatever the firmware does.
        assert!(oer_devices::console::is_reset_line(
            "rst:0x17 (CHIP_USB_UART_RESET),boot:0x5f (SPI_FAST_FLASH_BOOT)"
        ));
        assert!(oer_devices::console::is_reset_line(
            "rst:0x1 (POWERON),boot:0x4 (DOWNLOAD(USB/UART0))"
        ));
        assert!(!oer_devices::console::is_reset_line("garbled output"));
    }

    #[test]
    fn an_image_that_did_not_answer_after_a_boot_is_remembered() {
        assert!(!image_silent("test-silent-image"));
        mark_image_silent("test-silent-image");
        mark_image_silent("test-silent-image");
        assert!(image_silent("test-silent-image"));
        assert!(!image_silent("test-other-image"));
    }

    /// Set in the child copy of the test binary, which runs with desktop
    /// notifications off: a quarantine notifies the user.
    const QUARANTINE_CHILD: &str = "OER_HIL_LAB_TEST_QUARANTINE_CHILD";

    #[test]
    fn an_unrecorded_quarantine_is_an_error_and_a_recorded_one_holds() {
        const TEST: &str =
            "recovery::tests::an_unrecorded_quarantine_is_an_error_and_a_recorded_one_holds";
        if std::env::var_os(QUARANTINE_CHILD).is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--test-threads=1"])
                .env(QUARANTINE_CHILD, "1")
                .env("OER_STAND_NOTIFY", "0")
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        let evidence = directory.path().join("post-mortem");
        // The maintenance record cannot be written.
        std::fs::create_dir(directory.path().join("maintenance.json")).unwrap();
        let error = quarantine(
            &arbiter,
            "02:00:00:00:31:7A",
            QuarantineTrigger::Unreachable,
            String::from("its ROM stays silent"),
            &evidence,
        )
        .err()
        .expect("an unrecorded quarantine is an error");
        assert!(error.is::<UnrecordedQuarantine>(), "{error}");
        assert!(
            error
                .to_string()
                .contains("its quarantine was not recorded"),
            "{error}"
        );
        assert!(!device_quarantined());
        std::fs::remove_dir(directory.path().join("maintenance.json")).unwrap();
        let recorded = quarantine(
            &arbiter,
            "02:00:00:00:31:7A",
            QuarantineTrigger::Unreachable,
            String::from("its ROM stays silent"),
            &evidence,
        )
        .unwrap();
        assert!(recorded.quarantined());
        assert!(arbiter.is_quarantined("02:00:00:00:31:7A").unwrap());
        assert!(device_quarantined());
    }

    #[test]
    fn a_cancelled_run_never_quarantines_its_board() {
        assert!(!judges_board(true));
        assert!(judges_board(false));
    }

    #[test]
    fn a_bootloader_that_resets_the_same_way_again_is_a_boot_loop() {
        let looping = "ESP-ROM:chip-a\nrst:0x1 (POWERON),boot:0x58 (SPI_FAST_FLASH_BOOT)\n\
            I (48) boot: Multicore bootloader\n\
            rst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x58 (SPI_FAST_FLASH_BOOT)\n\
            Core0 Saved PC:0x2f06f5f6\nI (48) boot: Multicore bootloader\n\
            rst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x58 (SPI_FAST_FLASH_BOOT)\n\
            I (48) boot: Multicore bootloader\n";
        assert_eq!(
            boot_loop(looping),
            Some(BootLoop {
                reset_line: String::from(
                    "rst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x58 (SPI_FAST_FLASH_BOOT)"
                ),
                resets: 2,
            })
        );
        // One reset, then the image: no loop.
        assert_eq!(
            boot_loop("rst:0x3 (SW_SYS_RESET),boot:0x58 (SPI_FAST_FLASH_BOOT)\nOER_BOOT\n"),
            None
        );
        // A different reset before the last one: no loop yet.
        assert_eq!(
            boot_loop(
                "rst:0x3 (SW_SYS_RESET),boot:0x58\nrst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x58\n"
            ),
            None
        );
        assert_eq!(boot_loop(""), None);
    }

    #[test]
    fn a_reset_escalation_records_every_step_it_tried() {
        let escalation = ResetEscalation {
            boot_loop: BootLoop {
                reset_line: String::from("rst:0x7 (HP_SYS_HP_WDT0_RESET),boot:0x58"),
                resets: 3,
            },
            ladder: Ladder {
                steps: vec![oer_devices::reset::LadderStep {
                    step: RecoveryStep::JtagReset,
                    outcome: Ok(Some(String::from("rst:0x3 (SW_SYS_RESET),boot:0x58"))),
                    saved_pc: None,
                    cleared: true,
                }],
                end: LadderEnd::Cleared,
            },
        };
        let json = serde_json::to_value(&escalation).unwrap();
        assert_eq!(json["ladder"]["steps"][0]["step"], "jtag-reset");
        assert_eq!(json["ladder"]["end"]["end"], "cleared");
        let back = serde_json::from_value::<ResetEscalation>(json).unwrap();
        assert_eq!(back.ladder, escalation.ladder);
    }

    #[test]
    fn the_rom_banner_names_where_core_zero_was() {
        let banner = "ESP-ROM:chip-a-20251218\nrst:0x17 (CHIP_USB_UART_RESET),boot:0x5f (SPI_FAST_FLASH_BOOT)\nCore0 Saved PC:0x50050cd4\nSPI mode:DIO\n";
        assert_eq!(oer_devices::reset::saved_pc(banner), Some(0x5005_0cd4));
        assert_eq!(oer_devices::reset::saved_pc("rst:0x1 (POWERON)\n"), None);
    }

    #[test]
    fn a_console_that_ends_in_download_mode_is_a_hardware_failure() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("uart.log"),
            "app\nrst:0x18 (JTAG_CPU),boot:0x0 (DOWNLOAD(UART0/SDIO_FEI_FEO))\nwait uart0/sdio download\n",
        )
        .unwrap();
        assert!(console_waits_for_download(directory.path()));
        std::fs::write(directory.path().join("uart.log"), "app\n").unwrap();
        assert!(!console_waits_for_download(directory.path()));
    }
}

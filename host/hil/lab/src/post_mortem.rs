//! How a failed repetition's target says it ended.
//!
//! After a repetition fails, the runner attaches to the target without
//! resetting it and asks for its boot evidence: the boot that follows a hang
//! watchdog reset reports the hang, and any boot reports why it started. A
//! hang becomes a `hang` failure with each hart's interrupted code named from
//! the image's ELF; a reset the scenario did not cause becomes an
//! `unexpected-reset` failure. Either names the last checkpoints the ended
//! boot passed. Nothing is reclassified when the target reports neither.

use std::{path::Path, time::Duration};

use oer_hil_protocol::base::{BootEvidence, Checkpoint, Fault, HangFault, ResetReason};

use oer_hil_link::SerialCapture;
use oer_hil_runs::format::run::Failure;
use oer_hil_runs::format::run::FailureKind;

/// How long a target may take to answer after a failure: a hang is reset by
/// its watchdog about seven seconds after it starts.
const ANSWER_WITHIN: Duration = Duration::from_secs(20);
/// Checkpoints named in a failure message.
const NAMED_CHECKPOINTS: usize = 6;

/// What the target reported after a failed repetition.
#[derive(Clone)]
pub struct Finding {
    pub boot: BootEvidence,
    pub checkpoints: Vec<Checkpoint>,
    /// The failure the report establishes, when it establishes one.
    pub failure: Option<Failure>,
}

/// The registers a JTAG post-mortem reads: where the hart was, where it
/// came from, and the trap it last took.
const JTAG_REGISTERS: [&str; 7] = ["pc", "ra", "sp", "mcause", "mepc", "mtval", "mstatus"];
/// The registers that hold code addresses, named from the image's ELF.
const JTAG_CODE_REGISTERS: [&str; 3] = ["pc", "ra", "mepc"];
/// Where a JTAG post-mortem is written in the repetition's directory.
pub const JTAG_POST_MORTEM_FILE: &str = "post-mortem/jtag.json";

/// Read through the chip's JTAG where a target that stopped answering is,
/// without the reset recovery would do: halt it, read its current hart's
/// registers, let it run on, name the code addresses from `elf` and write
/// them to [`JTAG_POST_MORTEM_FILE`]. It never changes the repetition's
/// outcome; a board without JTAG or OpenOCD leaves nothing.
/// [`jtag_snapshot`] through the stand's OpenOCD, when the ESP-IDF tools
/// hold one.
pub fn jtag_snapshot_through_stand_openocd(
    chip: &str,
    access: &oer_lab_board::lock::DeviceAccess,
    output: &Path,
    elf: Option<&[u8]>,
) {
    match oer_lab_board::openocd::Openocd::locate() {
        Ok(openocd) => jtag_snapshot(&openocd, chip, access, output, elf),
        Err(error) => eprintln!("hil: no JTAG post-mortem of the silent target: {error}"),
    }
}

pub fn jtag_snapshot(
    openocd: &oer_lab_board::openocd::Openocd,
    chip: &str,
    access: &oer_lab_board::lock::DeviceAccess,
    output: &Path,
    elf: Option<&[u8]>,
) {
    let operation = match access.operation() {
        Ok(operation) => operation,
        Err(error) => {
            eprintln!("hil: no JTAG post-mortem: {error}");
            return;
        }
    };
    let registers = match openocd.registers(
        chip,
        access.id(),
        &JTAG_REGISTERS,
        Duration::from_secs(30),
        operation.lifetime(),
    ) {
        Ok(registers) => registers,
        Err(error) => {
            eprintln!("hil: no JTAG post-mortem of the silent target: {error}");
            return;
        }
    };
    let loader = elf.and_then(|elf| oer_analysis_elf::dwarf::Symbolizer::new(elf).ok());
    let value = |name: &str| {
        registers
            .iter()
            .find(|(register, _)| register == name)
            .map(|(_, value)| *value)
    };
    let symbols = JTAG_CODE_REGISTERS
        .iter()
        .filter_map(|name| Some((*name, symbol(loader.as_ref(), value(name)?))))
        .collect::<std::collections::BTreeMap<_, _>>();
    let _ = oer_os_durable::atomic_json(
        &output.join(JTAG_POST_MORTEM_FILE),
        &serde_json::json!({
            "schema": 1,
            "registers": registers
                .iter()
                .map(|(name, value)| (name.clone(), format!("{value:#010x}")))
                .collect::<std::collections::BTreeMap<_, _>>(),
            "symbols": symbols,
        }),
    );
    eprintln!(
        "hil: the silent target was at {} (mcause {}); {}",
        symbols
            .get("pc")
            .map_or("an unknown address", String::as_str),
        value("mcause").map_or_else(|| String::from("unknown"), |cause| format!("{cause:#x}")),
        output.join(JTAG_POST_MORTEM_FILE).display()
    );
}

/// Attach to the target of board `mac` at `port` without resetting it,
/// record the exchange under `output/post-mortem`, and classify what it
/// reports; `None` when it does not answer.
pub fn inspect(
    port: &Path,
    access: &oer_lab_board::lock::DeviceAccess,
    output: &Path,
    elf: Option<&[u8]>,
) -> Option<Finding> {
    let _operation = access.operation().ok()?;
    let mac = access.id();
    // A reset can make the board's USB Serial/JTAG port re-enumerate under
    // another name; its MAC finds it again.
    let port = if port.exists() {
        port.to_owned()
    } else {
        oer_lab_board::discovery::wait_for(mac, ANSWER_WITHIN)?
    };
    let capture =
        SerialCapture::attach(crate::attach_console(&port), &output.join("post-mortem")).ok()?;
    let started = std::time::Instant::now();
    let answer = loop {
        // Attached without a reset, the capture has seen no hello: discovery,
        // the one command a target answers without its boot identity, names
        // it first. A target that rebooted has sent its hello already.
        let boot_status = || {
            capture.request(
                0,
                oer_hil_protocol::base::GetBootStatus,
                Duration::from_secs(5),
            )
        };
        let status = if capture.latest_boot_id().is_none() {
            capture
                .discover(Duration::from_secs(2))
                .and_then(|_| boot_status())
        } else {
            boot_status()
        };
        match status {
            Ok(boot) => break Some(boot),
            Err(_) if started.elapsed() < ANSWER_WITHIN => {
                if oer_os_process::sleep(Duration::from_millis(500)).is_err() {
                    break None;
                }
            }
            Err(_) => break None,
        }
    };
    let finding = answer.map(|boot| {
        let checkpoints = boot
            .post_mortem
            .as_ref()
            .and_then(|summary| capture.post_mortem_checkpoints(summary.checkpoints).ok())
            .unwrap_or_default();
        let symbols = elf.and_then(|elf| oer_analysis_elf::dwarf::Symbolizer::new(elf).ok());
        let name = |address: u32| symbol(symbols.as_ref(), address);
        let failure = classify(&boot, &checkpoints, &name);
        Finding {
            boot,
            checkpoints,
            failure,
        }
    });
    let _ = capture.finish_with(Ok(()));
    finding
}

/// The failure `boot` establishes, naming code addresses with `name`.
pub fn classify(
    boot: &BootEvidence,
    checkpoints: &[Checkpoint],
    name: &dyn Fn(u32) -> String,
) -> Option<Failure> {
    let trail = trail(checkpoints);
    if let Some(Fault::Hang(hang)) = boot.post_mortem.as_ref().and_then(|p| p.fault.as_ref()) {
        return Some(Failure::new(
            FailureKind::Hang,
            format!("{}{trail}", describe_hang(hang, name)),
        ));
    }
    match boot.reset_reason {
        // The runner's own resets and those it cannot attribute.
        ResetReason::UsbSerialJtag | ResetReason::Jtag | ResetReason::Other => None,
        reason => Some(Failure::new(
            FailureKind::UnexpectedReset,
            format!(
                "unexpected reset: {reason:?} (reset code {:#04x}){trail}",
                boot.raw_reset_reason
            ),
        )),
    }
}

/// Which executors stopped and where each hart was. Core 1's timers are
/// driven from core 0, so while core 0 is stalled a stalled core 1 is
/// reported as depending on it rather than as stalled on its own.
fn describe_hang(hang: &HangFault, name: &dyn Fn(u32) -> String) -> String {
    let protocol = hang.stalled_executors & 0b01 != 0;
    let network = hang.stalled_executors & 0b10 != 0;
    let task;
    let stalled = match (protocol, network) {
        (false, false) if let Some(stall) = hang.stalled_task => {
            task = format!(
                "task {} made no progress for {} ms while executors ran",
                stall.slot.id(),
                stall.pending_ms
            );
            task.as_str()
        }
        (true, true) => {
            "the core 0 protocol executor stalled (core 1's network heartbeat \
                         stopped with it: its timers depend on core 0)"
        }
        (true, false) => "the core 0 protocol executor stalled",
        (false, true) => "the core 1 network executor stalled",
        (false, false) => "an executor stalled",
    };
    let hart = |index: usize| {
        let state = &hang.harts[index];
        if state.responded {
            format!(
                "core {index} at {} called from {} (sp {:#010x})",
                name(state.mepc),
                name(state.ra),
                state.sp
            )
        } else {
            format!("core {index} did not answer: its interrupts are masked")
        }
    };
    // A stalled task is sampled on core 0, like a stalled core 0.
    let sampled = if network && !protocol { 1 } else { 0 };
    let mut samples = hang.samples.to_vec();
    samples.sort_unstable();
    samples.dedup();
    let mut loop_at = Vec::new();
    for named in samples.iter().map(|address| name(*address)) {
        if !loop_at.contains(&named) && loop_at.len() < 4 {
            loop_at.push(named);
        }
    }
    let loop_at = loop_at.join(", ");
    format!(
        "hang after {} ms: {stalled}; {}; {}; core {sampled} kept running {loop_at}",
        hang.detected_uptime_ms,
        hart(0),
        hart(1)
    )
}

/// `; last checkpoints: a(1)@10ms, b(2)@20ms` for the newest checkpoints.
fn trail(checkpoints: &[Checkpoint]) -> String {
    if checkpoints.is_empty() {
        return String::new();
    }
    let newest = checkpoints
        .iter()
        .rev()
        .take(NAMED_CHECKPOINTS)
        .rev()
        .map(|checkpoint| {
            format!(
                "{}({})@{}ms",
                checkpoint.name, checkpoint.arg, checkpoint.uptime_ms
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("; last checkpoints: {newest}")
}

/// `function (file.rs:34)` for `address` in the image from its debug
/// information, naming the function it was inlined into as `in outer`; the
/// address when the image does not describe it.
pub(crate) fn symbol(
    symbolizer: Option<&oer_analysis_elf::dwarf::Symbolizer>,
    address: u32,
) -> String {
    let raw = format!("{address:#010x}");
    let Some(Ok(frames)) = symbolizer.map(|s| s.frames(u64::from(address))) else {
        return raw;
    };
    let location = frames.iter().find_map(|frame| {
        let file = Path::new(frame.file.as_deref()?)
            .file_name()?
            .to_string_lossy()
            .into_owned();
        Some(format!("{file}:{}", frame.line?))
    });
    let names: Vec<&str> = frames
        .iter()
        .filter_map(|frame| frame.function.as_deref())
        .collect();
    let Some(innermost) = names.first() else {
        return raw;
    };
    let mut text = (*innermost).to_owned();
    if let Some(outer) = names.last().filter(|_| names.len() > 1) {
        text.push_str(&format!(" in {outer}"));
    }
    if let Some(location) = location {
        text.push_str(&format!(" ({location})"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_protocol::base::{HartState, PostMortemSummary};

    #[test]
    fn a_silent_target_is_read_through_its_jtag_without_a_reset() {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("openocd");
        let arguments = directory.path().join("arguments");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" > {}\n\
                 echo 'pc pc (/32): 0x42010736' >&2\n\
                 echo 'mcause mcause (/32): 0x30000007' >&2\n",
                arguments.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let openocd = oer_lab_board::openocd::Openocd {
            program: script,
            scripts: directory.path().to_owned(),
        };
        let access = oer_lab_board::lock::DeviceAccess::try_acquire_in(
            directory.path(),
            &oer_lab_board::lock::DeviceId::parse("02:00:00:00:25:64").unwrap(),
            "test",
        )
        .unwrap()
        .unwrap();
        let output = directory.path().join("repetition");
        jtag_snapshot(&openocd, "chip-b", &access, &output, None);
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join(JTAG_POST_MORTEM_FILE)).unwrap())
                .unwrap();
        assert_eq!(snapshot["registers"]["pc"], "0x42010736");
        assert_eq!(snapshot["registers"]["mcause"], "0x30000007");
        // Without the image's ELF the code address stays a number.
        assert_eq!(snapshot["symbols"]["pc"], "0x42010736");
        assert!(
            !std::fs::read_to_string(arguments)
                .unwrap()
                .contains("reset")
        );
        // An OpenOCD that reads nothing leaves nothing.
        let silent = directory.path().join("silent");
        std::fs::write(
            &silent,
            "#!/bin/sh
exit 1
",
        )
        .unwrap();
        std::fs::set_permissions(&silent, std::fs::Permissions::from_mode(0o755)).unwrap();
        let quiet = directory.path().join("quiet");
        jtag_snapshot(
            &oer_lab_board::openocd::Openocd {
                program: silent,
                scripts: directory.path().to_owned(),
            },
            "chip-b",
            &access,
            &quiet,
            None,
        );
        assert!(!quiet.join(JTAG_POST_MORTEM_FILE).exists());
    }

    fn boot(reset_reason: ResetReason, fault: Option<Fault>) -> BootEvidence {
        BootEvidence {
            reset_reason,
            raw_reset_reason: 0x03,
            post_mortem: Some(PostMortemSummary {
                boot_count: 2,
                checkpoints: 1,
                fault,
            }),
            platform_panic: None,
        }
    }

    fn hang(stalled: u8) -> Fault {
        let hart = |mepc| HartState {
            responded: true,
            mepc,
            ra: mepc + 4,
            sp: 0x2f01_9ef0,
            mcause: 0,
            mstatus: 0,
        };
        Fault::Hang(HangFault {
            detected_uptime_ms: 7505,
            stalled_executors: stalled,
            harts: [hart(0x5000_0010), hart(0x5000_0020)],
            samples: [0x5000_0020; 16],
            stalled_task: None,
        })
    }

    fn name(address: u32) -> String {
        format!("f{address:x}")
    }

    fn checkpoint(name: &str) -> Checkpoint {
        Checkpoint {
            name: name.try_into().unwrap(),
            arg: 1,
            uptime_ms: 10,
            hart: 0,
        }
    }

    #[test]
    fn a_hang_names_each_hart_and_the_checkpoints_before_it() {
        let failure = classify(
            &boot(ResetReason::Software, Some(hang(0b10))),
            &[checkpoint("coex.grant")],
            &name,
        )
        .unwrap();
        assert_eq!(failure.kind, FailureKind::Hang);
        let message = failure.message;
        assert!(
            message.contains("core 1 network executor stalled"),
            "{message}"
        );
        assert!(
            message.contains("core 0 at f50000010 called from f50000014"),
            "{message}"
        );
        assert!(
            message.contains("core 1 kept running f50000020"),
            "{message}"
        );
        assert!(
            message.contains("last checkpoints: coex.grant(1)@10ms"),
            "{message}"
        );
    }

    #[test]
    fn a_stalled_task_is_named_with_how_long_its_work_waited() {
        let Fault::Hang(mut stall) = hang(0) else {
            unreachable!()
        };
        stall.stalled_task = Some(oer_hil_protocol::base::TaskStall {
            slot: oer_hil_protocol::base::TaskSlot::Console,
            pending_ms: 5_250,
        });
        let failure = classify(
            &boot(ResetReason::Software, Some(Fault::Hang(stall))),
            &[],
            &name,
        )
        .unwrap();
        assert_eq!(failure.kind, FailureKind::Hang);
        assert!(
            failure.message.contains(
                "hang after 7505 ms: task console made no progress for 5250 ms while executors ran"
            ),
            "{}",
            failure.message
        );
    }

    #[test]
    fn core_one_stalled_with_core_zero_is_reported_as_dependent() {
        let failure = classify(&boot(ResetReason::Software, Some(hang(0b11))), &[], &name).unwrap();
        assert!(
            failure.message.contains("its timers depend on core 0"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn only_resets_the_runner_did_not_cause_are_unexpected() {
        let brownout = classify(&boot(ResetReason::Brownout, None), &[], &name).unwrap();
        assert_eq!(brownout.kind, FailureKind::UnexpectedReset);
        assert!(
            brownout.message.contains("Brownout"),
            "{}",
            brownout.message
        );
        for expected in [
            ResetReason::UsbSerialJtag,
            ResetReason::Jtag,
            ResetReason::Other,
        ] {
            assert!(classify(&boot(expected, None), &[], &name).is_none());
        }
    }
}

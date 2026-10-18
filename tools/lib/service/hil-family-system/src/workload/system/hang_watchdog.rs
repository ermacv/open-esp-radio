//! The correctness image's hang watchdog: a stalled executor, or a console
//! that stops taking commands while both executors run, leaves a post-mortem
//! naming the stall, and the chip resets.
use crate::Result;
use oer_hil_protocol::{
    base::{Fault, GetBootStatus, HangFault, ResetReason, TaskSlot},
    system::{HangInjected, HangTarget, InjectHang},
    telemetry::{ControlTrace, TraceControl, TraceState},
};
use oer_hil_workload::context::Context;
use std::{path::Path, time::Duration};

/// How long a request of this workload waits for its answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

const TARGETS: [HangTarget; 3] = [
    HangTarget::ProtocolExecutor,
    HangTarget::NetworkExecutor,
    HangTarget::Console,
];

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    // The chip model's region of the task stacks.
    let profile = oer_chip_profile::Profile::load(&oer_process::built_root(), context.lab.chip())?;
    let stacks = profile
        .memory
        .as_ref()
        .and_then(oer_chip_profile::Memory::task_stack_region)
        .ok_or_else(|| format!("chip {} names no [memory] task-stack region", profile.id))?;
    for target in TARGETS {
        let directory = output.join(scope(target));
        std::fs::create_dir_all(&directory)?;
        context.with_capture(&directory, |capture| {
            capture.prepare_startup(context.target())?;
            // An executor stall is detected after 3 s, a waiting command
            // after 5 s; sampling takes 4 s more.
            capture.expect_reboot(Duration::from_secs(5), Duration::from_secs(25))?;
            let HangInjected(injected) = capture.request(0, InjectHang(target), REQUEST_TIMEOUT)?;
            if injected != target {
                return Err(format!("hang injection {target:?} armed {injected:?}").into());
            }
            if target == HangTarget::Console {
                // The console now takes no command: this one waits, unanswered,
                // until the watchdog resets the chip.
                let _ = capture.request(0, GetBootStatus, REQUEST_TIMEOUT);
            }
            let reboot = capture.wait_expected_reboot()?;
            let fresh = capture.request(0, GetBootStatus, REQUEST_TIMEOUT)?;
            if fresh.reset_reason != ResetReason::Software {
                return Err(format!(
                    "expected the watchdog's software reset, not {:?} (0x{:02x})",
                    fresh.reset_reason, fresh.raw_reset_reason
                )
                .into());
            }
            let Some(Fault::Hang(hang)) = fresh.post_mortem.as_ref().and_then(|p| p.fault.clone())
            else {
                return Err(format!("no hang post-mortem after the reset: {fresh:?}").into());
            };
            if !names(target, &hang, &stacks) {
                return Err(
                    format!("the hang post-mortem does not name the stall: {hang:?}").into(),
                );
            }
            // The trace froze at the hang and waits, untouched, for the host.
            let TraceState(trace) =
                capture.request(0, ControlTrace(TraceControl::Status), REQUEST_TIMEOUT)?;
            let hang_kind = <oer_hil_trace::Hang as oer_trace::Event>::KIND.raw();
            if !trace.holding_previous || trace.trigger.map(|(kind, _)| kind) != Some(hang_kind) {
                return Err(format!("the trace did not freeze at the hang: {trace:?}").into());
            }
            let entries = capture.trace_entries(trace.entries)?;
            let TraceState(restarted) = capture.request(
                0,
                ControlTrace(TraceControl::Start { mask: u64::MAX }),
                REQUEST_TIMEOUT,
            )?;
            if !restarted.running || restarted.holding_previous {
                return Err(format!("the trace did not restart: {restarted:?}").into());
            }
            context.results.observe(
                scope(target),
                &serde_json::json!({
                    "target": target, "reboot": reboot, "fresh_boot": fresh,
                    "trace": trace, "trace_entries": entries.len(),
                }),
            );
            Ok(())
        })?;
    }
    Ok(())
}

/// Whether `hang` names the stall of `target` and where its hart was.
fn names(target: HangTarget, hang: &HangFault, stacks: &oer_chip_profile::Region) -> bool {
    let (stalled, task, hart) = match target {
        // Core 1's timers are driven from core 0, so a stalled protocol
        // executor stalls the network executor's heartbeat too.
        HangTarget::ProtocolExecutor => (hang.stalled_executors & 0b01 != 0, true, 0),
        HangTarget::NetworkExecutor => (hang.stalled_executors & 0b10 != 0, true, 1),
        HangTarget::Console => (
            hang.stalled_executors == 0,
            hang.stalled_task
                .is_some_and(|stall| stall.slot == TaskSlot::Console),
            0,
        ),
    };
    let hart = &hang.harts[hart];
    stalled
        && task
        && hart.responded
        && hart.mepc != 0
        && on_task_stack(hart.sp, stacks)
        && !hang.samples.contains(&0)
}

/// Whether `sp` lies in the region of the task stacks the chip profile
/// names (`[memory] task-stacks`), apart from the interrupt stack the
/// watchdog's own handler runs on: a stalled task reported with an `sp`
/// outside it names the handler's frame, not the task.
fn on_task_stack(sp: u32, stacks: &oer_chip_profile::Region) -> bool {
    (stacks.origin..stacks.end()).contains(&sp)
}

fn scope(target: HangTarget) -> &'static str {
    match target {
        HangTarget::ProtocolExecutor => "protocol-executor",
        HangTarget::NetworkExecutor => "network-executor",
        HangTarget::Console => "console",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_protocol::base::{HartState, TaskStall};

    /// A task-stack region, as a chip's profile names it.
    fn stacks() -> oer_chip_profile::Region {
        oer_chip_profile::Region {
            origin: 0x5000_0000,
            length: 0x0100_0000,
        }
    }

    fn hang(stalled_executors: u8, stalled_task: Option<TaskSlot>) -> HangFault {
        let hart = HartState {
            responded: true,
            mepc: 0x4200_0000,
            sp: 0x5004_0000,
            ..HartState::default()
        };
        HangFault {
            detected_uptime_ms: 9_000,
            stalled_executors,
            harts: [hart; 2],
            samples: [0x4200_0000; 16],
            stalled_task: stalled_task.map(|slot| TaskStall {
                slot,
                pending_ms: 5_001,
            }),
        }
    }

    #[test]
    fn a_console_hang_must_name_the_console_and_no_executor() {
        assert!(names(
            HangTarget::Console,
            &hang(0, Some(TaskSlot::Console)),
            &stacks()
        ));
        assert!(!names(HangTarget::Console, &hang(0b01, None), &stacks()));
        assert!(!names(
            HangTarget::Console,
            &hang(0, Some(TaskSlot::SessionEvidence)),
            &stacks()
        ));
        assert!(names(
            HangTarget::ProtocolExecutor,
            &hang(0b11, None),
            &stacks()
        ));
        assert!(!names(
            HangTarget::NetworkExecutor,
            &hang(0b01, None),
            &stacks()
        ));
    }

    #[test]
    fn a_stalled_hart_on_its_interrupt_stack_is_not_named() {
        let mut fault = hang(0b11, None);
        fault.harts[0].sp = 0x2f07_0000;
        assert!(!names(HangTarget::ProtocolExecutor, &fault, &stacks()));
    }
}

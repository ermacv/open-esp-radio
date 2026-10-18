//! esp-hal's inter-processor call into each core, core 0 included: the
//! call's line enters through the platform's own stack-switching vector
//! entry and the vectored dispatcher.

use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::once_lock::OnceLock;
use embassy_time::{Duration, Timer};
use esp_hal::{interrupt::ipc::Ipc, system::Cpu};
use oer_hil_protocol::system::CoresCalled;

/// A route's function has not run.
const NOT_RUN: u8 = u8::MAX;
/// How long the posted functions get to run.
const DEADLINE: Duration = Duration::from_millis(100);

static IPC: OnceLock<Ipc> = OnceLock::new();
/// The core each route's function ran on: core 0 to core 0, core 0 to
/// core 1, core 1 to core 0.
static RAN_ON: [AtomicU8; 3] = [const { AtomicU8::new(NOT_RUN) }; 3];

/// Serve the call with `ipc`, once both cores run.
pub(crate) fn install(ipc: Ipc) {
    let _ = IPC.init(ipc);
}

fn core0_to_core0() {
    RAN_ON[0].store(Cpu::current() as u8, Ordering::Release);
}

fn core0_to_core1() {
    RAN_ON[1].store(Cpu::current() as u8, Ordering::Release);
    if let Some(ipc) = IPC.try_get() {
        ipc.call_function(Cpu::ProCpu, core1_to_core0);
    }
}

fn core1_to_core0() {
    RAN_ON[2].store(Cpu::current() as u8, Ordering::Release);
}

/// Post the three routes' functions and report where each ran; `None`
/// before [`install`].
pub(crate) async fn call_across_cores() -> Option<CoresCalled> {
    let ipc = *IPC.try_get()?;
    for slot in &RAN_ON {
        slot.store(NOT_RUN, Ordering::Relaxed);
    }
    ipc.call_function(Cpu::ProCpu, core0_to_core0);
    ipc.call_function(Cpu::AppCpu, core0_to_core1);
    let ran = |slot: usize| match RAN_ON[slot].load(Ordering::Acquire) {
        NOT_RUN => None,
        core => Some(core),
    };
    let started = embassy_time::Instant::now();
    while (0..RAN_ON.len()).any(|slot| ran(slot).is_none()) && started.elapsed() < DEADLINE {
        Timer::after_millis(1).await;
    }
    Some(CoresCalled {
        core0_to_core0: ran(0),
        core0_to_core1: ran(1),
        core1_to_core0: ran(2),
    })
}

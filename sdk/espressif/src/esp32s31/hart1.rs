//! The second boot on hart 1 (`hart1` feature).
//!
//! Hart 0 ends its boot, grants hart 1's executor system timer comparator 2
//! and launches hart 1 through the image's mailbox
//! (`secondary_entry!` in `entry!`), handing it its software line, its
//! executor's wake and alarm and what the boot handed the second boot
//! ([`Hart1Parts`]). Hart 1 makes its own interrupt context, routes its
//! alarm, calls the image's `boot1` and reports it booted; hart 0 waits for
//! that report before it enables its own interrupts, so both boots end
//! before any executor runs. Hart 1 then runs its thread-mode executor
//! (`FROM_CPU1`) with the second boot's IP service and future as its roots.
//!
//! No waker crosses harts: each future that waits on the other hart waits
//! on its own executor's signal, which the other hart raises
//! ([`APPLICATION`], [`SECOND`], and the station's in `wifi`).

use core::sync::atomic::{AtomicBool, Ordering};

use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    cores::{Cores, LaunchPermit},
    peripherals::{
        reset_cause::ResetCauses,
        software_interrupt::{FromCpu1, FromCpu1Irq},
        system_timer::SystimerAlarm2,
    },
    platform::{Hart1Grant, Port},
};
use oer_espressif_boot::wait::{BootBudget, BootDeadline, BootWait};
use oer_espressif_executor::{AlarmGrant, Executor, Root, TimeDriverReady};
use oer_espressif_interrupt_table::signal::{FromPeerHart, IrqSignal, PeerNotify};
use oer_espressif_staged_runtime::{Mailbox, Running, SecondaryHandoff};

use super::{
    Error, Esp32s31, Hart1AlarmToken, Hart1Boot, Hart1Peripherals, Hart1Started, Hart1WakeToken,
    INTERRUPT_TABLE, NoNet, WakeToken, fail, run_app, run_net,
};

/// The application's executor, on hart 0.
pub(crate) type AppExecutor = Executor<Esp32s31, 0>;
/// Hart 1's executor.
pub(crate) type Hart1Executor = Executor<Esp32s31, 1>;

oer_espressif_executor::tasks! {
    /// Hart 1's executor's tasks: none of the library's; the second boot's
    /// IP service and future are its roots.
    pub Hart1Tasks: Executor<Esp32s31, 1> {
        @observer: super::executor_observer::<2>();
    }
}

oer_espressif_staged_runtime::placed_static! {
    static APPLICATION_SIGNAL: IrqSignal<AppExecutor> = IrqSignal::new::<FromPeerHart<Hart1Executor, Hart1WakeToken>, WakeToken>(), in critical_data "hart_peers";
    static SECOND_SIGNAL: IrqSignal<Hart1Executor> = IrqSignal::new::<FromPeerHart<AppExecutor, WakeToken>, Hart1WakeToken>(), in critical_data "hart_peers";
    /// The application's future on hart 0: the IP status and its channel
    /// ends wait on it.
    pub(crate) static APPLICATION: PeerNotify<AppExecutor> = PeerNotify::new(&APPLICATION_SIGNAL), in critical_data "hart_peers";
    /// The second boot's future on hart 1: its channel ends wait on it.
    pub(crate) static SECOND: PeerNotify<Hart1Executor> = PeerNotify::new(&SECOND_SIGNAL), in critical_data "hart_peers";
}

/// Hart 1 ended its second boot.
static BOOTED: AtomicBool = AtomicBool::new(false);

/// From hart 1's ready report to the end of its second boot: its interrupt
/// context, its alarm's route and the image's `boot1`, which builds the IP
/// service in place. A bound, not a measurement: the boot's work is
/// constructors without waits.
const BOOTED_WAIT: BootWait = const {
    match BootWait::new("hart1-booted", oer_time::Duration::from_millis(50)) {
        Ok(wait) => wait,
        Err(_) => panic!("a boot wait has a nonzero bound"),
    }
};

/// The deadline of the wait for hart 1's second boot, below the RTC
/// watchdog the runtime armed.
const BOOTED_BUDGET: BootBudget = match BootBudget::below(
    oer_time::Duration::from_millis(100),
    oer_time::Duration::from_millis(
        oer_esp32s31_hal::peripherals::watchdog::RTC_WATCHDOG_BUDGET_MS,
    ),
) {
    Some(budget) => budget,
    None => panic!("the second boot's budget lies below the RTC watchdog"),
};

const _: () = {
    if !BOOTED_BUDGET.admits(BOOTED_WAIT) {
        panic!("the second boot's wait fits its budget");
    }
};

/// What hart 1 takes at its launch: its software line and its executor's
/// wake, its executor's timer grant and what the boot handed the second
/// boot. Not an API.
pub struct Hart1Parts {
    line: FromCpu1,
    irq: FromCpu1Irq,
    wake: Hart1WakeToken,
    alarm: AlarmGrant<Esp32s31, 1, Hart1AlarmToken>,
    handed: Hart1Peripherals,
}

/// What hart 0 launches hart 1 with.
pub(crate) struct Launch {
    pub(crate) permit: LaunchPermit,
    pub(crate) causes: ResetCauses,
    pub(crate) line: FromCpu1,
    pub(crate) irq: FromCpu1Irq,
    pub(crate) wake: Hart1WakeToken,
    pub(crate) alarm: Hart1AlarmToken,
}

/// Grant hart 1's executor comparator 2, launch hart 1 with `handed`
/// through `mailbox` and wait until its second boot ended. Every failure
/// records and resets.
pub(crate) fn launch(
    mailbox: &'static Mailbox<Hart1Grant, Hart1Parts>,
    timers: (TimeDriverReady<Esp32s31>, SystimerAlarm2),
    launch: Launch,
    handed: Hart1Peripherals,
) -> Running<Port, 1> {
    let (mut ready, comparator) = timers;
    let alarm =
        oer_espressif_executor::grant::<Esp32s31, 1, _>(&mut ready, launch.alarm, comparator);
    let Ok(Some(mut cores)) = Cores::take() else {
        fail(oer_diagnostics::name!("hart1-cores"; 32));
    };
    let running = oer_esp32s31_compose::secondary::launch_hart1(
        &mut cores,
        launch.permit,
        launch.causes,
        &ready,
        mailbox,
        Hart1Parts {
            line: launch.line,
            irq: launch.irq,
            wake: launch.wake,
            alarm,
            handed,
        },
    )
    .unwrap_or_else(|failure| failure.reset());
    let deadline = BootDeadline::start(&EmbassyClock, BOOTED_BUDGET);
    if deadline
        .poll(BOOTED_WAIT, || BOOTED.load(Ordering::Acquire).then_some(()))
        .is_err()
    {
        fail(oer_diagnostics::name!("hart1-boot"; 32));
    }
    running
}

/// Hart 1's main, on its own stacks with its interrupt stack and exception
/// path installed: make its interrupt context, route its executor's alarm,
/// run the second boot, report it booted, enable its interrupts and run its
/// executor with the second boot's IP service and future. Not an API.
pub fn hart1_main<H: Hart1Started>(
    handoff: SecondaryHandoff<Port, 1>,
    parts: Hart1Parts,
    boot1: fn(Hart1Peripherals, Hart1Boot) -> Result<H, Error>,
) -> ! {
    let Hart1Parts {
        line,
        irq,
        wake,
        alarm,
        handed,
    } = parts;
    let mut adopted = handoff
        .adopt(INTERRUPT_TABLE)
        .unwrap_or_else(|error| fail(error.name()));
    {
        let (hart, router) = adopted.parts();
        alarm
            .bind(hart, router)
            .unwrap_or_else(|failure| failure.reset());
    }
    let (net, second) = boot1(handed, Hart1Boot { net: NoNet })
        .unwrap_or_else(|error| fail(error.name()))
        .into_parts();
    BOOTED.store(true, Ordering::Release);
    let executor = Hart1Executor::new(line);
    let (mut router, hart) = adopted
        .enable_interrupts()
        .unwrap_or_else(|error| fail(error.name()));
    // The second boot's futures stay in this frame, which never returns.
    let mut net = core::pin::pin!(run_net(net));
    observed_root!(net, Ip);
    let mut second = core::pin::pin!(second);
    let mut second = core::pin::pin!(run_app(second.as_mut()));
    observed_root!(second, Second);
    let roots = (Root::new(net.as_mut()), Root::new(second.as_mut()));
    match executor
        .run::<Hart1Tasks, _, _, _, 1, _>(&hart, &mut router, wake, irq, (), roots, |_| {})
        .unwrap_or_else(|failure| failure.reset()) {}
}

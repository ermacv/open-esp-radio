//! The panic-reset image: the HIL console serving a host-requested reset
//! through the platform's retained `HilRequested` path.
use oer_hil_agent::base::PanicRequest;
use oer_hil_protocol::system::InjectPanic;

use crate::transport::CONSOLE;

oer_espressif_executor::tasks! {
    /// The panic-reset image's tasks.
    PanicResetTasks: Executor<0> {
        reset_on_request() = super::panic::reset_on_request;
        run(usb: crate::transport::Usb, boot: u64) = run;
        rtc_watchdog(watchdog: RtcWatchdog) = crate::feed_rtc_watchdog;
    }
}

/// The RTC watchdog the root armed.
type RtcWatchdog =
    oer_esp32s31_hal::watchdog::ArmedRtcWatchdog<oer_esp32s31_hal::watchdog::RwdtRegisters>;

pub(crate) fn start<C: oer_espressif_interrupt_table::Controller>(
    executor: crate::Executor<0>,
    hart: &oer_espressif_staged_runtime::Hart<{ oer_espressif_staged_runtime::BOOT_HART }>,
    router: &mut oer_espressif_interrupt_table::Router<C>,
    (wake, line): (crate::ExecutorWake, oer_esp32s31_hal::platform::FromCpu0Irq),
    (usb, _pads): (
        crate::transport::Usb,
        oer_esp32s31_hal::usb_serial_jtag::UsbPads,
    ),
    mut entropy: crate::Rng,
    watchdog: RtcWatchdog,
) -> ! {
    let boot = u64::from_le_bytes(entropy.random_bytes()).max(1);
    CONSOLE.start(boot);
    match executor
        .run(hart, router, wake, line, (), |spawner| {
            PanicResetTasks::reset_on_request(spawner).expect("requested-reset task");
            PanicResetTasks::run(spawner, usb, boot).expect("system console task");
            if PanicResetTasks::rtc_watchdog(spawner, watchdog).is_err() {
                crate::fail(crate::fatal::Failure::TaskAllocation);
            }
        })
        .unwrap_or_else(|failure| failure.reset()) {};
}

async fn run(usb: crate::transport::Usb, boot: u64) {
    crate::transport::serve(
        usb,
        boot,
        0,
        |request, PanicRequest::Inject(InjectPanic)| super::panic::queue(request),
    )
    .await
}

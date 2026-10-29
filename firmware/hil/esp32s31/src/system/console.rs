//! The radio-free SoC diagnostic image: the HIL console serving the SoC
//! deadline watchdog, an explicit host-requested retained reset and,
//! in the `usb-jtag-off` image, the switch of the USB Serial/JTAG off its
//! pads.
use oer_esp32s31_compose::Esp32s31;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, DeadlineBudget, MainWatchdog, Mwdt1Registers, RwdtRegisters,
};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{RejectReason, Rejected};
use oer_hil_protocol::system::{WatchdogArmed, WatchdogTest, WatchdogTestMode};

use crate::transport::CONSOLE;
use oer_hil_agent::{base::Either, lifecycle::Request as ResetRequest};
use crate::services::WatchdogRequest;

/// The SoC deadline service on timer group 1's main watchdog, measured on
/// the image clock.
pub(crate) type DeadlineWatchdog =
    oer_esp32s31_hal::peripherals::watchdog::DeadlineWatchdog<oer_embassy_time::EmbassyClock>;

/// Watchdog tests the console handed over, one at a time.
static TESTS: Channel<CriticalSectionRawMutex, (RequestIdentity, WatchdogTestMode), 1> =
    Channel::new();

/// The USB Serial/JTAG's pads.
type Pads = oer_esp32s31_compose::console::Pads;

oer_espressif_executor::tasks! {
    /// The watchdog image's tasks.
    WatchdogTasks: Executor<Esp32s31, 0> {
        test_watchdog(service: DeadlineWatchdog) = test_watchdog;
        reset_on_request() = super::panic::reset_on_request;
        #[cfg(feature = "usb-jtag-off")]
        disable_on_request(pads: Pads) = usb_off::disable_on_request;
        run(usb: crate::transport::Usb, boot: u64) = run;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = crate::feed_rtc_watchdog;
    }
}

pub(crate) fn start<C: oer_espressif_interrupt_table::Controller>(
    executor: crate::Executor<Esp32s31, 0>,
    hart: &oer_espressif_staged_runtime::Hart<{ oer_espressif_staged_runtime::BOOT_HART }>,
    router: &mut oer_espressif_interrupt_table::Router<C>,
    (wake, line): (crate::ExecutorWake, oer_esp32s31_hal::peripherals::software_interrupt::FromCpu0Irq),
    (usb, pads): (crate::transport::Usb, Pads),
    mut entropy: crate::Rng,
    main1: MainWatchdog<Mwdt1Registers>,
    rtc_watchdog: ArmedRtcWatchdog<RwdtRegisters>,
) -> ! {
    let service = DeadlineWatchdog::new(main1, oer_embassy_time::EmbassyClock);
    let boot = u64::from_le_bytes(entropy.random_bytes()).max(1);
    crate::check_entropy_source(&entropy);
    #[cfg(not(feature = "usb-jtag-off"))]
    let _pads = pads;
    CONSOLE.start(boot);
    match executor
        .run(hart, router, wake, line, (), |spawner| {
            WatchdogTasks::test_watchdog(spawner, service).expect("system watchdog test task");
            WatchdogTasks::reset_on_request(spawner).expect("requested-reset task");
            #[cfg(feature = "usb-jtag-off")]
            WatchdogTasks::disable_on_request(spawner, pads).expect("USB switch task");
            WatchdogTasks::run(spawner, usb, boot).expect("system console task");
            if WatchdogTasks::rtc_watchdog(spawner, rtc_watchdog).is_err() {
                crate::fail(crate::fatal::Failure::TaskAllocation);
            }
        })
        .unwrap_or_else(|failure| failure.reset()) {};
}

fn queue_test(
    request: RequestIdentity,
    WatchdogRequest::Test(WatchdogTest(mode)): WatchdogRequest,
) {
    if TESTS.try_send((request, mode)).is_err() {
        CONSOLE.publish(
            request.session_id,
            request.request_id,
            &Rejected(RejectReason::Busy),
        );
    }
}

#[cfg(not(feature = "usb-jtag-off"))]
async fn run(usb: crate::transport::Usb, boot: u64) {
    crate::transport::serve(
        usb,
        boot,
        crate::services::WATCHDOG_KEYS,
        0,
        |request, decoded: Either<WatchdogRequest, ResetRequest>| match decoded {
            Either::First(test) => queue_test(request, test),
            Either::Second(ResetRequest::Reset(_)) => super::panic::queue(request),
        },
    )
    .await
}

#[cfg(feature = "usb-jtag-off")]
async fn run(usb: crate::transport::Usb, boot: u64) {
    use crate::services::UsbRequest;
    crate::transport::serve(
        usb,
        boot,
        crate::services::USB_JTAG_OFF_KEYS,
        0,
        |request, decoded: Either<Either<WatchdogRequest, UsbRequest>, ResetRequest>| match decoded
        {
            Either::First(Either::First(test)) => queue_test(request, test),
            Either::First(Either::Second(UsbRequest::Disable(_))) => usb_off::queue(request),
            Either::Second(ResetRequest::Reset(_)) => super::panic::queue(request),
        },
    )
    .await
}

/// The switch of the USB Serial/JTAG off its pads.
#[cfg(feature = "usb-jtag-off")]
mod usb_off {
    use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
    use oer_hil_protocol::RequestIdentity;
    use oer_hil_protocol::base::{RejectReason, Rejected};
    use oer_hil_protocol::system::UsbDisabled;

    use crate::transport::CONSOLE;

    static REQUESTS: Channel<CriticalSectionRawMutex, RequestIdentity, 1> = Channel::new();

    pub(super) fn queue(request: RequestIdentity) {
        if REQUESTS.try_send(request).is_err() {
            CONSOLE.publish(
                request.session_id,
                request.request_id,
                &Rejected(RejectReason::Busy),
            );
        }
    }

    /// Acknowledges the request, then takes the USB Serial/JTAG off its
    /// pads and drops its pull-ups: the host sees the device leave USB. Only a reset of the chip,
    /// which restores the pads for the ROM, brings it back.
    pub(super) async fn disable_on_request(pads: super::Pads) {
        let request = REQUESTS.receive().await;
        let sequence = CONSOLE
            .publish_reliably(request.session_id, request.request_id, &UsbDisabled)
            .await;
        // The host must see the acknowledgement before the device leaves.
        if let Err(fault) = CONSOLE.written(sequence).await {
            crate::system::receiver_limit(fault);
        }
        // Off its pads alone, the device stops answering but the host keeps
        // it enumerated: the D+ pull-up must go too for the host to see a
        // disconnect.
        pads.disconnect();
    }
}

/// Arms the SoC deadline watchdog for each test and misbehaves as asked.
async fn test_watchdog(mut service: DeadlineWatchdog) {
    loop {
        let (request, mode) = TESTS.receive().await;
        let budget = DeadlineBudget::from_micros(
            const { core::num::NonZeroU32::new(1_000_000).expect("one second is nonzero") },
        );
        let lease = match service.arm(budget) {
            Ok(lease) => lease,
            Err((returned, _error)) => {
                service = returned;
                let refused = Rejected(RejectReason::InvalidState);
                CONSOLE
                    .publish_reliably(request.session_id, request.request_id, &refused)
                    .await;
                continue;
            }
        };
        // The host finds this in the next boot's post-mortem.
        super::postmortem::checkpoint("watchdog.arm", mode as u32);
        let armed = WatchdogArmed(mode);
        if mode == WatchdogTestMode::Complete {
            service = lease.complete().expect("immediate diagnostic completion");
            CONSOLE
                .publish_reliably(request.session_id, request.request_id, &armed)
                .await;
        } else {
            let sequence = CONSOLE
                .publish_reliably(request.session_id, request.request_id, &armed)
                .await;
            // The host must see the acknowledgement before the reset.
            if let Err(fault) = CONSOLE.written(sequence).await {
                crate::system::receiver_limit(fault);
            }
            super::watchdog::inject(mode, lease).await;
        }
    }
}

//! The radio-free SoC diagnostic image: the HIL console serving the SoC
//! deadline watchdog and, in the `usb-jtag-off` image, the switch of the USB
//! Serial/JTAG off its pads.
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_esp32s31_soc_esp_hal::watchdog::{DeadlineBudget, DeadlineWatchdog};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{RejectReason, Rejected};
use oer_hil_protocol::system::{WatchdogArmed, WatchdogTest, WatchdogTestMode};
use static_cell::StaticCell;

use crate::transport::CONSOLE;
use oer_hil_agent::base::WatchdogRequest;

/// Watchdog tests the console handed over, one at a time.
static TESTS: Channel<CriticalSectionRawMutex, (RequestIdentity, WatchdogTestMode), 1> =
    Channel::new();

pub(crate) fn start(
    executor: &'static mut crate::Executor<0>,
    wake: crate::ExecutorWake,
    usb: crate::transport::Usb,
    rng: esp_hal::peripherals::RNG<'static>,
    timer: esp_hal::peripherals::TIMG1<'static>,
) -> ! {
    static SERVICE: StaticCell<DeadlineWatchdog> = StaticCell::new();
    let service = SERVICE.init(DeadlineWatchdog::new(timer));
    let entropy = esp_hal::rng::TrngSource::new(rng);
    let rng = esp_hal::rng::Trng::try_new().expect("HIL boot entropy");
    let boot = ((u64::from(rng.random()) << 32) | u64::from(rng.random())).max(1);
    drop(rng);
    drop(entropy);
    // `Rng` users keep reading the TRNG after its source owner is gone.
    crate::check_entropy_source();
    CONSOLE.start(boot);
    crate::transport::init_logger();
    executor.run(wake, |spawner| {
        spawner.spawn(test_watchdog(service).expect("system watchdog test task"));
        #[cfg(feature = "usb-jtag-off")]
        spawner.spawn(usb_off::disable_on_request().expect("USB switch task"));
        spawner.spawn(run(usb, boot).expect("system console task"));
    });
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
#[embassy_executor::task]
async fn run(usb: crate::transport::Usb, boot: u64) {
    crate::transport::serve(usb, boot, 0, queue_test).await
}

#[cfg(feature = "usb-jtag-off")]
#[embassy_executor::task]
async fn run(usb: crate::transport::Usb, boot: u64) {
    use oer_hil_agent::base::{Either, UsbRequest};
    crate::transport::serve(
        usb,
        boot,
        0,
        |request, decoded: Either<WatchdogRequest, UsbRequest>| match decoded {
            Either::First(test) => queue_test(request, test),
            Either::Second(UsbRequest::Disable(_)) => usb_off::queue(request),
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
    #[embassy_executor::task]
    pub(super) async fn disable_on_request() {
        let request = REQUESTS.receive().await;
        let sequence = CONSOLE
            .publish_reliably(request.session_id, request.request_id, &UsbDisabled)
            .await;
        // The host must see the acknowledgement before the device leaves.
        CONSOLE.written(sequence).await;
        // Off its pads alone, the device stops answering but the host keeps
        // it enumerated: the D+ pull-up must go too for the host to see a
        // disconnect.
        esp_hal::peripherals::USB_DEVICE::regs()
            .conf0()
            .modify(|_, w| {
                w.pad_pull_override()
                    .set_bit()
                    .dp_pullup()
                    .clear_bit()
                    .dm_pullup()
                    .clear_bit()
                    .usb_pad_enable()
                    .clear_bit()
            });
    }
}

/// Arms the SoC deadline watchdog for each test and misbehaves as asked.
#[embassy_executor::task]
async fn test_watchdog(service: &'static DeadlineWatchdog) {
    loop {
        let (request, mode) = TESTS.receive().await;
        let budget = DeadlineBudget::from_micros(core::num::NonZeroU32::new(1_000_000).unwrap());
        let Ok(lease) = service.arm(budget) else {
            let refused = Rejected(RejectReason::InvalidState);
            CONSOLE
                .publish_reliably(request.session_id, request.request_id, &refused)
                .await;
            continue;
        };
        // The host finds this in the next boot's post-mortem.
        super::postmortem::checkpoint("watchdog.arm", mode as u32);
        let armed = WatchdogArmed(mode);
        if mode == WatchdogTestMode::Complete {
            lease.complete().expect("immediate diagnostic completion");
            CONSOLE
                .publish_reliably(request.session_id, request.request_id, &armed)
                .await;
        } else {
            let sequence = CONSOLE
                .publish_reliably(request.session_id, request.request_id, &armed)
                .await;
            // The host must see the acknowledgement before the reset.
            CONSOLE.written(sequence).await;
            super::watchdog::inject(mode, lease).await;
        }
    }
}

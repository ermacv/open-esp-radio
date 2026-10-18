//! The radio-free SoC image's HIL console: the target core's console over
//! this chip's USB Serial/JTAG endpoint, serving the SoC deadline watchdog.
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use oer_esp32c5_soc_esp_hal::watchdog::{DeadlineBudget, DeadlineWatchdog};
use oer_hil_agent::base::{Intake, Platform, WatchdogRequest};
use oer_hil_agent::console::Console as HilConsole;
use oer_hil_image_keys::{ImageKeys, image_keys};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{
    BootEvidence, ImageKeySet, PostMortemCheckpoints, RejectReason, Rejected,
};
use oer_hil_protocol::system::{WatchdogArmed, WatchdogTest, WatchdogTestMode};
use static_cell::StaticCell;

unsafe extern "C" {
    fn ets_printf(format: *const core::ffi::c_char, ...) -> i32;
}

/// Writes one line through the ROM, independently of the USB driver.
fn rom_line(line: &core::ffi::CStr) {
    // SAFETY: the format consumes exactly one NUL-terminated string argument.
    unsafe {
        ets_printf(c"%s\r\n".as_ptr(), line.as_ptr());
    }
}

pub(crate) static CONSOLE: HilConsole<256, 8, 8> = HilConsole::new(rom_line);

/// Watchdog tests the console handed over, one at a time.
static TESTS: Channel<CriticalSectionRawMutex, (RequestIdentity, WatchdogTestMode), 1> =
    Channel::new();

struct Chip;

impl Platform for Chip {
    fn boot_evidence(&self) -> BootEvidence {
        super::boot_evidence()
    }

    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
        super::post_mortem_checkpoints(first)
    }
}

/// What the console task serves with: the USB Serial/JTAG peripheral with
/// the token of its table entry, whose handler is esp-hal's async driver's,
/// and the SoC deadline watchdog.
pub(crate) struct Console {
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    route: crate::ConsoleUsb,
    service: &'static DeadlineWatchdog,
}

impl Console {
    pub(crate) fn new(
        usb: esp_hal::peripherals::USB_DEVICE<'static>,
        route: crate::ConsoleUsb,
        watchdog: esp_hal::peripherals::TIMG1<'static>,
    ) -> Self {
        static SERVICE: StaticCell<DeadlineWatchdog> = StaticCell::new();
        Self {
            usb,
            route,
            service: SERVICE.init(DeadlineWatchdog::new(watchdog)),
        }
    }
}

/// Serve the host until the chip resets. It routes the console's source
/// before the driver turns async, which requires the route.
#[embassy_executor::task]
pub(crate) async fn run(console: Console) {
    oer_espressif_interrupt_table_esp_hal::enable(&console.route)
        .unwrap_or_else(|error| panic!("the console's USB route: {error:?}"));
    let usb = UsbSerialJtag::new(console.usb).into_async();
    // The boot identity only has to differ between boots.
    let rng = esp_hal::rng::Rng::new();
    let boot = ((u64::from(rng.random()) << 32) | u64::from(rng.random())).max(1);
    CONSOLE.start(boot);

    // This image's keys: the same function of its Cargo features the host
    // derives the system-watchdog class's keys with.
    static KEYS: StaticCell<ImageKeys> = StaticCell::new();
    let keys = KEYS.init(image_keys(&|feature| feature == "system-watchdog"));
    let intake = Intake::new(boot, ImageKeySet::new(keys), 0);
    let (rx, tx) = usb.split();
    let serving = CONSOLE.run(
        rx,
        tx,
        intake,
        &Chip,
        |request, WatchdogRequest::Test(WatchdogTest(mode))| {
            if TESTS.try_send((request, mode)).is_err() {
                CONSOLE.publish(
                    request.session_id,
                    request.request_id,
                    &Rejected(RejectReason::Busy),
                );
            }
        },
    );
    // Keep the large console future in its final storage; joining pinned
    // references avoids moving it into another future on the task stack.
    let serving = core::pin::pin!(serving);
    let testing = core::pin::pin!(test_watchdog(console.service));
    embassy_futures::join::join(serving, testing).await;
}

/// Arms the SoC deadline watchdog for each test and misbehaves as asked.
async fn test_watchdog(service: &'static DeadlineWatchdog) -> ! {
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

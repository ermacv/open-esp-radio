//! The radio-free SoC image's HIL console: the target core's console over
//! this chip's USB Serial/JTAG endpoint, serving the SoC deadline watchdog
//! and an intentional panic through the HIL panic hook.
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use oer_esp32c5_esp_hal_soc::watchdog::{DeadlineBudget, DeadlineWatchdog};
use oer_esp32c5_hal::platform::{Entropy, ResetCauses};
use oer_hil_agent::base::{Either, Intake, PanicRequest, Platform, WatchdogRequest};
use oer_hil_agent::console::{Console as HilConsole, Immediate};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{
    BootEvidence, ImageKeySet, PostMortemCheckpoints, RejectReason, Rejected,
};
use oer_hil_protocol::feature_keys::{ImageKeys, image_keys};
use oer_hil_protocol::system::{WatchdogArmed, WatchdogTest, WatchdogTestMode};
use static_cell::StaticCell;

/// Writes a note through the early USB writer, until the driver owns the
/// peripheral.
pub(crate) struct EarlyUsb;

impl Immediate for EarlyUsb {
    fn note(note: &oer_diagnostics::Note<'_>) -> bool {
        oer_espressif_usb_fifo::note(note)
    }

    fn take_lost() -> u32 {
        oer_espressif_usb_fifo::take_lost()
    }
}

pub(crate) static CONSOLE: HilConsole<8, 8, EarlyUsb> = HilConsole::new();

/// Watchdog tests the console handed over, one at a time.
static TESTS: Channel<CriticalSectionRawMutex, (RequestIdentity, WatchdogTestMode), 1> =
    Channel::new();

/// The platform the console reports: why the core last left reset, read
/// once at boot.
struct Chip {
    causes: ResetCauses,
}

impl Platform for Chip {
    fn boot_evidence(&self) -> BootEvidence {
        super::boot_evidence(self.causes)
    }

    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
        super::post_mortem_checkpoints(first)
    }
}

/// The image's entropy owner.
pub(crate) type Rng = Entropy<oer_embassy_time::EmbassyClock>;

/// What the console task serves with: the USB Serial/JTAG peripheral with
/// the token of its table entry, whose handler is esp-hal's async driver's,
/// the SoC deadline watchdog, the entropy owner and the boot's reset cause.
pub(crate) struct Console {
    usb: esp_hal::peripherals::USB_DEVICE<'static>,
    route: crate::ConsoleUsb,
    service: DeadlineWatchdog,
    rng: Rng,
    causes: ResetCauses,
}

impl Console {
    pub(crate) fn new(
        usb: esp_hal::peripherals::USB_DEVICE<'static>,
        route: crate::ConsoleUsb,
        watchdog: esp_hal::peripherals::TIMG1<'static>,
        rng: Rng,
        causes: ResetCauses,
    ) -> Self {
        Self {
            usb,
            route,
            service: DeadlineWatchdog::new(watchdog),
            rng,
            causes,
        }
    }
}

/// Serve the host until the chip resets. It routes the console's source
/// before the driver turns async, which requires the route.
pub(crate) async fn run(mut console: Console) {
    oer_espressif_esp_hal_irq::enable(&console.route)
        .unwrap_or_else(|error| panic!("the console's USB route: {error:?}"));
    let usb = UsbSerialJtag::new(oer_espressif_usb_fifo::hand_over(console.usb)).into_async();
    // The boot identity only has to differ between boots.
    let boot = u64::from_le_bytes(console.rng.random_bytes()).max(1);
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
        &Chip {
            causes: console.causes,
        },
        |request, decoded: Either<WatchdogRequest, PanicRequest>| match decoded {
            Either::First(WatchdogRequest::Test(WatchdogTest(mode))) => {
                if TESTS.try_send((request, mode)).is_err() {
                    CONSOLE.publish(
                        request.session_id,
                        request.request_id,
                        &Rejected(RejectReason::Busy),
                    );
                }
            }
            Either::Second(PanicRequest::Inject(_)) => super::panic::queue(request),
        },
    );
    // Keep the large console future in its final storage; joining pinned
    // references avoids moving it into another future on the task stack.
    let serving = core::pin::pin!(serving);
    let testing = core::pin::pin!(test_watchdog(console.service));
    let panicking = core::pin::pin!(super::panic::panic_on_request());
    embassy_futures::join::join3(serving, testing, panicking).await;
}

/// Arms the SoC deadline watchdog for each test and misbehaves as asked.
async fn test_watchdog(mut service: DeadlineWatchdog) -> ! {
    loop {
        let (request, mode) = TESTS.receive().await;
        let budget = DeadlineBudget::from_micros(core::num::NonZeroU32::new(1_000_000).unwrap());
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
            CONSOLE.written(sequence).await;
            super::watchdog::inject(mode, lease).await;
        }
    }
}

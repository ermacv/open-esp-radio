//! The target core's HIL console on this chip: its USB Serial/JTAG endpoint,
//! its ROM line output and its platform. Every HIL image of this runtime
//! serves the host through [`serve`].

use core::ffi::CStr;

use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use oer_hil_agent::base::{Intake, Platform, Requests};
use oer_hil_agent::console::{Console, Logger};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{BootEvidence, ImageKeySet, PostMortemCheckpoints};

const LINE: usize = 384;
const LINES: usize = 8;
const MESSAGES: usize = 8;

unsafe extern "C" {
    fn ets_printf(format: *const core::ffi::c_char, ...) -> i32;
}

/// Writes one line through the ROM, independently of the USB driver.
fn rom_line(line: &CStr) {
    // SAFETY: the format consumes exactly one NUL-terminated string argument.
    unsafe {
        ets_printf(c"%s\r\n".as_ptr(), line.as_ptr());
    }
}

#[unsafe(link_section = ".critical.data.logging")]
pub(crate) static CONSOLE: Console<LINE, LINES, MESSAGES> = Console::new(rom_line);

/// Installs the `log` facade over [`CONSOLE`]. Calling this more than once
/// is harmless.
pub(crate) fn init_logger() {
    static LOGGER: Logger<LINE, LINES, MESSAGES> = Logger(&CONSOLE);
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

struct Chip;

impl Platform for Chip {
    fn boot_evidence(&self) -> BootEvidence {
        crate::system::boot_evidence()
    }

    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
        crate::system::post_mortem_checkpoints(first)
    }
}

/// The USB Serial/JTAG peripheral with the token of its table entry, whose
/// handler is esp-hal's async driver's.
pub(crate) struct Usb {
    device: esp_hal::peripherals::USB_DEVICE<'static>,
    route: crate::ConsoleUsb,
}

impl Usb {
    pub(crate) fn new(
        device: esp_hal::peripherals::USB_DEVICE<'static>,
        route: crate::ConsoleUsb,
    ) -> Self {
        Self { device, route }
    }
}

/// Serves the host over `usb` until the chip resets: the base module here,
/// the image's own requests through `serve`, which must not block. The boot
/// must have started the console (`CONSOLE.start`) before any task
/// publishes. It routes the console's source before the driver turns async,
/// which requires the route.
pub(crate) async fn serve<C: Requests>(
    usb: Usb,
    boot: u64,
    maximum_payload_bytes: u16,
    serve: impl FnMut(RequestIdentity, C),
) -> ! {
    let intake = Intake::new(
        boot,
        ImageKeySet::new(crate::image_features::keys()),
        maximum_payload_bytes,
    );
    oer_espressif_interrupt_table_esp_hal::enable(&usb.route)
        .unwrap_or_else(|error| panic!("the console's USB route: {error:?}"));
    let (rx, tx) = UsbSerialJtag::new(usb.device).into_async().split();
    CONSOLE.run(rx, tx, intake, &Chip, serve).await
}

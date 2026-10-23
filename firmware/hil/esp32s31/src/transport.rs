//! The target core's HIL console on this chip: its USB Serial/JTAG endpoint,
//! its ROM line output and its platform. Every HIL image of this runtime
//! serves the host through [`serve`].

use esp_hal::usb::usb_serial_jtag::{
    UsbPadControl, UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx,
};
use oer_hil_agent::base::{Intake, Platform, Requests};
use oer_hil_agent::console::{Console, Immediate};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{BootEvidence, ImageKeySet, PostMortemCheckpoints};

const NOTES: usize = 8;
const MESSAGES: usize = 8;

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

oer_espressif_staged_runtime::placed_static! {
    pub(crate) static CONSOLE: Console<NOTES, MESSAGES, EarlyUsb> = Console::new(), in critical_data "logging";
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
    rx: UsbSerialJtagRx<'static, esp_hal::Async>,
    tx: UsbSerialJtagTx<'static, esp_hal::Async>,
}

impl Usb {
    pub(crate) fn new(
        device: esp_hal::peripherals::USB_DEVICE<'static>,
        route: crate::ConsoleUsb,
    ) -> (Self, UsbPadControl<'static>) {
        oer_espressif_esp_hal_irq::enable(&route)
            .unwrap_or_else(|error| panic!("the console's USB route: {error:?}"));
        let (rx, tx, pads) = UsbSerialJtag::new(oer_espressif_usb_fifo::hand_over(device))
            .into_async()
            .split_with_pad_control();
        (Self { rx, tx }, pads)
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
    CONSOLE.run(usb.rx, usb.tx, intake, &Chip, serve).await
}

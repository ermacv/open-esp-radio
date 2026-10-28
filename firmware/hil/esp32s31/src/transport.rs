//! The target core's HIL console on this chip: its USB Serial/JTAG endpoint,
//! its early writer and its platform. Every HIL image of this runtime
//! serves the host through [`serve`].

use oer_hil_agent::base::{Intake, Platform, Requests};
use oer_hil_agent::console::{Console, Immediate};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{BootEvidence, PostMortemCheckpoints};

const NOTES: usize = 8;
const MESSAGES: usize = 8;

/// Writes a note through the early USB writer, until the console opens.
pub(crate) struct EarlyUsb;

impl Immediate for EarlyUsb {
    fn note(note: &oer_diagnostics::Note<'_>) -> bool {
        crate::usb::early(note)
    }

    fn take_lost() -> u32 {
        oer_esp32s31_hal::usb_serial_jtag::take_early_lost()
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

/// The console's endpoint.
pub(crate) type Usb = crate::usb::Console;

/// Serves the host over `usb` until the chip resets: the base module here,
/// the image's own requests through `serve`, which must not block. The boot
/// must have started the console (`CONSOLE.start`) before any task
/// publishes.
pub(crate) async fn serve<C: Requests>(
    usb: Usb,
    boot: u64,
    maximum_payload_bytes: u16,
    serve: impl FnMut(RequestIdentity, C),
) -> ! {
    let intake = Intake::new(
        boot,
        crate::image_features::keys().as_set(),
        maximum_payload_bytes,
    );
    let failure = CONSOLE.run(usb, intake, &Chip, serve).await;
    console_fault(failure.cause)
}

pub(crate) fn console_fault(fault: oer_hil_agent::console::ConsoleFault) -> ! {
    use oer_hil_agent::console::ConsoleFault;
    let observations = match fault {
        ConsoleFault::ReadLength { length, capacity } => [0, length as u32, capacity as u32, 0],
        ConsoleFault::CountersBorrowed { writing } => [1, u32::from(writing), 0, 0],
    };
    oer_espressif_staged_runtime::panic::reset_fault::<oer_esp32s31_hal::platform::Port>(
        oer_espressif_staged_runtime::panic::FaultReason::HilConsole,
        observations,
    )
}

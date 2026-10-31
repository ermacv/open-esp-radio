//! The target core's HIL console on this chip: its USB Serial/JTAG endpoint,
//! its early writer and the boot evidence it serves. The image serves the
//! host through [`serve`].

use oer_hil_agent::base::{Intake, Requests};
use oer_hil_agent::console::{Console, Immediate};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::ImageKeySet;

const NOTES: usize = 8;
const MESSAGES: usize = 8;

/// Writes a note through the early USB writer, until the console opens.
pub(crate) struct EarlyUsb;

impl Immediate for EarlyUsb {
    fn note(note: &oer_diagnostics::Note<'_>) -> oer_diagnostics::Written {
        crate::usb::early(note)
    }

    fn take_lost() -> u32 {
        oer_esp32s31_compose::console::take_early_lost()
    }
}

/// The image's console.
pub(crate) type HilConsole = Console<NOTES, MESSAGES, EarlyUsb>;

oer_espressif_staged_runtime::placed_static! {
    pub(crate) static CONSOLE: HilConsole = Console::new(), in critical_data "logging";
}

/// The console's endpoint.
pub(crate) type Usb = crate::usb::Console;

/// Serves the host over `usb` until the chip resets: the base module here,
/// with `evidence` as its platform, the image's own requests through
/// `serve`, which must not block. The boot must have started the console
/// (`CONSOLE.start`) before any task publishes.
pub(crate) async fn serve<C: Requests>(
    usb: Usb,
    boot: u64,
    keys: ImageKeySet<'static>,
    maximum_payload_bytes: u16,
    evidence: crate::system::Evidence,
    serve: impl FnMut(RequestIdentity, C),
) -> ! {
    let intake = Intake::new(
        boot,
        keys,
        maximum_payload_bytes,
        oer_espressif_staged_runtime::layout::payload_crc32(),
    );
    let failure = CONSOLE.run(usb, intake, &evidence, serve).await;
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

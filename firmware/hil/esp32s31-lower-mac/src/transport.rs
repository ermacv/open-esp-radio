use oer_esp32s31_hal::usb_serial_jtag::{
    ConsoleEndpoint, ConsoleInterrupt, UsbPads, retire_early_writer,
};
use oer_espressif_executor::Executor;
use oer_espressif_usb_serial_jtag::{Endpoint, Interrupt};
use oer_hil_agent::console::{Console, Immediate};
use oer_hil_protocol::base::{BootEvidence, PostMortemCheckpoints, ResetReason};

pub(crate) struct EarlyUsb;

impl Immediate for EarlyUsb {
    fn note(note: &oer_diagnostics::Note<'_>) -> bool {
        oer_esp32s31_hal::usb_serial_jtag::early_note(note)
    }

    fn take_lost() -> u32 {
        oer_esp32s31_hal::usb_serial_jtag::take_early_lost()
    }
}

oer_espressif_staged_runtime::placed_static! {
    pub(crate) static CONSOLE: Console<8, 8, EarlyUsb> = Console::new(), in critical_data "logging";
    /// The console's interrupt: the endpoint's signal.
    static USB_INTERRUPT: Interrupt<Executor<0>, ConsoleInterrupt> = Interrupt::new(), in critical_data "console_usb";
}

/// The console's `Hardware` interrupt handler, over the part its entry
/// holds: it acknowledges the events it read and raises them to the
/// console's task, calling no waker.
pub(crate) fn usb_interrupt(part: &mut ConsoleInterrupt) {
    USB_INTERRUPT.service(part);
}

/// The image's platform: why the chip last reset, read once at boot.
pub struct Chip(pub oer_esp32s31_hal::platform::ResetCauses);
impl oer_hil_agent::base::Platform for Chip {
    fn boot_evidence(&self) -> BootEvidence {
        use oer_esp32s31_hal::platform::{HpCore, ResetCause};
        // The console serves boot evidence from core 0.
        let cause = self.0.of(HpCore::Core0);
        BootEvidence {
            reset_reason: match cause {
                ResetCause::CoreSoftware => ResetReason::Software,
                ResetCause::ChipPowerOn => ResetReason::PowerOn,
                _ => ResetReason::Other,
            },
            raw_reset_reason: cause.code(),
            post_mortem: None,
            platform_panic: None,
        }
    }
    fn post_mortem_checkpoints(&self, first: u8) -> PostMortemCheckpoints {
        PostMortemCheckpoints {
            first,
            checkpoints: Default::default(),
        }
    }
}

/// The console's endpoint, polled by the core-0 executor's console task.
pub type Usb = Endpoint<Executor<0>, ConsoleEndpoint>;

/// Opens the console over the block the early writer retires: the router
/// moves the interrupt's part to its handler and routes it. The image never
/// switches the pads, which it keeps.
pub fn open_usb<C: oer_interrupt_table::Controller>(
    router: &mut oer_interrupt_table::Router<C>,
    token: crate::ConsoleUsb,
) -> Result<(Usb, UsbPads), &'static str> {
    let (endpoint, interrupt, pads) = retire_early_writer().map_err(|_| "console-retire")?;
    let _bound = router.bind(token, interrupt).map_err(|_| "console-route")?;
    Ok((Endpoint::new(endpoint, USB_INTERRUPT.signal()), pads))
}

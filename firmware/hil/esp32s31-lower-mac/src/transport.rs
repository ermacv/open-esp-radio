use oer_esp32s31_hal::usb_serial_jtag::{
    ConsoleEndpoint, ConsoleInterrupt, UsbSerialJtagRegisters, console_parts,
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
    /// The console's interrupt: its handler's registers and the endpoint's
    /// signal.
    static USB_INTERRUPT: Interrupt<Executor<0>, ConsoleInterrupt> = Interrupt::new(), in critical_data "console_usb";
}

/// The console's `Hardware` interrupt handler: it acknowledges the events
/// it read and raises them to the console's task, calling no waker.
pub(crate) fn usb_interrupt() {
    let _ = USB_INTERRUPT.service();
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

/// Opens the console over the block the esp-hal adapter took from the
/// early writer and esp-hal (`take_usb_serial_jtag`): it gives the handler
/// its registers, then routes the interrupt. The image never switches the
/// pads.
pub fn open_usb(registers: UsbSerialJtagRegisters, route: crate::ConsoleUsb) -> Usb {
    let (endpoint, interrupt, _pads) = console_parts(registers);
    let signal = USB_INTERRUPT.install(interrupt);
    let routed = oer_espressif_esp_hal_irq::enable(&route);
    let (Ok(signal), Ok(())) = (signal, routed) else {
        panic!("the console's USB interrupt");
    };
    Endpoint::new(endpoint, signal)
}

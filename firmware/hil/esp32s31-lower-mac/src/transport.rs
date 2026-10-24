use esp_hal::usb::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use oer_hil_agent::console::{Console, Immediate};
use oer_hil_protocol::base::{BootEvidence, PostMortemCheckpoints, ResetReason};

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
    pub(crate) static CONSOLE: Console<8, 8, EarlyUsb> = Console::new(), in critical_data "logging";
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

pub struct Usb {
    pub rx: UsbSerialJtagRx<'static, esp_hal::Async>,
    pub tx: UsbSerialJtagTx<'static, esp_hal::Async>,
}
impl Usb {
    pub fn new(
        device: esp_hal::peripherals::USB_DEVICE<'static>,
        route: crate::ConsoleUsb,
    ) -> Self {
        oer_espressif_esp_hal_irq::enable(&route).expect("diagnostic console route");
        let (rx, tx) = UsbSerialJtag::new(oer_espressif_usb_fifo::hand_over(device))
            .into_async()
            .split();
        Self { rx, tx }
    }
}

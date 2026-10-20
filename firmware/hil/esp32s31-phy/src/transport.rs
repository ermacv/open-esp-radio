use esp_hal::usb::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use oer_hil_agent::console::Console;
use oer_hil_protocol::base::{BootEvidence, PostMortemCheckpoints, ResetReason};

fn rom_line(line: &core::ffi::CStr) {
    oer_esp32s31_board::rom::line(line);
}

oer_espressif_staged_runtime::placed_static! {
    pub(crate) static CONSOLE: Console<384, 8, 8> = Console::new(rom_line), in critical_data "logging";
}

pub struct Chip;
impl oer_hil_agent::base::Platform for Chip {
    fn boot_evidence(&self) -> BootEvidence {
        let raw = esp_hal::system::reset_reason();
        BootEvidence {
            reset_reason: match raw {
                Some(esp_hal::rtc_cntl::SocResetReason::CoreSw) => ResetReason::Software,
                Some(esp_hal::rtc_cntl::SocResetReason::ChipPowerOn) => ResetReason::PowerOn,
                _ => ResetReason::Other,
            },
            raw_reset_reason: raw.map_or(0, |reason| reason as u8),
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
        let (rx, tx) = UsbSerialJtag::new(device).into_async().split();
        Self { rx, tx }
    }
}

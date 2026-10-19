//! Sole output owner for the minimal boot-smoke image.
//!
//! The normal HIL image uses the framed transport in `console`. The smoke
//! image intentionally does not link that protocol, but it must not keep using
//! the ROM printf backend after ESP-HAL has initialized the USB peripheral.

use esp_hal::{Blocking, peripherals::USB_DEVICE, usb::usb_serial_jtag::UsbSerialJtag};

pub(crate) struct BootSmokeConsole {
    usb: UsbSerialJtag<'static, Blocking>,
}

impl BootSmokeConsole {
    pub(crate) fn new(peripheral: USB_DEVICE<'static>) -> Self {
        Self {
            usb: UsbSerialJtag::new(peripheral),
        }
    }

    pub(crate) fn embassy_started(&mut self) {
        let _ = self.usb.write(b"OPEN_RADIO_HIL embassy=START\r\n");
    }

    pub(crate) fn timer_passed(&mut self) {
        let _ = self
            .usb
            .write(b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS\r\n");
    }
}

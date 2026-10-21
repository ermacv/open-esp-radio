//! Sole output owner for the minimal boot-smoke image.
//!
//! The normal HIL image uses the framed transport in `console`. The smoke
//! image intentionally does not link that protocol, but it must not keep using
//! the ROM printf backend after ESP-HAL has initialized the USB peripheral.

use core::fmt::Write;

use esp_hal::{Blocking, peripherals::USB_DEVICE, usb::usb_serial_jtag::UsbSerialJtag};

use crate::boot_smoke_rearm::Measurement;

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
            .write(b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS rearm=PASS\r\n");
    }

    pub(crate) fn measurement(&mut self, measurement: &Measurement) {
        let _ = write!(
            self.usb,
            "OPEN_RADIO_HIL rearm step={} delay_ms={} elapsed_us={}\r\n",
            measurement.step, measurement.delay_ms, measurement.elapsed_us
        );
    }

    pub(crate) fn rearm_failed(&mut self, failure: &Measurement) {
        let _ = write!(
            self.usb,
            "OPEN_RADIO_HIL boot-smoke=FAIL rearm=FAIL step={} elapsed_us={}\r\n",
            failure.step, failure.elapsed_us
        );
    }
}

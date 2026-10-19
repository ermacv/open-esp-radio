//! The boot-smoke image: `hal=INIT` once esp-hal has initialized the chip
//! and stage two adopted PSRAM, `embassy=START` once the executor polls its
//! first task, then one Embassy timer wake and the pass line the host waits
//! for, all on the USB Serial/JTAG console.

use embassy_time::{Duration, Timer};
use esp_hal::{Blocking, peripherals::USB_DEVICE, usb::usb_serial_jtag::UsbSerialJtag};

/// How long the smoke image waits on its timer before it passes.
const TIMER_WAKE: Duration = Duration::from_millis(100);

/// The console, opened before the executor starts, so a fault while it
/// starts still leaves the lines before it on the console.
pub(crate) fn start(device: USB_DEVICE<'static>) -> UsbSerialJtag<'static, Blocking> {
    let mut console = UsbSerialJtag::new(device);
    let _ = console.write(b"OPEN_RADIO_HIL hal=INIT\r\n");
    console
}

#[embassy_executor::task]
pub(crate) async fn run(mut console: UsbSerialJtag<'static, Blocking>) {
    let _ = console.write(b"OPEN_RADIO_HIL embassy=START\r\n");
    Timer::after(TIMER_WAKE).await;
    let _ = console.write(b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS\r\n");
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

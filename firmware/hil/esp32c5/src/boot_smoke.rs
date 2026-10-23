//! The boot-smoke image: `hal=INIT` once esp-hal has initialized the chip
//! and stage two adopted PSRAM, `embassy=START` once the executor polls its
//! first task, then Embassy timer wakes and rearming before the host's pass
//! line, all on the USB Serial/JTAG console.

use embassy_time::{Duration, Timer};
use esp_hal::{Blocking, peripherals::USB_DEVICE, usb::usb_serial_jtag::UsbSerialJtag};
use oer_diagnostics::{Note, emit_note};

/// How long the smoke image waits on its timer before it passes.
const TIMER_WAKE: Duration = Duration::from_millis(100);

/// The console, opened before the executor starts, so a fault while it
/// starts still leaves the lines before it on the console.
pub(crate) fn start(device: USB_DEVICE<'static>) -> UsbSerialJtag<'static, Blocking> {
    let mut console = UsbSerialJtag::new(oer_espressif_usb_fifo::hand_over(device));
    let _ = console.write(b"OPEN_RADIO_HIL hal=INIT\r\n");
    console
}

#[embassy_executor::task]
pub(crate) async fn run(mut console: UsbSerialJtag<'static, Blocking>) {
    let _ = console.write(b"OPEN_RADIO_HIL embassy=START\r\n");
    Timer::after(TIMER_WAKE).await;
    if let Err(failure) = crate::boot_smoke_rearm::run(|measurement| {
        let _ = emit_note(
            &Note::rearm(
                u32::from(measurement.step),
                measurement.delay_ms,
                measurement.elapsed_us,
            ),
            |bytes| console.write(bytes),
        );
    })
    .await
    {
        let _ = emit_note(
            &Note::rearm_failed(u32::from(failure.step), failure.elapsed_us),
            |bytes| console.write(bytes),
        );
        core::future::pending::<()>().await;
    }
    let _ = console.write(b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS rearm=PASS\r\n");
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

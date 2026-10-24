//! The boot-smoke image: `hal=INIT` once esp-hal has initialized the chip
//! and stage two adopted PSRAM, `embassy=START` once the executor polls its
//! first task, then Embassy timer wakes and rearming before the host's pass
//! line, all on the USB Serial/JTAG console.

use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use oer_diagnostics::{MAX_NOTE_LINE, Note, emit_note};

/// The smoke's console.
pub(crate) type Console = crate::usb::Console;

/// How long the smoke image waits on its timer before it passes.
const TIMER_WAKE: Duration = Duration::from_millis(100);

/// The console, opened before the executor starts: the early writer writes
/// the first line, so a fault while the executor starts still leaves it on
/// the console.
pub(crate) fn start(
    device: esp_hal::peripherals::USB_DEVICE<'static>,
    route: crate::ConsoleUsb,
) -> Console {
    oer_esp32c5_hal::usb_serial_jtag::early_text(b"OPEN_RADIO_HIL hal=INIT\r\n");
    crate::usb::open(
        oer_esp32c5_esp_hal_soc::handoff::take_usb_serial_jtag(device)
            .expect("no early record holds the USB Serial/JTAG at the console handoff"),
        route,
    )
}

pub(crate) async fn run(mut console: Console) {
    let _ = console.write_all(b"OPEN_RADIO_HIL embassy=START\r\n").await;
    Timer::after(TIMER_WAKE).await;
    let rearm =
        crate::boot_smoke_rearm::run(async |measurement: &crate::boot_smoke_rearm::Measurement| {
            note(
                &mut console,
                &Note::rearm(
                    u32::from(measurement.step),
                    measurement.delay_ms,
                    measurement.elapsed_us,
                ),
            )
            .await;
        })
        .await;
    if let Err(failure) = rearm {
        note(
            &mut console,
            &Note::rearm_failed(u32::from(failure.step), failure.elapsed_us),
        )
        .await;
        core::future::pending::<()>().await;
    }
    let _ = console
        .write_all(b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS rearm=PASS\r\n")
        .await;
    loop {
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// Writes `note` as one line: encoded first, then written whole.
async fn note(console: &mut Console, note: &Note<'_>) {
    let mut line = [0; MAX_NOTE_LINE];
    let mut length = 0;
    let encoded = emit_note(note, |bytes: &[u8]| {
        let end = length + bytes.len();
        let slot = line.get_mut(length..end).ok_or(())?;
        slot.copy_from_slice(bytes);
        length = end;
        Ok::<(), ()>(())
    });
    if encoded.is_ok()
        && let Some(bytes) = line.get(..length)
    {
        let _ = console.write_all(bytes).await;
    }
}

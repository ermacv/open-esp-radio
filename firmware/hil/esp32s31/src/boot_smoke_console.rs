//! Sole output owner for the minimal boot-smoke image.
//!
//! The normal HIL image uses the framed transport in `console`. The smoke
//! image intentionally does not link that protocol: it writes its lines on
//! the console's endpoint, which takes the block from the early writer
//! before the smoke starts. Each line waits for the host on the executor.

use embedded_io_async::Write;
use oer_diagnostics::{MAX_NOTE_LINE, Note, emit_note};

use crate::boot_smoke_rearm::Measurement;

pub(crate) struct BootSmokeConsole {
    usb: crate::usb::Console,
    /// The console's pads, kept so that nothing switches them off.
    _pads: oer_esp32s31_hal::usb_serial_jtag::UsbPads,
}

impl BootSmokeConsole {
    pub(crate) fn new(
        (usb, pads): (
            crate::usb::Console,
            oer_esp32s31_hal::usb_serial_jtag::UsbPads,
        ),
    ) -> Self {
        Self { usb, _pads: pads }
    }

    pub(crate) async fn embassy_started(&mut self) {
        let _ = self
            .usb
            .write_all(b"OPEN_RADIO_HIL embassy=START\r\n")
            .await;
    }

    pub(crate) async fn timer_passed(&mut self) {
        let _ = self
            .usb
            .write_all(b"OPEN_RADIO_HIL boot-smoke=PASS timer=PASS rearm=PASS\r\n")
            .await;
    }

    pub(crate) async fn measurement(&mut self, measurement: &Measurement) {
        self.note(&Note::rearm(
            u32::from(measurement.step),
            measurement.delay_ms,
            measurement.elapsed_us,
        ))
        .await;
    }

    pub(crate) async fn rearm_failed(&mut self, failure: &Measurement) {
        self.note(&Note::rearm_failed(
            u32::from(failure.step),
            failure.elapsed_us,
        ))
        .await;
    }

    /// Writes `note` as one line: encoded first, then written whole.
    async fn note(&mut self, note: &Note<'_>) {
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
            let _ = self.usb.write_all(bytes).await;
        }
    }
}

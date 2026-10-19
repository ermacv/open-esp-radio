//! Writing an image's segments through one espflash connection to the ROM
//! stub. A segment whose flash contents already match (MD5) is skipped; the
//! segments go in their order, the OTA selection last.

use std::{path::Path, time::Duration};

use espflash::{
    connection::{Connection, ResetAfterOperation, ResetBeforeOperation},
    flasher::Flasher,
    image_format::Segment,
    target::ProgressCallbacks,
};

use super::port;
use crate::{Result, bundle::SnapshotSegment};

/// Where the writer leaves the chip after the last segment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum After {
    /// Reset into the written image.
    HardReset,
    /// Stay in the flasher stub: the caller resets the chip itself, with its
    /// console already open.
    StayInStub,
}

/// What happened to one segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Written {
    pub description: &'static str,
    pub offset: u32,
    pub bytes: usize,
    pub skipped: bool,
}

/// Records whether espflash skipped each segment. It finishes every segment
/// once, in order, and a skipped one without starting it first.
#[derive(Default)]
struct Progress {
    skipped: Vec<bool>,
}

impl ProgressCallbacks for Progress {
    fn init(&mut self, _addr: u32, _total: usize) {}
    fn update(&mut self, _current: usize) {}
    fn verifying(&mut self) {}
    fn finish(&mut self, skipped: bool) {
        self.skipped.push(skipped);
    }
}

/// Write `segments` into the flash of the board `mac`, an espflash `chip`,
/// at `tty`.
pub fn write(
    tty: &Path,
    mac: &str,
    chip: espflash::target::Chip,
    segments: &[SnapshotSegment],
    after: After,
) -> Result<Vec<Written>> {
    // espflash's handshake needs a bounded read before it sets its own.
    let serial = port::open_kept(tty, Duration::from_secs(3))?;
    let usb = serialport::UsbPortInfo {
        vid: 0x303a,
        pid: 0x1001,
        serial_number: Some(mac.to_owned()),
        manufacturer: None,
        product: None,
    };
    let connection = Connection::new(
        serial,
        usb,
        match after {
            After::HardReset => ResetAfterOperation::HardReset,
            After::StayInStub => ResetAfterOperation::NoResetNoStub,
        },
        ResetBeforeOperation::DefaultReset,
        port::BAUD,
    );
    let mut flasher =
        Flasher::connect(connection, true, false, true, Some(chip), None).map_err(|error| {
            format!(
                "connect to the ROM loader through {}: {error}",
                tty.display()
            )
        })?;
    let regions = segments
        .iter()
        .map(|segment| Segment::new(segment.offset, &segment.data))
        .collect::<Vec<_>>();
    let mut progress = Progress::default();
    flasher
        .write_bins_to_flash(&regions, &mut progress)
        .map_err(|error| format!("write through {}: {error}", tty.display()))?;
    Ok(segments
        .iter()
        .zip(
            progress
                .skipped
                .iter()
                .copied()
                .chain(std::iter::repeat(false)),
        )
        .map(|(segment, skipped)| Written {
            description: segment.description,
            offset: segment.offset,
            bytes: segment.data.len(),
            skipped,
        })
        .collect())
}

//! The responder's console lines, written without `core::fmt`. The host
//! program (`hil/scenarios/beacon-stamp`) reads them:
//!
//! ```text
//! ACKRESP v1 begin address=<12 hex> bssid=<12 hex> channel=<dec> window_s=<dec>
//! ACKRESP listening
//! ACKRESP end received=<dec> rxfull=<dec> error=<name|-> mac=<name> dma=<name> finish=<name>
//! ```

use crate::line::{Line, LineFull};

/// The station address the responder answers on: the probe's calibration
/// frames go to it (`hil/probes/esp32s31-beacon-stamp/src/frames.rs`).
pub(crate) const ADDRESS: [u8; 6] = [0x02, 0x4f, 0x45, 0x00, 0x00, 0x02];
/// The one BSS it admits: the calibration frames' transmitter and BSSID.
pub(crate) const BSSID: [u8; 6] = [0x02, 0x4f, 0x45, 0x00, 0x00, 0x03];

pub(crate) fn previous_line(kind: u32, phase: u32) -> Result<Line<64>, LineFull> {
    let mut line = Line::new();
    line.text(b"ACKRESP previous kind=")?
        .decimal(u64::from(kind))?
        .text(b" phase=")?
        .decimal(u64::from(phase))?
        .text(b"\n")?;
    Ok(line)
}

pub(crate) fn begin_line(channel: u8, window_s: u32) -> Result<Line<128>, LineFull> {
    let mut line = Line::new();
    line.text(b"ACKRESP v1 begin address=")?
        .hex_bytes(&ADDRESS)?
        .text(b" bssid=")?
        .hex_bytes(&BSSID)?
        .text(b" channel=")?
        .decimal(u64::from(channel))?
        .text(b" window_s=")?
        .decimal(u64::from(window_s))?
        .text(b"\n")?;
    Ok(line)
}

/// How the window ended, for its end line.
pub(crate) struct End<'a> {
    pub(crate) received: usize,
    pub(crate) rx_full: u16,
    pub(crate) cause: Option<&'a [u8]>,
    pub(crate) mac: &'a [u8],
    pub(crate) dma: &'a [u8],
    pub(crate) finish: &'a [u8],
}

pub(crate) fn end_line(end: &End<'_>) -> Result<Line<192>, LineFull> {
    let mut line = Line::new();
    line.text(b"ACKRESP end received=")?
        .decimal(end.received as u64)?
        .text(b" rxfull=")?
        .decimal(u64::from(end.rx_full))?
        .text(b" error=")?
        .text(end.cause.unwrap_or(b"-"))?
        .text(b" mac=")?
        .text(end.mac)?
        .text(b" dma=")?
        .text(end.dma)?
        .text(b" finish=")?
        .text(end.finish)?
        .text(b"\n")?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_name_the_address_the_window_and_every_end() {
        assert_eq!(
            begin_line(13, 120).unwrap().as_bytes(),
            b"ACKRESP v1 begin address=024f45000002 bssid=024f45000003 channel=13 window_s=120\n"
        );
        let end = End {
            received: 48,
            rx_full: 0,
            cause: None,
            mac: b"inactive",
            dma: b"stopped",
            finish: b"passed",
        };
        assert_eq!(
            end_line(&end).unwrap().as_bytes(),
            b"ACKRESP end received=48 rxfull=0 error=- mac=inactive dma=stopped finish=passed\n"
        );
        let failed = End {
            cause: Some(b"initial-fatal"),
            mac: b"still-active",
            dma: b"unconfirmed",
            finish: b"not-run",
            ..end
        };
        assert!(
            end_line(&failed).unwrap().as_bytes().ends_with(
                b"error=initial-fatal mac=still-active dma=unconfirmed finish=not-run\n"
            )
        );
        assert_eq!(
            previous_line(1, 3).unwrap().as_bytes(),
            b"ACKRESP previous kind=1 phase=3\n"
        );
    }
}

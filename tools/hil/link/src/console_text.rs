//! The human text of the target's console: lines of typed diagnostics
//! (`OER-DIAG:01…`) become the text the host prints for them, every other
//! line stays as it came. A diagnostic that is lost, garbled or unknown is
//! shown as an `OER-DIAG-ERROR` line with the raw line, never dropped.

use oer_diagnostics::render::{Gap, Next, Stream};
use oer_diagnostics::{MAX_PAYLOAD, Record};

/// `bytes` of one boot's console with its diagnostic lines rendered. A
/// capture's first byte starts a boot (`anchored`), so its first diagnostic
/// must be number zero; a slice taken later joins at its first diagnostic.
/// A last line without its line end is left raw: more of it may follow.
pub(crate) fn render(bytes: &[u8], anchored: bool) -> Vec<u8> {
    let mut stream = if anchored {
        Stream::new()
    } else {
        Stream::unanchored()
    };
    let mut out = Vec::with_capacity(bytes.len());
    for raw in bytes.split_inclusive(|&byte| byte == b'\n') {
        let Some(body) = raw.strip_suffix(b"\n") else {
            out.extend_from_slice(raw);
            continue;
        };
        let end = if body.ends_with(b"\r") {
            &raw[raw.len() - 2..]
        } else {
            b"\n"
        };
        let line = body.strip_suffix(b"\r").unwrap_or(body);
        if oer_lab_board::console::is_reset_line(&String::from_utf8_lossy(line))
            || crate::protocol::console_shows_boot(line)
        {
            stream.restart();
        }
        if !line.starts_with(b"OER-DIAG:") {
            out.extend_from_slice(raw);
            continue;
        }
        out.extend_from_slice(&diagnostic(&mut stream, line, end));
    }
    out
}

/// The text for a diagnostic line: the error line for a gap before it (with
/// its own line end), then the record's text.
fn diagnostic(stream: &mut Stream, line: &[u8], end: &[u8]) -> Vec<u8> {
    let mut scratch = [0; MAX_PAYLOAD + 7];
    let error = |kind: String| format!("OER-DIAG-ERROR {kind}: {}", String::from_utf8_lossy(line));
    let mut out = Vec::new();
    let text = match stream.line(line, &mut scratch) {
        Ok(Some(Next { gap, record })) => {
            if let Some(Gap { expected, received }) = gap {
                out.extend_from_slice(
                    format!(
                        "OER-DIAG-ERROR loss or reorder, expected sequence {expected}, received {received}"
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(end);
            }
            match record {
                Record::Note(note) => {
                    let mut text = String::new();
                    match oer_diagnostics::render::note(&note, &mut text) {
                        Ok(()) => text,
                        Err(_) => error(format!("note does not render ({:?})", note.topic)),
                    }
                }
                other => error(format!("unexpected record {other:?}")),
            }
        }
        Ok(None) => error("not a diagnostic".to_owned()),
        Err(failure) => error(format!("decode {failure:?}")),
    };
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(end);
    out
}

#[cfg(test)]
mod tests {
    use super::render;
    use oer_diagnostics::{Console, DiagPath, Note};

    fn frame(sequence: u32, note: &Note<'_>) -> Vec<u8> {
        let mut bytes = Vec::new();
        Console::starting_at(
            |chunk: &[u8]| {
                bytes.extend_from_slice(chunk);
                Ok::<_, ()>(())
            },
            sequence,
        )
        .note(note)
        .unwrap();
        bytes
    }

    fn text(bytes: &[u8], anchored: bool) -> String {
        String::from_utf8(render(bytes, anchored)).unwrap()
    }

    #[test]
    fn a_note_prints_as_its_line_among_other_console_text() {
        let mut console = b"ESP-ROM:esp32s31\r\nplain\r\n".to_vec();
        console.extend(frame(0, &Note::fatal_panic(1, 0, 0xdead_beef, 0x2a, 0)));
        console.extend(frame(1, &Note::rearm(3, 20, 20_010)));
        assert_eq!(
            text(&console, true),
            "ESP-ROM:esp32s31\r\nplain\r\n\r\n\
             OPEN_RADIO_HIL runtime=PANIC boot=previous hart=1 stage=0 mcause=deadbeef mepc=0000002a mtval=00000000\r\n\
             \r\n\
             OPEN_RADIO_HIL rearm step=3 delay_ms=20 elapsed_us=20010\r\n"
        );
    }

    #[test]
    fn a_lost_frame_is_shown_and_the_next_still_prints() {
        let mut console = frame(0, &Note::rearm(1, 20, 20_000));
        console.extend(frame(2, &Note::fatal_panic(1, 0, 0xdead_beef, 0x2a, 0)));
        let shown = text(&console, true);
        let gap = shown
            .find("OER-DIAG-ERROR loss or reorder, expected sequence 1, received 2")
            .expect("the gap is reported");
        let panic = shown
            .find("OPEN_RADIO_HIL runtime=PANIC boot=previous")
            .expect("the record after the gap prints");
        assert!(gap < panic);
    }

    #[test]
    fn a_boot_banner_without_its_reset_line_restarts_the_numbering() {
        for banner in [&b"ESP-ROM:esp32s31\r\n"[..], b"I (30) boot: ESP-IDF\r\n"] {
            let mut console = frame(0, &Note::rearm(1, 20, 20_000));
            console.extend(banner);
            console.extend(frame(0, &Note::rearm(1, 20, 20_000)));
            assert!(!text(&console, true).contains("OER-DIAG-ERROR"));
        }
    }

    #[test]
    fn a_damaged_or_unknown_line_is_an_error_line_with_the_raw_text() {
        let mut damaged = frame(0, &Note::rearm(1, 20, 20_000));
        let flip = damaged.len() - 5;
        damaged[flip] = if damaged[flip] == b'0' { b'1' } else { b'0' };
        let shown = text(&damaged, true);
        assert!(shown.contains("OER-DIAG-ERROR decode Checksum: OER-DIAG:01"));
        assert!(text(b"OER-DIAG:02abcd\r\n", true).contains("OER-DIAG-ERROR decode Version"));
    }

    #[test]
    fn an_unfinished_last_line_stays_raw_and_a_late_slice_joins_in_the_middle() {
        let frames = frame(7, &Note::rearm(2, 20, 20_000));
        let cut = &frames[..frames.len() - 4];
        assert_eq!(render(cut, true), cut);
        assert!(text(&frames, false).contains("OPEN_RADIO_HIL rearm step=2"));
        assert!(text(&frames, true).contains("expected sequence 0, received 7"));
    }

    #[test]
    fn a_reset_line_restarts_the_numbering() {
        let mut console = frame(0, &Note::rearm(1, 20, 20_000));
        console.extend(b"rst:0xc (SW_CPU_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\r\n");
        console.extend(frame(0, &Note::rearm(1, 20, 20_000)));
        assert!(!text(&console, true).contains("OER-DIAG-ERROR"));
    }

    #[test]
    fn a_failure_path_prints_nested_with_its_numbers() {
        let mut path = DiagPath::new();
        for name in ["Err", "Failed", "Run", "Port", "HardwareEdgeTimedOut"] {
            path.name(name);
        }
        path.word(7);
        assert_eq!(
            text(&frame(0, &Note::hold_fail(3, &path)), true).trim(),
            "OPEN_RADIO_HIL hold=FAIL tick=3 result=Err(Failed(Run(Port(HardwareEdgeTimedOut)))) numbers=7"
        );
    }
}

//! The device's diagnostic lines (`OER-DIAG:01…`) between its frames, as the
//! text the host prints for them. A line that is lost, garbled or unknown
//! becomes an `OER-DIAG-ERROR` text with the raw line, never dropped.

use oer_diagnostics::MAX_PAYLOAD;
use oer_diagnostics::render::{Gap, Next, Stream};

const MARKER: &[u8] = b"OER-DIAG:";
/// The longest diagnostic line kept while it is incomplete.
const MAX_LINE: usize = 2 * (MAX_PAYLOAD + 64);

/// Diagnostic lines of one serial stream.
pub(crate) struct Notes {
    stream: Stream,
    /// The diagnostic line being read, from its marker.
    line: Vec<u8>,
    /// How far into the marker the last bytes matched.
    matched: usize,
}

impl Notes {
    pub(crate) fn new() -> Self {
        Self {
            stream: Stream::unanchored(),
            line: Vec::new(),
            matched: 0,
        }
    }

    /// Feeds serial bytes; each complete diagnostic line's text goes to `out`.
    pub(crate) fn feed(&mut self, bytes: &[u8], out: &mut Vec<String>) {
        for &byte in bytes {
            if !self.line.is_empty() {
                if matches!(byte, b'\r' | b'\n' | 0) || self.line.len() >= MAX_LINE {
                    let line = std::mem::take(&mut self.line);
                    out.extend(self.render(&line));
                } else {
                    self.line.push(byte);
                }
                continue;
            }
            self.matched = match MARKER.get(self.matched) {
                Some(&expected) if expected == byte => self.matched.saturating_add(1),
                _ if MARKER.first() == Some(&byte) => 1,
                _ => 0,
            };
            if self.matched == MARKER.len() {
                self.line.extend_from_slice(MARKER);
                self.matched = 0;
            }
        }
    }

    /// The device restarted: its diagnostic numbering starts again.
    pub(crate) fn restart(&mut self) {
        self.stream.restart();
    }

    fn render(&mut self, line: &[u8]) -> Vec<String> {
        let mut scratch = [0; MAX_PAYLOAD + 7];
        let error =
            |kind: String| format!("OER-DIAG-ERROR {kind}: {}", String::from_utf8_lossy(line));
        match self.stream.line(line, &mut scratch) {
            Ok(Some(Next { gap, record })) => {
                let mut texts = Vec::new();
                if let Some(Gap { expected, received }) = gap {
                    texts.push(format!(
                        "OER-DIAG-ERROR loss or reorder, expected sequence {expected}, received {received}"
                    ));
                }
                texts.push(match record {
                    oer_diagnostics::Record::Note(note) => {
                        let mut text = String::new();
                        match oer_diagnostics::render::note(&note, &mut text) {
                            Ok(()) => text,
                            Err(_) => error(format!("note does not render ({:?})", note.topic)),
                        }
                    }
                    other => error(format!("unexpected record {other:?}")),
                });
                texts
            }
            Ok(None) => vec![error("not a diagnostic".to_owned())],
            Err(failure) => vec![error(format!("decode {failure:?}"))],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The line of diagnostic number `sequence` carrying `note`.
    fn line(sequence: u32, note: &oer_diagnostics::Note<'_>) -> Vec<u8> {
        let bytes = std::cell::RefCell::new(Vec::new());
        let mut console = oer_diagnostics::Console::new(|chunk: &[u8]| {
            bytes.borrow_mut().extend_from_slice(chunk);
            Ok::<_, ()>(())
        });
        for _ in 0..sequence {
            console
                .note(&oer_diagnostics::Note::placement_pass())
                .unwrap();
        }
        let skip = bytes.borrow().len();
        console.note(note).unwrap();
        bytes.into_inner().split_off(skip)
    }

    #[test]
    fn diagnostic_lines_between_binary_bytes_render_in_order_and_report_a_gap() {
        let mut stream = vec![0x00, 0x13, 0x0a, b'O', 0x00];
        stream.extend(line(0, &oer_diagnostics::Note::placement_pass()));
        stream.extend([0x05, b'x', 0x00]);
        stream.extend(line(2, &oer_diagnostics::Note::placement_pass()));
        let mut notes = Notes::new();
        let mut out = Vec::new();
        // Split mid-marker and mid-line to follow serial chunking.
        for chunk in stream.chunks(7) {
            notes.feed(chunk, &mut out);
        }
        assert_eq!(out.len(), 3, "{out:?}");
        assert!(!out[0].starts_with("OER-DIAG-ERROR"), "{out:?}");
        assert!(
            out[1].starts_with("OER-DIAG-ERROR loss or reorder"),
            "{out:?}"
        );
        assert!(!out[2].starts_with("OER-DIAG-ERROR"), "{out:?}");
    }

    #[test]
    fn a_garbled_line_is_shown_raw_never_dropped() {
        let mut notes = Notes::new();
        let mut out = Vec::new();
        notes.feed(b"\r\nOER-DIAG:01 garbage\r\n", &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].starts_with("OER-DIAG-ERROR"));
        assert!(out[0].contains("garbage"));
    }
}

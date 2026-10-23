//! The human view of the boot-only typed console. HIL agents keep their
//! existing base/boot envelopes; their protocol and captures are unchanged.

use std::borrow::Cow;

use oer_diagnostics::render::{Gap, Next};
use oer_diagnostics::{BootSmokePassed, MAX_PAYLOAD, Record};
use oer_espressif_esp_hal_radio::diagnostics::RadioRoot;
use oer_trace::Event;

#[derive(Default)]
pub(crate) struct Decoder {
    stream: oer_diagnostics::render::Stream,
}

impl Decoder {
    pub(crate) fn line<'a>(&mut self, line: &'a [u8]) -> oer_image_build::Result<Cow<'a, str>> {
        if oer_lab_board::console::is_reset_line(&String::from_utf8_lossy(line)) {
            self.stream.restart();
        }
        let mut scratch = [0; MAX_PAYLOAD + 7];
        let decoded = self
            .stream
            .line(line, &mut scratch)
            .map_err(|error| format!("boot diagnostic decode error: {error:?}"))?;
        let Some(Next { gap, record }) = decoded else {
            return Ok(String::from_utf8_lossy(line));
        };
        if let Some(Gap { expected, received }) = gap {
            return Err(format!(
                "boot diagnostic loss/order error: expected sequence {expected}, received {received}"
            )
            .into());
        }
        Ok(Cow::Owned(match record {
            Record::PreviousPanic(panic) => {
                let mut text = format!(
                    "OER-PANIC previous {}:{}:{} on hart {}",
                    panic.file, panic.line, panic.column, panic.hart
                );
                if let Some(pc) = panic.interrupted_pc {
                    text.push_str(&format!(" in an interrupt of {pc:#010x}"));
                }
                // Preserve the flag even in a record without a PC.
                else if panic.in_interrupt {
                    text.push_str(" in an interrupt");
                }
                match panic.message {
                    Some(message) => text.push_str(&format!(": {message}")),
                    None => text.push_str(" (formatted message not kept)"),
                }
                text
            }
            Record::Note(note) => {
                let mut text = String::new();
                oer_diagnostics::render::note(&note, &mut text)
                    .map_err(|_| format!("boot diagnostic note does not render: {note:?}"))?;
                text
            }
            Record::Event { kind, words } => {
                if kind == BootSmokePassed::KIND && BootSmokePassed::decode(words).is_some() {
                    return Ok(Cow::Borrowed("OER-RESULT: pass boot-smoke"));
                }
                let event = (kind == RadioRoot::KIND)
                    .then(|| RadioRoot::decode(words))
                    .flatten()
                    .ok_or_else(|| format!("unknown boot diagnostic event: {kind:?} {words:?}"))?;
                match event {
                    RadioRoot::Handoff => "OER radio-root handoff=ok".into(),
                    RadioRoot::RootTaken => {
                        "OER-RESULT: fail radio-root take_radio=RootTaken".into()
                    }
                    RadioRoot::ClockReleased(clock) => {
                        format!("OER radio-root clock={clock:?} acquired released")
                    }
                    RadioRoot::ClockRefused(clock) => {
                        format!("OER radio-root clock={clock:?} refused=PlatformClockError")
                    }
                    RadioRoot::Passed => "OER-RESULT: pass radio-root".into(),
                    RadioRoot::ClockFailed => "OER-RESULT: fail radio-root clock".into(),
                }
            }
        }))
    }
}

/// Replay the raw console lines without opening a board. Diagnostic lines
/// require their terminating newline; an interrupted last record is an error.
pub(crate) fn decode_file(path: &std::path::Path) -> oer_image_build::Result<()> {
    let mut reader = std::io::BufReader::new(std::fs::File::open(path)?);
    render_stream(&mut reader, &mut std::io::stdout().lock())?;
    Ok(())
}

fn render_stream(
    reader: &mut impl std::io::BufRead,
    output: &mut impl std::io::Write,
) -> oer_image_build::Result<()> {
    let mut decoder = Decoder::default();
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line)? != 0 {
        let complete = line.last() == Some(&b'\n');
        if complete {
            line.pop();
        }
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if !complete && line.starts_with(b"OER-DIAG:") {
            return Err("truncated boot diagnostic at end of console stream".into());
        }
        writeln!(output, "{}", decoder.line(&line)?)?;
        line.clear();
    }
    Ok(())
}

#[cfg(test)]
mod tests;

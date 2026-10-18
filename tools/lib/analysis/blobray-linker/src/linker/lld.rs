//! LLD ELF dialect, map and why-extract evidence.
use super::*;
use records::{hex, occurrence, trim};
pub(super) fn link(
    request: &LinkInvocation<'_>,
    sink: &mut dyn LinkOutputSink,
    control: &mut dyn RunControl,
) -> Result<()> {
    let (map_read, map_write) = pipe()?;
    let (extract_read, extract_write) = pipe()?;
    let mut command = command(request.executable);
    semantic_arguments(request, &mut command)?;
    command
        .args(["--threads=1", "--icf=none", "-o", "-"])
        .arg(format!("--Map=/proc/self/fd/{}", map_write.as_raw_fd()))
        .arg(format!(
            "--why-extract=/proc/self/fd/{}",
            extract_write.as_raw_fd()
        ));
    inherit(
        &mut command,
        vec![map_write.as_raw_fd(), extract_write.as_raw_fd()],
        None,
    );
    let mut child = ChildOwner(command.spawn().map_err(io)?);
    drop(map_write);
    drop(extract_write);
    let stdout = child.0.stdout.take().unwrap();
    let stderr = child.0.stderr.take().unwrap();
    let mut observed = records::Observed::new(sink, request, Parser);
    drain(
        &mut child,
        &[
            (stdout.as_raw_fd(), LinkOutput::Elf),
            (map_read.as_raw_fd(), LinkOutput::Map),
            (extract_read.as_raw_fd(), LinkOutput::Extraction),
            (stderr.as_raw_fd(), LinkOutput::Stderr),
        ],
        &mut observed,
        control,
    )?;
    observed.finish(control)
}
pub(super) struct Parser;
impl records::Parser for Parser {
    fn line(
        &mut self,
        channel: LinkOutput,
        line: &[u8],
        evidence: LinkEvidenceSpan,
        members: &[blobray_application::linking::LinkMember],
        sink: &mut dyn LinkOutputSink,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if channel == LinkOutput::Extraction {
            let line = line.strip_suffix(b"\n").unwrap_or(line);
            if line == b"reference\textracted\tsymbol" || line.is_empty() {
                return Ok(());
            }
            let fields: Vec<_> = line.splitn(3, |b| *b == b'\t').collect();
            if fields.len() != 3 {
                return Err(blocked("malformed LLD extraction record"));
            }
            let referring = if fields[0] == b"<internal>" {
                None
            } else {
                Some(occurrence(members, fields[0])?)
            };
            return sink.observe(
                LinkObservation::ArchiveExtraction {
                    object: occurrence(members, fields[1])?,
                    cause: Some(fields[2].into()),
                    referring,
                    evidence,
                },
                control,
            );
        }
        let mut cursor = 0;
        let mut fields = Vec::new();
        for _ in 0..4 {
            while cursor < line.len() && line[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            let start = cursor;
            while cursor < line.len() && !line[cursor].is_ascii_whitespace() {
                cursor += 1;
            }
            fields.push(&line[start..cursor]);
        }
        let padding = cursor;
        while cursor < line.len() && line[cursor] == b' ' {
            cursor += 1;
        }
        // LLD's input column is distinct from its output and symbol columns.
        if cursor - padding != 9 {
            return Ok(());
        }
        let tail = trim(&line[cursor..]);
        let Some(split) = tail.windows(2).position(|s| s == b":(") else {
            return Ok(());
        };
        let alias = &tail[..split];
        if alias == b"<internal>" {
            return Ok(());
        }
        let Some(section) = tail[split + 2..].strip_suffix(b")") else {
            return Err(blocked("malformed LLD section record"));
        };
        sink.observe(
            LinkObservation::SectionPlacement {
                object: occurrence(members, alias)?,
                section: section.into(),
                address: hex(fields[0])?,
                size: hex(fields[2])?,
                evidence,
            },
            control,
        )
    }
}

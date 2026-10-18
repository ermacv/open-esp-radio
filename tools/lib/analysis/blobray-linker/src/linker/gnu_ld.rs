//! GNU BFD ld ELF dialect. Seekable output has a pre-admitted hard size ceiling.
use super::*;
use records::{hex, occurrence, trim};
pub(super) fn link(
    request: &LinkInvocation<'_>,
    sink: &mut dyn LinkOutputSink,
    control: &mut dyn RunControl,
) -> Result<()> {
    let output = request.workspace.elf_output()?;
    let mut command = command(request.executable);
    semantic_arguments(request, &mut command)?;
    command.args(["-o", "image.elf", "-M"]);
    inherit(&mut command, Vec::new(), Some(output.maximum()));
    // Declared after output: cancellation/drop reaps the writer before its lease.
    let mut child = ChildOwner(command.spawn().map_err(io)?);
    let stdout = child.0.stdout.take().unwrap();
    let stderr = child.0.stderr.take().unwrap();
    let mut observed = records::Observed::new(sink, request, Parser::default());
    let result = drain(
        &mut child,
        &[
            (stdout.as_raw_fd(), LinkOutput::Map),
            (stderr.as_raw_fd(), LinkOutput::Stderr),
        ],
        &mut observed,
        control,
    );
    let limit_hit = child
        .0
        .try_wait()
        .map_err(io)?
        .is_some_and(|s| s.signal() == Some(libc::SIGXFSZ));
    drop(child);
    if limit_hit {
        return Err(Error::new(
            ErrorCode::ResourceLimited,
            "GNU linker exceeded its admitted ELF extent",
        ));
    }
    result?;
    observed.finish(control)?;
    observed.elf(output.finish()?)
}
#[derive(Default)]
pub(super) struct Parser {
    map: bool,
    extraction: bool,
    pending_section: Option<(Vec<u8>, LinkEvidenceSpan)>,
    pending_member: Option<(Vec<u8>, LinkEvidenceSpan)>,
}
impl records::Parser for Parser {
    fn finish(&self) -> Result<()> {
        if !self.map || self.pending_section.is_some() || self.pending_member.is_some() {
            return Err(blocked("incomplete GNU map evidence"));
        }
        Ok(())
    }

    fn line(
        &mut self,
        _: LinkOutput,
        line: &[u8],
        mut evidence: LinkEvidenceSpan,
        members: &[blobray_application::linking::LinkMember],
        sink: &mut dyn LinkOutputSink,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let text = trim(line);
        if text == b"Archive member included to satisfy reference by file (symbol)" {
            self.extraction = true;
            return Ok(());
        }
        if text == b"Linker script and memory map" {
            self.map = true;
            self.extraction = false;
            return Ok(());
        }
        if self.extraction {
            if text.is_empty() {
                return Ok(());
            }
            let mut parts = text.splitn(2, |b| b.is_ascii_whitespace());
            let first = parts.next().unwrap();
            let (member, rest) = if let Some((member, span)) = self.pending_member.take() {
                evidence.length = evidence.offset + evidence.length - span.offset;
                evidence.offset = span.offset;
                (member, text)
            } else if members.iter().any(|m| m.alias.as_bytes() == first) {
                let rest = trim(parts.next().unwrap_or_default());
                if rest.is_empty() {
                    self.pending_member = Some((first.into(), evidence));
                    return Ok(());
                }
                (first.to_vec(), rest)
            } else {
                if !matches!(
                    text,
                    b"Merging object attributes"
                        | b"Merging program properties"
                        | b"Allocating common symbols"
                        | b"Discarded input sections"
                        | b"Memory Configuration"
                ) {
                    return Err(blocked("unrecognized GNU archive extraction record"));
                }
                self.extraction = false;
                return Ok(());
            };
            let split = rest
                .windows(2)
                .position(|s| s == b" (")
                .ok_or_else(|| blocked("malformed GNU archive extraction"))?;
            let symbol = rest[split + 2..]
                .strip_suffix(b")")
                .ok_or_else(|| blocked("malformed GNU extraction cause"))?;
            let referring = if rest[..split] == *b"(--undefined)" {
                None
            } else {
                Some(occurrence(members, trim(&rest[..split]))?)
            };
            return sink.observe(
                LinkObservation::ArchiveExtraction {
                    object: occurrence(members, &member)?,
                    cause: Some(symbol.into()),
                    referring,
                    evidence,
                },
                control,
            );
        }
        if !self.map {
            return Ok(());
        }
        let (section, rest) = if let Some((section, span)) = self.pending_section.take() {
            if !text.starts_with(b"0x") {
                return Err(blocked("missing GNU section continuation"));
            }
            evidence.length = evidence.offset + evidence.length - span.offset;
            evidence.offset = span.offset;
            (section, text)
        } else {
            // GNU input section lines have exactly one leading space. Output
            // sections, symbols, wildcard rules and fills are different records.
            if !line.starts_with(b" ")
                || line.get(1).is_none_or(u8::is_ascii_whitespace)
                || text.starts_with(b"*fill*")
                || text.starts_with(b"*(")
                || text.starts_with(b"INPUT_SECTION_FLAGS")
            {
                return Ok(());
            }
            let Some(split) = text
                .windows(3)
                .position(|s| s[0].is_ascii_whitespace() && &s[1..] == b"0x")
            else {
                self.pending_section = Some((text.into(), evidence));
                return Ok(());
            };
            (trim(&text[..split]).to_vec(), trim(&text[split..]))
        };
        let fields: Vec<_> = rest
            .split(|b| b.is_ascii_whitespace())
            .filter(|s| !s.is_empty())
            .collect();
        if fields.len() != 3 {
            return Err(blocked("malformed GNU input section"));
        }
        if fields[2] == b"linker" {
            return Ok(());
        }
        sink.observe(
            LinkObservation::SectionPlacement {
                object: occurrence(members, fields[2])?,
                section,
                address: hex(fields[0])?,
                size: hex(fields[1])?,
                evidence,
            },
            control,
        )
    }
}

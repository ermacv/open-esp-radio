//! Bounded incremental raw-output framing. Dialects never retain complete maps.
use super::*;
pub(super) trait Parser {
    fn finish(&self) -> Result<()> {
        Ok(())
    }
    fn line(
        &mut self,
        channel: LinkOutput,
        line: &[u8],
        span: LinkEvidenceSpan,
        members: &[blobray_application::linking::LinkMember],
        sink: &mut dyn LinkOutputSink,
        control: &mut dyn RunControl,
    ) -> Result<()>;
}
pub(super) struct Observed<'a, 'r, P> {
    pub sink: &'a mut dyn LinkOutputSink,
    pub request: &'a LinkInvocation<'r>,
    pub parser: P,
    lines: [Vec<u8>; 2],
    offsets: [u64; 2],
}
impl<'a, 'r, P: Parser> Observed<'a, 'r, P> {
    pub fn new(
        sink: &'a mut dyn LinkOutputSink,
        request: &'a LinkInvocation<'r>,
        parser: P,
    ) -> Self {
        Self {
            sink,
            request,
            parser,
            lines: Default::default(),
            offsets: [0; 2],
        }
    }
    fn emit(&mut self, index: usize, control: &mut dyn RunControl) -> Result<()> {
        let length = self.lines[index].len() as u64;
        let (channel, source) = if index == 0 {
            (LinkOutput::Map, LinkEvidenceSource::Map)
        } else {
            (LinkOutput::Extraction, LinkEvidenceSource::Extraction)
        };
        self.parser.line(
            channel,
            &self.lines[index],
            LinkEvidenceSpan {
                source,
                offset: self.offsets[index],
                length,
            },
            &self.request.members,
            self.sink,
            control,
        )?;
        self.offsets[index] += length;
        self.lines[index].clear();
        Ok(())
    }
    pub fn finish(&mut self, control: &mut dyn RunControl) -> Result<()> {
        for index in 0..2 {
            if !self.lines[index].is_empty() {
                self.emit(index, control)?;
            }
        }
        self.parser.finish()
    }
}
impl<P: Parser> LinkOutputSink for Observed<'_, '_, P> {
    fn exited(&mut self, code: Option<i32>, signal: Option<i32>) {
        self.sink.exited(code, signal);
    }
    fn observe(&mut self, observation: LinkObservation, c: &mut dyn RunControl) -> Result<()> {
        self.sink.observe(observation, c)
    }
    fn elf(&mut self, bytes: Vec<u8>) -> Result<()> {
        self.sink.elf(bytes)
    }
    fn write(
        &mut self,
        channel: LinkOutput,
        bytes: &[u8],
        control: &mut dyn RunControl,
    ) -> Result<()> {
        self.sink.write(channel, bytes, control)?;
        let index = match channel {
            LinkOutput::Map => 0,
            LinkOutput::Extraction => 1,
            _ => return Ok(()),
        };
        for part in bytes.split_inclusive(|b| *b == b'\n') {
            control.checkpoint(1)?;
            if self.lines[index].len() + part.len() > 65536 {
                return Err(Error::new(
                    ErrorCode::ResourceLimited,
                    "linker record exceeds 64 KiB",
                ));
            }
            self.lines[index].extend_from_slice(part);
            if part.ends_with(b"\n") {
                self.emit(index, control)?;
            }
        }
        Ok(())
    }
}
pub(super) fn occurrence(
    members: &[blobray_application::linking::LinkMember],
    alias: &[u8],
) -> Result<ObjectId> {
    members
        .iter()
        .find(|m| m.alias.as_bytes() == alias)
        .map(|m| m.object.clone())
        .ok_or_else(|| blocked("unknown object in linker evidence"))
}
pub(super) fn hex(bytes: &[u8]) -> Result<u64> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok())
        .ok_or_else(|| blocked("invalid hexadecimal linker evidence"))
}
pub(super) fn trim(bytes: &[u8]) -> &[u8] {
    bytes.trim_ascii()
}

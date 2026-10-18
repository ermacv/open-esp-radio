//! Narrow streaming ports. Computing code receives bytes, never origin paths.
use crate::*;

pub struct SourceRange<'a> {
    source: &'a dyn ByteSource,
    offset: u64,
    length: u64,
}
impl<'a> SourceRange<'a> {
    pub fn new(source: &'a dyn ByteSource, offset: u64, length: u64) -> Result<Self> {
        if offset
            .checked_add(length)
            .is_none_or(|end| end > source.len())
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "payload range outside captured content",
            ));
        }
        Ok(Self {
            source,
            offset,
            length,
        })
    }
}
impl ByteSource for SourceRange<'_> {
    fn len(&self) -> u64 {
        self.length
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8], control: &mut dyn RunControl) -> Result<()> {
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(Error::new(
                ErrorCode::Integrity,
                "read outside payload range",
            ));
        }
        self.source.read_at(self.offset + offset, bytes, control)
    }
}

pub fn read_scratch<'a>(
    source: &dyn ByteSource,
    memory: &'a WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<ScratchBytes<'a>> {
    let length = usize::try_from(source.len()).map_err(|_| {
        Error::new(
            ErrorCode::ResourceLimited,
            "payload length exceeds host address space",
        )
    })?;
    let mut bytes = memory.bytes(length, control.position())?;
    source.read_at(0, &mut bytes, control)?;
    Ok(bytes)
}

/// A synchronous consumer must finish using each borrowed record before return.
/// Retaining a clone requires the consumer's own admitted memory capacity.
pub trait ElfSink {
    fn section(&mut self, record: &SectionRecord, control: &mut dyn RunControl) -> Result<()>;
    fn symbol(&mut self, record: &SymbolRecord, control: &mut dyn RunControl) -> Result<()>;
    fn relocation(&mut self, record: &RelocationRecord, control: &mut dyn RunControl)
    -> Result<()>;
    fn diagnostic(&mut self, record: &Diagnostic, control: &mut dyn RunControl) -> Result<()>;
}

impl ElfSink for () {
    fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
}

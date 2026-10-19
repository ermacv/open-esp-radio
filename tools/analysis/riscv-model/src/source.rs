//! Narrow byte-source port. Computing code receives bytes, never origin paths.
use crate::*;

/// Stable captured content with positional, bounded reads. Implementations must
/// reject overflow/short reads; callers own the destination's memory reservation.
pub trait ByteSource {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8], control: &mut dyn RunControl) -> Result<()>;
}

pub fn hash_source(source: &dyn ByteSource, control: &mut dyn RunControl) -> Result<ArtifactId> {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    let mut buffer = [0; WORK_BLOCK];
    let mut offset = 0;
    while offset < source.len() {
        let size = (source.len() - offset).min(WORK_BLOCK as u64) as usize;
        source.read_at(offset, &mut buffer[..size], control)?;
        control.bytes(size)?;
        hash.update(&buffer[..size]);
        offset += size as u64;
    }
    format!("{:x}", hash.finalize()).parse()
}

impl ByteSource for &[u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }
    fn read_at(
        &self,
        offset: u64,
        destination: &mut [u8],
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let end = offset
            .checked_add(destination.len() as u64)
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "borrowed range overflow"))?;
        let start = usize::try_from(offset)
            .map_err(|_| Error::new(ErrorCode::Integrity, "borrowed offset overflow"))?;
        let end = usize::try_from(end)
            .map_err(|_| Error::new(ErrorCode::Integrity, "borrowed extent overflow"))?;
        let source = self
            .get(start..end)
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "borrowed range outside content"))?;
        for (source, destination) in source
            .chunks(WORK_BLOCK)
            .zip(destination.chunks_mut(WORK_BLOCK))
        {
            control.bytes(source.len())?;
            destination.copy_from_slice(source);
        }
        Ok(())
    }
}

//! A producer's data, written once into the port's slot when its frame is
//! built there: each octet goes into the UDP checksum as it is written, so
//! no pass reads the data again.

use oer_protocols_network_wire::checksum::Checksum;

/// The data region of one frame in the port's slot, exactly the length its
/// work was declared with, which its producer writes front to back. The
/// frame goes only once every octet of it is written.
#[derive(Debug)]
pub struct Payload<'a> {
    region: &'a mut [u8],
    written: usize,
    sum: Checksum,
}

/// What a write asked for does not fit behind what is written: nothing of
/// it was written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PayloadFull;

impl<'a> Payload<'a> {
    /// `region`, empty, whose sum continues `sum`.
    pub(crate) const fn new(region: &'a mut [u8], sum: Checksum) -> Self {
        Self {
            region,
            written: 0,
            sum,
        }
    }

    /// The octets still to write.
    #[must_use]
    pub const fn remaining(&self) -> usize {
        self.region.len().saturating_sub(self.written)
    }

    /// Write `octets` next: one read of them, for their copy and their sum.
    ///
    /// # Errors
    /// [`PayloadFull`]: they do not fit, and nothing was written.
    pub fn put(&mut self, octets: &[u8]) -> Result<(), PayloadFull> {
        let rest = self.region.get_mut(self.written..).ok_or(PayloadFull)?;
        self.sum = self.sum.copied(octets, rest).map_err(|_| PayloadFull)?;
        self.written = self.written.saturating_add(octets.len());
        Ok(())
    }

    /// Write `count` octets of `byte` next. Their sum takes no pass over
    /// them ([`Checksum::repeated`]).
    ///
    /// # Errors
    /// [`PayloadFull`]: they do not fit, and nothing was written.
    pub fn repeat(&mut self, byte: u8, count: usize) -> Result<(), PayloadFull> {
        let end = self.written.checked_add(count).ok_or(PayloadFull)?;
        let region = self.region.get_mut(self.written..end).ok_or(PayloadFull)?;
        region.fill(byte);
        self.sum = self.sum.repeated(byte, count);
        self.written = end;
        Ok(())
    }

    /// The sum of the data and what it continues, once every octet is
    /// written; `None` while some are not.
    pub(crate) fn finish(self) -> Option<Checksum> {
        (self.written == self.region.len()).then_some(self.sum)
    }
}

#[cfg(test)]
mod tests;

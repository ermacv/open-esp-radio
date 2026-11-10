//! A producer's data, written once into the port's slot when its frame is
//! built there: each octet goes into the UDP checksum as it is written, so
//! no pass reads the data again.

use oer_protocols_network_port::Held;
use oer_protocols_network_wire::checksum::Checksum;

/// The data region of one frame in the port's slot, exactly the length its
/// work was declared with, which its producer writes front to back. The
/// frame goes only once every octet of it is written.
#[derive(Debug)]
pub struct Payload<'a> {
    region: &'a mut [u8],
    written: usize,
    sum: Checksum,
    /// The run of one octet the region holds from its buffer's last frame,
    /// in the region's places.
    kept: Option<Held>,
    /// The run the last [`Self::repeat`] left, which the region holds once
    /// written.
    left: Option<Held>,
}

/// What a write asked for does not fit behind what is written: nothing of
/// it was written.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PayloadFull;

impl<'a> Payload<'a> {
    /// `region`, empty, whose sum continues `sum`; it holds `kept` from
    /// its buffer's last frame, as far as the port knows.
    pub(crate) const fn new(region: &'a mut [u8], sum: Checksum, kept: Option<Held>) -> Self {
        Self {
            region,
            written: 0,
            sum,
            kept,
            left: None,
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
    /// them ([`Checksum::repeated`]), and where the buffer holds them from
    /// its last frame already, they are not written again: the frame goes
    /// with every octet as written, and its buffer holds the run for the
    /// next.
    ///
    /// # Errors
    /// [`PayloadFull`]: they do not fit, and nothing was written.
    pub fn repeat(&mut self, byte: u8, count: usize) -> Result<(), PayloadFull> {
        let end = self.written.checked_add(count).ok_or(PayloadFull)?;
        let region = self.region.get_mut(self.written..end).ok_or(PayloadFull)?;
        let held = self
            .kept
            .is_some_and(|kept| kept.covers(self.written, count, byte));
        if !held {
            region.fill(byte);
        }
        self.sum = self.sum.repeated(byte, count);
        self.left = Some(Held {
            at: self.written,
            length: count,
            octet: byte,
        });
        self.written = end;
        Ok(())
    }

    /// The sum of the data and what it continues, and the run the region
    /// holds, once every octet is written; `None` while some are not. Only
    /// a run to the data's end is held: a station that moves a frame's
    /// payload down to a shorter header in place moves octets of such a run
    /// onto octets of it, and leaves the run's own octets behind it.
    pub(crate) fn finish(self) -> Option<(Checksum, Option<Held>)> {
        let end = self.region.len();
        let left = self
            .left
            .filter(|run| run.at.checked_add(run.length) == Some(end));
        (self.written == end).then_some((self.sum, left))
    }
}

#[cfg(test)]
mod tests;

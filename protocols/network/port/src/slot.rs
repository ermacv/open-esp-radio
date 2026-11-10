//! The room one frame is written into, with what its port knows the room
//! holds already: a run of one octet left by the frame that last wrote it,
//! which a writer may keep instead of writing it again.

/// A run of one octet a buffer holds, `length` octets from `at`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Held {
    pub at: usize,
    pub length: usize,
    pub octet: u8,
}

impl Held {
    /// Whether it holds `octet` in every place of `at..at + length`.
    #[must_use]
    pub const fn covers(&self, at: usize, length: usize, octet: u8) -> bool {
        let (Some(end), Some(held_end)) =
            (at.checked_add(length), self.at.checked_add(self.length))
        else {
            return false;
        };
        self.octet == octet && self.at <= at && end <= held_end
    }
}

/// One frame's room in a port's buffer, and the run the port knows it holds.
/// Opening the room hands that run to the writer and forgets it: whatever
/// the writer writes, the room holds no run the writer does not report
/// ([`Self::hold`]). A port that knows nothing of its buffers lends every
/// room with none.
#[derive(Debug)]
pub struct Slot<'s> {
    bytes: &'s mut [u8],
    held: Option<Held>,
}

impl<'s> Slot<'s> {
    /// `bytes`, which hold `held` as far as the port knows; a run past their
    /// end is none.
    pub fn new(bytes: &'s mut [u8], held: Option<Held>) -> Self {
        let length = bytes.len();
        let held = held.filter(|run| {
            run.at
                .checked_add(run.length)
                .is_some_and(|end| end <= length)
        });
        Self { bytes, held }
    }

    /// The room's octets, and the run they held: from here the room holds
    /// none, until the writer reports one.
    pub fn open(&mut self) -> (&mut [u8], Option<Held>) {
        (&mut *self.bytes, self.held.take())
    }

    /// The writer wrote `held` and left it: the room holds that run once the
    /// frame is written. A run past the room's end is none.
    pub fn hold(&mut self, held: Option<Held>) {
        let length = self.bytes.len();
        self.held = held.filter(|run| {
            run.at
                .checked_add(run.length)
                .is_some_and(|end| end <= length)
        });
    }

    /// The run the room holds now, which its port keeps for the buffer's
    /// next frame.
    #[must_use]
    pub const fn held(&self) -> Option<Held> {
        self.held
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    reason = "tests assert on what a room holds"
)]
mod tests;

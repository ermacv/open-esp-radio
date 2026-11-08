//! The ADDBA Requests the station's receive agreements took: the last one
//! of each TID and the requests by outcome, since boot.
//!
//! The station service, on the boot hart, is the counter's one writer. A
//! reader on any hart takes it whole, under the counter's sequence number.
//! Counts wrap at 2^32.

use core::sync::atomic::{AtomicU32, Ordering, fence};

use super::{Concurrent, bump};

/// The TIDs an ADDBA Request can name: its four-bit field.
pub const TIDS: usize = 16;

/// What became of one request, as the recipient names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// A new agreement started with `window`.
    Accepted { window: u16 },
    /// The running agreement's own request again, answered with its
    /// `window`.
    Repeated { window: u16 },
    /// Refused with this IEEE 802.11 status code.
    Refused { status: u16 },
    /// No reply place or bank remained: it went unanswered.
    Unanswered,
}

/// One request: the Buffer Size the access point asked for as received
/// (zero asks the station to choose) and what became of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub requested: u16,
    pub outcome: Outcome,
}

/// One reading: the last request of each TID, and the requests by outcome.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Addba {
    pub last: [Option<Request>; TIDS],
    pub accepted: u32,
    pub repeated: u32,
    pub refused: u32,
    pub unanswered: u32,
}

/// The counts by outcome, in this order.
const ACCEPTED: usize = 0;
const REPEATED: usize = 1;
const REFUSED: usize = 2;
const UNANSWERED: usize = 3;
const OUTCOMES: usize = 4;

/// A TID's two words: the first is zero without a request, otherwise
/// its outcome's code (one to four, in the counts' order), a zero byte and
/// the requested Buffer Size; the second the outcome's window or status.
pub struct AddbaCounter {
    sequence: AtomicU32,
    last: [[AtomicU32; 2]; TIDS],
    counts: [AtomicU32; OUTCOMES],
}

impl AddbaCounter {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            last: [const { [const { AtomicU32::new(0) }; 2] }; TIDS],
            counts: [const { AtomicU32::new(0) }; OUTCOMES],
        }
    }

    /// Keep `request` as `tid`'s last and count its outcome; only the
    /// counter's writer calls it. A TID beyond [`TIDS`] is counted only.
    pub fn record(&self, tid: u8, request: Request) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        let (count, code, value) = match request.outcome {
            Outcome::Accepted { window } => (ACCEPTED, 1, window),
            Outcome::Repeated { window } => (REPEATED, 2, window),
            Outcome::Refused { status } => (REFUSED, 3, status),
            Outcome::Unanswered => (UNANSWERED, 4, 0),
        };
        if let Some([word, kept]) = self.last.get(usize::from(tid)) {
            let [low, high] = request.requested.to_le_bytes();
            word.store(u32::from_le_bytes([code, 0, low, high]), Ordering::Relaxed);
            kept.store(u32::from(value), Ordering::Relaxed);
        }
        if let Some(count) = self.counts.get(count) {
            bump(count, 1);
        }
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }

    /// The requests now.
    ///
    /// # Errors
    /// [`Concurrent`] while the writer writes.
    pub fn read(&self) -> Result<Addba, Concurrent> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return Err(Concurrent);
        }
        let words = self
            .last
            .each_ref()
            .map(|[word, kept]| (word.load(Ordering::Relaxed), kept.load(Ordering::Relaxed)));
        let [accepted, repeated, refused, unanswered] = self
            .counts
            .each_ref()
            .map(|count| count.load(Ordering::Relaxed));
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != sequence {
            return Err(Concurrent);
        }
        Ok(Addba {
            last: words.map(|(word, kept)| request(word, kept)),
            accepted,
            repeated,
            refused,
            unanswered,
        })
    }
}

impl Default for AddbaCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// The request a TID's words hold; `None` for the words of no request.
fn request(word: u32, kept: u32) -> Option<Request> {
    let [code, _, low, high] = word.to_le_bytes();
    let value = u16::try_from(kept).ok()?;
    let outcome = match code {
        1 => Outcome::Accepted { window: value },
        2 => Outcome::Repeated { window: value },
        3 => Outcome::Refused { status: value },
        4 => Outcome::Unanswered,
        _ => return None,
    };
    Some(Request {
        requested: u16::from_le_bytes([low, high]),
        outcome,
    })
}

#[cfg(test)]
mod tests;

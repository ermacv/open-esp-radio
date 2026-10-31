//! The count of one run's datagrams as they arrive, shared by the image and
//! the host. A run is bounded, so each sequence number below the tally's
//! capacity (64 per word) is remembered exactly: a duplicate is never
//! counted as new, however late it comes. A sequence number at or past the
//! capacity cannot be told from a duplicate: it is counted apart as
//! untracked, never as a distinct datagram.

/// What one arrival was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Arrival {
    /// The highest sequence number so far.
    New,
    /// Below the highest one, seen for the first time.
    Late,
    Duplicate,
    /// At or past the capacity.
    Untracked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Tally<const WORDS: usize> {
    seen: [u64; WORDS],
    /// Distinct datagrams and their payload bytes.
    pub datagrams: u64,
    pub bytes: u64,
    /// Distinct datagrams that came after a higher sequence number.
    pub reordered: u64,
    pub duplicates: u64,
    /// Arrivals at or past the capacity.
    pub untracked: u64,
    pub highest: Option<u64>,
}

impl<const WORDS: usize> Default for Tally<WORDS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const WORDS: usize> Tally<WORDS> {
    /// Sequence numbers the tally remembers: `0..CAPACITY`.
    pub const CAPACITY: u64 = 64 * WORDS as u64;

    /// An empty tally; constant, so a large one can start in a static.
    pub const fn new() -> Self {
        Self {
            seen: [0; WORDS],
            datagrams: 0,
            bytes: 0,
            reordered: 0,
            duplicates: 0,
            untracked: 0,
            highest: None,
        }
    }

    /// Empty the tally in place, for the next run, without moving it.
    pub fn clear(&mut self) {
        self.seen.fill(0);
        self.datagrams = 0;
        self.bytes = 0;
        self.reordered = 0;
        self.duplicates = 0;
        self.untracked = 0;
        self.highest = None;
    }

    /// Count the arrival of `sequence` with `bytes` of payload.
    pub fn count(&mut self, sequence: u64, bytes: u64) -> Arrival {
        let word = usize::try_from(sequence / 64)
            .ok()
            .and_then(|word| self.seen.get_mut(word));
        let Some(word) = word else {
            self.untracked = self.untracked.saturating_add(1);
            return Arrival::Untracked;
        };
        let bit = 1_u64 << (sequence % 64);
        if *word & bit != 0 {
            self.duplicates = self.duplicates.saturating_add(1);
            return Arrival::Duplicate;
        }
        *word |= bit;
        self.datagrams = self.datagrams.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes);
        match self.highest {
            Some(highest) if sequence < highest => {
                self.reordered = self.reordered.saturating_add(1);
                Arrival::Late
            }
            _ => {
                self.highest = Some(sequence);
                Arrival::New
            }
        }
    }

    /// Sequence numbers up to the highest one that never arrived; exact
    /// below the capacity.
    pub fn lost(&self) -> u64 {
        self.highest.map_or(0, |highest| {
            highest
                .saturating_add(1)
                .min(Self::CAPACITY)
                .saturating_sub(self.datagrams)
        })
    }
}

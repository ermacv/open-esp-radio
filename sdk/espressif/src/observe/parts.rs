//! Named parts of the image's work and their polls.
//!
//! A part is a task or a future inside one: the library's own
//! ([`LibraryPart`]), the station service's spans, and up to
//! [`IMAGE_PARTS`] the image names. Each part has one writer, the context
//! of the executor that polls it. A poll's time includes its preemption by
//! higher executors and interrupts, so nested parts and the parts of
//! different executors overlap. Counts and times wrap at 2^32; a reader
//! takes their differences, which hold for readings closer than some 71
//! minutes.
//!
//! The longest poll belongs to an epoch: a reader starts a new one, and a
//! part's longest counts only for the epoch it was written in. A poll that
//! ends while the reader starts an epoch may be kept for the previous one.

use core::sync::atomic::{AtomicU32, Ordering, fence};

use super::{Concurrent, bump};

/// The library's parts, in the order of their numbers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LibraryPart {
    /// The application's future, on the application executor.
    Application,
    /// The library's tasks on the application executor: the RTC
    /// watchdog's feed, the AES service and the station's runner.
    Watchdog,
    Aes,
    Station,
    /// The radio executor's tasks: the shared RF actor and the Wi-Fi driver.
    RadioActor,
    Driver,
    /// The second boot's IP service and future.
    Ip,
    Second,
    /// Inside the station: its service's spans.
    Step,
    Pending,
    Sources,
    Received,
    Deliver,
    Queue,
    Terminal,
    Transmit,
    Recycle,
}

impl LibraryPart {
    pub const ALL: [LibraryPart; LIBRARY_PARTS] = [
        LibraryPart::Application,
        LibraryPart::Watchdog,
        LibraryPart::Aes,
        LibraryPart::Station,
        LibraryPart::RadioActor,
        LibraryPart::Driver,
        LibraryPart::Ip,
        LibraryPart::Second,
        LibraryPart::Step,
        LibraryPart::Pending,
        LibraryPart::Sources,
        LibraryPart::Received,
        LibraryPart::Deliver,
        LibraryPart::Queue,
        LibraryPart::Terminal,
        LibraryPart::Transmit,
        LibraryPart::Recycle,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            LibraryPart::Application => "application",
            LibraryPart::Watchdog => "watchdog",
            LibraryPart::Aes => "aes",
            LibraryPart::Station => "station",
            LibraryPart::RadioActor => "radio-actor",
            LibraryPart::Driver => "driver",
            LibraryPart::Ip => "ip",
            LibraryPart::Second => "second",
            LibraryPart::Step => "step",
            LibraryPart::Pending => "pending",
            LibraryPart::Sources => "sources",
            LibraryPart::Received => "received",
            LibraryPart::Deliver => "deliver",
            LibraryPart::Queue => "queue",
            LibraryPart::Terminal => "terminal",
            LibraryPart::Transmit => "transmit",
            LibraryPart::Recycle => "recycle",
        }
    }
}

/// The library's parts.
pub const LIBRARY_PARTS: usize = 17;
/// The parts an image names, after the library's.
pub const IMAGE_PARTS: usize = 4;
/// Every part: the library's, then the image's.
pub const PARTS: usize = LIBRARY_PARTS + IMAGE_PARTS;

const _: () = {
    let mut index = 0;
    let mut rest: &[LibraryPart] = &LibraryPart::ALL;
    while let [part, tail @ ..] = rest {
        if *part as usize != index {
            panic!("a library part's position is its number");
        }
        index += 1;
        rest = tail;
    }
};

/// What one part accumulated: wrapping counts and microseconds, and the
/// longest poll of the epoch it was read in. `poll_us` includes the open
/// poll's time so far; `ended_us` holds only the ended polls, as `polls`
/// does.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PartSample {
    pub polls: u32,
    pub poll_us: u32,
    pub ended_us: u32,
    pub longest_us: Option<u32>,
}

/// One part's reading: its name, none for an image's part it did not name,
/// and its sample.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PartReading {
    pub name: Option<&'static str>,
    pub sample: PartSample,
}

/// The polls of one part.
pub struct PartCounter {
    sequence: AtomicU32,
    polls: AtomicU32,
    poll_us: AtomicU32,
    /// The start of the open poll, and whether one is open.
    start: AtomicU32,
    open: AtomicU32,
    longest: AtomicU32,
    /// The epoch `longest` belongs to.
    epoch: AtomicU32,
}

impl PartCounter {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            polls: AtomicU32::new(0),
            poll_us: AtomicU32::new(0),
            start: AtomicU32::new(0),
            open: AtomicU32::new(0),
            longest: AtomicU32::new(0),
            epoch: AtomicU32::new(0),
        }
    }

    /// Account a poll's start (`end` false) or end at `at_us` in `epoch`;
    /// only the part's executor calls it. An end without its start adds
    /// nothing.
    pub fn observe(&self, at_us: u32, end: bool, epoch: u32) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        if !end {
            self.start.store(at_us, Ordering::Relaxed);
            self.open.store(1, Ordering::Relaxed);
        } else if self.open.load(Ordering::Relaxed) != 0 {
            let span = at_us.wrapping_sub(self.start.load(Ordering::Relaxed));
            bump(&self.polls, 1);
            bump(&self.poll_us, span);
            let longest = if self.epoch.load(Ordering::Relaxed) == epoch {
                self.longest.load(Ordering::Relaxed).max(span)
            } else {
                span
            };
            self.longest.store(longest, Ordering::Relaxed);
            self.epoch.store(epoch, Ordering::Relaxed);
            self.open.store(0, Ordering::Relaxed);
        }
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }

    /// The totals at `now_us` (activity microseconds), the open poll's time
    /// included, and the longest poll of `epoch`.
    ///
    /// # Errors
    /// [`Concurrent`] while the part's executor writes.
    pub fn sample(&self, now_us: u32, epoch: u32) -> Result<PartSample, Concurrent> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return Err(Concurrent);
        }
        let polls = self.polls.load(Ordering::Relaxed);
        let poll_us = self.poll_us.load(Ordering::Relaxed);
        let start = self.start.load(Ordering::Relaxed);
        let open = self.open.load(Ordering::Relaxed) != 0;
        let longest = self.longest.load(Ordering::Relaxed);
        let longest_epoch = self.epoch.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != sequence {
            return Err(Concurrent);
        }
        let open_us = if open { now_us.wrapping_sub(start) } else { 0 };
        Ok(PartSample {
            polls,
            poll_us: poll_us.wrapping_add(open_us),
            ended_us: poll_us,
            longest_us: (polls != 0 && longest_epoch == epoch).then_some(longest),
        })
    }
}

impl Default for PartCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// The names of the image's parts, each given once.
pub struct ImageNames {
    names: [oer_memory::OnceValue<&'static str>; IMAGE_PARTS],
}

impl ImageNames {
    pub const fn new() -> Self {
        Self {
            names: [const { oer_memory::OnceValue::new() }; IMAGE_PARTS],
        }
    }

    /// Name the first free part `name`: its number among every part, none
    /// when the image named every one of its parts.
    pub fn claim(&self, name: &'static str) -> Option<usize> {
        let slot = self.names.iter().position(|slot| slot.set(name).is_ok())?;
        LIBRARY_PARTS.checked_add(slot)
    }

    /// The name of part `number`: a library part's, or the image's once it
    /// named it.
    pub fn name(&self, number: usize) -> Option<&'static str> {
        match number.checked_sub(LIBRARY_PARTS) {
            None => LibraryPart::ALL.get(number).map(|part| part.name()),
            Some(slot) => self.names.get(slot).and_then(oer_memory::OnceValue::get),
        }
    }
}

impl Default for ImageNames {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

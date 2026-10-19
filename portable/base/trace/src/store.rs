//! Retained storage and the trace that writes it.
//!
//! [`Retained`] only ever sees plain loads and stores, so it may live in
//! memory where the chip does not guarantee read-modify-write atomics. Each
//! entry and snapshot slot is guarded by a commit word: the writer zeroes it,
//! writes the body and then stores the final word, so a reader, or the next
//! boot, drops a slot whose body it did not see complete.

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering, fence};

use crate::{Event, Kind};

const MAGIC: u32 = u32::from_le_bytes(*b"OETR");
/// Layout of [`Retained`]; storage of another layout is not reported.
const LAYOUT: u32 = 1;
/// Sequence tags run through `1..=TAG_PERIOD`; zero marks an empty slot.
const TAG_PERIOD: u32 = 0xffff;
const RUNNING_WINDOW: u32 = u32::MAX;
const TRUNCATED: u32 = 1 << 31;

const HELD: u8 = 0;
const RUNNING: u8 = 1;
const FROZEN: u8 = 2;

fn tag(sequence: u32) -> u16 {
    (sequence % TAG_PERIOD) as u16 + 1
}

fn commit(kind: Kind, sequence: u32) -> u32 {
    (u32::from(kind.raw()) << 16) | u32::from(tag(sequence))
}

#[repr(C)]
struct Header {
    magic: AtomicU32,
    layout: AtomicU32,
    entries: AtomicU32,
    slots: AtomicU32,
    words: AtomicU32,
    /// A commit word naming the freeze trigger and the first entry after it.
    trigger: AtomicU32,
}

#[repr(C)]
pub(crate) struct Entry {
    commit: AtomicU32,
    t_us: AtomicU32,
    words: [AtomicU32; 2],
}

impl Entry {
    const fn empty() -> Self {
        Self {
            commit: AtomicU32::new(0),
            t_us: AtomicU32::new(0),
            words: [AtomicU32::new(0), AtomicU32::new(0)],
        }
    }

    fn write(&self, commit: u32, t_us: u32, words: [u32; 2]) {
        self.commit.store(0, Ordering::Relaxed);
        fence(Ordering::Release);
        self.t_us.store(t_us, Ordering::Relaxed);
        self.words[0].store(words[0], Ordering::Relaxed);
        self.words[1].store(words[1], Ordering::Relaxed);
        self.commit.store(commit, Ordering::Release);
    }

    fn read(&self) -> Option<Record> {
        let commit = self.commit.load(Ordering::Acquire);
        if commit == 0 {
            return None;
        }
        let t_us = self.t_us.load(Ordering::Relaxed);
        let words = [
            self.words[0].load(Ordering::Relaxed),
            self.words[1].load(Ordering::Relaxed),
        ];
        fence(Ordering::Acquire);
        (self.commit.load(Ordering::Relaxed) == commit).then_some(Record {
            tag: commit as u16,
            kind: (commit >> 16) as u16,
            t_us,
            words,
        })
    }

    #[cfg(test)]
    pub(crate) fn tear(&self) {
        self.commit.store(0, Ordering::Relaxed);
    }
}

#[repr(C)]
struct SlotHeader {
    commit: AtomicU32,
    t_us: AtomicU32,
    /// Stored words, with [`TRUNCATED`] when `fill` pushed more.
    len: AtomicU32,
}

impl SlotHeader {
    const fn empty() -> Self {
        Self {
            commit: AtomicU32::new(0),
            t_us: AtomicU32::new(0),
            len: AtomicU32::new(0),
        }
    }
}

/// Storage for `ENTRIES` trace entries (16 bytes each) and `SLOTS` snapshot
/// slots of `WORDS` words, for the image to place in memory that survives
/// the resets it wants to diagnose. Any bit pattern is a valid value, so it
/// may sit in a section that is never initialized.
#[repr(C)]
pub struct Retained<const ENTRIES: usize, const SLOTS: usize, const WORDS: usize> {
    header: Header,
    entries: [Entry; ENTRIES],
    slots: [SlotHeader; SLOTS],
    words: [[AtomicU32; WORDS]; SLOTS],
}

impl<const ENTRIES: usize, const SLOTS: usize, const WORDS: usize> Retained<ENTRIES, SLOTS, WORDS> {
    /// Fails to compile in a static when a geometry cannot be ordered back
    /// by sequence tags (more than half the tag period) or recorded in a
    /// slot length.
    pub const fn new() -> Self {
        assert!(ENTRIES > 0 && ENTRIES <= (TAG_PERIOD / 2) as usize);
        assert!(SLOTS <= (TAG_PERIOD / 2) as usize);
        assert!(WORDS < TRUNCATED as usize);
        Self {
            header: Header {
                magic: AtomicU32::new(0),
                layout: AtomicU32::new(0),
                entries: AtomicU32::new(0),
                slots: AtomicU32::new(0),
                words: AtomicU32::new(0),
                trigger: AtomicU32::new(0),
            },
            entries: [const { Entry::empty() }; ENTRIES],
            slots: [const { SlotHeader::empty() }; SLOTS],
            words: [const { [const { AtomicU32::new(0) }; WORDS] }; SLOTS],
        }
    }
}

impl<const ENTRIES: usize, const SLOTS: usize, const WORDS: usize> Default
    for Retained<ENTRIES, SLOTS, WORDS>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Capacities of a trace's storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub entries: usize,
    pub slots: usize,
    pub words_per_slot: usize,
}

/// A trace over one [`Retained`]. Its own counters stay in the memory the
/// `Trace` static lives in, which must support atomic read-modify-write.
pub struct Trace {
    header: &'static Header,
    pub(crate) entries: &'static [Entry],
    slots: &'static [SlotHeader],
    words: &'static [AtomicU32],
    words_per_slot: usize,
    state: AtomicU8,
    sequence: AtomicU32,
    /// Entries still recorded after a freeze; [`RUNNING_WINDOW`] before one.
    remaining: AtomicU32,
    snapshot_sequence: AtomicU32,
}

#[cfg(feature = "record")]
#[allow(unsafe_code, reason = "the image's clock is a link-time function")]
// SAFETY: the declaration matches the definition every recording image must
// provide (`fn() -> u64`, no preconditions), so calling it is safe; the link
// fails without one.
unsafe extern "Rust" {
    /// The image's monotonic time in microseconds, which stamps records:
    /// every image that records declares it once with [`clock!`](crate::clock).
    safe fn __oer_trace_now_micros() -> u64;
}

impl Trace {
    /// A trace over `retained`, whose records the image's [`clock!`](crate::clock)
    /// stamps.
    pub const fn new<const ENTRIES: usize, const SLOTS: usize, const WORDS: usize>(
        retained: &'static Retained<ENTRIES, SLOTS, WORDS>,
    ) -> Self {
        Self {
            header: &retained.header,
            entries: &retained.entries,
            slots: &retained.slots,
            words: retained.words.as_flattened(),
            words_per_slot: WORDS,
            state: AtomicU8::new(HELD),
            sequence: AtomicU32::new(0),
            remaining: AtomicU32::new(RUNNING_WINDOW),
            snapshot_sequence: AtomicU32::new(0),
        }
    }

    /// The record timestamp now: the low 32 bits of the clock's
    /// microseconds, which wrap every 71 minutes; the host unwraps them along
    /// the sequence order.
    #[cfg(feature = "record")]
    #[inline(always)]
    pub(crate) fn now_us(&self) -> u32 {
        __oer_trace_now_micros() as u32
    }

    pub fn geometry(&self) -> Geometry {
        Geometry {
            entries: self.entries.len(),
            slots: self.slots.len(),
            words_per_slot: self.words_per_slot,
        }
    }

    fn geometry_words(&self) -> [u32; 3] {
        [
            self.entries.len() as u32,
            self.slots.len() as u32,
            self.words_per_slot as u32,
        ]
    }

    /// Stop writing and report what the storage holds from before.
    pub(crate) fn hold(&self) -> Boot {
        self.state.store(HELD, Ordering::Release);
        Boot {
            previous: self.previous(),
        }
    }

    fn previous(&self) -> Option<Previous> {
        let header = self.header;
        let stored = [
            header.entries.load(Ordering::Relaxed),
            header.slots.load(Ordering::Relaxed),
            header.words.load(Ordering::Relaxed),
        ];
        if header.magic.load(Ordering::Relaxed) != MAGIC
            || header.layout.load(Ordering::Relaxed) != LAYOUT
            || stored != self.geometry_words()
        {
            return None;
        }
        Some(Previous {
            trigger: self.trigger(),
            entries: self.records().count(),
            snapshots: self.snapshots().count(),
        })
    }

    /// Clear the storage and record from now on.
    pub(crate) fn restart(&self) {
        self.state.store(HELD, Ordering::Release);
        for entry in self.entries {
            entry.commit.store(0, Ordering::Relaxed);
        }
        for slot in self.slots {
            slot.commit.store(0, Ordering::Relaxed);
        }
        let header = self.header;
        let [entries, slots, words] = self.geometry_words();
        header.trigger.store(0, Ordering::Relaxed);
        header.entries.store(entries, Ordering::Relaxed);
        header.slots.store(slots, Ordering::Relaxed);
        header.words.store(words, Ordering::Relaxed);
        header.layout.store(LAYOUT, Ordering::Relaxed);
        header.magic.store(MAGIC, Ordering::Relaxed);
        self.sequence.store(0, Ordering::Relaxed);
        self.snapshot_sequence.store(0, Ordering::Relaxed);
        self.remaining.store(RUNNING_WINDOW, Ordering::Relaxed);
        self.state.store(RUNNING, Ordering::Release);
    }

    pub fn is_running(&self) -> bool {
        self.state.load(Ordering::Acquire) == RUNNING
    }

    pub fn is_frozen(&self) -> bool {
        self.state.load(Ordering::Acquire) == FROZEN
    }

    #[cfg_attr(
        not(any(feature = "record", test)),
        allow(dead_code, reason = "only recording images write")
    )]
    /// Returns whether this entry was the last one of a freeze window.
    #[inline(always)]
    pub(crate) fn record(&self, kind: Kind, words: [u32; 2], t_us: u32) -> bool {
        let mut last = false;
        if self.remaining.load(Ordering::Relaxed) != RUNNING_WINDOW {
            match self
                .remaining
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |left| {
                    (left != 0 && left != RUNNING_WINDOW).then(|| left - 1)
                }) {
                Ok(left) => last = left == 1,
                Err(_) => return false,
            }
        }
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.entries[sequence as usize % self.entries.len()].write(
            commit(kind, sequence),
            t_us,
            words,
        );
        if last {
            self.state.store(FROZEN, Ordering::Release);
        }
        last
    }

    #[cfg_attr(
        not(any(feature = "record", test)),
        allow(dead_code, reason = "only recording images write")
    )]
    /// Returns whether recording must stop now.
    #[inline(always)]
    pub(crate) fn freeze(&self, trigger: Kind, post: u32) -> bool {
        if self.state.load(Ordering::Acquire) != RUNNING
            || self
                .remaining
                .compare_exchange(RUNNING_WINDOW, post, Ordering::Relaxed, Ordering::Relaxed)
                .is_err()
        {
            return false;
        }
        self.header.trigger.store(
            commit(trigger, self.sequence.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        if post == 0 {
            self.state.store(FROZEN, Ordering::Release);
        }
        post == 0
    }

    #[cfg_attr(
        not(any(feature = "record", test)),
        allow(dead_code, reason = "only recording images write")
    )]
    #[inline(always)]
    pub(crate) fn capture(
        &self,
        point: Kind,
        t_us: u32,
        fill: impl FnOnce(&mut SnapshotWriter<'_>),
    ) {
        if self.state.load(Ordering::Acquire) == HELD || self.slots.is_empty() {
            return;
        }
        let sequence = self.snapshot_sequence.fetch_add(1, Ordering::Relaxed);
        let slot = sequence as usize % self.slots.len();
        let header = &self.slots[slot];
        header.commit.store(0, Ordering::Relaxed);
        fence(Ordering::Release);
        let mut writer = SnapshotWriter {
            words: self.slot_words(slot),
            len: 0,
            truncated: false,
        };
        fill(&mut writer);
        header.t_us.store(t_us, Ordering::Relaxed);
        header.len.store(
            writer.len as u32 | if writer.truncated { TRUNCATED } else { 0 },
            Ordering::Relaxed,
        );
        header
            .commit
            .store(commit(point, sequence), Ordering::Release);
    }

    fn slot_words(&self, slot: usize) -> &'static [AtomicU32] {
        let start = slot * self.words_per_slot;
        &self.words[start..start + self.words_per_slot]
    }

    /// The freeze trigger of the current or, while held, the previous run.
    pub fn trigger(&self) -> Option<Trigger> {
        let word = self.header.trigger.load(Ordering::Relaxed);
        (word != 0).then_some(Trigger {
            kind: (word >> 16) as u16,
            tag: word as u16,
        })
    }

    /// The complete entry in storage slot `slot`, if any.
    pub fn entry(&self, slot: usize) -> Option<Record> {
        self.entries.get(slot)?.read()
    }

    /// Every complete entry, in storage order; see [`oldest_first`].
    pub fn records(&self) -> impl Iterator<Item = Record> + '_ {
        self.entries.iter().filter_map(Entry::read)
    }

    /// Every complete snapshot, in slot order.
    pub fn snapshots(&self) -> impl Iterator<Item = SnapshotRecord> + '_ {
        (0..self.slots.len()).filter_map(|slot| self.snapshot(slot))
    }

    pub fn snapshot(&self, slot: usize) -> Option<SnapshotRecord> {
        let header = self.slots.get(slot)?;
        let commit = header.commit.load(Ordering::Acquire);
        if commit == 0 {
            return None;
        }
        let t_us = header.t_us.load(Ordering::Relaxed);
        let len = header.len.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        (header.commit.load(Ordering::Relaxed) == commit).then_some(SnapshotRecord {
            slot,
            commit,
            point: (commit >> 16) as u16,
            tag: commit as u16,
            t_us,
            len: ((len & !TRUNCATED) as usize).min(self.words_per_slot),
            truncated: len & TRUNCATED != 0,
        })
    }

    /// Copy the words of `snapshot` from `offset` into `out`, returning how
    /// many; `None` once the slot holds another snapshot, in which case the
    /// copied words are stale.
    pub fn read_snapshot(
        &self,
        snapshot: &SnapshotRecord,
        offset: usize,
        out: &mut [u32],
    ) -> Option<usize> {
        let header = self.slots.get(snapshot.slot)?;
        if header.commit.load(Ordering::Acquire) != snapshot.commit {
            return None;
        }
        let words = self
            .slot_words(snapshot.slot)
            .get(offset..snapshot.len)
            .unwrap_or_default();
        let count = words.len().min(out.len());
        for (out, word) in out.iter_mut().zip(words) {
            *out = word.load(Ordering::Relaxed);
        }
        fence(Ordering::Acquire);
        (header.commit.load(Ordering::Relaxed) == snapshot.commit).then_some(count)
    }
}

/// What [`crate::install`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Boot {
    /// `None` when the storage holds no trace of this layout and geometry,
    /// as after a power-on.
    pub previous: Option<Previous>,
}

/// What the previous boot left, readable until [`crate::start`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Previous {
    pub trigger: Option<Trigger>,
    pub entries: usize,
    pub snapshots: usize,
}

/// Why and where recording was frozen: `tag` is that of the first entry
/// recorded after the trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trigger {
    pub kind: u16,
    pub tag: u16,
}

impl Trigger {
    pub fn kind(&self) -> Option<Kind> {
        Kind::from_raw(self.kind)
    }
}

/// One complete trace entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Record {
    /// Sequence tag in `1..=0xffff`; see [`oldest_first`].
    pub tag: u16,
    pub kind: u16,
    pub t_us: u32,
    pub words: [u32; 2],
}

impl Record {
    pub fn kind(&self) -> Option<Kind> {
        Kind::from_raw(self.kind)
    }

    /// This entry as `E`, when it is one.
    pub fn decode<E: Event>(&self) -> Option<E> {
        if self.kind == E::KIND.raw() {
            E::decode(self.words)
        } else {
            None
        }
    }
}

/// Put `records` of one trace in the order they were written. Their tags
/// span less than half the tag period, so the widest gap between sorted
/// tags is where the sequence starts.
pub fn oldest_first(records: &mut [Record]) {
    records.sort_unstable_by_key(|record| record.tag);
    let (Some(first), Some(last)) = (records.first(), records.last()) else {
        return;
    };
    let mut start = 0;
    let mut widest = u32::from(first.tag) + TAG_PERIOD - u32::from(last.tag);
    for (index, pair) in records.windows(2).enumerate() {
        let gap = u32::from(pair[1].tag - pair[0].tag);
        if gap > widest {
            widest = gap;
            start = index + 1;
        }
    }
    records.rotate_left(start);
}

/// One complete snapshot slot; read its words with [`Trace::read_snapshot`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotRecord {
    pub slot: usize,
    commit: u32,
    pub point: u16,
    pub tag: u16,
    pub t_us: u32,
    pub len: usize,
    pub truncated: bool,
}

impl SnapshotRecord {
    pub fn point(&self) -> Option<Kind> {
        Kind::from_raw(self.point)
    }
}

/// Appends words to the snapshot slot being captured.
pub struct SnapshotWriter<'a> {
    words: &'a [AtomicU32],
    len: usize,
    truncated: bool,
}

impl SnapshotWriter<'_> {
    #[inline(always)]
    pub fn push(&mut self, word: u32) {
        match self.words.get(self.len) {
            Some(slot) => {
                slot.store(word, Ordering::Relaxed);
                self.len += 1;
            }
            None => self.truncated = true,
        }
    }

    pub fn extend(&mut self, words: impl IntoIterator<Item = u32>) {
        for word in words {
            self.push(word);
        }
    }

    pub fn remaining(&self) -> usize {
        self.words.len() - self.len
    }
}

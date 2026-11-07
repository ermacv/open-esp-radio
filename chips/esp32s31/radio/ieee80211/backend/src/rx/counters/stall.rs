//! A trace build's snapshot of a stalled receive path, which the radio
//! owner takes when a diagnostic reader asks for one.
//!
//! [`RxStall`] is the one record between them. A reader on the radio
//! owner's executor asks with [`RxStall::request`], which wakes the task the
//! owner registered. The owner's next turn takes the snapshot before its
//! ordinary receive service ([`RxStallSnapshot`]) and, at its facts after
//! that turn's receive pull, records how far each boundary went since
//! ([`RxStallResume`]).
//! The reader takes both with [`RxStall::read`].
//!
//! Times are the low word of the owner's timer in microseconds; a reading
//! gives each one's age at the reader's own reading of that timer
//! ([`RxStallReading::age`]), exact for ages below 2^32 µs. The owner
//! writes each record under a sequence word, so a reader never takes a
//! record the owner was writing; it reads again later. Report-only: no
//! value takes part in descriptor ownership, and taking a snapshot changes
//! no register, descriptor or lease.

use core::sync::atomic::{AtomicU32, Ordering, fence};
use core::task::Waker;

use oer_portable_waker::AtomicWaker;

use super::mac::{COUNTS, MacBank, MacSample};

/// One request for a snapshot. Zero is never a ticket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxStallTicket(u32);

impl RxStallTicket {
    /// The ticket's number, as the record keeps it.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// How far one boundary went: its steps (wrapping; zero before the first)
/// and when it took its latest one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxProgress {
    pub count: u32,
    pub at: u32,
}

impl RxProgress {
    /// Whether the boundary ever moved.
    pub const fn seen(self) -> bool {
        self.count != 0
    }

    /// `by` more steps at `at`. A count that wraps to zero lands on one, so
    /// zero still means never.
    pub fn advance(&mut self, by: u32, at: u32) {
        if by == 0 {
            return;
        }
        self.count = self.count.wrapping_add(by).max(1);
        self.at = at;
    }
}

/// The walker's registers, read without writing one.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxWalkerView {
    /// `RX_CONTROL`: the walker enable and the reload doorbell.
    pub enabled: bool,
    pub reload_doorbell: bool,
    /// `RX_NEXT_DESCRIPTOR`: the low address and whether its unassigned
    /// upper field read nonzero.
    pub next_low: u32,
    pub next_upper: bool,
    /// `RX_LAST_DESCRIPTOR`, low address.
    pub last_low: u32,
    /// `RX_DESCRIPTOR_BASE`, where the platform reads it.
    pub base: Option<u32>,
    /// The MAC's hardware starvation counter, where it has one.
    pub buffer_full: Option<u16>,
    /// The MAC runtime's active state; zero is idle.
    pub runtime_state: u8,
}

/// The live ring's own view: its descriptor base, recycle frontier and
/// accepted tail (indices), descriptors returned to software in this
/// generation, newly completed descriptors at the frontier, the accepted
/// list left from NEXT (`None` when NEXT lies outside the ring) and whether
/// a reload it requested is still pending.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxRingView {
    pub base: u32,
    pub recycle_start: u32,
    pub accepted_tail: u32,
    pub observed: u32,
    pub completed: u32,
    pub remaining: Option<u32>,
    pub reload_pending: bool,
}

/// The ring's descriptors by their first word: `OWNER` set without
/// `RX_DONE` (armed for the walker, or a filled descriptor of a unit whose
/// last one is not done), `OWNER` and `RX_DONE` set (a completed unit's
/// last descriptor), `OWNER` clear (its buffer detached to software); and
/// the reserved descriptor the walker starts from, apart, by the same bits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxDescriptorCounts {
    pub total: u32,
    pub armed: u32,
    pub done: u32,
    pub software: u32,
    pub reserved_owner: bool,
    pub reserved_done: bool,
}

/// The session's owners: the descriptors, the buffers and the software side
/// of the interrupt line.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxSessionView {
    pub descriptors: RxDescriptorCounts,
    /// Buffers released for rearm, buffers detached as staged leases
    /// (the upper layers' included) and claimed staging slots.
    pub released: u32,
    pub detached: u32,
    pub staged: u32,
    /// The session masked both banks (a cut or a stop) and froze ingress.
    pub masked: bool,
    pub frozen: bool,
    /// A request to the line that its handler has not applied yet.
    pub line_request_pending: bool,
    /// A receive fact the line posted that no poll has taken yet; `None`
    /// outside the signal's executor.
    pub receive_fact_pending: Option<bool>,
    /// The counters' radio handler entries, RX unmasks it applied and
    /// ignored, and service passes, since the arena was bound.
    pub mac_irqs: u32,
    pub doorbell_unmasks: u32,
    pub ignored_unmasks: u32,
    pub service_passes: u32,
}

/// What the owner reads with its registers: `ring` is `None` without a live
/// ring.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxHardwareView {
    pub walker: RxWalkerView,
    pub ring: Option<RxRingView>,
    pub session: RxSessionView,
}

/// The device's own state: whether it holds ring work, a continuation, a
/// receive cut, a stop under way or a paused ring, and the staged units
/// it holds for the upper layer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxDeviceView {
    pub rx_pending: bool,
    pub continuation: bool,
    pub cut: bool,
    pub pausing: bool,
    pub paused: bool,
    pub queued: u32,
}

/// The owner's snapshot, taken before the turn's receive service.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxStallSnapshot {
    pub at: u32,
    /// The ring's descriptors observed by service passes, and the units
    /// the device handed to the upper layer, until the snapshot.
    pub dma: RxProgress,
    pub driver: RxProgress,
    pub device: RxDeviceView,
    /// `None` while the owner holds no registers (a retune in flight) or its
    /// receive session failed.
    pub hardware: Option<RxHardwareView>,
    /// The MAC statistics bank read now, and the periodic sample of it the
    /// turn keeps ([`super::mac::MAC_COUNTS`]); `None` where the owner has
    /// no bank or no sample yet.
    pub mac: Option<MacBank>,
    pub periodic: Option<MacSample>,
}

/// The boundaries at the owner's facts after a snapshot. `serviced` says
/// whether the turn's receive pull ran before them: until it did, each of
/// the owner's facts records them again; the first after the pull is the
/// last.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxStallResume {
    pub at: u32,
    pub dma: RxProgress,
    pub driver: RxProgress,
    pub serviced: bool,
}

/// A snapshot as a reader took it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxStallReading {
    /// The owner's timer when the reader took it.
    pub read_at: u32,
    pub snapshot: RxStallSnapshot,
    /// `None` until the owner's facts after it.
    pub resume: Option<RxStallResume>,
}

impl RxStallReading {
    /// How long before the reading the owner's timer read `at`.
    pub const fn age(&self, at: u32) -> u32 {
        self.read_at.wrapping_sub(at)
    }
}

/// The words of a snapshot: its ticket, then the fields in the order of
/// [`put_snapshot`]: 49 of the owner's, the periodic sample's time and two
/// banks, each a presence word, its counts and its frequency offset.
const SNAPSHOT_WORDS: usize = 1 + 49 + 1 + 2 * (2 + COUNTS);
/// The words of a resume: its ticket, then the fields.
const RESUME_WORDS: usize = 7;

/// The record between the radio owner and a diagnostic reader.
pub struct RxStall {
    /// The latest ticket asked for.
    request: AtomicU32,
    /// The owner's task.
    owner: AtomicWaker,
    snapshot: Record<SNAPSHOT_WORDS>,
    resume: Record<RESUME_WORDS>,
}

impl Default for RxStall {
    fn default() -> Self {
        Self::new()
    }
}

impl RxStall {
    pub const fn new() -> Self {
        Self {
            request: AtomicU32::new(0),
            owner: AtomicWaker::new(),
            snapshot: Record::new(),
            resume: Record::new(),
        }
    }

    /// Ask for a snapshot and wake the owner's task. The caller runs on the
    /// owner's hart: the wake goes through the task's own waker.
    pub fn request(&self) -> RxStallTicket {
        let previous = self.request.load(Ordering::Relaxed);
        let ticket = previous.wrapping_add(1).max(1);
        self.request.store(ticket, Ordering::Release);
        self.owner.wake();
        RxStallTicket(ticket)
    }

    /// The snapshot taken for `ticket`, with its resume once recorded, at
    /// `now` of the owner's timer; `None` while the owner has not taken it
    /// or was writing it.
    pub fn read(&self, ticket: RxStallTicket, now: u32) -> Option<RxStallReading> {
        let words = self.snapshot.read()?;
        let mut take = Take(words.iter().copied());
        if take.u32() != ticket.0 {
            return None;
        }
        let snapshot = take_snapshot(&mut take);
        let resume = self.resume.read().and_then(|words| {
            let mut take = Take(words.iter().copied());
            (take.u32() == ticket.0).then(|| RxStallResume {
                at: take.u32(),
                dma: take_progress(&mut take),
                driver: take_progress(&mut take),
                serviced: take.bool(),
            })
        });
        Some(RxStallReading {
            read_at: now,
            snapshot,
            resume,
        })
    }

    /// The owner's task, which a request wakes.
    pub(crate) fn register(&self, waker: &Waker) {
        self.owner.register(waker);
    }

    /// The ticket asked for since the owner served `served`.
    pub(crate) fn requested(&self, served: u32) -> Option<RxStallTicket> {
        let ticket = self.request.load(Ordering::Acquire);
        (ticket != served).then_some(RxStallTicket(ticket))
    }

    /// The owner's snapshot for `ticket`; it replaces any earlier one.
    pub(crate) fn record_snapshot(&self, ticket: RxStallTicket, snapshot: &RxStallSnapshot) {
        let mut words = [0; SNAPSHOT_WORDS];
        let mut put = Put::new(&mut words);
        put.u32(ticket.0);
        put_snapshot(&mut put, snapshot);
        self.snapshot.write(&words);
    }

    /// The owner's resume after the snapshot for `ticket`.
    pub(crate) fn record_resume(&self, ticket: RxStallTicket, resume: &RxStallResume) {
        let mut words = [0; RESUME_WORDS];
        let mut put = Put::new(&mut words);
        put.u32(ticket.0);
        put.u32(resume.at);
        put_progress(&mut put, resume.dma);
        put_progress(&mut put, resume.driver);
        put.bool(resume.serviced);
        self.resume.write(&words);
    }
}

/// `N` words under a sequence word: odd while the one writer writes them,
/// advanced by two per record (zero: none yet).
struct Record<const N: usize> {
    sequence: AtomicU32,
    words: [AtomicU32; N],
}

impl<const N: usize> Record<N> {
    const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            words: [const { AtomicU32::new(0) }; N],
        }
    }

    fn write(&self, words: &[u32; N]) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1) | 1, Ordering::Relaxed);
        fence(Ordering::Release);
        for (slot, word) in self.words.iter().zip(words) {
            slot.store(*word, Ordering::Relaxed);
        }
        self.sequence
            .store((sequence | 1).wrapping_add(1), Ordering::Release);
    }

    fn read(&self) -> Option<[u32; N]> {
        let before = self.sequence.load(Ordering::Acquire);
        if before == 0 || before & 1 != 0 {
            return None;
        }
        let words = self
            .words
            .each_ref()
            .map(|word| word.load(Ordering::Relaxed));
        fence(Ordering::Acquire);
        (self.sequence.load(Ordering::Relaxed) == before).then_some(words)
    }
}

/// A writer of words in order. A word past the end would be lost: the
/// round-trip tests read every field back.
struct Put<'a> {
    words: core::slice::IterMut<'a, u32>,
}

impl<'a> Put<'a> {
    fn new(words: &'a mut [u32]) -> Self {
        Self {
            words: words.iter_mut(),
        }
    }

    fn u32(&mut self, value: u32) {
        if let Some(word) = self.words.next() {
            *word = value;
        }
    }

    fn bool(&mut self, value: bool) {
        self.u32(u32::from(value));
    }

    fn i32(&mut self, value: i32) {
        self.u32(u32::from_ne_bytes(value.to_ne_bytes()));
    }

    fn option(&mut self, value: Option<u32>) {
        self.bool(value.is_some());
        self.u32(value.unwrap_or(0));
    }
}

/// A reader of words in order; past the end it reads zeros.
struct Take<I>(I);

impl<I: Iterator<Item = u32>> Take<I> {
    fn u32(&mut self) -> u32 {
        self.0.next().unwrap_or(0)
    }

    fn bool(&mut self) -> bool {
        self.u32() != 0
    }

    fn i32(&mut self) -> i32 {
        i32::from_ne_bytes(self.u32().to_ne_bytes())
    }

    fn option(&mut self) -> Option<u32> {
        let present = self.bool();
        let value = self.u32();
        present.then_some(value)
    }
}

fn put_progress(put: &mut Put<'_>, progress: RxProgress) {
    put.u32(progress.count);
    put.u32(progress.at);
}

fn take_progress<I: Iterator<Item = u32>>(take: &mut Take<I>) -> RxProgress {
    RxProgress {
        count: take.u32(),
        at: take.u32(),
    }
}

fn put_bank(put: &mut Put<'_>, bank: Option<&MacBank>) {
    put.bool(bank.is_some());
    let bank = bank.copied().unwrap_or_default();
    for count in bank.counts {
        put.u32(count);
    }
    put.i32(bank.frequency_offset);
}

fn take_bank<I: Iterator<Item = u32>>(take: &mut Take<I>) -> Option<MacBank> {
    let present = take.bool();
    let counts: [u32; COUNTS] = core::array::from_fn(|_| take.u32());
    let frequency_offset = take.i32();
    present.then_some(MacBank {
        counts,
        frequency_offset,
    })
}

fn put_snapshot(put: &mut Put<'_>, snapshot: &RxStallSnapshot) {
    put.u32(snapshot.at);
    put_progress(put, snapshot.dma);
    put_progress(put, snapshot.driver);
    let device = &snapshot.device;
    put.bool(device.rx_pending);
    put.bool(device.continuation);
    put.bool(device.cut);
    put.bool(device.pausing);
    put.bool(device.paused);
    put.u32(device.queued);
    put.bool(snapshot.hardware.is_some());
    let hardware = snapshot.hardware.unwrap_or_default();
    let walker = &hardware.walker;
    put.bool(walker.enabled);
    put.bool(walker.reload_doorbell);
    put.u32(walker.next_low);
    put.bool(walker.next_upper);
    put.u32(walker.last_low);
    put.option(walker.base);
    put.option(walker.buffer_full.map(u32::from));
    put.u32(u32::from(walker.runtime_state));
    put.bool(hardware.ring.is_some());
    let ring = hardware.ring.unwrap_or_default();
    put.u32(ring.base);
    put.u32(ring.recycle_start);
    put.u32(ring.accepted_tail);
    put.u32(ring.observed);
    put.u32(ring.completed);
    put.option(ring.remaining);
    put.bool(ring.reload_pending);
    let session = &hardware.session;
    put.u32(session.descriptors.total);
    put.u32(session.descriptors.armed);
    put.u32(session.descriptors.done);
    put.u32(session.descriptors.software);
    put.bool(session.descriptors.reserved_owner);
    put.bool(session.descriptors.reserved_done);
    put.u32(session.released);
    put.u32(session.detached);
    put.u32(session.staged);
    put.bool(session.masked);
    put.bool(session.frozen);
    put.bool(session.line_request_pending);
    put.option(session.receive_fact_pending.map(u32::from));
    put.u32(session.mac_irqs);
    put.u32(session.doorbell_unmasks);
    put.u32(session.ignored_unmasks);
    put.u32(session.service_passes);
    put_bank(put, snapshot.mac.as_ref());
    put.u32(snapshot.periodic.map_or(0, |sample| sample.at));
    put_bank(put, snapshot.periodic.as_ref().map(|sample| &sample.bank));
}

fn take_snapshot<I: Iterator<Item = u32>>(take: &mut Take<I>) -> RxStallSnapshot {
    let at = take.u32();
    let dma = take_progress(take);
    let driver = take_progress(take);
    let device = RxDeviceView {
        rx_pending: take.bool(),
        continuation: take.bool(),
        cut: take.bool(),
        pausing: take.bool(),
        paused: take.bool(),
        queued: take.u32(),
    };
    let has_hardware = take.bool();
    let walker = RxWalkerView {
        enabled: take.bool(),
        reload_doorbell: take.bool(),
        next_low: take.u32(),
        next_upper: take.bool(),
        last_low: take.u32(),
        base: take.option(),
        buffer_full: take
            .option()
            .map(|count| u16::try_from(count).unwrap_or(u16::MAX)),
        runtime_state: u8::try_from(take.u32()).unwrap_or(u8::MAX),
    };
    let has_ring = take.bool();
    let ring = RxRingView {
        base: take.u32(),
        recycle_start: take.u32(),
        accepted_tail: take.u32(),
        observed: take.u32(),
        completed: take.u32(),
        remaining: take.option(),
        reload_pending: take.bool(),
    };
    let session = RxSessionView {
        descriptors: RxDescriptorCounts {
            total: take.u32(),
            armed: take.u32(),
            done: take.u32(),
            software: take.u32(),
            reserved_owner: take.bool(),
            reserved_done: take.bool(),
        },
        released: take.u32(),
        detached: take.u32(),
        staged: take.u32(),
        masked: take.bool(),
        frozen: take.bool(),
        line_request_pending: take.bool(),
        receive_fact_pending: take.option().map(|pending| pending != 0),
        mac_irqs: take.u32(),
        doorbell_unmasks: take.u32(),
        ignored_unmasks: take.u32(),
        service_passes: take.u32(),
    };
    let mac = take_bank(take);
    let periodic_at = take.u32();
    let periodic_bank = take_bank(take);
    RxStallSnapshot {
        at,
        dma,
        driver,
        device,
        hardware: has_hardware.then_some(RxHardwareView {
            walker,
            ring: has_ring.then_some(ring),
            session,
        }),
        mac,
        periodic: periodic_bank.map(|bank| MacSample {
            at: periodic_at,
            bank,
        }),
    }
}

#[cfg(test)]
mod tests;

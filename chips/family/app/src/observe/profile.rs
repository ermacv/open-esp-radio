//! The network core's turn profile and the station's return bursts, for a
//! fit of a turn's cost to the work it did.
//!
//! [`ProfileTally`] hears each turn of the network's service: its beginning
//! and end, each on the hart's cycle counter, the received frames it took
//! in, the frames its port accepted, the notices it handed the policy and
//! the requests it took from the sockets' lanes. Each ended turn adds to the
//! sums of a least-squares fit of its active cycles to those four counts
//! ([`TurnProfile::moments`]), to cells of turns with one kind of work, to
//! histograms, and to the gaps between turns by how the pass after the turn
//! went on. Nothing is kept per turn.
//!
//! [`ReturnCounter`] counts the bursts in which the station's datapath
//! actor gave the network's memory back through the return queue.
//!
//! Each counter has one writer. A [`ProfileCounter`] word of 64 bits is two
//! atomic words, which a reader on the writer's executor, between the
//! writer's polls, reads whole; a reader elsewhere may see the two halves of
//! different writes. Sums wrap at 2^64, return counts at 2^32: a reader takes
//! the differences of two readings ([`TurnProfile::window`],
//! [`Returns::window`]).

use core::sync::atomic::{AtomicU32, Ordering};

/// The variables a turn's moments multiply: one, the frames taken in, the
/// frames accepted, the notices, the requests, and the active cycles.
pub const VARIABLES: usize = 6;
/// The moments: every product of two variables, each pair once.
pub const MOMENTS: usize = VARIABLES * (VARIABLES + 1) / 2;
/// The bins of [`TurnProfile::cycles_by_log2`]: below 2^9 cycles, then one
/// per power of two, the last 2^23 and more.
pub const CYCLE_BINS: usize = 16;
/// The frame counts of [`TurnProfile::receive_only`], one to eight, and the
/// accepted counts of [`TurnProfile::transmit_only`].
pub const ITEM_CELLS: usize = 8;
/// The bins of [`TurnProfile::notice_only`]: 1, 2, 3 to 4, 5 to 8, 9 to
/// 16, 17 to 32, and 33 or more notices.
pub const NOTICE_CELLS: usize = 7;
/// The bins of [`TurnProfile::notices_by_turn`]: 0, then the notice cells'.
pub const NOTICE_BINS: usize = NOTICE_CELLS + 1;
/// The bins of [`TurnProfile::frames_by_turn`]: 0 to 7 frames, and 8 or
/// more.
pub const FRAME_BINS: usize = 9;

/// How the pass after a turn went on, which names the gap before the next.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Gap {
    /// The task yielded to its executor after a turn that moved.
    Yielded = 0,
    /// A wait found something as it registered: the next turn runs in the
    /// same poll.
    Ready = 1,
    /// A wait found nothing: the poll ended, and the executor ran other
    /// tasks or slept.
    Pending = 2,
}

/// The gaps by [`Gap`].
pub const GAPS: usize = 3;

/// What the network's turns did since boot, as sums.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TurnProfile {
    /// The sums over the turns of each product `v[i] * v[j]`, `i <= j`, of
    /// the variables `v = [1, frames, accepted, notices, requests, cycles]`,
    /// row by row: `[n, Σf, Σa, Σn, Σr, Σc, Σf², Σfa, …, Σc²]`. Cycles above
    /// 2^32 count as 2^32 − 1.
    pub moments: [u64; MOMENTS],
    /// Turns with no item of the four (the variables but one all zero):
    /// their count, cycles and cycles squared.
    pub empty: [u64; 3],
    /// Turns a wait followed, which moved nothing: their count and cycles.
    pub idle: [u64; 2],
    /// The gaps from a turn's end to the next turn's beginning, by [`Gap`]:
    /// their count and cycles.
    pub gaps: [[u64; 2]; GAPS],
    /// Turns by their cycles ([`CYCLE_BINS`]).
    pub cycles_by_log2: [u64; CYCLE_BINS],
    /// Turns that took in `1 + index` frames and neither accepted a frame nor
    /// handed on a notice: their count, cycles and requests.
    pub receive_only: [[u64; 3]; ITEM_CELLS],
    /// Turns whose port accepted `1 + index` frames, which took in no frame
    /// and handed on no notice: their count, cycles and requests.
    pub transmit_only: [[u64; 3]; ITEM_CELLS],
    /// Turns that handed on notices ([`NOTICE_CELLS`]), took in no frame and
    /// had no frame accepted: their count, cycles and notices.
    pub notice_only: [[u64; 3]; NOTICE_CELLS],
    /// Turns by the notices they handed on ([`NOTICE_BINS`]).
    pub notices_by_turn: [u64; NOTICE_BINS],
    /// Turns by the frames they took in ([`FRAME_BINS`]).
    pub frames_by_turn: [u64; FRAME_BINS],
    /// Turns that handed on a notice, those of them whose port accepted a
    /// frame, and the frames accepted in them: publications in the turn
    /// that settled.
    pub settling: [u64; 3],
}

/// The 64-bit words of a [`TurnProfile`], in its fields' order.
pub const WORDS: usize = MOMENTS
    + 3
    + 2
    + 2 * GAPS
    + CYCLE_BINS
    + 3 * ITEM_CELLS
    + 3 * ITEM_CELLS
    + 3 * NOTICE_CELLS
    + NOTICE_BINS
    + FRAME_BINS
    + 3;

/// The first word of each field of a [`TurnProfile`].
mod word {
    use super::*;
    pub const MOMENTS: usize = 0;
    pub const EMPTY: usize = MOMENTS + super::MOMENTS;
    pub const IDLE: usize = EMPTY + 3;
    pub const GAPS: usize = IDLE + 2;
    pub const CYCLES: usize = GAPS + 2 * super::GAPS;
    pub const RECEIVE_ONLY: usize = CYCLES + CYCLE_BINS;
    pub const TRANSMIT_ONLY: usize = RECEIVE_ONLY + 3 * ITEM_CELLS;
    pub const NOTICE_ONLY: usize = TRANSMIT_ONLY + 3 * ITEM_CELLS;
    pub const NOTICES: usize = NOTICE_ONLY + 3 * NOTICE_CELLS;
    pub const FRAMES: usize = NOTICES + NOTICE_BINS;
    pub const SETTLING: usize = FRAMES + FRAME_BINS;
    pub const END: usize = SETTLING + 3;
}

const _: () = {
    if word::END != WORDS {
        panic!("the profile's fields fill its words");
    }
};

impl TurnProfile {
    /// The profile from its words, in its fields' order.
    pub fn from_words(words: &[u64; WORDS]) -> Self {
        let at = |index: usize| words.get(index).copied().unwrap_or(0);
        let rows = |first: usize, width: usize, row: usize, column: usize| {
            at(first
                .saturating_add(row.saturating_mul(width))
                .saturating_add(column))
        };
        Self {
            moments: core::array::from_fn(|index| at(word::MOMENTS.saturating_add(index))),
            empty: core::array::from_fn(|index| at(word::EMPTY.saturating_add(index))),
            idle: core::array::from_fn(|index| at(word::IDLE.saturating_add(index))),
            gaps: core::array::from_fn(|gap| {
                core::array::from_fn(|column| rows(word::GAPS, 2, gap, column))
            }),
            cycles_by_log2: core::array::from_fn(|bin| at(word::CYCLES.saturating_add(bin))),
            receive_only: core::array::from_fn(|cell| {
                core::array::from_fn(|column| rows(word::RECEIVE_ONLY, 3, cell, column))
            }),
            transmit_only: core::array::from_fn(|cell| {
                core::array::from_fn(|column| rows(word::TRANSMIT_ONLY, 3, cell, column))
            }),
            notice_only: core::array::from_fn(|cell| {
                core::array::from_fn(|column| rows(word::NOTICE_ONLY, 3, cell, column))
            }),
            notices_by_turn: core::array::from_fn(|bin| at(word::NOTICES.saturating_add(bin))),
            frames_by_turn: core::array::from_fn(|bin| at(word::FRAMES.saturating_add(bin))),
            settling: core::array::from_fn(|index| at(word::SETTLING.saturating_add(index))),
        }
    }

    /// The profile's words, in its fields' order.
    pub fn words(&self) -> [u64; WORDS] {
        let mut words = [0; WORDS];
        let mut at = words.iter_mut();
        let fields = self
            .moments
            .iter()
            .chain(&self.empty)
            .chain(&self.idle)
            .chain(self.gaps.iter().flatten())
            .chain(&self.cycles_by_log2)
            .chain(self.receive_only.iter().flatten())
            .chain(self.transmit_only.iter().flatten())
            .chain(self.notice_only.iter().flatten())
            .chain(&self.notices_by_turn)
            .chain(&self.frames_by_turn)
            .chain(&self.settling);
        for (place, value) in at.by_ref().zip(fields) {
            *place = *value;
        }
        words
    }

    /// The sums between readings `start` and `end`, wrapping at 2^64.
    pub fn window(start: &Self, end: &Self) -> Self {
        let (start, end) = (start.words(), end.words());
        let mut words = [0; WORDS];
        for ((place, end), start) in words.iter_mut().zip(end).zip(start) {
            *place = end.wrapping_sub(start);
        }
        Self::from_words(&words)
    }

    /// The moment `Σ v[i] v[j]`, in either order.
    pub fn moment(&self, i: usize, j: usize) -> u64 {
        let (i, j) = (i.min(j), i.max(j));
        self.moments.get(moment_index(i, j)).copied().unwrap_or(0)
    }
}

/// The place of moment `(i, j)`, `i <= j`, in [`TurnProfile::moments`].
const fn moment_index(i: usize, j: usize) -> usize {
    // Rows 0 to i - 1 hold VARIABLES, VARIABLES - 1, … moments.
    let before = i
        .saturating_mul(VARIABLES)
        .saturating_sub(i.saturating_mul(i.saturating_sub(1)) / 2);
    before.saturating_add(j.saturating_sub(i))
}

/// The bin of a turn of `cycles`.
const fn cycle_bin(cycles: u32) -> usize {
    match cycles.checked_ilog2() {
        None | Some(0..=8) => 0,
        Some(log) if log >= 23 => CYCLE_BINS - 1,
        Some(log) => (log - 8) as usize,
    }
}

/// The notice cell of `notices`, one or more.
const fn notice_cell(notices: u32) -> usize {
    match notices {
        0..=2 => notices.saturating_sub(1) as usize,
        3..=4 => 2,
        5..=8 => 3,
        9..=16 => 4,
        17..=32 => 5,
        _ => 6,
    }
}

/// The network's turn profile, published word by word.
pub struct ProfileCounter {
    /// Each 64-bit word as its low and its high half.
    halves: [AtomicU32; 2 * WORDS],
}

impl ProfileCounter {
    pub const fn new() -> Self {
        Self {
            halves: [const { AtomicU32::new(0) }; 2 * WORDS],
        }
    }

    fn word(&self, index: usize) -> Option<(&AtomicU32, &AtomicU32)> {
        let low = index.checked_mul(2)?;
        Some((self.halves.get(low)?, self.halves.get(low.checked_add(1)?)?))
    }

    fn add(&self, index: usize, by: u64) {
        if let Some((low, high)) = self.word(index) {
            let was = u64::from(low.load(Ordering::Relaxed))
                | (u64::from(high.load(Ordering::Relaxed)) << 32);
            let now = was.wrapping_add(by);
            // Halves of a 64-bit word.
            low.store(now as u32, Ordering::Relaxed);
            high.store((now >> 32) as u32, Ordering::Relaxed);
        }
    }

    fn load(&self, index: usize) -> u64 {
        self.word(index).map_or(0, |(low, high)| {
            u64::from(low.load(Ordering::Relaxed)) | (u64::from(high.load(Ordering::Relaxed)) << 32)
        })
    }

    /// The sums now.
    pub fn read(&self) -> TurnProfile {
        let words: [u64; WORDS] = core::array::from_fn(|index| self.load(index));
        TurnProfile::from_words(&words)
    }
}

impl Default for ProfileCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// The turn under way, as the network's task holds it: when it began, what
/// it did so far, and when the last one ended and how its pass went on.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProfileTally {
    began: Option<u64>,
    frames: u32,
    accepted: u32,
    notices: u32,
    requests: u32,
    /// The last ended turn's end and cycles.
    ended: Option<(u64, u32)>,
    /// How the pass after the last turn went on.
    gap: Option<Gap>,
}

impl ProfileTally {
    pub const fn new() -> Self {
        Self {
            began: None,
            frames: 0,
            accepted: 0,
            notices: 0,
            requests: 0,
            ended: None,
            gap: None,
        }
    }

    /// A turn began at cycle `now`: the gap since the last one's end counts
    /// by how its pass went on.
    pub fn began(&mut self, counter: &ProfileCounter, now: u64) {
        if let (Some((end, _)), Some(gap)) = (self.ended, self.gap) {
            let first = word::GAPS.saturating_add(2 * gap as usize);
            counter.add(first, 1);
            counter.add(first.saturating_add(1), now.wrapping_sub(end));
        }
        *self = Self {
            began: Some(now),
            ended: self.ended,
            ..Self::new()
        };
    }

    /// The turn took in one received frame.
    pub fn frame(&mut self) {
        self.frames = self.frames.saturating_add(1);
    }

    /// The port accepted one frame of the turn.
    pub fn accepted(&mut self) {
        self.accepted = self.accepted.saturating_add(1);
    }

    /// The turn handed the policy one notice.
    pub fn noticed(&mut self) {
        self.notices = self.notices.saturating_add(1);
    }

    /// The turn took one request from a socket's lane.
    pub fn took(&mut self) {
        self.requests = self.requests.saturating_add(1);
    }

    /// The turn ended at cycle `now`. One that never said it began counts
    /// nothing.
    pub fn ended(&mut self, counter: &ProfileCounter, now: u64) {
        let Some(began) = self.began.take() else {
            return;
        };
        let cycles = u32::try_from(now.wrapping_sub(began)).unwrap_or(u32::MAX);
        let Self {
            frames,
            accepted,
            notices,
            requests,
            ..
        } = *self;
        let variables = [1, frames, accepted, notices, requests, cycles];
        let mut index = word::MOMENTS;
        for (i, left) in variables.iter().enumerate() {
            for right in variables.iter().skip(i) {
                counter.add(index, u64::from(*left).wrapping_mul(u64::from(*right)));
                index = index.saturating_add(1);
            }
        }
        let cycles = u64::from(cycles);
        let squared = cycles.wrapping_mul(cycles);
        if frames == 0 && accepted == 0 && notices == 0 && requests == 0 {
            counter.add(word::EMPTY, 1);
            counter.add(word::EMPTY.saturating_add(1), cycles);
            counter.add(word::EMPTY.saturating_add(2), squared);
        }
        counter.add(
            word::CYCLES.saturating_add(cycle_bin(u32::try_from(cycles).unwrap_or(u32::MAX))),
            1,
        );
        let cell = |first: usize, cell: usize, third: u32| {
            let first = first.saturating_add(cell.saturating_mul(3));
            counter.add(first, 1);
            counter.add(first.saturating_add(1), cycles);
            counter.add(first.saturating_add(2), u64::from(third));
        };
        let items = |count: u32| usize::try_from(count.min(ITEM_CELLS as u32)).unwrap_or(0);
        match (frames, accepted, notices) {
            (1.., 0, 0) => cell(
                word::RECEIVE_ONLY,
                items(frames).saturating_sub(1),
                requests,
            ),
            (0, 1.., 0) => cell(
                word::TRANSMIT_ONLY,
                items(accepted).saturating_sub(1),
                requests,
            ),
            (0, 0, 1..) => cell(word::NOTICE_ONLY, notice_cell(notices), notices),
            _ => {}
        }
        let notice_bin = if notices == 0 {
            0
        } else {
            notice_cell(notices).saturating_add(1)
        };
        counter.add(word::NOTICES.saturating_add(notice_bin), 1);
        let frame_bin = usize::try_from(frames.min(FRAME_BINS as u32 - 1)).unwrap_or(0);
        counter.add(word::FRAMES.saturating_add(frame_bin), 1);
        if notices != 0 {
            counter.add(word::SETTLING, 1);
            counter.add(word::SETTLING.saturating_add(1), u64::from(accepted != 0));
            counter.add(word::SETTLING.saturating_add(2), u64::from(accepted));
        }
        self.ended = Some((now, u32::try_from(cycles).unwrap_or(u32::MAX)));
        self.gap = None;
    }

    /// The pass after the last turn yielded to its executor.
    pub fn yielded(&mut self) {
        self.gap = Some(Gap::Yielded);
    }

    /// The pass after the last turn waited, `ready` when something was there
    /// as it registered: that turn moved nothing.
    pub fn waited(&mut self, counter: &ProfileCounter, ready: bool) {
        self.gap = Some(if ready { Gap::Ready } else { Gap::Pending });
        if let Some((_, cycles)) = self.ended {
            counter.add(word::IDLE, 1);
            counter.add(word::IDLE.saturating_add(1), u64::from(cycles));
        }
    }
}

/// The bins of [`Returns::queued_by_burst`]: bursts of 1, 2, 3 to 4, 5 to
/// 8, 9 to 12, 13 to 16, and 17 or more memories.
pub const BURST_BINS: usize = 7;
/// The bins of [`Returns::before_by_burst`] and [`Returns::after_by_burst`]:
/// a queue holding 0, 1 to 4, 5 to 8, 9 to 12, 13 to 15, and 16 or more.
pub const OCCUPANCY_BINS: usize = 6;

/// The station's bursts of returned memory since boot: each ended by one
/// flush of the return queue.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Returns {
    /// Bursts: flushes of the return queue.
    pub bursts: u32,
    /// Memories the bursts queued.
    pub queued: u32,
    pub queued_by_burst: [u32; BURST_BINS],
    /// The queue's occupancy before and after each burst.
    pub before_by_burst: [u32; OCCUPANCY_BINS],
    pub after_by_burst: [u32; OCCUPANCY_BINS],
    /// Bursts after which memory stayed with the actor for want of room, and
    /// those memories, summed over the bursts.
    pub held_bursts: u32,
    pub held: u32,
}

/// The words of [`ReturnCounter`].
const RETURN_WORDS: usize = 4 + BURST_BINS + 2 * OCCUPANCY_BINS;

impl Returns {
    /// The counts between readings `start` and `end`, wrapping at 2^32.
    pub fn window(start: &Self, end: &Self) -> Self {
        let (start, end) = (start.words(), end.words());
        let mut words = [0; RETURN_WORDS];
        for ((place, end), start) in words.iter_mut().zip(end).zip(start) {
            *place = end.wrapping_sub(start);
        }
        Self::from_words(&words)
    }

    fn words(&self) -> [u32; RETURN_WORDS] {
        let mut words = [0; RETURN_WORDS];
        let fields = [self.bursts, self.queued, self.held_bursts, self.held]
            .into_iter()
            .chain(self.queued_by_burst)
            .chain(self.before_by_burst)
            .chain(self.after_by_burst);
        for (place, value) in words.iter_mut().zip(fields) {
            *place = value;
        }
        words
    }

    fn from_words(words: &[u32; RETURN_WORDS]) -> Self {
        let at = |index: usize| words.get(index).copied().unwrap_or(0);
        Self {
            bursts: at(0),
            queued: at(1),
            held_bursts: at(2),
            held: at(3),
            queued_by_burst: core::array::from_fn(|bin| at(4usize.saturating_add(bin))),
            before_by_burst: core::array::from_fn(|bin| at((4 + BURST_BINS).saturating_add(bin))),
            after_by_burst: core::array::from_fn(|bin| {
                at((4 + BURST_BINS + OCCUPANCY_BINS).saturating_add(bin))
            }),
        }
    }
}

/// The bin of a burst of `queued` memories, one or more.
const fn burst_bin(queued: u8) -> usize {
    match queued {
        0..=1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=8 => 3,
        9..=12 => 4,
        13..=16 => 5,
        _ => 6,
    }
}

/// The bin of a queue holding `held`.
const fn occupancy_bin(held: u8) -> usize {
    match held {
        0 => 0,
        1..=4 => 1,
        5..=8 => 2,
        9..=12 => 3,
        13..=15 => 4,
        _ => 5,
    }
}

/// The station's return bursts, counted word by word; the datapath actor is
/// their one writer, and a reader takes each word on its own.
pub struct ReturnCounter {
    counts: [AtomicU32; RETURN_WORDS],
}

impl ReturnCounter {
    pub const fn new() -> Self {
        Self {
            counts: [const { AtomicU32::new(0) }; RETURN_WORDS],
        }
    }

    fn bump(&self, index: usize, by: u32) {
        if let Some(count) = self.counts.get(index) {
            super::bump(count, by);
        }
    }

    /// One burst queued `queued` memories into a queue that held `before`
    /// and then `after`, and left `held` with the actor.
    pub fn burst(&self, queued: u8, before: u8, after: u8, held: u8) {
        self.bump(0, 1);
        self.bump(1, u32::from(queued));
        self.bump(2, u32::from(held != 0));
        self.bump(3, u32::from(held));
        self.bump(4usize.saturating_add(burst_bin(queued)), 1);
        self.bump((4 + BURST_BINS).saturating_add(occupancy_bin(before)), 1);
        self.bump(
            (4 + BURST_BINS + OCCUPANCY_BINS).saturating_add(occupancy_bin(after)),
            1,
        );
    }

    /// The counts now.
    pub fn read(&self) -> Returns {
        let words: [u32; RETURN_WORDS] = core::array::from_fn(|index| {
            self.counts
                .get(index)
                .map_or(0, |count| count.load(Ordering::Relaxed))
        });
        Returns::from_words(&words)
    }
}

impl Default for ReturnCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

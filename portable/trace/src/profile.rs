//! Program-counter samples and the buffer that keeps a window's samples.
//!
//! A sampler interrupts a hart on a fixed schedule and reports, for each
//! scheduled tick, a [`PcSample`]: the tick's number from the window's
//! first, the interrupted program counter and the interrupted level. An
//! image's observer keeps them in a [`Thinning`] buffer, which covers a
//! window of any length with its fixed number of samples.

use crate::event::{Channel, Domain, Event, Kind};

/// One scheduled tick of a sampler: where the hart was when it fired.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PcSample {
    tick: u32,
    pc: u32,
    level: u8,
}

impl PcSample {
    /// The last tick a sample encodes: 2^24 - 1, some 70 minutes at
    /// 250 microseconds a tick.
    pub const MAX_TICK: u32 = 0x00ff_ffff;

    /// A sample of tick `tick`, interrupted program counter `pc` and the
    /// interrupted code's raw level `level`; `None` past [`Self::MAX_TICK`].
    pub const fn new(tick: u32, pc: u32, level: u8) -> Option<Self> {
        if tick > Self::MAX_TICK {
            return None;
        }
        Some(Self { tick, pc, level })
    }

    pub const fn tick(&self) -> u32 {
        self.tick
    }

    pub const fn pc(&self) -> u32 {
        self.pc
    }

    pub const fn level(&self) -> u8 {
        self.level
    }
}

impl Event for PcSample {
    const KIND: Kind = Kind::new(Domain::Platform, 43);
    /// The platform's profile channel.
    const CHANNEL: Channel = match Channel::new(Domain::Platform, 6) {
        Ok(channel) => channel,
        Err(_) => panic!("the profile channel is in the platform range"),
    };

    fn encode(&self) -> [u32; 2] {
        // `new` bounds the tick to the word's upper 24 bits.
        [self.pc, self.tick.wrapping_shl(8) | u32::from(self.level)]
    }

    fn decode([pc, mark]: [u32; 2]) -> Option<Self> {
        let [level, ..] = mark.to_le_bytes();
        Some(Self {
            tick: tick_of(mark),
            pc,
            level,
        })
    }
}

/// The tick an encoded sample's second word holds.
const fn tick_of(mark: u32) -> u32 {
    mark.wrapping_shr(8)
}

/// A window's samples in `N` slots by stride doubling.
///
/// The buffer keeps every `stride`-th tick from the window's first. Once
/// full, the stride doubles and each new kept tick overwrites the next
/// sample the new stride drops; no tick does work beyond its own sample
/// (the search for the next dropped sample checks at most a dozen slots at
/// 2048, two in the mean). Until the new stride's ticks have replaced all
/// it drops, the buffer mixes both strides, so [`normalize`](Self::normalize)
/// runs once the window ended: it keeps only the ticks of one stride, which
/// then cover the window evenly.
pub struct Thinning<const N: usize> {
    period_us: u32,
    ticks: u32,
    stride: u32,
    /// Slots written, up to `N`.
    kept: usize,
    /// Once full, where the search for the next dropped sample resumes,
    /// and how many the stride still drops.
    cursor: usize,
    dropped: usize,
    samples: [[u32; 2]; N],
}

impl<const N: usize> Thinning<N> {
    const GEOMETRY: () = {
        if !N.is_power_of_two() || N < 2 {
            panic!("stride doubling halves a full buffer");
        }
    };

    /// The samples a doubled stride drops from a full buffer.
    const HALF: usize = N.wrapping_shr(1);

    pub const fn new() -> Self {
        let () = Self::GEOMETRY;
        Self {
            period_us: 0,
            ticks: 0,
            stride: 1,
            kept: 0,
            cursor: 0,
            dropped: 0,
            samples: [[0; 2]; N],
        }
    }

    /// Forget the last window for one sampled every `period_us`.
    pub fn restart(&mut self, period_us: u32) {
        self.period_us = period_us;
        self.ticks = 0;
        self.stride = 1;
        self.kept = 0;
        self.cursor = 0;
        self.dropped = 0;
    }

    /// Keep `sample` when its tick is one of the stride's.
    pub fn record(&mut self, sample: PcSample) {
        self.ticks = self.ticks.max(sample.tick.saturating_add(1));
        if sample.tick.checked_rem(self.stride) != Some(0) {
            return;
        }
        let slot = if self.kept < N {
            let slot = self.kept;
            self.kept = slot.saturating_add(1);
            slot
        } else {
            let Some(slot) = self.next_dropped() else {
                return;
            };
            self.cursor = slot.saturating_add(1);
            self.dropped = self.dropped.saturating_sub(1);
            slot
        };
        if let Some(stored) = self.samples.get_mut(slot) {
            *stored = sample.encode();
        }
        if self.kept == N && self.dropped == 0 {
            // Full of every stride-th tick: half of them are every new
            // stride-th. 2^24 ticks bound the stride well below overflow.
            self.stride = self.stride.saturating_mul(2);
            self.cursor = 0;
            self.dropped = Self::HALF;
        }
    }

    /// The first slot from the cursor whose tick the stride drops.
    fn next_dropped(&self) -> Option<usize> {
        let stride = self.stride;
        let offset = self
            .samples
            .get(self.cursor..)?
            .iter()
            .position(|&[_, mark]| tick_of(mark).checked_rem(stride) != Some(0))?;
        self.cursor.checked_add(offset)
    }

    /// End the window: keep only the ticks of one stride, each of them up
    /// to the last tick. A buffer that never filled keeps every tick; one
    /// whose new stride replaced nothing yet keeps the stride before it.
    /// Recording afterwards needs a [`restart`](Self::restart).
    pub fn normalize(&mut self) {
        if self.kept < N {
            return;
        }
        if self.dropped == Self::HALF {
            self.stride = self.stride.wrapping_shr(1).max(1);
        } else {
            let stride = self.stride;
            let mut kept = 0;
            for read in 0..self.kept {
                let Some(&sample) = self.samples.get(read) else {
                    break;
                };
                let [_, mark] = sample;
                if tick_of(mark).checked_rem(stride) == Some(0) {
                    if let Some(slot) = self.samples.get_mut(kept) {
                        *slot = sample;
                    }
                    kept = kept.saturating_add(1);
                }
            }
            self.kept = kept;
        }
        self.cursor = 0;
        self.dropped = 0;
    }

    /// The sampling period.
    pub fn period_us(&self) -> u32 {
        self.period_us
    }

    /// The ticks the window held.
    pub fn ticks(&self) -> u32 {
        self.ticks
    }

    /// Every how many ticks a sample is kept.
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// The samples kept.
    pub fn len(&self) -> usize {
        self.kept
    }

    pub fn is_empty(&self) -> bool {
        self.kept == 0
    }

    /// The kept sample in slot `slot`.
    pub fn get(&self, slot: usize) -> Option<PcSample> {
        if slot >= self.kept {
            return None;
        }
        PcSample::decode(*self.samples.get(slot)?)
    }
}

impl<const N: usize> Default for Thinning<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::disallowed_methods,
    clippy::disallowed_macros
)]
mod tests {
    extern crate std;
    use super::*;
    use std::{boxed::Box, vec::Vec};

    const N: usize = 2048;

    fn recorded(ticks: u32) -> Box<Thinning<N>> {
        let mut buffer = Box::new(Thinning::<N>::new());
        buffer.restart(1000);
        for tick in 0..ticks {
            let sample = PcSample::new(tick, 0x4000_0000 + tick * 4, (tick % 3) as u8).unwrap();
            buffer.record(sample);
        }
        buffer
    }

    fn kept(buffer: &Thinning<N>) -> Vec<u32> {
        let mut ticks: Vec<u32> = (0..buffer.len())
            .map(|slot| buffer.get(slot).unwrap().tick())
            .collect();
        ticks.sort_unstable();
        ticks
    }

    #[test]
    fn a_sample_round_trips_through_its_event_words() {
        let sample = PcSample::new(PcSample::MAX_TICK, u32::MAX, 0x7f).unwrap();
        assert_eq!(PcSample::decode(sample.encode()), Some(sample));
        assert_eq!(PcSample::new(PcSample::MAX_TICK + 1, 0, 0), None);
    }

    #[test]
    fn a_buffer_that_never_fills_keeps_every_tick() {
        let mut buffer = recorded(100);
        buffer.normalize();
        assert_eq!(kept(&buffer), (0..100).collect::<Vec<_>>());
        assert_eq!((buffer.ticks(), buffer.stride()), (100, 1));
        let sample = buffer.get(1).unwrap();
        assert_eq!((sample.pc(), sample.level()), (0x4000_0004, 1));
        assert_eq!(buffer.get(100), None);
    }

    #[test]
    fn a_normalized_window_keeps_one_stride_up_to_its_last_tick() {
        let n = N as u32;
        for ticks in [n, n + 1, 2 * n - 2, 2 * n, 5000, 4 * n + n / 2 + 3, 8 * n] {
            let mut buffer = recorded(ticks);
            buffer.normalize();
            let stride = buffer.stride();
            assert_eq!(
                kept(&buffer),
                (0..ticks).step_by(stride as usize).collect::<Vec<_>>(),
                "{ticks} ticks"
            );
            assert!(buffer.len() > N / 4 && buffer.len() <= N, "{ticks} ticks");
        }
    }

    #[test]
    fn equal_parts_of_a_window_keep_equal_samples() {
        // 5000 ticks: before normalizing, the quarters keep 25 %, 25 %,
        // 30.5 % and 19.5 % of the samples.
        let mut buffer = recorded(5000);
        let quarters = |buffer: &Thinning<N>| {
            let mut counts = [0u32; 4];
            for tick in kept(buffer) {
                counts[(tick / 1250) as usize] += 1;
            }
            counts
        };
        assert_ne!(quarters(&buffer), [quarters(&buffer)[0]; 4]);
        buffer.normalize();
        let counts = quarters(&buffer);
        assert!(
            counts.iter().all(|count| count.abs_diff(counts[0]) <= 1),
            "{counts:?}"
        );
    }

    #[test]
    fn each_sample_keeps_its_own_pc_and_level() {
        let buffer = recorded(N as u32 * 3);
        for slot in 0..buffer.len() {
            let sample = buffer.get(slot).unwrap();
            assert_eq!(sample.pc(), 0x4000_0000 + sample.tick() * 4);
            assert_eq!(u32::from(sample.level()), sample.tick() % 3);
        }
    }

    #[test]
    fn a_restart_forgets_the_last_window() {
        let mut buffer = recorded(N as u32 * 2);
        buffer.restart(250);
        assert_eq!(
            (
                buffer.period_us(),
                buffer.ticks(),
                buffer.stride(),
                buffer.len()
            ),
            (250, 0, 1, 0)
        );
    }
}

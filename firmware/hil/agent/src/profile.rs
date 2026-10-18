//! A statistical program-counter profile of both harts, independent of the
//! chip.
//!
//! The host arms the profile ([`Profiler::arm`]); the workload opens and
//! closes its measured window ([`Profiler::window_begin`],
//! [`Profiler::window_end`]); and the chip's sampling interrupts, driven by
//! its [`ProfileTimer`], record each hart's interrupted program counter and
//! return address ([`Profiler::record`]) while the profile is armed and the
//! window is open. Each hart's samples have one writer, its own sampling
//! handler, which publishes a sample by advancing the hart's fill count
//! after writing it; readers acquire that count. A closed window is paged
//! out raw ([`Profiler::page`]); an open one has no pages, so the host never
//! reads half a window.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use oer_hil_protocol::{
    telemetry::PROFILE_SAMPLE_PAGE, telemetry::ProfileHarts, telemetry::ProfileSamplesPage,
    telemetry::ProfileStatus,
};

/// The chip's sampling timer. Its handlers call [`Profiler::record`] for
/// each hart the profile samples.
pub trait ProfileTimer {
    /// Start sampling every `period_us` microseconds.
    fn start(&mut self, period_us: u32);
    /// Stop sampling.
    fn stop(&mut self);
}

/// Why a closed window has no page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageRefusal {
    /// The window is still open: its samples are not complete.
    WindowOpen,
    /// No such hart.
    Hart,
}

/// The samples of up to `N` per hart and the profile's state.
#[derive(bytemuck::Zeroable)]
pub struct Profiler<const N: usize> {
    pc: [[AtomicU32; N]; 2],
    ra: [[AtomicU32; N]; 2],
    filled: [AtomicU32; 2],
    overflow: [AtomicU32; 2],
    armed: AtomicBool,
    open: AtomicBool,
    harts: AtomicU8,
    period_us: AtomicU32,
    window_start_us: AtomicU32,
    window_us: AtomicU32,
}

impl<const N: usize> Default for Profiler<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Profiler<N> {
    pub const fn new() -> Self {
        Self {
            pc: [const { [const { AtomicU32::new(0) }; N] }; 2],
            ra: [const { [const { AtomicU32::new(0) }; N] }; 2],
            filled: [const { AtomicU32::new(0) }; 2],
            overflow: [const { AtomicU32::new(0) }; 2],
            armed: AtomicBool::new(false),
            open: AtomicBool::new(false),
            harts: AtomicU8::new(0),
            period_us: AtomicU32::new(0),
            window_start_us: AtomicU32::new(0),
            window_us: AtomicU32::new(0),
        }
    }

    /// Discard the previous profile and sample the next window of `harts`
    /// every `period_us`. The caller starts its [`ProfileTimer`] after this.
    pub fn arm(&self, harts: ProfileHarts, period_us: u32) {
        // A handler of the previous profile stops recording before the
        // samples are cleared, so no stale sample lands in a cleared slot.
        self.armed.store(false, Ordering::Release);
        self.open.store(false, Ordering::Release);
        self.clear();
        self.window_us.store(0, Ordering::Relaxed);
        self.harts.store(encode(harts), Ordering::Relaxed);
        self.period_us.store(period_us, Ordering::Relaxed);
        self.armed.store(true, Ordering::Release);
    }

    /// Stop recording. The caller stops its [`ProfileTimer`] after this.
    pub fn disarm(&self) {
        self.armed.store(false, Ordering::Release);
    }

    /// The workload's measured window begins at `now_us`. A window begun
    /// again without an end starts over, so repeated phases never merge.
    pub fn window_begin(&self, now_us: u32) {
        self.open.store(false, Ordering::Release);
        self.clear();
        self.window_start_us.store(now_us, Ordering::Relaxed);
        self.window_us.store(0, Ordering::Relaxed);
        self.open.store(true, Ordering::Release);
    }

    /// The workload's measured window ends at `now_us`.
    pub fn window_end(&self, now_us: u32) {
        if self.open.swap(false, Ordering::AcqRel) {
            let start = self.window_start_us.load(Ordering::Relaxed);
            self.window_us
                .store(now_us.wrapping_sub(start), Ordering::Relaxed);
        }
    }

    /// Whether the sampling handler of `hart` should record now.
    pub fn samples_wanted(&self, hart: usize) -> bool {
        self.armed.load(Ordering::Acquire)
            && self.open.load(Ordering::Acquire)
            && decode(self.harts.load(Ordering::Relaxed)).includes(hart)
    }

    /// Record one sample of `hart`; called only by that hart's sampling
    /// handler, the fill count's single writer.
    pub fn record(&self, hart: usize, pc: u32, ra: u32) {
        if hart >= 2 || !self.samples_wanted(hart) {
            return;
        }
        let index = self.filled[hart].load(Ordering::Relaxed) as usize;
        if index >= N {
            self.overflow[hart].fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.pc[hart][index].store(pc, Ordering::Relaxed);
        self.ra[hart][index].store(ra, Ordering::Relaxed);
        self.filled[hart].store(index as u32 + 1, Ordering::Release);
    }

    pub fn status(&self) -> ProfileStatus {
        ProfileStatus {
            armed: self.armed.load(Ordering::Acquire),
            open: self.open.load(Ordering::Acquire),
            harts: decode(self.harts.load(Ordering::Relaxed)),
            period_us: self.period_us.load(Ordering::Relaxed),
            window_us: self.window_us.load(Ordering::Relaxed),
            capacity: N as u32,
            samples: [0, 1].map(|hart| self.filled[hart].load(Ordering::Acquire)),
            overflow: [0, 1].map(|hart| self.overflow[hart].load(Ordering::Relaxed)),
        }
    }

    /// The closed window's samples of `hart` from `first`, one page.
    pub fn page(&self, hart: usize, first: u32) -> Result<ProfileSamplesPage, PageRefusal> {
        if hart >= 2 {
            return Err(PageRefusal::Hart);
        }
        if self.open.load(Ordering::Acquire) {
            return Err(PageRefusal::WindowOpen);
        }
        let total = self.filled[hart].load(Ordering::Acquire);
        let mut samples = heapless::Vec::new();
        for index in (first..total).take(PROFILE_SAMPLE_PAGE) {
            let index = index as usize;
            let _ = samples.push((
                self.pc[hart][index].load(Ordering::Relaxed),
                self.ra[hart][index].load(Ordering::Relaxed),
            ));
        }
        Ok(ProfileSamplesPage {
            hart: hart as u8,
            first,
            total,
            samples,
        })
    }

    fn clear(&self) {
        for hart in 0..2 {
            self.filled[hart].store(0, Ordering::Release);
            self.overflow[hart].store(0, Ordering::Relaxed);
        }
    }
}

const fn encode(harts: ProfileHarts) -> u8 {
    match harts {
        ProfileHarts::Both => 0,
        ProfileHarts::Core0 => 1,
        ProfileHarts::Core1 => 2,
    }
}

const fn decode(value: u8) -> ProfileHarts {
    match value {
        1 => ProfileHarts::Core0,
        2 => ProfileHarts::Core1,
        _ => ProfileHarts::Both,
    }
}

#[cfg(test)]
mod tests;

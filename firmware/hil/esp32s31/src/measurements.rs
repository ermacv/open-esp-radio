//! Image-owned information for workload reports; no acceptance thresholds.
//! CPU windows must be sampled within 2^31 microseconds. Inclusive executor
//! work intervals overlap when one executor preempts another; never add them
//! to estimate total load. Total busy time is window minus masked WFI time.
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering, fence};
use oer_espressif_executor::observation::{Activity, Phase};
use oer_ieee80211_instrumentation::{Evidence, Observation, ReorderCause};
use oer_trace::{Event, Observer};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SampleError {
    Unmeasured,
    Concurrent,
    Sequence,
    Window,
}

/// One executor's cumulative wrapping microseconds, including open intervals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuSample {
    pub at_micros: u32,
    pub idle_micros: u32,
    pub work_micros: u32,
    pub executor: u8,
    pub hart: u8,
    pub level: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuWindow {
    pub window_micros: u32,
    pub busy_micros: Option<u32>,
    pub work_inclusive_micros: u32,
}
impl CpuSample {
    pub fn until(self, end: Self) -> Result<CpuWindow, SampleError> {
        let window = end.at_micros.wrapping_sub(self.at_micros);
        if window == 0
            || window >= 1 << 31
            || self.executor != end.executor
            || self.hart != end.hart
            || self.level != end.level
        {
            return Err(SampleError::Window);
        }
        let idle = end.idle_micros.wrapping_sub(self.idle_micros);
        let work = end.work_micros.wrapping_sub(self.work_micros);
        if work > window {
            return Err(SampleError::Sequence);
        }
        Ok(CpuWindow {
            window_micros: window,
            busy_micros: if self.level == 0 {
                Some(window.checked_sub(idle).ok_or(SampleError::Sequence)?)
            } else {
                None
            },
            work_inclusive_micros: work,
        })
    }
}
impl CpuWindow {
    pub fn busy_per_mille(self) -> Option<u32> {
        u64::from(self.busy_micros?)
            .saturating_mul(1000)
            .checked_div(u64::from(self.window_micros))
            .and_then(|value| u32::try_from(value).ok())
    }
}

/// Single writer: the executor named by each event. Readers never spin or
/// take a task/handler lock; concurrent publication returns a typed refusal.
pub struct CpuCounter {
    sequence: AtomicU32,
    idle: AtomicU32,
    work: AtomicU32,
    start: AtomicU32,
    context: AtomicU32,
    phase: AtomicU32,
    failed: AtomicBool,
}
impl CpuCounter {
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            idle: AtomicU32::new(0),
            work: AtomicU32::new(0),
            start: AtomicU32::new(0),
            context: AtomicU32::new(0),
            phase: AtomicU32::new(0),
            failed: AtomicBool::new(false),
        }
    }
    pub fn observe(&self, event: Activity) {
        self.sequence.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
        let context = u32::from_le_bytes([event.hart, event.level, event.executor, 0]);
        let phase = self.phase.load(Ordering::Relaxed);
        if phase != 0 && context != self.context.load(Ordering::Relaxed) {
            self.failed.store(true, Ordering::Relaxed);
        }
        let (expected, next, total) = match event.phase {
            Phase::WorkStart => (1, 2, None),
            Phase::WorkEnd => (2, 1, Some(&self.work)),
            Phase::IdleStart => (1, 3, None),
            Phase::IdleEnd => (3, 1, Some(&self.idle)),
        };
        // The first boundary establishes context; every later one is paired.
        if phase != expected
            && !(phase == 0 && matches!(event.phase, Phase::WorkStart | Phase::IdleStart))
        {
            self.failed.store(true, Ordering::Relaxed);
        }
        if let Some(total) = total {
            total.fetch_add(
                event
                    .at_micros
                    .wrapping_sub(self.start.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        } else {
            self.start.store(event.at_micros, Ordering::Relaxed);
        }
        self.context.store(context, Ordering::Relaxed);
        self.phase.store(next, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }
    pub fn sample(&self, now: u32) -> Result<CpuSample, SampleError> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return Err(SampleError::Concurrent);
        }
        let phase = self.phase.load(Ordering::Relaxed);
        let start = self.start.load(Ordering::Relaxed);
        let elapsed = now.wrapping_sub(start);
        let idle = self.idle.load(Ordering::Relaxed);
        let work = self.work.load(Ordering::Relaxed);
        let [hart, level, executor, _] = self.context.load(Ordering::Relaxed).to_le_bytes();
        let failed = self.failed.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != sequence {
            return Err(SampleError::Concurrent);
        }
        if phase == 0 {
            return Err(SampleError::Unmeasured);
        }
        if failed {
            return Err(SampleError::Sequence);
        }
        if (phase == 2 || phase == 3) && elapsed >= 1 << 31 {
            return Err(SampleError::Window);
        }
        Ok(CpuSample {
            at_micros: now,
            idle_micros: idle.wrapping_add(if phase == 3 { elapsed } else { 0 }),
            work_micros: work.wrapping_add(if phase == 2 { elapsed } else { 0 }),
            executor,
            hart,
            level,
        })
    }
}
impl Default for CpuCounter {
    fn default() -> Self {
        Self::new()
    }
}
pub static CPU: [CpuCounter; 4] = [const { CpuCounter::new() }; 4];
/// Selected by this image's task sets. It owns no executor or register owner.
pub struct CpuObserver;
impl Observer for CpuObserver {
    fn event<E: Event + Copy + 'static>(&self, event: E) {
        if let Some(event) = Activity::of(&event)
            && let Some(counter) = CPU.get(usize::from(event.executor))
        {
            counter.observe(event);
        }
    }
}

pub struct RadioCounters {
    pub ampdu_mpdus: AtomicU32,
    pub ampdu_normalized_bytes: AtomicU32,
    pub s_mpdus: AtomicU32,
    pub containment_unknown: AtomicU32,
    pub held: AtomicU32,
    pub released: AtomicU32,
    pub timeout_released: AtomicU32,
    pub skipped: AtomicU32,
    pub duplicates: AtomicU32,
    pub rejected: AtomicU32,
    pub cleared: AtomicU32,
    pub ba_started: AtomicU32,
    pub ba_stopped: AtomicU32,
    pub rate_samples: AtomicU32,
    pub selected_kbps: AtomicU32,
    pub rate_known: AtomicBool,
    pub incomplete: AtomicBool,
}
impl RadioCounters {
    pub const fn new() -> Self {
        Self {
            ampdu_mpdus: AtomicU32::new(0),
            ampdu_normalized_bytes: AtomicU32::new(0),
            s_mpdus: AtomicU32::new(0),
            containment_unknown: AtomicU32::new(0),
            held: AtomicU32::new(0),
            released: AtomicU32::new(0),
            timeout_released: AtomicU32::new(0),
            skipped: AtomicU32::new(0),
            duplicates: AtomicU32::new(0),
            rejected: AtomicU32::new(0),
            cleared: AtomicU32::new(0),
            ba_started: AtomicU32::new(0),
            ba_stopped: AtomicU32::new(0),
            rate_samples: AtomicU32::new(0),
            selected_kbps: AtomicU32::new(0),
            rate_known: AtomicBool::new(false),
            incomplete: AtomicBool::new(false),
        }
    }
    fn add(&self, counter: &AtomicU32, amount: u32) {
        if counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(amount)
            })
            .is_err()
        {
            self.incomplete.store(true, Ordering::Relaxed);
        }
    }
    pub fn observe<E: Event>(&self, event: &E) {
        match Observation::of(event) {
            Some(Observation::RxContainment(event)) => {
                if matches!(event.ampdu, Evidence::HardwareYes | Evidence::ProtocolYes) {
                    self.add(&self.ampdu_mpdus, 1);
                    match event.bytes {
                        Some(bytes) => self.add(&self.ampdu_normalized_bytes, bytes),
                        None => self.incomplete.store(true, Ordering::Relaxed),
                    }
                } else if event.ampdu == Evidence::Unavailable {
                    self.add(&self.containment_unknown, 1);
                }
                if matches!(event.s_mpdu, Evidence::HardwareYes | Evidence::ProtocolYes) {
                    self.add(&self.s_mpdus, 1);
                }
            }
            Some(Observation::RxReorder(event)) => {
                self.add(&self.held, u32::from(event.held));
                self.add(&self.skipped, u32::from(event.missing));
                if event.cause == ReorderCause::Clear {
                    self.add(&self.cleared, u32::from(event.released));
                } else {
                    self.add(&self.released, u32::from(event.released));
                }
                match event.cause {
                    ReorderCause::Timeout => {
                        self.add(&self.timeout_released, u32::from(event.released))
                    }
                    ReorderCause::Duplicate => self.add(&self.duplicates, 1),
                    ReorderCause::Rejected => self.add(&self.rejected, 1),
                    _ => {}
                }
            }
            Some(Observation::RxBaStarted(_)) => self.add(&self.ba_started, 1),
            Some(Observation::RxBaStopped(_)) => self.add(&self.ba_stopped, 1),
            Some(Observation::TxRateSelected(event)) => {
                self.add(&self.rate_samples, 1);
                self.rate_known
                    .store(event.kbps.is_some(), Ordering::Relaxed);
                if let Some(kbps) = event.kbps {
                    self.selected_kbps.store(kbps, Ordering::Relaxed);
                }
            }
            _ => {}
        }
    }
}
impl Default for RadioCounters {
    fn default() -> Self {
        Self::new()
    }
}
pub static RADIO: RadioCounters = RadioCounters::new();

#[cfg(test)]
mod tests;

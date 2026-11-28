//! The registration's profile (`tasks/448`): a session of the PC sampler on
//! TIMG1 T0 at the profile level, and the L1 cache's counters of
//! instruction bus 0 and data bus 0, both started right before the PHY's
//! registration and stopped right after it. The ordinary build samples the
//! program counter and its level alone.
//!
//! The handler keeps each tick's sample in the one buffer and adds its own
//! entries and cycles; the radio level's task reads them only after the
//! session was handed back.

use core::sync::atomic::{AtomicU32, Ordering};

use oer_chips_esp32s31_hal::peripherals::{
    cache_counters::{Bus, Buses, CacheCounters, Snapshot},
    timg1_alarm::Timg1AlarmInterrupt,
};
use oer_chips_esp32s31_platform_runtime::sampler::{self, Storage, Timer};
use oer_hil_services_radio_pair as service;
use oer_portable_memory::ClaimCell;
use oer_portable_trace::profile::Thinning;
use static_cell::ConstStaticCell;

use crate::image::SamplerToken;

/// The sampling period: 100 µs.
pub(crate) const PERIOD_US: u32 = 100;
/// The samples kept: 2 048 cover the 1.1 s at about 540 µs a sample.
const SAMPLES: usize = 2048;

type Kept = ClaimCell<&'static mut Thinning<SAMPLES>>;

static SAMPLER: Storage<SamplerToken, oer_portable_trace::Noop> = Storage::new();
static BUFFER: ConstStaticCell<Thinning<SAMPLES>> = ConstStaticCell::new(Thinning::new());
static KEPT: Kept = ClaimCell::new();
/// The handler's entries and its cycles inside them: its one writer is the
/// handler, read once the session ended.
static ENTRIES: AtomicU32 = AtomicU32::new(0);
static CYCLES: AtomicU32 = AtomicU32::new(0);

/// The sampler's interrupt entry (TIMG1 T0).
pub(crate) fn on_sampler(part: &mut Timg1AlarmInterrupt) {
    let began = oer_portable_riscv::cycle64();
    SAMPLER.interrupt_into(part, |tick| {
        let _ = KEPT.lend(|buffer| buffer.record(tick.sample));
    });
    let spent = oer_portable_riscv::cycle64().wrapping_sub(began);
    ENTRIES.store(
        ENTRIES.load(Ordering::Relaxed).wrapping_add(1),
        Ordering::Relaxed,
    );
    CYCLES.store(
        CYCLES
            .load(Ordering::Relaxed)
            .saturating_add(u32::try_from(spent).unwrap_or(u32::MAX)),
        Ordering::Relaxed,
    );
}

/// The registration's instruments.
pub(crate) struct Profiler {
    sampler: sampler::Sampler<Timer<SamplerToken>>,
    cache: CacheCounters,
    state: State,
}

/// Where the profile is.
#[derive(Clone, Copy)]
enum State {
    /// No session ran yet.
    Idle,
    /// The session runs or was asked to stop; the cache's counts at the
    /// registration's end.
    Sampling(Snapshot),
    /// The session did not start.
    NotStarted,
    /// The session was handed back: its summary.
    Ended(service::ProfileSummary),
    /// The handler did not hand it back.
    Lost,
}

impl Profiler {
    /// Stop and mask TIMG1 T0, bind its handler on the boot hart and
    /// install the buffer; `None` when the storage was taken before.
    pub(crate) fn new<const H: u8>(
        timg1: oer_chips_esp32s31_hal::peripherals::timg1_alarm::Timg1AlarmParts,
        cache: CacheCounters,
        router: &mut oer_chips_family_platform_interrupt_table::Router<
            impl oer_chips_family_platform_interrupt_table::Controller,
            H,
            crate::image::InterruptTable,
        >,
        token: SamplerToken,
    ) -> Option<Self> {
        let sampler = SAMPLER.init(timg1, router, token)?;
        let buffer = BUFFER.try_take()?;
        KEPT.install(buffer).ok()?;
        Some(Self {
            sampler,
            cache,
            state: State::Idle,
        })
    }

    /// Take the session back and normalize the buffer to one stride.
    pub(crate) async fn end(&mut self, cycles_per_micro: u32) {
        let State::Sampling(cache) = self.state else {
            return;
        };
        self.state = match self.sampler.stop(PERIOD_US).await {
            Ok(Some(ended)) => {
                let kept = KEPT.lend(|buffer| {
                    buffer.normalize();
                    (buffer.stride(), buffer.len())
                });
                match kept {
                    Ok((stride, kept)) => State::Ended(service::ProfileSummary {
                        period_us: PERIOD_US,
                        ticks: ended.ticks,
                        stride,
                        kept: u16::try_from(kept).unwrap_or(u16::MAX),
                        stray: self.sampler.shared().stray(),
                        cut: ended.cut,
                        handler_entries: ENTRIES.load(Ordering::Relaxed),
                        handler_cycles: CYCLES.load(Ordering::Relaxed),
                        cycles_per_micro,
                        ibus0: service::BusCounts {
                            hits: cache.ibus0.hits,
                            misses: cache.ibus0.misses,
                            next_level_reads: cache.ibus0.next_level_reads,
                            overflowed: cache.ibus0.overflowed,
                        },
                        dbus0: service::BusCounts {
                            hits: cache.dbus0.hits,
                            misses: cache.dbus0.misses,
                            next_level_reads: cache.dbus0.next_level_reads,
                            overflowed: cache.dbus0.overflowed,
                        },
                    }),
                    Err(_) => State::Lost,
                }
            }
            Ok(None) => State::NotStarted,
            Err(sampler::Lost) => State::Lost,
        };
    }

    /// The page of samples from `from` on.
    pub(crate) fn page(&self, from: u16) -> service::Profiled {
        let State::Ended(summary) = self.state else {
            return service::Profiled::NoProfile;
        };
        let samples = KEPT.lend(|buffer| {
            (usize::from(from)..buffer.len())
                .filter_map(|slot| buffer.get(slot))
                .take(service::PROFILE_PAGE)
                .map(|sample| service::ProfileSample {
                    pc: sample.pc(),
                    level: sample.level(),
                })
                .collect()
        });
        match samples {
            Ok(samples) => service::Profiled::Page { summary, samples },
            Err(_) => service::Profiled::NoProfile,
        }
    }
}

impl crate::radio::AroundRegister for Profiler {
    fn before(&mut self) {
        if !matches!(self.state, State::Idle)
            || KEPT.lend(|buffer| buffer.restart(PERIOD_US)).is_err()
        {
            self.state = State::NotStarted;
            return;
        }
        self.cache
            .start(Buses::of(Bus::Ibus0).union(Buses::of(Bus::Dbus0)));
        self.state = match self.sampler.start_masked(PERIOD_US) {
            Ok(()) => State::Sampling(Snapshot::default()),
            Err(_) => State::NotStarted,
        };
    }

    fn after(&mut self) {
        if let State::Sampling(_) = self.state {
            let cache = self.cache.snapshot();
            self.sampler.request_stop();
            self.state = State::Sampling(cache);
        }
    }
}

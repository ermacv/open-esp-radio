//! The image's PC sampler: `oer_esp32s31_compose::sampler` on TIMG1 T0 at
//! P4, above every other level, reporting through the image's observer
//! into one [`Thinning`] buffer of [`CAPACITY`] samples in SRAM, each with
//! the interrupted `ra` the staged runtime's interrupt entry recorded.
//!
//! The load server starts it at a sampled run's window start and asks it
//! to stop at the window's end; once the handler handed the session back,
//! the buffer is normalized to one stride and served in pages. Only an
//! `oer_trace` build reports samples, so only it serves a profile.

use oer_services_load::{MAX_PAGE_SAMPLES, Sample, Samples};
use oer_trace::profile::Thinning;

/// The samples one run keeps: 24 KiB of SRAM (pc, tick and level, and the
/// return address), which the sampler's handler writes while a session
/// runs.
pub(crate) const CAPACITY: usize = 2048;
const _: () = {
    if CAPACITY > u16::MAX as usize {
        panic!("a page numbers its samples in u16");
    }
};

/// The page of `buffer` from the sample numbered `from`, of a session that
/// reported `ticks` ticks.
pub(crate) fn page(
    buffer: &Thinning<CAPACITY>,
    from: u16,
    ticks: u32,
    stray: u32,
    cut: bool,
) -> Samples {
    let kept = buffer.len();
    let from = usize::from(from).min(kept);
    let to = from.saturating_add(MAX_PAGE_SAMPLES).min(kept);
    Samples {
        period_us: buffer.period_us(),
        ticks,
        stride: buffer.stride(),
        kept: u16::try_from(kept).unwrap_or(u16::MAX),
        stray,
        cut,
        samples: (from..to)
            .filter_map(|slot| Some((buffer.get(slot)?, buffer.return_address(slot))))
            .map(|(sample, ra)| Sample {
                tick: sample.tick(),
                pc: sample.pc(),
                level: sample.level(),
                ra,
            })
            .collect(),
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) use chip::{Profiler, init, interrupt};
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub(crate) use chip::{record, record_return};

#[cfg(target_arch = "riscv32")]
mod chip {
    use super::{CAPACITY, page};
    use oer_esp32s31_compose::sampler::{self, Ended, Storage, Timer};
    use oer_esp32s31_hal::peripherals::timg1_alarm::{Timg1AlarmInterrupt, Timg1AlarmParts};
    use oer_espressif_interrupt_table::{Controller, Router};
    use oer_memory::ClaimCell;
    use oer_services_load::{LoadError, Samples};
    use oer_trace::profile::Thinning;
    use static_cell::ConstStaticCell;

    use crate::SamplerToken;
    use crate::fatal::Failure;
    use crate::load::parts::DriverObserver;

    oer_espressif_staged_runtime::placed_static! {
        static SAMPLER: Storage<SamplerToken, DriverObserver> = Storage::new(), in critical_data "pc_sampler";
        static BUFFER: ConstStaticCell<Thinning<CAPACITY>> = ConstStaticCell::new(Thinning::new()), in critical_data "pc_samples";
        /// The buffer, lent to the observer in the handler while a session
        /// runs and to the load server once it ended.
        static SAMPLES: ClaimCell<&'static mut Thinning<CAPACITY>> = ClaimCell::new(), in critical_data "pc_samples_claim";
    }

    /// Stop and mask TIMG1 T0, bind its handler and install the buffer.
    pub(crate) fn init<const H: u8>(
        parts: Timg1AlarmParts,
        router: &mut Router<impl Controller, H>,
        token: SamplerToken,
    ) -> Profiler {
        let (Some(sampler), Some(buffer)) = (SAMPLER.init(parts, router, token), BUFFER.try_take())
        else {
            crate::fail(Failure::Storage);
        };
        if SAMPLES.install(buffer).is_err() {
            crate::fail(Failure::Storage);
        }
        Profiler {
            sampler,
            period_us: 0,
            ended: None,
        }
    }

    /// The sampler's interrupt entry.
    pub(crate) fn interrupt(part: &mut Timg1AlarmInterrupt) {
        SAMPLER.interrupt(part);
    }

    /// The observer's end: keep `sample`. A buffer lent elsewhere (never
    /// while a session runs) drops it.
    #[cfg(oer_trace)]
    pub(crate) fn record(sample: oer_trace::profile::PcSample) {
        let _ = SAMPLES.lend(|buffer| buffer.record(sample));
    }

    /// The observer's end: give the sample kept last its return address.
    #[cfg(oer_trace)]
    pub(crate) fn record_return(ret: oer_trace::profile::ReturnSample) {
        let _ = SAMPLES.lend(|buffer| buffer.record_return(ret));
    }

    /// The load server's end of the sampler.
    pub(crate) struct Profiler {
        sampler: sampler::Sampler<Timer<SamplerToken>>,
        period_us: u32,
        /// How the last session ended, kept until the next starts.
        ended: Option<Ended>,
    }

    impl Profiler {
        /// Whether a sampled run can start: the image reports samples and
        /// no session runs.
        pub(crate) fn available(&self) -> bool {
            cfg!(oer_trace) && self.sampler.available()
        }

        /// Forget the last run's samples and start sampling every
        /// `period_us`.
        ///
        /// # Errors
        /// [`LoadError::NoSampler`] when the sampler cannot start.
        pub(crate) fn start(&mut self, period_us: u32) -> Result<(), LoadError> {
            if !self.available() || SAMPLES.lend(|buffer| buffer.restart(period_us)).is_err() {
                return Err(LoadError::NoSampler);
            }
            self.ended = None;
            self.period_us = period_us;
            self.sampler
                .start_masked(period_us)
                .map_err(|_| LoadError::NoSampler)
        }

        /// Ask the running session to stop at its next tick.
        pub(crate) fn request_stop(&self) {
            self.sampler.request_stop();
        }

        /// Stop the running session, take it back and normalize its
        /// samples.
        ///
        /// # Errors
        /// [`LoadError::NoSampler`] when the handler kept the session.
        pub(crate) async fn stop(&mut self) -> Result<(), LoadError> {
            let ended = self
                .sampler
                .stop(self.period_us)
                .await
                .map_err(|sampler::Lost| LoadError::NoSampler)?;
            if let Some(ended) = ended {
                let _ = SAMPLES.lend(|buffer| buffer.normalize());
                self.ended = Some(ended);
            }
            Ok(())
        }

        /// The last session's page from the sample numbered `from`.
        pub(crate) fn page(&self, from: u16) -> Option<Samples> {
            let ended = self.ended?;
            let stray = self.sampler.shared().stray();
            SAMPLES
                .lend(|buffer| page(buffer, from, ended.ticks, stray, ended.cut))
                .ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_trace::profile::{PcSample, ReturnSample};

    #[test]
    fn pages_cover_every_kept_sample_once_and_stop_at_the_end() {
        let mut buffer = Box::new(Thinning::<CAPACITY>::new());
        buffer.restart(1000);
        let ticks = CAPACITY as u32 * 3;
        for tick in 0..ticks {
            buffer.record(PcSample::new(tick, 0x4000_0000 + tick, 1).unwrap());
            if tick.is_multiple_of(2) {
                buffer.record_return(ReturnSample::new(tick, 0x4200_0000 + tick).unwrap());
            }
        }
        buffer.normalize();
        let mut seen = Vec::new();
        let mut from = 0;
        loop {
            let page = page(&buffer, from, ticks, 7, false);
            assert_eq!(
                (page.ticks, page.stride, page.kept as usize, page.stray),
                (ticks, buffer.stride(), buffer.len(), 7)
            );
            if page.samples.is_empty() {
                break;
            }
            assert!(page.samples.len() <= MAX_PAGE_SAMPLES);
            for sample in &page.samples {
                let ra = sample
                    .tick
                    .is_multiple_of(2)
                    .then_some(0x4200_0000 + sample.tick);
                assert_eq!(sample.ra, ra, "tick {}", sample.tick);
            }
            seen.extend(page.samples.iter().map(|sample| sample.tick));
            from += page.samples.len() as u16;
        }
        seen.sort_unstable();
        assert_eq!(
            seen,
            (0..ticks)
                .step_by(buffer.stride() as usize)
                .collect::<Vec<_>>()
        );
        assert!(page(&buffer, u16::MAX, ticks, 0, false).samples.is_empty());
    }
}

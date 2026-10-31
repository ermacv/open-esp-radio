//! The PC sampler: where the serving hart spends a run's window.
//!
//! TIMG1 T0, which no executor or product timer uses, interrupts the hart
//! every period at level 4, above every executor and handler of the image,
//! so the handler reads the interrupted code's program counter (`mepc`)
//! and level (`mcause.mpil`) before anything else can trap. Code that runs
//! with the hart's interrupts masked is seen only where it unmasks them.
//!
//! The sampler is armed only for a run that asks for it. Disarmed, its
//! timer is stopped and masked: no tick and no handler entry. Arming moves
//! the timer's program part, its binding and the [`Recording`] to the
//! handler as one session; each tick records and rearms. Stopping asks the
//! handler to stop at its next tick and hand the session back. The handler
//! and the load server share only the session's two one-slot queues and the
//! stop and stray counts, never a lock.

use oer_services_load::{MAX_PAGE_SAMPLES, Sample, Samples};

/// The samples one run keeps: some 18 KiB of SRAM, which the handler alone
/// writes while armed.
pub(crate) const CAPACITY: usize = 2048;
const _: () = {
    if !CAPACITY.is_power_of_two() || CAPACITY > u16::MAX as usize {
        panic!("stride doubling halves a full buffer; a page numbers its samples in u16");
    }
};

/// One run's ticks and the samples it kept, with stride doubling: the
/// ticks a full buffer holds are every `stride`-th from the first; once it
/// is full the stride doubles and each new kept tick overwrites the next
/// sample the new stride drops. The kept samples so cover the whole window
/// evenly, and no tick does work beyond its own sample: the search for the
/// next dropped sample checks at most a dozen samples at this capacity,
/// two in the mean.
pub(crate) struct Recording {
    period_us: u32,
    ticks: u32,
    stride: u32,
    /// Samples written, up to the capacity.
    kept: usize,
    /// Once full, where the search for the next dropped sample resumes,
    /// and how many the current stride still drops.
    cursor: usize,
    dropped: usize,
    pcs: [u32; CAPACITY],
    sample_ticks: [u32; CAPACITY],
    levels: [u8; CAPACITY],
}

impl Recording {
    pub(crate) const fn new() -> Self {
        Self {
            period_us: 0,
            ticks: 0,
            stride: 1,
            kept: 0,
            cursor: 0,
            dropped: 0,
            pcs: [0; CAPACITY],
            sample_ticks: [0; CAPACITY],
            levels: [0; CAPACITY],
        }
    }

    /// Forget the last run's samples for a run sampled every `period_us`.
    pub(crate) fn restart(&mut self, period_us: u32) {
        self.period_us = period_us;
        self.ticks = 0;
        self.stride = 1;
        self.kept = 0;
        self.cursor = 0;
        self.dropped = 0;
    }

    /// One tick: the interrupted code was at `pc` on level `level`. Ticks
    /// past 2^32 are neither counted nor kept.
    pub(crate) fn tick(&mut self, pc: u32, level: u8) {
        let tick = self.ticks;
        let Some(next) = tick.checked_add(1) else {
            return;
        };
        self.ticks = next;
        if tick.checked_rem(self.stride) != Some(0) {
            return;
        }
        let slot = if self.kept < CAPACITY {
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
        if let (Some(pc_slot), Some(tick_slot), Some(level_slot)) = (
            self.pcs.get_mut(slot),
            self.sample_ticks.get_mut(slot),
            self.levels.get_mut(slot),
        ) {
            *pc_slot = pc;
            *tick_slot = tick;
            *level_slot = level;
        }
        let full = self.kept == CAPACITY && self.dropped == 0;
        if full {
            // The buffer holds exactly every stride-th tick from the first:
            // half of them are every new stride-th.
            // 2^32 ticks bound the stride to 2^21 at this capacity.
            self.stride = self.stride.saturating_mul(2);
            self.cursor = 0;
            self.dropped = CAPACITY / 2;
        }
    }

    /// The first sample from the cursor on whose tick the stride drops.
    fn next_dropped(&self) -> Option<usize> {
        let from = self.sample_ticks.get(self.cursor..)?;
        let offset = from
            .iter()
            .position(|tick| tick.checked_rem(self.stride) != Some(0))?;
        self.cursor.checked_add(offset)
    }

    /// The page of samples from the one numbered `from`, with `stray`
    /// ticks that found no run.
    pub(crate) fn page(&self, from: u16, stray: u32) -> Samples {
        let kept = self.kept.min(CAPACITY);
        let from = usize::from(from).min(kept);
        let to = from.saturating_add(MAX_PAGE_SAMPLES).min(kept);
        let samples = (from..to)
            .filter_map(|slot| {
                Some(Sample {
                    tick: *self.sample_ticks.get(slot)?,
                    pc: *self.pcs.get(slot)?,
                    level: *self.levels.get(slot)?,
                })
            })
            .collect();
        Samples {
            period_us: self.period_us,
            ticks: self.ticks,
            stride: self.stride,
            kept: u16::try_from(kept).unwrap_or(u16::MAX),
            stray,
            samples,
        }
    }
}

#[cfg(target_arch = "riscv32")]
pub(crate) use hardware::{Sampler, init, interrupt};

#[cfg(target_arch = "riscv32")]
mod hardware {
    use super::Recording;
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use oer_esp32s31_hal::peripherals::timg1_alarm::{
        Timg1AlarmInterrupt, Timg1AlarmParts, Timg1AlarmProgram,
    };
    use oer_espressif_interrupt_table::{Bound, Controller, Router};
    use oer_memory::{
        AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, CompactSpscCursor, HandlerCell,
    };
    use oer_services_load::{LoadError, Samples};
    use oer_time::{Duration, Instant, Timer as _};
    use static_cell::{ConstStaticCell, StaticCell};

    use crate::SamplerToken;
    use crate::fatal::Failure;

    /// The timer's program part, its binding and the recording: the load
    /// server holds them disarmed, the handler armed.
    struct Session {
        program: Timg1AlarmProgram,
        bound: Bound<SamplerToken>,
        period: Duration,
        recording: &'static mut Recording,
    }

    type Queue = AffineSpscQueue<Session, 1, CompactSpscCursor>;
    type Sender = AffineSpscSender<'static, Session, 1, CompactSpscCursor>;
    type Receiver = AffineSpscReceiver<'static, Session, 1, CompactSpscCursor>;

    /// The session's way to the handler and back.
    struct Cells {
        request: Queue,
        reply: Queue,
    }

    /// The handler's state: the armed session and its ends of the queues.
    struct Handler {
        active: Option<Session>,
        receive: Receiver,
        send: Sender,
    }

    /// Everything the handler reaches, in SRAM.
    struct Storage {
        cells: StaticCell<Cells>,
        state: StaticCell<Handler>,
        handler: HandlerCell<Handler>,
        recording: ConstStaticCell<Recording>,
        /// The load server asks the handler to stop at its next tick.
        stop: AtomicBool,
        /// Ticks that found no session; the handler is the one writer.
        stray: AtomicU32,
    }

    oer_espressif_staged_runtime::placed_static! {
        static SAMPLER: Storage = Storage {
            cells: StaticCell::new(),
            state: StaticCell::new(),
            handler: HandlerCell::new(),
            recording: ConstStaticCell::new(Recording::new()),
            stop: AtomicBool::new(false),
            stray: AtomicU32::new(0),
        }, in critical_data "pc_sampler";
    }

    /// The load server's end of the sampler.
    pub(crate) struct Sampler {
        /// The session while disarmed; `None` while armed, and for good
        /// once the handler kept it past a stop.
        idle: Option<Session>,
        /// The armed run's period.
        armed_us: Option<u32>,
        send: Sender,
        receive: Receiver,
    }

    /// The handler did not hand the session back: the sampler is gone for
    /// the rest of the image's run.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub(crate) struct Stopped;

    /// Stop and mask the timer, discard a stale event and bind the handler.
    pub(crate) fn init(
        parts: Timg1AlarmParts,
        router: &mut Router<impl Controller>,
        token: SamplerToken,
    ) -> Sampler {
        let Timg1AlarmParts {
            mut program,
            mut interrupt,
        } = parts;
        program.disarm();
        let _stale = interrupt.acknowledge();
        let (Some(cells), Some(recording)) = (
            SAMPLER.cells.try_init_with(|| Cells {
                request: Queue::new(),
                reply: Queue::new(),
            }),
            SAMPLER.recording.try_take(),
        ) else {
            crate::fail(Failure::Storage);
        };
        let (send, receive) = cells.request.split();
        let (reply, replies) = cells.reply.split();
        let Some(state) = SAMPLER.state.try_init_with(|| Handler {
            active: None,
            receive,
            send: reply,
        }) else {
            crate::fail(Failure::Storage);
        };
        if SAMPLER.handler.install(state).is_err() {
            crate::fail(Failure::Storage);
        }
        let bound = router.bind(token, interrupt).unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<
                oer_esp32s31_hal::platform::Port,
                _,
            >(error)
        });
        Sampler {
            idle: Some(Session {
                program,
                bound,
                period: Duration::from_micros(0),
                recording,
            }),
            armed_us: None,
            send,
            receive: replies,
        }
    }

    /// The sampler's interrupt: record where the hart was and rearm, or
    /// stop and hand the session back.
    pub(crate) fn interrupt(part: &mut Timg1AlarmInterrupt) {
        // SOURCE: RISC-V CLIC specification 3f8ee744, "mcause" (`mpil` in
        // bits 23:16) and the staged entry (`vectors.s`), which restores
        // `mepc` and `mcause` only on exit; no level above this one traps
        // in between.
        let pc = oer_riscv::read_mepc() as u32;
        let level = (oer_riscv::read_mcause() >> 16) as u8;
        let _ = SAMPLER
            .handler
            .with(|handler| handler.tick(part, pc, level));
    }

    impl Handler {
        fn tick(&mut self, part: &mut Timg1AlarmInterrupt, pc: u32, level: u8) {
            if !part.acknowledge().pending {
                return;
            }
            let Some(mut session) = self
                .active
                .take()
                .or_else(|| self.receive.try_receive().ok())
            else {
                let stray = SAMPLER.stray.load(Ordering::Relaxed);
                SAMPLER
                    .stray
                    .store(stray.wrapping_add(1), Ordering::Relaxed);
                return;
            };
            session.recording.tick(pc, level);
            if !SAMPLER.stop.load(Ordering::Acquire)
                && session
                    .program
                    .arm_after(session.period, &session.bound)
                    .is_ok()
            {
                self.active = Some(session);
                return;
            }
            session.program.disarm();
            if let Err(refused) = self.send.try_send(session) {
                // The reply slot holds a session already: keep this one
                // disarmed; the load server reports the sampler gone.
                self.active = Some(refused.0);
            }
        }
    }

    impl Sampler {
        /// Arm for a run sampled every `period_us`, forgetting the last
        /// run's samples.
        ///
        /// # Errors
        /// [`LoadError::NoSampler`] when the handler kept the session.
        pub(crate) fn start(&mut self, period_us: u32) -> Result<(), LoadError> {
            let Some(mut session) = self.idle.take() else {
                return Err(LoadError::NoSampler);
            };
            session.recording.restart(period_us);
            session.period = Duration::from_micros(u64::from(period_us));
            SAMPLER.stop.store(false, Ordering::Release);
            // Masked, the first tick cannot find the timer armed without
            // its session queued.
            let refused = oer_riscv::interrupt_free(|| {
                if session
                    .program
                    .arm_after(session.period, &session.bound)
                    .is_err()
                {
                    return Some(session);
                }
                self.send.try_send(session).err().map(|refused| refused.0)
            });
            match refused {
                None => {
                    self.armed_us = Some(period_us);
                    Ok(())
                }
                Some(mut session) => {
                    session.program.disarm();
                    self.idle = Some(session);
                    Err(LoadError::InvalidParameters)
                }
            }
        }

        /// Stop at the handler's next tick and take the session back,
        /// waiting at most four periods and 10 ms.
        ///
        /// # Errors
        /// [`Stopped`] when the handler did not hand it back.
        pub(crate) async fn stop(&mut self) -> Result<(), Stopped> {
            let Some(period_us) = self.armed_us.take() else {
                return Ok(());
            };
            let period = u64::from(period_us);
            SAMPLER.stop.store(true, Ordering::Release);
            let deadline = now().saturating_add(period.saturating_mul(4).saturating_add(10_000));
            loop {
                if let Ok(session) = self.receive.try_receive() {
                    self.idle = Some(session);
                    return Ok(());
                }
                let at = now();
                if at >= deadline {
                    return Err(Stopped);
                }
                oer_embassy_time::EmbassyClock
                    .wait_until(Instant::from_micros(
                        at.saturating_add(period).min(deadline),
                    ))
                    .await;
            }
        }

        /// Whether the sampler is disarmed and can start a run.
        pub(crate) fn available(&self) -> bool {
            self.idle.is_some()
        }

        /// The disarmed recording's page from the sample numbered `from`.
        pub(crate) fn page(&self, from: u16) -> Option<Samples> {
            let session = self.idle.as_ref()?;
            Some(
                session
                    .recording
                    .page(from, SAMPLER.stray.load(Ordering::Relaxed)),
            )
        }
    }

    fn now() -> u64 {
        oer_embassy_time::now_micros()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded(ticks: u32) -> Box<Recording> {
        let mut recording = Box::new(Recording::new());
        recording.restart(1000);
        for tick in 0..ticks {
            recording.tick(0x4000_0000 + tick * 4, (tick % 3) as u8);
        }
        recording
    }

    fn kept_ticks(recording: &Recording) -> Vec<u32> {
        let mut ticks = recording.sample_ticks[..recording.kept].to_vec();
        ticks.sort_unstable();
        ticks
    }

    #[test]
    fn a_buffer_that_never_fills_keeps_every_tick_in_order() {
        let recording = recorded(100);
        assert_eq!(kept_ticks(&recording), (0..100).collect::<Vec<_>>());
        let page = recording.page(0, 0);
        assert_eq!((page.ticks, page.stride, page.kept), (100, 1, 100));
        assert_eq!(page.samples.len(), MAX_PAGE_SAMPLES);
        assert_eq!(
            page.samples[1],
            Sample {
                tick: 1,
                pc: 0x4000_0004,
                level: 1
            }
        );
    }

    #[test]
    fn a_buffer_filled_to_a_power_of_two_holds_every_stride_th_tick_of_the_window() {
        let capacity = CAPACITY as u32;
        for every in [1, 2, 8, 64] {
            let recording = recorded(capacity * every);
            // Full: the stride already doubled for the next tick.
            assert_eq!(recording.stride, every * 2);
            assert_eq!(
                kept_ticks(&recording),
                (0..capacity * every)
                    .step_by(every as usize)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn between_doublings_the_new_stride_overwrites_only_ticks_it_drops() {
        let capacity = CAPACITY as u32;
        let ticks = capacity * 4 + capacity / 2 + 3;
        let recording = recorded(ticks);
        assert_eq!(recording.stride, 8);
        let kept = kept_ticks(&recording);
        assert_eq!(kept.len(), CAPACITY);
        assert!(kept.iter().all(|tick| tick % 4 == 0 && *tick < ticks));
        for tick in (0..ticks).step_by(8) {
            assert!(kept.contains(&tick), "{tick}");
        }
    }

    #[test]
    fn each_sample_keeps_its_own_tick_pc_and_level() {
        let recording = recorded(CAPACITY as u32 * 3);
        for slot in 0..CAPACITY {
            let tick = recording.sample_ticks[slot];
            assert_eq!(recording.pcs[slot], 0x4000_0000 + tick * 4);
            assert_eq!(u32::from(recording.levels[slot]), tick % 3);
        }
    }

    #[test]
    fn pages_cover_every_kept_sample_once_and_stop_at_the_end() {
        let recording = recorded(CAPACITY as u32 * 3);
        let mut ticks = Vec::new();
        let mut from = 0;
        loop {
            let page = recording.page(from, 7);
            assert_eq!((page.kept as usize, page.stray), (CAPACITY, 7));
            if page.samples.is_empty() {
                break;
            }
            ticks.extend(page.samples.iter().map(|sample| sample.tick));
            from += page.samples.len() as u16;
        }
        assert_eq!(ticks, recording.sample_ticks.to_vec());
        assert!(recording.page(u16::MAX, 0).samples.is_empty());
    }

    #[test]
    fn a_restart_forgets_the_last_run() {
        let mut recording = recorded(CAPACITY as u32 * 2);
        recording.restart(250);
        let page = recording.page(0, 0);
        assert_eq!(
            (page.period_us, page.ticks, page.stride, page.kept),
            (250, 0, 1, 0)
        );
        assert!(page.samples.is_empty());
    }
}

//! A program-counter sampler on a TIMG1 timer, for any image that selects
//! it: T0 for hart 0, T1 for hart 1.
//!
//! The image binds the timer's interrupt above every level it wants to see
//! and names the [`oer_portable_trace::Observer`] that receives the samples. A
//! session runs only between [`Sampler::start`] and [`Sampler::stop`];
//! otherwise the timer is stopped and masked: no tick, no handler entry.
//!
//! The schedule is absolute: tick `k` is due `(k + 1) * period` after the
//! start on the timer's own counter, which runs on from zero without
//! reloads. Each tick moves the alarm one period on, so the handler's
//! latency never shifts later ticks, and a tick whose time passed while
//! the hart's interrupts were masked fires as soon as they open, at the
//! code that opened them. Every scheduled tick reports one
//! [`oer_portable_trace::profile::PcSample`] and, in a trace build, right after it a
//! [`oer_portable_trace::profile::ReturnSample`] of the interrupted `ra`, which the
//! staged runtime's interrupt entry recorded
//! (`oer_chips_family_staged_runtime::stacks::interrupted_ra`): in a leaf
//! function such as a memory copy it names the caller. Ticks requested to
//! stop report none.
//!
//! Starting hands the timer's program part and binding to the handler as
//! one session through a one-slot queue, with the hart's interrupts masked
//! so that the first tick finds it; stopping asks the handler to hand it
//! back at its next tick. The handler and the starting task share only the
//! queues and two atomics, never a lock.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use oer_portable_memory::{AffineSpscReceiver, AffineSpscSender, CompactSpscCursor};
use oer_portable_trace::profile::{PcSample, ReturnSample};

/// The schedule's program: the timer's operations a session needs.
pub trait Alarm {
    /// Start the counter from zero with its first alarm `period_us` on.
    fn start(&mut self, period_us: u64) -> bool;
    /// Move the alarm to `target_us` on the running counter.
    fn alarm_at(&mut self, target_us: u64) -> bool;
    /// Stop and mask the timer.
    fn disarm(&mut self);
}

/// The timer's program part and the handler's binding, with the schedule.
pub struct Session<A> {
    alarm: A,
    period_us: u64,
    next_us: u64,
    tick: u32,
    /// The alarm refused a target: the session stopped before it was asked.
    cut: bool,
}

impl<A> Session<A> {
    pub const fn new(alarm: A) -> Self {
        Self {
            alarm,
            period_us: 0,
            next_us: 0,
            tick: 0,
            cut: false,
        }
    }
}

type Sender<A> = AffineSpscSender<'static, Session<A>, 1, CompactSpscCursor>;
type Receiver<A> = AffineSpscReceiver<'static, Session<A>, 1, CompactSpscCursor>;

/// What the handler and the starting task share.
pub struct Shared {
    stop: AtomicBool,
    stray: AtomicU32,
}

impl Shared {
    pub const fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            stray: AtomicU32::new(0),
        }
    }

    /// Ticks that found no session: stale timer events.
    pub fn stray(&self) -> u32 {
        self.stray.load(Ordering::Relaxed)
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

/// One reported tick: its sample and, when the interrupted `ra` is known,
/// its return sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tick {
    pub sample: PcSample,
    pub ret: Option<ReturnSample>,
}

/// The handler's state: the running session and its ends of the queues.
pub struct Handler<A: 'static> {
    active: Option<Session<A>>,
    receive: Receiver<A>,
    send: Sender<A>,
}

impl<A: Alarm + 'static> Handler<A> {
    pub fn new(receive: Receiver<A>, send: Sender<A>) -> Self {
        Self {
            active: None,
            receive,
            send,
        }
    }

    /// One timer event, `pending` as acknowledged, which interrupted `pc`
    /// at raw level `level` with return address `ra`, when known: the due
    /// tick's sample to report, with the alarm moved on; or, when asked, no
    /// sample, the timer stopped and the session handed back.
    pub fn tick(
        &mut self,
        shared: &Shared,
        pending: bool,
        pc: u32,
        level: u8,
        ra: Option<u32>,
    ) -> Option<Tick> {
        if !pending {
            return None;
        }
        let Some(mut session) = self
            .active
            .take()
            .or_else(|| self.receive.try_receive().ok())
        else {
            let stray = shared.stray.load(Ordering::Relaxed);
            shared.stray.store(stray.wrapping_add(1), Ordering::Relaxed);
            return None;
        };
        let mut sample = None;
        if !shared.stop.load(Ordering::Acquire) {
            sample = PcSample::new(session.tick, pc, level).map(|sample| Tick {
                sample,
                ret: ra.and_then(|ra| ReturnSample::new(sample.tick(), ra)),
            });
            session.tick = session.tick.saturating_add(1);
            session.next_us = session.next_us.saturating_add(session.period_us);
            if session.alarm.alarm_at(session.next_us) {
                self.active = Some(session);
                return sample;
            }
            session.cut = true;
        }
        session.alarm.disarm();
        if let Err(refused) = self.send.try_send(session) {
            // The reply slot is taken: keep the stopped session; the task
            // finds the sampler gone.
            self.active = Some(refused.0);
        }
        sample
    }
}

/// How a session ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ended {
    /// The ticks it reported.
    pub ticks: u32,
    /// The alarm refused a target before the stop: the window's end has
    /// no samples.
    pub cut: bool,
}

/// Why the sampler did not start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartError {
    /// A session runs, or the handler kept the last one.
    Unavailable,
    /// The timer refused the period.
    Period,
}

/// The handler did not hand the session back: the sampler is gone for the
/// rest of the image's run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lost;

/// The starting task's end of the sampler.
pub struct Sampler<A: 'static> {
    idle: Option<Session<A>>,
    send: Sender<A>,
    receive: Receiver<A>,
    shared: &'static Shared,
}

impl<A: Alarm + 'static> Sampler<A> {
    pub fn new(alarm: A, send: Sender<A>, receive: Receiver<A>, shared: &'static Shared) -> Self {
        Self {
            idle: Some(Session::new(alarm)),
            send,
            receive,
            shared,
        }
    }

    /// Whether no session runs and one can start.
    pub fn available(&self) -> bool {
        self.idle.is_some()
    }

    /// Start a session of one tick every `period_us`. `masked` runs its
    /// closure with the hart's interrupts masked, so that the first tick
    /// cannot find the timer started and the session not yet queued.
    ///
    /// # Errors
    /// [`StartError`]; the sampler stays idle.
    pub fn start(
        &mut self,
        period_us: u32,
        masked: impl FnOnce(&mut dyn FnMut()),
    ) -> Result<(), StartError> {
        let Some(mut session) = self.idle.take() else {
            return Err(StartError::Unavailable);
        };
        let period = u64::from(period_us);
        session.period_us = period;
        session.next_us = period;
        session.tick = 0;
        session.cut = false;
        self.shared.stop.store(false, Ordering::Release);
        let mut session = Some(session);
        let mut refused = None;
        masked(&mut || {
            let Some(mut started) = session.take() else {
                return;
            };
            if !started.alarm.start(period) {
                refused = Some((started, StartError::Period));
                return;
            }
            if let Err(full) = self.send.try_send(started) {
                refused = Some((full.0, StartError::Unavailable));
            }
        });
        match refused {
            None => Ok(()),
            Some((mut session, error)) => {
                session.alarm.disarm();
                self.idle = Some(session);
                Err(error)
            }
        }
    }

    /// Ask the handler to stop at its next tick.
    pub fn request_stop(&self) {
        self.shared.stop.store(true, Ordering::Release);
    }

    /// The session back from the handler, once it handed it back.
    pub fn take_back(&mut self) -> Option<Ended> {
        let session = self.receive.try_receive().ok()?;
        let ended = Ended {
            ticks: session.tick,
            cut: session.cut,
        };
        self.idle = Some(session);
        Some(ended)
    }

    pub fn shared(&self) -> &'static Shared {
        self.shared
    }
}

#[cfg(target_arch = "riscv32")]
pub use chip::{SamplerTimer, Storage, Timer, Timer0, Timer1};

#[cfg(target_arch = "riscv32")]
mod chip {
    use super::{Alarm, Ended, Handler, Lost, Sampler, Session, Shared, StartError};
    use core::marker::PhantomData;
    use oer_chips_esp32s31_hal::peripherals::timg1_alarm::{
        Timg1AlarmInterrupt, Timg1AlarmParts, Timg1AlarmProgram, Timg1Timer1Interrupt,
        Timg1Timer1Parts, Timg1Timer1Program,
    };
    use oer_chips_family_interrupt_table::{
        Bound, Controller, Entry, EventAuthority, InterruptSources, Router,
    };
    use oer_portable_memory::{AffineSpscQueue, CompactSpscCursor, HandlerCell};
    use oer_portable_time::{Duration, Instant, Timer as _};
    use oer_portable_trace::Observer;
    use static_cell::StaticCell;

    /// The timer a sampler ticks on: its parts, program and interrupt part.
    pub trait SamplerTimer: 'static {
        type Parts;
        type Program: 'static;
        type Interrupt: InterruptSources + Send + 'static;
        /// The program and interrupt parts, the timer stopped and a stale
        /// event discarded.
        fn quiet(parts: Self::Parts) -> (Self::Program, Self::Interrupt);
        fn arm_after(
            program: &mut Self::Program,
            period_us: u64,
            authority: &impl EventAuthority<Self::Interrupt>,
        ) -> bool;
        fn alarm_at(
            program: &mut Self::Program,
            target_us: u64,
            authority: &impl EventAuthority<Self::Interrupt>,
        ) -> bool;
        fn disarm(program: &mut Self::Program);
        /// Acknowledge the handler's event; whether it was pending.
        fn acknowledge(interrupt: &mut Self::Interrupt) -> bool;
    }

    /// TIMG1 T0: hart 0's sampler.
    pub enum Timer0 {}

    impl SamplerTimer for Timer0 {
        type Parts = Timg1AlarmParts;
        type Program = Timg1AlarmProgram;
        type Interrupt = Timg1AlarmInterrupt;
        fn quiet(parts: Timg1AlarmParts) -> (Timg1AlarmProgram, Timg1AlarmInterrupt) {
            let Timg1AlarmParts {
                mut program,
                mut interrupt,
            } = parts;
            program.disarm();
            let _stale = interrupt.acknowledge();
            (program, interrupt)
        }
        fn arm_after(
            program: &mut Timg1AlarmProgram,
            period_us: u64,
            authority: &impl EventAuthority<Timg1AlarmInterrupt>,
        ) -> bool {
            program
                .arm_after(Duration::from_micros(period_us), authority)
                .is_ok()
        }
        fn alarm_at(
            program: &mut Timg1AlarmProgram,
            target_us: u64,
            authority: &impl EventAuthority<Timg1AlarmInterrupt>,
        ) -> bool {
            program
                .alarm_at(Duration::from_micros(target_us), authority)
                .is_ok()
        }
        fn disarm(program: &mut Timg1AlarmProgram) {
            program.disarm();
        }
        fn acknowledge(interrupt: &mut Timg1AlarmInterrupt) -> bool {
            interrupt.acknowledge().pending
        }
    }

    /// TIMG1 T1: hart 1's sampler. Its enable lane is T0's program
    /// owner's to open ([`Timg1AlarmProgram::open_timer1`]).
    pub enum Timer1 {}

    impl SamplerTimer for Timer1 {
        type Parts = Timg1Timer1Parts;
        type Program = Timg1Timer1Program;
        type Interrupt = Timg1Timer1Interrupt;
        fn quiet(parts: Timg1Timer1Parts) -> (Timg1Timer1Program, Timg1Timer1Interrupt) {
            let Timg1Timer1Parts {
                mut program,
                mut interrupt,
            } = parts;
            program.disarm();
            interrupt.acknowledge();
            (program, interrupt)
        }
        fn arm_after(
            program: &mut Timg1Timer1Program,
            period_us: u64,
            authority: &impl EventAuthority<Timg1Timer1Interrupt>,
        ) -> bool {
            program
                .arm_after(Duration::from_micros(period_us), authority)
                .is_ok()
        }
        fn alarm_at(
            program: &mut Timg1Timer1Program,
            target_us: u64,
            authority: &impl EventAuthority<Timg1Timer1Interrupt>,
        ) -> bool {
            program
                .alarm_at(Duration::from_micros(target_us), authority)
                .is_ok()
        }
        fn disarm(program: &mut Timg1Timer1Program) {
            program.disarm();
        }
        fn acknowledge(interrupt: &mut Timg1Timer1Interrupt) -> bool {
            // T1's line asserts only for its pending alarm: the entry is
            // the observation.
            interrupt.acknowledge();
            true
        }
    }

    /// The timer's program part with the handler's binding.
    pub struct Timer<I: Entry<Part = T::Interrupt> + 'static, T: SamplerTimer = Timer0> {
        program: T::Program,
        bound: Bound<I>,
    }

    impl<I: Entry<Part = T::Interrupt> + 'static, T: SamplerTimer> Alarm for Timer<I, T> {
        fn start(&mut self, period_us: u64) -> bool {
            T::arm_after(&mut self.program, period_us, &self.bound)
        }

        fn alarm_at(&mut self, target_us: u64) -> bool {
            T::alarm_at(&mut self.program, target_us, &self.bound)
        }

        fn disarm(&mut self) {
            T::disarm(&mut self.program);
        }
    }

    type Queue<I, T> = AffineSpscQueue<Session<Timer<I, T>>, 1, CompactSpscCursor>;

    struct Cells<I: Entry<Part = T::Interrupt> + 'static, T: SamplerTimer> {
        request: Queue<I, T>,
        reply: Queue<I, T>,
    }

    /// The sampler's storage: place it once in the image's critical data,
    /// as the handler reaches it. `O` receives the samples; `T` is the
    /// timer it ticks on.
    pub struct Storage<I: Entry<Part = T::Interrupt> + 'static, O, T: SamplerTimer = Timer0> {
        cells: StaticCell<Cells<I, T>>,
        state: StaticCell<Handler<Timer<I, T>>>,
        handler: HandlerCell<Handler<Timer<I, T>>>,
        shared: Shared,
        observer: PhantomData<fn() -> O>,
    }

    impl<I, O, T> Storage<I, O, T>
    where
        I: Entry<Part = T::Interrupt> + 'static,
        O: Observer + Default,
        T: SamplerTimer,
    {
        pub const fn new() -> Self {
            Self {
                cells: StaticCell::new(),
                state: StaticCell::new(),
                handler: HandlerCell::new(),
                shared: Shared::new(),
                observer: PhantomData,
            }
        }

        /// Stop the timer, discard a stale event and bind the handler on
        /// `router`'s hart. `None` when the storage was initialized before.
        pub fn init<const H: u8>(
            &'static self,
            parts: T::Parts,
            router: &mut Router<impl Controller, H>,
            token: I,
        ) -> Option<Sampler<Timer<I, T>>> {
            let (program, interrupt) = T::quiet(parts);
            let cells = self.cells.try_init_with(|| Cells {
                request: Queue::new(),
                reply: Queue::new(),
            })?;
            let (send, receive) = cells.request.split();
            let (reply, replies) = cells.reply.split();
            let state = self.state.try_init_with(|| Handler::new(receive, reply))?;
            if self.handler.install(state).is_err() {
                return None;
            }
            let bound = router.bind(token, interrupt).unwrap_or_else(|error| {
                oer_chips_family_staged_runtime::interrupts::reset_binding::<
                    oer_chips_esp32s31_hal::platform::Port,
                    _,
                >(error)
            });
            Some(Sampler::new(
                Timer { program, bound },
                send,
                replies,
                &self.shared,
            ))
        }

        /// The interrupt: the image's table entry calls it with its part.
        #[cfg_attr(
            not(oer_trace),
            allow(unused_variables, reason = "only a trace build reports samples")
        )]
        pub fn interrupt(&self, part: &mut T::Interrupt) {
            // SOURCE: RISC-V CLIC specification 3f8ee744, "mcause" (`mpil`
            // in bits 23:16), and the staged entry (`vectors.s`), which
            // restores `mepc` and `mcause` only on exit: with no level
            // above this one bound, nothing traps in between.
            let pc = oer_portable_riscv::read_mepc() as u32;
            let level = (oer_portable_riscv::read_mcause() >> 16) as u8;
            // The entry of this handler, at the hart's highest bound level,
            // recorded the interrupted `ra` last.
            #[cfg(oer_trace)]
            let ra = oer_chips_family_staged_runtime::stacks::interrupted_ra();
            #[cfg(not(oer_trace))]
            let ra = None;
            let pending = T::acknowledge(part);
            let observer = O::default();
            if let Some(Some(tick)) = self
                .handler
                .with(|handler| handler.tick(&self.shared, pending, pc, level, ra))
            {
                oer_portable_trace::observe!(observer, tick.sample);
                if let Some(ret) = tick.ret {
                    oer_portable_trace::observe!(observer, ret);
                }
            }
        }
    }

    impl<I, O, T> Default for Storage<I, O, T>
    where
        I: Entry<Part = T::Interrupt> + 'static,
        O: Observer + Default,
        T: SamplerTimer,
    {
        fn default() -> Self {
            Self::new()
        }
    }

    impl<I: Entry<Part = T::Interrupt> + 'static, T: SamplerTimer> Sampler<Timer<I, T>> {
        /// Start a session of one tick every `period_us`.
        ///
        /// # Errors
        /// [`StartError`]; the sampler stays idle.
        pub fn start_masked(&mut self, period_us: u32) -> Result<(), StartError> {
            self.start(period_us, |body| oer_portable_riscv::interrupt_free(body))
        }

        /// Stop the running session and take it back, waiting at most four
        /// periods and 10 ms of `period_us`; `Ok(None)` when none ran.
        ///
        /// # Errors
        /// [`Lost`] when the handler did not hand it back.
        pub async fn stop(&mut self, period_us: u32) -> Result<Option<Ended>, Lost> {
            if self.available() {
                return Ok(None);
            }
            self.request_stop();
            let period = u64::from(period_us);
            let now = oer_portable_embassy_time::now_micros;
            let deadline = now().saturating_add(period.saturating_mul(4).saturating_add(10_000));
            loop {
                if let Some(ended) = self.take_back() {
                    return Ok(Some(ended));
                }
                let at = now();
                if at >= deadline {
                    return Err(Lost);
                }
                oer_portable_embassy_time::EmbassyClock
                    .wait_until(Instant::from_micros(
                        at.saturating_add(period).min(deadline),
                    ))
                    .await;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests;

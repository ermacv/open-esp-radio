//! Optional observations of the shared platform timer during an explicit window.
//! Counts include every task using that timer, not just the caller's futures.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Timing {
    pub count: u32,
    pub total_micros: u64,
    pub maximum_micros: u32,
}
impl Timing {
    fn record(&mut self, elapsed: u64) -> Option<()> {
        let count = self.count.checked_add(1)?;
        let total = self.total_micros.checked_add(elapsed)?;
        let maximum = self.maximum_micros.max(u32::try_from(elapsed).ok()?);
        *self = Self {
            count,
            total_micros: total,
            maximum_micros: maximum,
        };
        Some(())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    pub elapsed_micros: u64,
    pub invalid: bool,
    pub registrations: u32,
    pub due_at_registration: u32,
    pub due_at_program_start: u32,
    pub due_at_program_return: u32,
    pub irq_ack: Timing,
    pub programming: Timing,
    pub alarm_to_irq: Timing,
    pub deadline_lateness: Timing,
    pub irq_to_dispatch: Timing,
    pub dispatch: Timing,
    pub interrupts: u32,
    pub replaced: u32,
    pub stopped: u32,
    pub unmatched_interrupts: u32,
    /// IRQ entry overlaps the latest alarm programming; alarm latency is unattributable.
    pub overlapping_interrupts: u32,
    pub unmatched_dispatches: u32,
    pub coalesced_interrupts: u32,
    pub early_interrupts: u32,
    pub armed_at_end: bool,
    pub irq_pending_at_end: bool,
}

#[derive(Default)]
pub(crate) struct Recorder {
    started: Option<u64>,
    armed: Option<(u64, u64)>, // Requested absolute deadline, schedule() return.
    irq: Option<u64>,
    report: Report,
}

impl Recorder {
    pub(crate) const fn new() -> Self {
        Self {
            started: None,
            armed: None,
            irq: None,
            report: Report {
                elapsed_micros: 0,
                invalid: false,
                registrations: 0,
                due_at_registration: 0,
                due_at_program_start: 0,
                due_at_program_return: 0,
                irq_ack: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                programming: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                alarm_to_irq: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                deadline_lateness: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                irq_to_dispatch: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                dispatch: Timing {
                    count: 0,
                    total_micros: 0,
                    maximum_micros: 0,
                },
                interrupts: 0,
                replaced: 0,
                stopped: 0,
                unmatched_interrupts: 0,
                overlapping_interrupts: 0,
                unmatched_dispatches: 0,
                coalesced_interrupts: 0,
                early_interrupts: 0,
                armed_at_end: false,
                irq_pending_at_end: false,
            },
        }
    }
    pub(crate) fn enabled(&self) -> bool {
        self.started.is_some()
    }
    pub(crate) fn begin(&mut self, now: u64) -> bool {
        if self.enabled() {
            return false;
        }
        *self = Self {
            started: Some(now),
            ..Self::new()
        };
        true
    }
    pub(crate) fn finish(&mut self, now: u64) -> Report {
        let Some(started) = self.started.take() else {
            return Report {
                invalid: true,
                ..Report::default()
            };
        };
        self.report.elapsed_micros = now.checked_sub(started).unwrap_or_else(|| {
            self.report.invalid = true;
            0
        });
        self.report.armed_at_end = self.armed.is_some();
        self.report.irq_pending_at_end = self.irq.is_some();
        self.report
    }
    pub(crate) fn registration(&mut self, deadline: u64, now: u64) {
        if !self.enabled() {
            return;
        }
        increment(&mut self.report.registrations, &mut self.report.invalid);
        if deadline <= now {
            increment(
                &mut self.report.due_at_registration,
                &mut self.report.invalid,
            );
        }
    }
    pub(crate) fn arm(&mut self, deadline: u64, start: u64, end: u64) {
        if !self.enabled() {
            return;
        }
        record(
            &mut self.report.programming,
            start,
            end,
            &mut self.report.invalid,
        );
        if deadline <= start {
            increment(
                &mut self.report.due_at_program_start,
                &mut self.report.invalid,
            );
        }
        if deadline <= end {
            increment(
                &mut self.report.due_at_program_return,
                &mut self.report.invalid,
            );
        }
        if self.armed.replace((deadline, end)).is_some() {
            increment(&mut self.report.replaced, &mut self.report.invalid);
        }
    }
    pub(crate) fn stop(&mut self) {
        if self.enabled() && self.armed.take().is_some() {
            increment(&mut self.report.stopped, &mut self.report.invalid);
        }
    }
    pub(crate) fn interrupt(&mut self, now: u64, acknowledged: u64) {
        if !self.enabled() {
            return;
        }
        record(
            &mut self.report.irq_ack,
            now,
            acknowledged,
            &mut self.report.invalid,
        );
        increment(&mut self.report.interrupts, &mut self.report.invalid);
        if let Some((deadline, programmed)) = self.armed.take() {
            if now < programmed && programmed <= acknowledged {
                // Entry is sampled before acquiring the timer mutex. Another core
                // can finish a replacement alarm before this IRQ acquires it.
                // Neither that alarm's deadline nor its latency belongs to a
                // provably matched IRQ. Retire it without fabricating an interval.
                increment(
                    &mut self.report.overlapping_interrupts,
                    &mut self.report.invalid,
                );
            } else {
                record(
                    &mut self.report.alarm_to_irq,
                    programmed,
                    now,
                    &mut self.report.invalid,
                );
                if now < deadline {
                    increment(&mut self.report.early_interrupts, &mut self.report.invalid);
                }
                self.report.invalid |= self
                    .report
                    .deadline_lateness
                    .record(now.saturating_sub(deadline))
                    .is_none();
            }
        } else {
            increment(
                &mut self.report.unmatched_interrupts,
                &mut self.report.invalid,
            );
        }
        if self.irq.is_some() {
            increment(
                &mut self.report.coalesced_interrupts,
                &mut self.report.invalid,
            );
        } else {
            self.irq = Some(now);
        }
    }
    pub(crate) fn dispatch(&mut self, start: u64, end: u64) {
        if !self.enabled() {
            return;
        }
        if let Some(irq) = self.irq.take() {
            record(
                &mut self.report.irq_to_dispatch,
                irq,
                start,
                &mut self.report.invalid,
            );
        } else {
            increment(
                &mut self.report.unmatched_dispatches,
                &mut self.report.invalid,
            );
        }
        record(
            &mut self.report.dispatch,
            start,
            end,
            &mut self.report.invalid,
        );
    }
}

fn increment(count: &mut u32, invalid: &mut bool) {
    match count.checked_add(1) {
        Some(next) => *count = next,
        None => *invalid = true,
    }
}
fn record(timing: &mut Timing, start: u64, end: u64, invalid: &mut bool) {
    *invalid |= end
        .checked_sub(start)
        .and_then(|elapsed| timing.record(elapsed))
        .is_none();
}

#[cfg(test)]
mod tests;

/// Extend a wrapping trace timestamp within half its period of this observation.
/// Requested deadlines may be in the future; event timestamps may not.
fn timestamp(now: u64, value: u32, future: bool) -> Option<u64> {
    let delta = value.wrapping_sub(now as u32) as i32;
    if delta == i32::MIN || (!future && delta > 0) {
        return None;
    }
    now.checked_add_signed(i64::from(delta))
}

struct Collector {
    recorder: Recorder,
    arm: Option<(u64, u64)>,
    dispatch: Option<u64>,
}
impl Collector {
    const fn new() -> Self {
        Self {
            recorder: Recorder::new(),
            arm: None,
            dispatch: None,
        }
    }
    fn observe(
        &mut self,
        event: oer_espressif_executor::observation::timer::Observation,
        now: u64,
        irq: Option<(u64, u64)>,
    ) {
        use oer_espressif_executor::observation::timer::Phase;
        if !self.recorder.enabled() {
            return;
        }
        let Some(at) = timestamp(now, event.at_micros, false) else {
            self.recorder.report.invalid = true;
            return;
        };
        match event.phase {
            Phase::Registration | Phase::Arm => {
                let Some(deadline) = timestamp(now, event.value, true) else {
                    self.recorder.report.invalid = true;
                    return;
                };
                if event.phase == Phase::Registration {
                    self.recorder.registration(deadline, at);
                } else {
                    if self.arm.replace((deadline, at)).is_some() {
                        self.recorder.report.invalid = true;
                    }
                }
            }
            Phase::Armed => match self.arm.take() {
                Some((deadline, start)) => self.recorder.arm(deadline, start, at),
                None => self.recorder.report.invalid = true,
            },
            Phase::Stop => self.recorder.stop(),
            Phase::Dispatch => {
                if event.value != 0 {
                    match irq {
                        Some((entered, acknowledged)) => {
                            self.recorder.interrupt(entered, acknowledged)
                        }
                        None => self.recorder.report.invalid = true,
                    }
                }
                if self.dispatch.replace(at).is_some() {
                    self.recorder.report.invalid = true;
                }
            }
            Phase::Dispatched => match self.dispatch.take() {
                Some(start) => self.recorder.dispatch(start, at),
                None => self.recorder.report.invalid = true,
            },
            // IRQs publish only to the image's atomic mailbox, never this cell.
            Phase::Interrupt | Phase::Invalid => self.recorder.report.invalid = true,
        }
    }
    fn begin(&mut self, now: u64) -> bool {
        if !self.recorder.begin(now) {
            return false;
        }
        self.arm = None;
        self.dispatch = None;
        true
    }
}

/// IRQ-only writer, bounded task read. No lock or mutable report is shared
/// between the timer IRQ and the radio executor it preempts.
struct Irq {
    sequence: core::sync::atomic::AtomicU32,
    entered: core::sync::atomic::AtomicU32,
    acknowledged: core::sync::atomic::AtomicU32,
}
impl Irq {
    const fn new() -> Self {
        use core::sync::atomic::AtomicU32;
        Self {
            sequence: AtomicU32::new(0),
            entered: AtomicU32::new(0),
            acknowledged: AtomicU32::new(0),
        }
    }
    fn publish(&self, entered: u32, acknowledged: u32) {
        use core::sync::atomic::{Ordering, fence};
        self.sequence.fetch_add(1, Ordering::Relaxed);
        fence(Ordering::Release);
        self.entered.store(entered, Ordering::Relaxed);
        self.acknowledged.store(acknowledged, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }
    fn read(&self, now: u64) -> Option<(u64, u64)> {
        use core::sync::atomic::{Ordering, fence};
        let first = self.sequence.load(Ordering::Acquire);
        if first == 0 || first & 1 != 0 {
            return None;
        }
        let entered = self.entered.load(Ordering::Relaxed);
        let acknowledged = self.acknowledged.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != first {
            return None;
        }
        Some((
            timestamp(now, entered, false)?,
            timestamp(now, acknowledged, false)?,
        ))
    }
}

#[cfg(all(target_arch = "riscv32", oer_trace))]
mod image {
    use super::*;
    use oer_espressif_executor::observation::timer::{Observation, Phase};
    use oer_time::Clock;
    use oer_trace::{Event, Observer};
    static TIMER: oer_memory::HandlerCell<Collector> = oer_memory::HandlerCell::new();
    static STORAGE: static_cell::StaticCell<Collector> = static_cell::StaticCell::new();
    static IRQ: Irq = Irq::new();

    #[derive(Debug)]
    pub enum InstallError {
        StorageClaimed,
        CellOccupied,
    }
    pub fn install() -> Result<(), InstallError> {
        let storage = STORAGE
            .try_init(Collector::new())
            .ok_or(InstallError::StorageClaimed)?;
        TIMER
            .install(storage)
            .map(|_| ())
            .map_err(|_| InstallError::CellOccupied)
    }
    fn now() -> u64 {
        oer_embassy_time::EmbassyClock.now().as_micros()
    }

    pub struct TimerObserver;
    impl Observer for TimerObserver {
        fn event<E: Event>(&self, event: E) {
            let Some(event) = Observation::of(&event) else {
                return;
            };
            // This image selects timer reporting only for its radio executor.
            if event.executor != 2 {
                return;
            }
            if event.phase == Phase::Interrupt {
                IRQ.publish(event.at_micros, event.value);
                return;
            }
            let now = now();
            let irq = if event.phase == Phase::Dispatch && event.value != 0 {
                IRQ.read(now)
            } else {
                None
            };
            let _ = TIMER.with(|collector| collector.observe(event, now, irq));
        }
    }
    /// Exclusive window on the radio executor; cancellation stops recording.
    pub struct Window {
        _local: core::marker::PhantomData<*mut ()>,
    }
    impl Window {
        pub fn begin() -> Option<Self> {
            TIMER
                .with(|collector| collector.begin(now()))?
                .then_some(Self {
                    _local: core::marker::PhantomData,
                })
        }
        pub fn finish(self) -> Report {
            TIMER
                .with(|collector| collector.recorder.finish(now()))
                .unwrap_or(Report {
                    invalid: true,
                    ..Report::default()
                })
        }
    }
    impl Drop for Window {
        fn drop(&mut self) {
            let _ = TIMER.with(|collector| {
                if collector.recorder.enabled() {
                    let _ = collector.recorder.finish(now());
                }
            });
        }
    }
}
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub use image::{TimerObserver, Window, install};

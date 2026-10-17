use core::{
    sync::atomic::{AtomicBool, Ordering},
    task::Waker,
};

use crate::timer_queue::WakeQueue;
use embassy_time_driver::Driver;
use esp_hal::{
    Blocking,
    interrupt::Priority,
    peripherals::Interrupt,
    system::Cpu,
    time::{Duration, Instant},
    timer::{Error, OneShotTimer},
};
use esp_sync::NonReentrantMutex;
use oer_interrupt_table::Entry;

/// ESP-HAL timer capability accepted by [`init`].
pub type Timer = OneShotTimer<'static, Blocking>;

struct State {
    timer: Option<Timer>,
    queue: WakeQueue,
    current_alarm: u64,
    #[cfg(feature = "timer-observation")]
    observation: crate::timer_observation::Recorder,
}

impl State {
    const fn new() -> Self {
        Self {
            timer: None,
            queue: WakeQueue::new(),
            current_alarm: u64::MAX,
            #[cfg(feature = "timer-observation")]
            observation: crate::timer_observation::Recorder::new(),
        }
    }

    fn arm_next_wakeup(&mut self, now: u64) {
        let next_deadline = self.queue.next_deadline();
        if next_deadline == self.current_alarm {
            return;
        }
        let timer = self
            .timer
            .as_mut()
            .expect("oer_espressif_executor_embassy::init must run first");
        self.current_alarm = next_deadline;
        if next_deadline == u64::MAX {
            timer.stop();
            #[cfg(feature = "timer-observation")]
            self.observation.stop();
            return;
        }

        let mut timeout = Duration::from_micros(next_deadline.saturating_sub(now).max(1));
        #[cfg(feature = "timer-observation")]
        let program_start = self.observation.enabled().then(crate::time_driver::now);
        loop {
            match timer.schedule(timeout) {
                Ok(()) => break,
                Err(Error::InvalidTimeout) if timeout > Duration::from_micros(1) => {
                    timeout = timeout / 2;
                }
                Err(error) => panic!("failed to schedule Embassy timer: {error:?}"),
            }
        }
        #[cfg(feature = "timer-observation")]
        if let Some(start) = program_start {
            self.observation
                .arm(next_deadline, start, crate::time_driver::now());
        }
    }
}

struct EmbassyTimeDriver {
    state: NonReentrantMutex<State>,
}

oer_memory::zeroed_static! {
    /// Placement: the board linker owns this exported timer-state section.
    #[used]
    static EMBASSY_TIMER_FIRED: AtomicBool = zeroed in ".critical.bss.embassy_time";
}

impl EmbassyTimeDriver {
    const fn new() -> Self {
        Self {
            state: NonReentrantMutex::new(State::new()),
        }
    }

    #[inline(always)]
    fn acknowledge_interrupt(&self, #[cfg(feature = "timer-observation")] entered: u64) {
        self.state.with(|state| {
            let timer = state
                .timer
                .as_mut()
                .expect("Embassy timer interrupt fired before initialization");
            timer.clear_interrupt();
            state.current_alarm = u64::MAX;
            #[cfg(feature = "timer-observation")]
            state.observation.interrupt(entered, now());
        });
    }

    fn dispatch_expired(&self) {
        self.state.with(|state| {
            let now = now();
            state.queue.dispatch_expired(now);
            #[cfg(feature = "timer-observation")]
            if state.observation.enabled() {
                state.observation.dispatch(now, crate::time_driver::now());
            }
            state.arm_next_wakeup(now);
        });
    }
}

#[used]
#[allow(
    unsafe_code,
    reason = "Embassy requires one exported global time-driver instance"
)]
#[unsafe(link_section = ".critical.data.embassy_time")]
static EMBASSY_TIME_DRIVER: EmbassyTimeDriver = EmbassyTimeDriver::new();

#[allow(
    unsafe_code,
    reason = "Embassy time ABI requires this unique global symbol"
)]
#[unsafe(no_mangle)]
fn _embassy_time_now() -> u64 {
    <EmbassyTimeDriver as Driver>::now(&EMBASSY_TIME_DRIVER)
}

#[allow(
    unsafe_code,
    reason = "Embassy time ABI requires this unique global symbol"
)]
#[unsafe(no_mangle)]
fn _embassy_time_schedule_wake(at: u64, waker: &Waker) {
    <EmbassyTimeDriver as Driver>::schedule_wake(&EMBASSY_TIME_DRIVER, at, waker);
}

impl Driver for EmbassyTimeDriver {
    #[inline]
    fn now(&self) -> u64 {
        now()
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        self.state.with(|state| {
            #[cfg(feature = "timer-observation")]
            if state.observation.enabled() {
                state.observation.registration(at, now());
            }
            if let Some(now) = state.queue.schedule_wake(at, waker, now) {
                state.arm_next_wakeup(now);
            }
        });
    }
}

pub(crate) fn dispatch_pending() {
    if EMBASSY_TIMER_FIRED.swap(false, Ordering::AcqRel) {
        EMBASSY_TIME_DRIVER.dispatch_expired();
    }
}

/// The interrupt-table handler of the time driver's alarm source.
#[esp_hal::ram]
pub fn timer_interrupt() {
    EMBASSY_TIME_DRIVER.acknowledge_interrupt(
        #[cfg(feature = "timer-observation")]
        now(),
    );
    EMBASSY_TIMER_FIRED.store(true, Ordering::Release);
    crate::executor::mark_work::<0>();
}

/// Install the global Embassy time driver on the calling core. `alarm` is the
/// token of `timer`'s source in the image's interrupt table, whose entry names
/// [`timer_interrupt`] on this core; another source's token leaves the time
/// driver without its alarm.
///
/// # Panics
///
/// When the table routes the source to another core.
pub fn init<A>(mut timer: Timer, alarm: A)
where
    A: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
{
    timer.stop();
    timer.unlisten();
    timer.clear_interrupt();
    if let Err(error) = oer_espressif_interrupt_table_esp_hal::enable(&alarm) {
        panic!("time driver alarm: {error:?}");
    }
    timer.listen();

    EMBASSY_TIME_DRIVER.state.with(|state| {
        assert!(
            state.timer.is_none(),
            "Embassy time driver already initialized"
        );
        state.timer = Some(timer);
    });
}

#[inline]
fn now() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

#[cfg(feature = "timer-observation")]
pub(crate) fn begin_observation() -> bool {
    EMBASSY_TIME_DRIVER
        .state
        .with(|state| state.timer.is_some() && state.observation.begin(now()))
}
#[cfg(feature = "timer-observation")]
pub(crate) fn finish_observation() -> crate::timer_observation::Report {
    EMBASSY_TIME_DRIVER
        .state
        .with(|state| state.observation.finish(now()))
}

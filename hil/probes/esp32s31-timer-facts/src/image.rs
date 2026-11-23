//! The image: a staged boot, the boot hart's thread executor with the
//! console and the HIL agent, and hart 1 started for the concurrent case.
//! Every case runs in the console's command on the boot hart's thread
//! context, its trials with the hart's interrupts masked.

use core::sync::atomic::{AtomicU32, Ordering};

use oer_chips_esp32s31_hal::peripherals::software_interrupt::FromCpu0Irq;
use oer_chips_esp32s31_hal::peripherals::system_timer::{
    SystimerAlarm0, SystimerAlarm1, SystimerAlarm2,
};
use oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::ConsoleInterrupt;
use oer_chips_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_chips_esp32s31_hal::system::cores::Cores;
use oer_chips_esp32s31_hal::system::cycles::CycleCounter;
use oer_chips_esp32s31_hal::system::platform::Port;
use oer_chips_esp32s31_hal::system::root::Root;
use oer_chips_esp32s31_platform_board as board;
use oer_chips_esp32s31_platform_runtime::Esp32s31;
use oer_chips_family_platform_executor::Executor;
use oer_chips_family_platform_port::AlarmPart as _;
use oer_chips_family_platform_runtime::SecondaryHandoff;
use oer_hil_services_timer_facts::{
    self as service, ACT_AFTER, AFTER_ALARM_AHEAD, APART_BEHIND, APART_WAIT, Apart, Attempt,
    Calibrated, Case, Concurrent, EARLIER_BEFORE_STANDING, LATER_PAST_STANDING, LOOP_STEP, Looped,
    MOST_ATTEMPTS, MOST_TRIALS, New, RATE_TICKS, REFERENCE_AHEAD, Ran, Reached, Refusal, Request,
    STANDING_AHEAD, SWEEP_NEW, Spread, Swept, Tally, WINDOW, WordsRead,
};
use oer_portable_diagnostics::{Name, name};
use oer_portable_embassy_time::EmbassyClock;
use oer_portable_time::settle::{SettleCounter as _, SettleLimit};
use oer_portable_time::{Duration, Timer as _};

use crate::comparators::{One, Ticks, Two};
use crate::trial::{Rate, Seen, before, offset};
use crate::usb;

oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: oer_chips_family_platform_runtime::interrupts::LINES,
    handler_attributes: [];
    contexts { app: 1; }
    wake: WakeToken = FromCpu0Irq => oer_chips_family_platform_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 40, context app, hart 0, Hardware;
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 41, context app, hart 0, Hardware;
    alarm: AppAlarm = SystimerAlarm0 => oer_chips_family_platform_executor::alarm_interrupt::<AppTasks, 0, SystimerAlarm0>, line 42, context app, hart 0, Hardware;
}

oer_chips_family_platform_executor::tasks! {
    AppTasks: Executor<Esp32s31, 0> {
        watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_watchdog;
        host(host: Host) = host_task;
    }
}
oer_chips_family_platform_executor::time_driver!(Esp32s31);
oer_chips_family_platform_runtime::entry!(main, Port);
oer_chips_family_platform_runtime::secondary_entry!(1, HART1: Port, Hart1Parts => hart1_main);

/// The bound of one alarm's wait: 1 ms, far above every target's distance.
const ALARM_BOUND_MICROS: u32 = 1_000;
/// The bound of the boot's rate measurement: 20 ms, twice its span.
const RATE_BOUND_MICROS: u32 = 20_000;
/// Hart 1's answer to a stop: 20 waits of 1 ms, far above one bounded poll.
const HART1_ANSWER_WAITS: u32 = 20;

oer_chips_family_platform_runtime::placed_static! {
    /// The boot hart's latest request to hart 1: odd runs its loop, the next
    /// even number stops it. Written by the boot hart.
    static REQUEST: AtomicU32 = AtomicU32::new(0), in critical_data "hart_exchange";
    /// The first target of hart 1's loop, low and high word, published
    /// before an odd request. Written by the boot hart.
    static BASE: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)], in critical_data "hart_exchange";
    /// Hart 1's counts of its last loop: loads, alarms, missed. Written by
    /// hart 1 before `DONE`.
    static COUNTS: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)], in critical_data "hart_exchange";
    /// The stop request hart 1 answered last. Written by hart 1.
    static DONE: AtomicU32 = AtomicU32::new(0), in critical_data "hart_exchange";
}

/// What hart 1 takes at its launch: comparator 2, its alarm part and the
/// cycle counter with the cycles of one microsecond.
struct Hart1Parts {
    two: Two,
    alarm: SystimerAlarm2,
    cycles: CycleCounter,
    per_micro: u32,
}

fn main(handoff: oer_chips_family_platform_runtime::Handoff<Port>) -> ! {
    let Some(root) = Root::take() else {
        fail(name!("root"; 32));
    };
    let ((timer, software, watchdogs, reset_causes, hart1, cycles, entropy), _psram, mut adopted) =
        match oer_chips_family_platform_runtime::adoption::run::<Esp32s31, _, _, _>(
            handoff,
            root.into_runtime(),
            InterruptTable,
            board::BootstrapPsram,
            oer_chips_esp32s31_platform_runtime::phy_fault::report_previous,
            |hardware| {
                let reset_causes =
                    oer_chips_esp32s31_platform_runtime::reset_causes::read(hardware.reset_cause);
                (
                    hardware.system_timer,
                    hardware.software_interrupts,
                    hardware.watchdogs,
                    reset_causes,
                    hardware.hart1,
                    hardware.cycles,
                    hardware.entropy,
                )
            },
        ) {
            Ok(adopted) => adopted.into_parts(),
            Err(error) => fail(error.name()),
        };
    let Some(interrupts) = Interrupts::take() else {
        fail(name!("interrupts"; 32));
    };
    let timers = oer_chips_family_platform_executor::init::<Esp32s31, 0, _, _, _, _>(
        oer_chips_family_platform_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    // After the executor's bind of comparator 0 and before its first
    // program: the enables are written here, once.
    let (one, two, ticks) = crate::comparators::claim();
    let Some(micro) = SettleLimit::new(Duration::from_micros(1)) else {
        fail(name!("settle-limit"; 32));
    };
    let per_micro = cycles.cycles(micro);
    let mut bench = Bench {
        one,
        alarm: timer.alarm1,
        ticks,
        cycles,
        per_micro,
    };
    let rate = bench.calibrate();
    let mut rng = oer_chips_esp32s31_hal::peripherals::entropy::Entropy::new(entropy, cycles);
    let boot = match (rng.random(), rng.random()) {
        (Ok(high), Ok(low)) => (u64::from(high) << 32) | u64::from(low),
        _ => fail(name!("entropy"; 32)),
    };
    let records = oer_hil_agent::Records {
        reset_cause: reset_causes.of(oer_chips_esp32s31_hal::system::platform::BOOT_HART),
        previous_panic: None,
        previous_exception: None,
    };
    let Ok(Some(mut cores)) = Cores::take() else {
        fail(name!("cores"; 32));
    };
    let _hart1 = oer_chips_esp32s31_platform_runtime::secondary::launch_hart1(
        &mut cores,
        hart1,
        reset_causes,
        &timers,
        &HART1,
        Hart1Parts {
            two,
            alarm: timer.alarm2,
            cycles,
            per_micro,
        },
    )
    .unwrap_or_else(|failure| failure.reset());
    let (console, pads) = match usb::open(adopted.router(), interrupts.console_usb) {
        Ok(value) => value,
        Err(error) => fail(error.reason()),
    };
    // The pads stay held by `main`, which never returns.
    let _pads = pads;
    let Some(writer) = WRITER.try_take() else {
        fail(name!("writer"; 32));
    };
    let host = Host {
        console,
        boot,
        records,
        writer,
        owner: Owner {
            bench,
            rate,
            hart1: Hart1::Idle { last: 0 },
        },
    };
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(value) => value,
        Err(error) => fail(error.name()),
    };
    match Executor::<Esp32s31, 0>::new(software.line0)
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            (),
            move |spawner| {
                if AppTasks::watchdog(spawner, watchdogs.rtc).is_err()
                    || AppTasks::host(spawner, host).is_err()
                {
                    fail(name!("task"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

fn fail<const B: usize>(reason: Name<B>) -> ! {
    oer_chips_family_platform_runtime::fatal::boot_failure::<Port>(reason.as_str(), || {
        let _note = oer_chips_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
            &oer_portable_diagnostics::Note::runtime_fail(reason),
        );
    })
}

async fn feed_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    match watchdog
        .feed_every(
            &EmbassyClock,
            Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS),
        )
        .await {}
}

// ---------------------------------------------------------------- bench

/// Comparator 1 and what its trials need, on the boot hart.
struct Bench {
    one: One,
    alarm: SystimerAlarm1,
    ticks: Ticks,
    cycles: CycleCounter,
    per_micro: u32,
}

/// A trial's reference: its target and the cycle its alarm was seen at.
#[derive(Clone, Copy)]
struct Reference {
    target: u64,
    seen: u32,
}

/// What one trial of a [`Run`](service::Run) saw: the alarms before its
/// load, the load's first write and strobe, the alarms after it and the
/// cycle its first alarm after the load is expected at.
struct Observed {
    before: Seen,
    start: u32,
    end: u32,
    after: Seen,
    expected: u32,
}

impl Bench {
    fn sample(&self) -> u32 {
        self.cycles.sample()
    }

    /// The cycle `micros` after `from`.
    fn after_micros(&self, from: u32, micros: u32) -> u32 {
        from.wrapping_add(self.per_micro.saturating_mul(micros))
    }

    /// Comparator 1's alarms until `until`, each acknowledged when seen.
    fn watch(&mut self, until: u32, mut seen: Seen) -> Seen {
        loop {
            let now = self.sample();
            if self.one.raised() {
                seen = seen.with(now);
                self.alarm.acknowledge();
            }
            if !before(now, until) {
                return seen;
            }
        }
    }

    /// The first alarm before `until`, acknowledged: its cycle.
    fn first(&mut self, until: u32) -> Option<u32> {
        loop {
            let now = self.sample();
            if self.one.raised() {
                self.alarm.acknowledge();
                return Some(now);
            }
            if !before(now, until) {
                return None;
            }
        }
    }

    /// A target `ahead` of the counter loaded once the previous alarm was
    /// acknowledged, and its alarm: the trial's reference. `None` when the
    /// counter did not verify, the target could not be written or the
    /// alarm did not come within its bound.
    fn reference(&mut self, ahead: u64) -> Option<Reference> {
        let now = self.ticks.now().ok()?;
        let target = now.wrapping_add(ahead);
        self.one.load(target).ok()?;
        // The target lies ahead: what is acknowledged here is an earlier
        // target's.
        self.alarm.acknowledge();
        let bound = self.after_micros(self.sample(), ALARM_BOUND_MICROS);
        let seen = self.first(bound)?;
        Some(Reference { target, seen })
    }

    /// The boot's rate: two alarms [`RATE_TICKS`] apart.
    fn calibrate(&mut self) -> Calibrated {
        let Some(start) = self.reference(REFERENCE_AHEAD) else {
            return Calibrated::Missed;
        };
        if self
            .one
            .load(start.target.wrapping_add(RATE_TICKS))
            .is_err()
        {
            return Calibrated::Missed;
        }
        let bound = self.after_micros(start.seen, RATE_BOUND_MICROS);
        match (self.first(bound), u32::try_from(RATE_TICKS)) {
            (Some(end), Ok(ticks)) => Calibrated::Rate {
                ticks,
                cycles: end.wrapping_sub(start.seen),
            },
            _ => Calibrated::Missed,
        }
    }

    /// The load of `target`, timed: the cycles of its first write and of
    /// its strobe.
    fn timed_load(&mut self, target: u64) -> Option<(u32, u32)> {
        let start = self.sample();
        self.one.load(target).ok()?;
        Some((start, self.sample()))
    }

    /// Case a's and e's standing target, and the alarms until the act.
    fn stand(&mut self, standing: u64, until: u32) -> Option<Seen> {
        self.one.load(standing).ok()?;
        Some(self.watch(until, Seen::NONE))
    }

    /// One trial of `case`; `None` when it could not act (see
    /// [`Self::reference`]).
    fn trial(&mut self, case: Case, rate: Rate) -> Option<Observed> {
        let reference = self.reference(REFERENCE_AHEAD)?;
        let r = reference.target;
        let at = |ticks| rate.expected(reference.seen, ticks);
        let standing = r.wrapping_add(STANDING_AHEAD);
        let later = STANDING_AHEAD.saturating_add(LATER_PAST_STANDING);
        // What the trial loaded, and the ticks after the reference its alarm
        // is expected at: `None` for an alarm expected at once.
        let (before_load, (start, end), new) = match case {
            Case::Later => {
                let early = self.stand(standing, at(ACT_AFTER))?;
                (early, self.timed_load(r.wrapping_add(later))?, Some(later))
            }
            Case::Earlier => {
                let earlier = STANDING_AHEAD.saturating_sub(EARLIER_BEFORE_STANDING);
                let early = self.stand(standing, at(ACT_AFTER))?;
                (
                    early,
                    self.timed_load(r.wrapping_add(earlier))?,
                    Some(earlier),
                )
            }
            Case::AfterAlarm => (
                Seen::NONE,
                self.timed_load(r.wrapping_add(AFTER_ALARM_AHEAD))?,
                Some(AFTER_ALARM_AHEAD),
            ),
            Case::Behind { ticks } => {
                let now = self.ticks.now().ok()?;
                (
                    Seen::NONE,
                    self.timed_load(now.wrapping_sub(u64::from(ticks)))?,
                    None,
                )
            }
            Case::Apart(order) => {
                let early = match order {
                    // The standing target's HI over a passed LO.
                    Apart::LowFirst => {
                        let early = self.stand(standing, at(ACT_AFTER))?;
                        let passed = self.ticks.now().ok()?.wrapping_sub(APART_BEHIND);
                        self.one.write_low(passed).ok()?;
                        early
                    }
                    // A standing target one HI ahead over a passed LO, then
                    // the counter's HI.
                    Apart::HighFirst => {
                        let passed = self.ticks.now().ok()?.wrapping_sub(APART_BEHIND);
                        let early = self.stand(passed.wrapping_add(1 << 32), at(ACT_AFTER))?;
                        self.one.write_high(passed).ok()?;
                        early
                    }
                };
                let wait = self.sample().wrapping_add(rate.cycles_of(APART_WAIT));
                let early = self.watch(wait, early);
                (early, self.timed_load(r.wrapping_add(later))?, Some(later))
            }
        };
        let (until, expected) = match new {
            Some(new) => (at(new.saturating_add(WINDOW)), at(new)),
            None => (end.wrapping_add(rate.cycles_of(WINDOW)), end),
        };
        let after = self.watch(until, Seen::NONE);
        Some(Observed {
            before: before_load,
            start,
            end,
            after,
            expected,
        })
    }

    /// One trial of case d: the standing target, and the new one loaded
    /// `asked` cycles from the standing target's expected cycle.
    fn attempt(&mut self, asked: i32, new: New, rate: Rate) -> Attempt {
        self.attempted(asked, new, rate).unwrap_or(Attempt::Lost)
    }

    fn attempted(&mut self, asked: i32, new: New, rate: Rate) -> Option<Attempt> {
        let reference = self.reference(REFERENCE_AHEAD)?;
        let standing = reference.target.wrapping_add(STANDING_AHEAD);
        let expiry = rate.expected(reference.seen, STANDING_AHEAD);
        let target = match new {
            New::Future => standing.wrapping_add(SWEEP_NEW),
            New::Behind => standing.wrapping_sub(SWEEP_NEW),
        };
        let seen = self.stand(standing, expiry.wrapping_add_signed(asked))?;
        let (start, end) = self.timed_load(target)?;
        let until = rate.expected(
            reference.seen,
            STANDING_AHEAD
                .saturating_add(SWEEP_NEW)
                .saturating_add(WINDOW),
        );
        let seen = self.watch(until, seen);
        Some(Attempt::Made {
            offset: offset(start, expiry),
            loaded: offset(end, expiry),
            alarms: seen.count,
            first: seen.first.map(|cycle| offset(cycle, expiry)),
            second: seen.second.map(|cycle| offset(cycle, expiry)),
        })
    }

    /// Case g: a target `ahead` of the counter.
    fn far(&mut self, ahead: u64, rate: Rate) -> Reached {
        let Ok(now) = self.ticks.now() else {
            return Reached::Refused(Refusal::Unverified);
        };
        // Whatever the previous target raised is acknowledged before the
        // load: an alarm at once must stay seen.
        self.alarm.acknowledge();
        let Some((_, end)) = self.timed_load(now.wrapping_add(ahead)) else {
            return Reached::Refused(Refusal::Unverified);
        };
        match self.first(end.wrapping_add(rate.cycles_of(WINDOW))) {
            Some(cycle) => Reached::AtOnce {
                latency: cycle.wrapping_sub(end),
            },
            None => Reached::Waiting,
        }
    }
}

// ---------------------------------------------------------------- hart 1

/// Hart 1's loop on the boot hart's requests: each odd request runs it
/// from the published base until the next request, then publishes its
/// counts and the request it answered.
fn hart1_main(handoff: SecondaryHandoff<Port, 1>, parts: Hart1Parts) -> ! {
    // Hart 1 takes no interrupt: it adopts its handoff and never enables.
    let _adopted = match handoff.adopt(InterruptTable) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    let Hart1Parts {
        mut two,
        mut alarm,
        cycles,
        per_micro,
    } = parts;
    let bound = per_micro.saturating_mul(ALARM_BOUND_MICROS);
    let mut answered = 0;
    loop {
        let request = REQUEST.load(Ordering::Acquire);
        if request == answered || request.is_multiple_of(2) {
            continue;
        }
        let mut target = u64::from(BASE[0].load(Ordering::Acquire))
            | u64::from(BASE[1].load(Ordering::Acquire)) << 32;
        let (mut loads, mut alarms, mut missed) = (0_u32, 0_u32, 0_u32);
        while REQUEST.load(Ordering::Acquire) == request {
            // Acknowledged before the load: an alarm at once stays seen.
            alarm.acknowledge();
            loads = loads.wrapping_add(1);
            let written = two.load(target);
            target = target.wrapping_add(LOOP_STEP);
            // A target is taken modulo the counter's range: its write cannot
            // fail, and would count as missed.
            if written.is_err() {
                missed = missed.wrapping_add(1);
                continue;
            }
            let start = cycles.sample();
            loop {
                if two.raised() {
                    alarms = alarms.wrapping_add(1);
                    break;
                }
                if cycles.sample().wrapping_sub(start) >= bound {
                    missed = missed.wrapping_add(1);
                    break;
                }
            }
        }
        for (count, value) in COUNTS.iter().zip([loads, alarms, missed]) {
            count.store(value, Ordering::Relaxed);
        }
        answered = request.wrapping_add(1);
        DONE.store(answered, Ordering::Release);
    }
}

/// Hart 1's loop, as the boot hart's owner knows it.
#[derive(Clone, Copy)]
enum Hart1 {
    /// No loop runs; `last` is the request hart 1 answered last.
    Idle {
        last: u32,
    },
    Running {
        request: u32,
    },
}

// ---------------------------------------------------------------- host

oer_protocols_hil_agent::image_keys! {
    /// The probe: the base module and the timer facts' requests.
    KEYS = [Request]
}

const IMAGE: oer_hil_agent::Image = oer_hil_agent::Image {
    keys: KEYS,
    maximum_payload_bytes: service::MAXIMUM_PAYLOAD_BYTES,
};

/// The console's state: only the console's task touches it.
static WRITER: static_cell::ConstStaticCell<oer_hil_agent::Writer> =
    static_cell::ConstStaticCell::new(oer_hil_agent::Writer::new());

/// What the console answers at once: nothing; every request is a command.
struct Observing;

impl service::Observer for Observing {}

/// The comparators' owner on the boot hart: the bench, the boot's rate and
/// hart 1's loop.
struct Owner {
    bench: Bench,
    rate: Calibrated,
    hart1: Hart1,
}

impl Owner {
    fn rate(&self) -> Option<Rate> {
        match self.rate {
            Calibrated::Rate { ticks, cycles } => Some(Rate { ticks, cycles }),
            Calibrated::Missed => None,
        }
    }
}

impl service::Commander for Owner {
    async fn calibrate(&mut self, _request: service::Calibrate) -> Calibrated {
        self.rate
    }

    async fn run(&mut self, request: service::Run) -> Ran {
        let Some(rate) = self.rate() else {
            return Ran::Refused(Refusal::NoRate);
        };
        if request.trials == 0 || request.trials > MOST_TRIALS {
            return Ran::Refused(Refusal::Trials);
        }
        let mut tally = Tally {
            trials: 0,
            lost: 0,
            after_load: [0; 3],
            before_load: 0,
            at: Spread::EMPTY,
            load: Spread::EMPTY,
        };
        for _ in 0..request.trials {
            let observed =
                oer_portable_riscv::interrupt_free(|| self.bench.trial(request.case, rate));
            tally.trials = tally.trials.saturating_add(1);
            let Some(observed) = observed else {
                tally.lost = tally.lost.saturating_add(1);
                continue;
            };
            if observed.before.count > 0 {
                tally.before_load = tally.before_load.saturating_add(1);
            }
            if let Some(bin) = tally.after_load.get_mut(observed.after.bin()) {
                *bin = bin.saturating_add(1);
            }
            tally.load = tally.load.with(offset(observed.end, observed.start));
            if let Some(first) = observed.after.first {
                tally.at = tally.at.with(offset(first, observed.expected));
            }
        }
        Ran::Done(tally)
    }

    async fn sweep(&mut self, request: service::Sweep) -> Swept {
        let Some(rate) = self.rate() else {
            return Swept::Refused(Refusal::NoRate);
        };
        if request.trials == 0 || usize::from(request.trials) > MOST_ATTEMPTS {
            return Swept::Refused(Refusal::Trials);
        }
        let mut attempts = heapless::Vec::new();
        for _ in 0..request.trials {
            let attempt = oer_portable_riscv::interrupt_free(|| {
                self.bench.attempt(request.offset, request.new, rate)
            });
            // At most `MOST_ATTEMPTS` trials: the list has room for each.
            if attempts.push(attempt).is_err() {
                break;
            }
        }
        Swept::Done(attempts)
    }

    async fn far(&mut self, request: service::Far) -> Reached {
        let Some(rate) = self.rate() else {
            return Reached::Refused(Refusal::NoRate);
        };
        oer_portable_riscv::interrupt_free(|| self.bench.far(request.ahead, rate))
    }

    async fn concurrent(&mut self, request: Concurrent) -> Looped {
        match (request, self.hart1) {
            (Concurrent::Start, Hart1::Idle { last }) => {
                let Ok(now) = self.bench.ticks.now() else {
                    return Looped::Refused(Refusal::Hart1Silent);
                };
                let [low, high]: [u32; 2] = bytemuck::cast(now.wrapping_add(STANDING_AHEAD));
                BASE[0].store(low, Ordering::Relaxed);
                BASE[1].store(high, Ordering::Relaxed);
                let request = last.wrapping_add(1);
                REQUEST.store(request, Ordering::Release);
                self.hart1 = Hart1::Running { request };
                Looped::Started
            }
            (Concurrent::Start, Hart1::Running { .. }) => Looped::Refused(Refusal::Running),
            (Concurrent::Stop, Hart1::Idle { .. }) => Looped::Refused(Refusal::NotRunning),
            (Concurrent::Stop, Hart1::Running { request }) => {
                let stop = request.wrapping_add(1);
                REQUEST.store(stop, Ordering::Release);
                for _ in 0..HART1_ANSWER_WAITS {
                    if DONE.load(Ordering::Acquire) == stop {
                        self.hart1 = Hart1::Idle { last: stop };
                        let [loads, alarms, missed] =
                            COUNTS.each_ref().map(|count| count.load(Ordering::Relaxed));
                        return Looped::Stopped {
                            loads,
                            alarms,
                            missed,
                        };
                    }
                    if EmbassyClock
                        .wait_for(Duration::from_millis(1))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Looped::Refused(Refusal::Hart1Silent)
            }
        }
    }

    async fn words(&mut self, _request: service::Words) -> WordsRead {
        let (work, interrupt) = self.bench.one.enables();
        WordsRead { work, interrupt }
    }
}

/// What the host's link takes at boot.
struct Host {
    console: usb::Console,
    boot: u64,
    records: oer_hil_agent::Records,
    writer: &'static mut oer_hil_agent::Writer,
    owner: Owner,
}

/// How the console's serving ends: its transport broke.
enum Ended {
    Console,
}

/// The application level's task: the HIL console, which runs each command
/// on the comparators' owner.
async fn host_task(host: Host) {
    let Host {
        console,
        boot,
        records,
        writer,
        mut owner,
    } = host;
    let mut observing = Observing;
    match oer_hil_agent::serve::<_, Request, _, _, _>(
        console,
        boot,
        records,
        IMAGE,
        oer_hil_agent::Mount {
            writer,
            observer: &mut observing,
            commander: &mut owner,
        },
        Ended::Console,
    )
    .await
    {
        Err(Ended::Console) => fail(name!("console"; 32)),
        Ok(never) => match never {},
    }
}

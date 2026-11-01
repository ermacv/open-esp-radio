use core::sync::atomic::{AtomicU32, Ordering};

use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_hal::cores::Cores;
use oer_esp32s31_hal::peripherals::reset_cause::ResetCause;
use oer_esp32s31_hal::peripherals::usb_serial_jtag;
use oer_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_esp32s31_hal::peripherals::{
    software_interrupt::{FromCpu0Irq, FromCpu1Irq},
    system_timer::SystimerAlarm0,
};
use oer_esp32s31_hal::platform::{BOOT_HART, Port};
use oer_esp32s31_hal::root::Root;
use oer_espressif_executor::peer::liveness::{self, Beat, Period, Responder, Supervisor};
use oer_espressif_executor::peer::{Obligations, PeerClient, PeerServer};
use oer_espressif_executor::{Executor, InterruptExecutor};
use oer_espressif_interrupt_table::signal::{FromPeerHart, IrqSignal};
use oer_espressif_staged_runtime::panic::{self, FaultReason, PreviousFault};
use oer_espressif_staged_runtime::SecondaryHandoff;
use oer_memory::{InPlace, RequestSlot};
use oer_time::{Clock as _, Timer as _};

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; hart1_executor: 2; }
    /// The boot hart's executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The boot hart's executor's timer alarm; it only acknowledges and pends.
    alarm: AlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<Tasks, 0, SystimerAlarm0>, line 17, context app_wake, hart 0, Hardware;
    /// Hart 1's interrupt-mode executor on `FROM_CPU1`, routed to hart 1's bank.
    hart1: Hart1Token = FromCpu1Irq => oer_espressif_executor::Round<Hart1Tasks>, line 16, context hart1_executor, hart 1, Executor;
}

/// The boot hart's thread-mode executor on `FROM_CPU0`, the time driver's
/// writer.
type Hart0Executor = Executor<Esp32s31, 0>;
/// Hart 1's interrupt-mode executor on `FROM_CPU1`.
type Hart1Executor = InterruptExecutor<Esp32s31, 1>;

oer_espressif_executor::tasks! {
    /// The boot hart's tasks.
    Tasks: Executor<Esp32s31, 0> {
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
        supervise(supervisor: LivenessSupervisor) = supervise_hart1;
        abandon(client: Client) = abandon_when_armed;
    }
}

oer_espressif_executor::tasks! {
    /// Hart 1's tasks.
    pub Hart1Tasks: InterruptExecutor<Esp32s31, 1> {
        respond(server: Server) = respond;
        beat(responder: LivenessResponder) = answer_beats;
    }
}

/// The watched request: a value hart 1 squares in place.
struct Ask {
    value: u32,
    answer: u32,
}

/// The boot hart's end of the watched channel.
type Client = PeerClient<Ask, Hart1Executor, Hart0Executor>;
/// Hart 1's end of the watched channel.
type Server = PeerServer<Ask, Hart1Executor, Hart0Executor>;
/// The boot hart's liveness supervisor of hart 1 and of the channel.
type LivenessSupervisor = Supervisor<Hart1Executor, Hart0Executor, WATCHED>;
/// Hart 1's end of the liveness supervision.
type LivenessResponder = Responder<Hart1Executor, Hart0Executor>;

/// The liveness supervision of hart 1, as boot-smoke's: a beat every
/// 100 ms, each echoed within 50 ms. Stated configuration.
const LIVENESS: Period = match Period::new(
    oer_time::Duration::from_millis(100),
    oer_time::Duration::from_millis(50),
) {
    Ok(period) => period,
    Err(_) => panic!("the liveness deadline lies within its period"),
};

/// The abandoned request's deadline after its post: longer than two beat
/// periods, so beats complete after the post and before the deadline.
const ABANDON: oer_time::Duration = oer_time::Duration::from_millis(300);

/// How often the probe looks for the stand's arming word.
const ARM_POLL: oer_time::Duration = oer_time::Duration::from_millis(50);

/// The one watched channel: the supervisor issues its seat 0.
const WATCHED: usize = 1;
/// The channel the retained record must name.
const CHANNEL: u32 = 0;
/// The supervisor's mark in a `PeerUnanswered` record.
const SUPERVISOR_MARK: u32 = 2;

oer_espressif_staged_runtime::placed_static! {
    /// The request was posted for hart 1's task: raised by the boot hart's task.
    static POSTED: IrqSignal<Hart1Executor> = IrqSignal::new::<FromPeerHart<Hart0Executor, WakeToken>, Hart1Token>(), in critical_data "hart_exchange";
    /// The request was answered: raised by hart 1's task.
    static ANSWERED: IrqSignal<Hart0Executor> = IrqSignal::new::<FromPeerHart<Hart1Executor, Hart1Token>, WakeToken>(), in critical_data "hart_exchange";
    /// The watched channel.
    static SLOT: RequestSlot<Ask, (), InPlace> = RequestSlot::new_in_place(), in critical_data "hart_exchange";
    /// A liveness beat was posted for hart 1.
    static BEAT_POSTED: IrqSignal<Hart1Executor> = IrqSignal::new::<FromPeerHart<Hart0Executor, WakeToken>, Hart1Token>(), in critical_data "hart_exchange";
    /// Hart 1 echoed the beat.
    static BEAT_ANSWERED: IrqSignal<Hart0Executor> = IrqSignal::new::<FromPeerHart<Hart1Executor, Hart1Token>, WakeToken>(), in critical_data "hart_exchange";
    /// The liveness supervision's durable seat, claimed once.
    static BEAT_SEAT: RequestSlot<Beat, (), InPlace> = RequestSlot::new_in_place(), in critical_data "hart_exchange";
    /// The requests hart 1 served: written by hart 1, read by the boot hart.
    static SERVED: AtomicU32 = AtomicU32::new(0), in critical_data "hart_exchange";
}

/// The seat of the watched channel: written and read on the boot hart only.
static OBLIGATIONS: Obligations<Hart0Executor, WATCHED> = Obligations::new();

/// The beats completed since the post; zero before it.
static ROUNDS_AFTER_POST: AtomicU32 = AtomicU32::new(0);
/// Whether the request was posted: written and read on the boot hart only.
static ABANDONED: AtomicU32 = AtomicU32::new(0);

// The probe's own retained words: its state, the posted request's deadline
// (low, high word), the beats completed since the post, the requests hart 1
// served, and a check word over the first five. The stand writes `ARMED`
// into the first word over JTAG to arm one injection.
oer_espressif_staged_runtime::retained_static! { static PROBE: [u32; 6]; }

/// The stand armed one injection.
const ARMED: u32 = 0x4152_4d44;
/// The request was posted and never taken; the next boot checks it.
const INJECTED: u32 = 0x494e_4a44;
/// Mixed into the check word.
const CHECK: u32 = 0x5a5a_5a5a;

/// The probe's retained state, as the previous boot left it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Injected {
    deadline: u64,
    rounds: u32,
    served: u32,
}

/// Write the injected state: the deadline, the beats since the post and the
/// requests served.
fn store_injected(state: Injected) {
    if let Some(mut words) = PROBE.try_lock() {
        let low = state.deadline as u32;
        let high = (state.deadline >> 32) as u32;
        *words = [
            INJECTED,
            low,
            high,
            state.rounds,
            state.served,
            INJECTED ^ low ^ high ^ state.rounds ^ state.served ^ CHECK,
        ];
    }
}

/// The injected state the previous boot left, with an intact check word.
fn read_injected() -> Option<Injected> {
    let words = PROBE.try_lock()?;
    let [state, low, high, rounds, served, check] = *words;
    (state == INJECTED && check == state ^ low ^ high ^ rounds ^ served ^ CHECK).then_some(
        Injected {
            deadline: (u64::from(high) << 32) | u64::from(low),
            rounds,
            served,
        },
    )
}

/// Whether the stand armed an injection; read anew each time, as the stand
/// writes the word while the image runs.
fn armed() -> bool {
    PROBE
        .try_lock()
        .is_some_and(|words| {
            let [state, ..] = oer_memory::read_volatile(&*words);
            state == ARMED
        })
}

/// Clear the probe's state: nothing is armed or injected.
fn clear() {
    if let Some(mut words) = PROBE.try_lock() {
        *words = [0; 6];
    }
}

/// What hart 1 takes at its launch.
struct Hart1Parts {
    executor: Hart1Executor,
    irq: FromCpu1Irq,
    token: Hart1Token,
    server: Server,
    liveness: LivenessResponder,
}

oer_espressif_staged_runtime::entry!(main, Port);
oer_espressif_staged_runtime::secondary_entry!(1, HART1: Port, Hart1Parts => hart1_main);

fn main(handoff: oer_espressif_staged_runtime::Handoff<Port>) -> ! {
    // Before the runtime reports and clears it.
    let previous = panic::peek_previous_fault();
    let injected = read_injected();
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32));
    };
    let ((runtime_system_timer, runtime_software_interrupts, runtime_watchdogs, reset_causes, hart1), _psram, mut adopted) =
        match oer_espressif_staged_runtime::adoption::run::<oer_esp32s31_compose::Esp32s31, _, _>(
            handoff,
            root.into_runtime(),
            INTERRUPT_TABLE,
            board::BootstrapPsram,
            oer_esp32s31_compose::phy_fault::report_previous,
            |hardware| {
                let reset_causes = oer_esp32s31_compose::reset_causes::read(hardware.reset_cause);
                (
                    hardware.system_timer,
                    hardware.software_interrupts,
                    hardware.watchdogs,
                    reset_causes,
                    hardware.hart1,
                )
            },
        ) {
            Ok(adopted) => adopted.into_parts(),
            Err(error) => fail(error.name()),
        };
    let cause = reset_causes.of(BOOT_HART);
    let verdict = match (injected, cause) {
        (None, _) => None,
        // A flash or a power cycle since the injection: a stale state.
        (Some(_), ResetCause::ChipPowerOn | ResetCause::Other(_)) => {
            text(b"OER-PROBE peer-overdue stale-state discarded\r\n");
            None
        }
        (Some(state), cause) => Some(verify(previous, state, cause)),
    };
    clear();
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_diagnostics::name!("interrupt-table"; 32));
    };
    let software = runtime_software_interrupts;
    let hart1_executor = InterruptExecutor::<Esp32s31, 1>::new(software.line1);
    let Some((supervisor, responder)) =
        liveness::pair(&BEAT_SEAT, &BEAT_POSTED, &BEAT_ANSWERED, LIVENESS, &OBLIGATIONS)
    else {
        fail(oer_diagnostics::name!("liveness-claimed"; 32));
    };
    let Some(seat) = supervisor.watch() else {
        fail(oer_diagnostics::name!("obligation-seat"; 32));
    };
    let Some((client, server)) = oer_espressif_executor::peer::pair(&SLOT, &POSTED, &ANSWERED, seat)
    else {
        fail(oer_diagnostics::name!("channel-claimed"; 32));
    };
    let timer = runtime_system_timer;
    let timers = oer_espressif_executor::init::<Esp32s31, 0, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let watchdog = runtime_watchdogs.rtc;
    if let Some(pass) = verdict {
        // The check ends here: hart 1 stays contained and only the RTC
        // watchdog's feed runs, so the image waits without another injection.
        let result: &[u8] = if pass {
            b"OER-RESULT: pass peer-overdue\r\n"
        } else {
            b"OER-RESULT: fail peer-overdue\r\n"
        };
        text(result);
        let (mut router, hart) = match adopted.enable_interrupts() {
            Ok(router) => router,
            Err(error) => fail(error.name()),
        };
        match executor
            .run(&hart, &mut router, interrupts.wake, software.irq0, (), (), move |spawner| {
                if Tasks::rtc_watchdog(spawner, watchdog).is_err() {
                    fail(oer_diagnostics::name!("task-start"; 32));
                }
            })
            .unwrap_or_else(|failure| failure.reset()) {}
    }
    let Ok(Some(mut cores)) = Cores::take() else {
        fail(oer_diagnostics::name!("cores-taken"; 32));
    };
    let hart1 = oer_esp32s31_compose::secondary::launch_hart1(
        &mut cores,
        hart1,
        reset_causes,
        &timers,
        &HART1,
        Hart1Parts {
            executor: hart1_executor,
            irq: software.irq1,
            token: interrupts.hart1,
            server,
            liveness: responder,
        },
    )
    .unwrap_or_else(|failure| failure.reset());
    let mut line = *b"OER-PROBE peer-overdue ready hart=00000000\r\n";
    put_hex(&mut line, 34, u32::from(hart1.hart()));
    text(&line);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    match executor
        .run(&hart, &mut router, interrupts.wake, software.irq0, (), (), move |spawner| {
            if Tasks::rtc_watchdog(spawner, watchdog).is_err()
                || Tasks::supervise(spawner, supervisor).is_err()
                || Tasks::abandon(spawner, client).is_err()
            {
                fail(oer_diagnostics::name!("task-start"; 32));
            }
        })
        .unwrap_or_else(|failure| failure.reset()) {}
}

/// The boot after an injection: the retained runtime fault must be the
/// supervisor's `PeerUnanswered` for the channel and deadline the probe
/// stored, after a software reset of the boot hart, with at least one beat
/// completed after the post and the request served by hart 1.
fn verify(previous: Option<PreviousFault>, state: Injected, cause: ResetCause) -> bool {
    let expected = PreviousFault {
        reason: FaultReason::PeerUnanswered,
        hart: 0,
        observations: [
            CHANNEL,
            state.deadline as u32,
            (state.deadline >> 32) as u32,
            SUPERVISOR_MARK,
        ],
    };
    let mut line = *b"OER-PROBE peer-overdue checked deadline=0000000000000000 rounds=00000000 served=00000000 cause=00000000\r\n";
    put_hex(&mut line, 40, (state.deadline >> 32) as u32);
    put_hex(&mut line, 48, state.deadline as u32);
    put_hex(&mut line, 64, state.rounds);
    put_hex(&mut line, 80, state.served);
    put_hex(&mut line, 95, u32::from(cause.code()));
    text(&line);
    previous == Some(expected)
        && cause == ResetCause::CoreSoftware
        && state.rounds >= 1
        && state.served >= 1
}

/// Hart 1's main: its own interrupt context and executor, the channel's
/// server and the beat responder.
fn hart1_main(handoff: SecondaryHandoff<Port, 1>, parts: Hart1Parts) -> ! {
    let Hart1Parts {
        executor,
        irq,
        token,
        server,
        liveness,
    } = parts;
    let mut adopted = match handoff.adopt(INTERRUPT_TABLE) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    let (hart, router) = adopted.parts();
    let spawner = executor
        .start::<Hart1Tasks, _, _, 1, _>(hart, router, token, irq, ())
        .unwrap_or_else(|failure| failure.reset());
    if Hart1Tasks::respond(&spawner, server).is_err() || Hart1Tasks::beat(&spawner, liveness).is_err()
    {
        fail(oer_diagnostics::name!("hart1-task-start"; 32));
    }
    match adopted.enable_interrupts() {
        Ok(_router_and_hart) => oer_espressif_staged_runtime::idle(),
        Err(error) => fail(error.name()),
    }
}

/// Hart 1's task: answer each request in place and count it.
async fn respond(mut server: Server) {
    let _foreign = server
        .serve(|ask| {
            ask.answer = ask.value.wrapping_mul(ask.value);
            SERVED.fetch_add(1, Ordering::Release);
        })
        .await;
    panic::reset_fault::<Port>(panic::FaultReason::SignalContext, [1, 0, 0, 0])
}

/// Hart 1's liveness task: echo every beat.
async fn answer_beats(mut responder: LivenessResponder) {
    let _foreign = responder.serve().await;
    panic::reset_fault::<Port>(panic::FaultReason::SignalContext, [1, 2, 0, 0])
}

/// The boot hart's supervisor of hart 1 and of the channel, for good. After
/// the post it keeps the probe's retained state current: the beats completed
/// since and the requests hart 1 served. Its end records its own reason and
/// resets; for the abandoned request that is `PeerUnanswered`.
async fn supervise_hart1(supervisor: LivenessSupervisor) {
    let error = supervisor
        .supervise(&EmbassyClock, |_round| {
            if ABANDONED.load(Ordering::Acquire) == 0 {
                return;
            }
            let rounds = ROUNDS_AFTER_POST.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
            if let Some(state) = read_injected() {
                store_injected(Injected {
                    rounds,
                    served: SERVED.load(Ordering::Acquire),
                    ..state
                });
            }
        })
        .await;
    let (reason, words) = error.diagnostic();
    panic::reset_fault::<Port>(reason, words)
}

/// The boot hart's client: once the stand arms the probe, post one request
/// to hart 1 and never take its answer, holding the client for good.
async fn abandon_when_armed(mut client: Client) {
    while !armed() {
        if EmbassyClock.wait_for(ARM_POLL).await.is_err() {
            fail(oer_diagnostics::name!("arm-poll-time"; 32));
        }
    }
    let Ok(deadline) = EmbassyClock.deadline_after(ABANDON) else {
        fail(oer_diagnostics::name!("abandon-deadline"; 32));
    };
    // The next boot's expectation exists before the obligation does.
    store_injected(Injected {
        deadline: deadline.as_micros(),
        rounds: 0,
        served: 0,
    });
    ABANDONED.store(1, Ordering::Release);
    if client.post(Ask { value: 7, answer: 0 }, deadline).is_err() {
        fail(oer_diagnostics::name!("abandon-post"; 32));
    }
    let mut line = *b"OER-PROBE peer-overdue posted channel=00000000 deadline=0000000000000000\r\n";
    put_hex(&mut line, 38, CHANNEL);
    put_hex(&mut line, 56, (deadline.as_micros() >> 32) as u32);
    put_hex(&mut line, 64, deadline.as_micros() as u32);
    text(&line);
    // The answer is never taken: the client stays alive and waits for good.
    core::future::pending::<()>().await;
    drop(client);
}

/// Feed the RTC watchdog for good.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Write `bytes` through the early writer, within its bounded budget.
fn text(bytes: &[u8]) {
    let _written = usb_serial_jtag::early_text(bytes);
}

/// Write `word` as eight lowercase hex digits into `line` from `at`.
fn put_hex(line: &mut [u8], at: usize, word: u32) {
    for (index, shift) in (0..8_u32).rev().map(|digit| digit * 4).enumerate() {
        let nibble = ((word >> shift) & 15) as u8;
        if let Some(digit) = line.get_mut(at.wrapping_add(index)) {
            *digit = if nibble < 10 {
                b'0'.wrapping_add(nibble)
            } else {
                b'a'.wrapping_add(nibble.wrapping_sub(10))
            };
        }
    }
}

/// Retain and report a failed boot step before explicitly resetting the image.
fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    oer_espressif_staged_runtime::fatal::boot_failure::<Port>(reason.as_str(), || {
        let _note = usb_serial_jtag::fatal_note(&oer_diagnostics::Note::runtime_fail(reason));
    })
}

oer_espressif_executor::time_driver!(Esp32s31);

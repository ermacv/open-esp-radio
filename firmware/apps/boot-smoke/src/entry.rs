use core::sync::atomic::{AtomicU32, Ordering};

use oer_crypto::{Aes128Key, KeyData, KeyUnwrap as _, KeyUnwrapRequest, KeyUnwrapResponse};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_compose::crypto::KeyUnwrapEngine;
use oer_esp32s31_hal::cores::Cores;
use oer_esp32s31_hal::peripherals::aes::{AesInterrupt, AesParts};
use oer_esp32s31_hal::peripherals::usb_serial_jtag;
use oer_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_esp32s31_hal::peripherals::{
    software_interrupt::{FromCpu0Irq, FromCpu1Irq},
    system_timer::{SystimerAlarm0, SystimerAlarm1},
};
use oer_esp32s31_hal::platform::Port;
use oer_esp32s31_hal::root::Root;
use oer_espressif_executor::peer::liveness::{self, Beat, Period, Responder, Supervisor};
use oer_espressif_executor::peer::{Obligations, PeerClient, PeerError, PeerServer, PostError};
use oer_espressif_executor::{AlarmGrant, Executor, InterruptExecutor};
use oer_espressif_interrupt_table::signal::{FromPeerHart, IrqSignal};
use oer_espressif_staged_runtime::diagnostics::{
    self, BootSmokePassed, Console, HartAes, HartExchange, HartLiveness, SecondaryReady,
};
use oer_espressif_staged_runtime::{SecondaryHandoff, panic};
use oer_memory::{InPlace, RequestSlot};
use oer_time::{Clock as _, Timer as _};

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; hart1_executor: 2; hart1_alarm: 3; }
    /// The boot hart's executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The boot hart's executor's timer alarm; it only acknowledges and pends.
    alarm: AlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<Tasks, 0, SystimerAlarm0>, line 17, context app_wake, hart 0, Hardware;
    /// Hart 1's interrupt-mode executor on `FROM_CPU1`, routed to hart 1's bank.
    hart1: Hart1Token = FromCpu1Irq => oer_espressif_executor::Round<Hart1Tasks>, line 16, context hart1_executor, hart 1, Executor;
    /// Hart 1's executor's timer alarm, above that executor on hart 1; it
    /// only acknowledges and pends.
    hart1_alarm: Hart1AlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<Hart1Tasks, 1, SystimerAlarm1>, line 17, context hart1_alarm, hart 1, Hardware;
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
        check(peers: Peers) = check_hart1;
    }
}

oer_espressif_executor::tasks! {
    /// Hart 1's tasks.
    pub Hart1Tasks: InterruptExecutor<Esp32s31, 1> {
        respond(server: Server, hart: u16) = respond;
        unwrap(server: AesServer, engine: KeyUnwrapEngine, hart: u16) = serve_aes;
        beat(responder: LivenessResponder) = answer_beats;
    }
}

/// One request to hart 1 and its answer, exchanged in place.
struct Exchange {
    value: u16,
    answer: u32,
    hart: u16,
}

/// The request the boot hart sends hart 1.
const REQUEST: u16 = 7;
/// How long the boot hart waits for each of hart 1's answers before it
/// records and resets: far above one software interrupt to hart 1 and back,
/// and above one RFC 3394 unwrap of two semiblocks (twelve blocks of at
/// most the engine's 1 ms each, on hart 1's timers).
const EXCHANGE_DEADLINE: oer_time::Duration = oer_time::Duration::from_millis(100);

/// The liveness supervision of hart 1: a beat every 100 ms, each echoed
/// within 50 ms. Stated configuration, not a measured latency.
const LIVENESS: Period = match Period::new(
    oer_time::Duration::from_millis(100),
    oer_time::Duration::from_millis(50),
) {
    Ok(period) => period,
    Err(_) => panic!("the liveness deadline lies within its period"),
};
/// When the boot task looks for the supervisor's first round again: by then
/// that round completed or its expiry reset the image.
const FIRST_ROUND: oer_time::Duration =
    match LIVENESS.deadline().checked_add(oer_time::Duration::from_millis(1)) {
        Some(bound) => bound,
        None => panic!("the first round's bound fits"),
    };

/// How long hart 1's AES task waits on its own timer before each unwrap:
/// a wait the boot hart's writer must program on hart 1's comparator.
const HART1_TIMER: oer_time::Duration = oer_time::Duration::from_millis(2);

/// The unwrap buffer: RFC 3394 §4.1's 24 wrapped bytes fit.
const AES_CAPACITY: usize = 32;

/// RFC 3394 §4.1 (Wrap 128 bits of Key Data with a 128-bit KEK): the KEK.
const RFC3394_4_1_KEK: [u8; 16] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
];
/// RFC 3394 §4.1: the key data.
const RFC3394_4_1_KEY_DATA: [u8; 16] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];
/// RFC 3394 §4.1: the ciphertext.
const RFC3394_4_1_CIPHERTEXT: [u8; 24] = [
    0x1f, 0xa6, 0x8b, 0x0a, 0x81, 0x12, 0xb4, 0x47, 0xae, 0xf3, 0x4b, 0xd8, 0xfb, 0x5a, 0x7b, 0x82,
    0x9d, 0x3e, 0x86, 0x23, 0x71, 0xd2, 0xcf, 0xe5,
];

/// One key unwrap on hart 1's AES, in two exchanges: the first hands hart 1
/// the request, which it unwraps between the exchanges on its own timers;
/// the second takes the response back.
struct AesExchange {
    request: Option<KeyUnwrapRequest<AES_CAPACITY>>,
    response: Option<KeyUnwrapResponse<AES_CAPACITY>>,
}

/// The boot hart's end of the exchange.
type Client = PeerClient<Exchange, Hart1Executor, Hart0Executor>;
/// Hart 1's end of the exchange.
type Server = PeerServer<Exchange, Hart1Executor, Hart0Executor>;
/// The boot hart's end of the key unwrap.
type AesClient = PeerClient<AesExchange, Hart1Executor, Hart0Executor>;
/// Hart 1's end of the key unwrap.
type AesServer = PeerServer<AesExchange, Hart1Executor, Hart0Executor>;
/// The boot hart's liveness supervisor of hart 1.
type LivenessSupervisor = Supervisor<Hart1Executor, Hart0Executor, WATCHED>;
/// Hart 1's end of the liveness supervision.
type LivenessResponder = Responder<Hart1Executor, Hart0Executor>;

/// The boot task's ends of its two requests to hart 1.
struct Peers {
    exchange: Client,
    aes: AesClient,
}

oer_espressif_staged_runtime::placed_static! {
    /// A request was posted for hart 1's task: raised by the boot hart's task.
    static POSTED: IrqSignal<Hart1Executor> = IrqSignal::new::<FromPeerHart<Hart0Executor, WakeToken>, Hart1Token>(), in critical_data "hart_exchange";
    /// The request was answered: raised by hart 1's task.
    static ANSWERED: IrqSignal<Hart0Executor> = IrqSignal::new::<FromPeerHart<Hart1Executor, Hart1Token>, WakeToken>(), in critical_data "hart_exchange";
    /// The one exchange between the two harts.
    static SLOT: RequestSlot<Exchange, (), InPlace> = RequestSlot::new_in_place(), in critical_data "hart_exchange";
    /// A key unwrap exchange was posted for hart 1's AES task.
    static AES_POSTED: IrqSignal<Hart1Executor> = IrqSignal::new::<FromPeerHart<Hart0Executor, WakeToken>, Hart1Token>(), in critical_data "hart_exchange";
    /// Hart 1's AES task answered.
    static AES_ANSWERED: IrqSignal<Hart0Executor> = IrqSignal::new::<FromPeerHart<Hart1Executor, Hart1Token>, WakeToken>(), in critical_data "hart_exchange";
    /// The key unwrap exchanges.
    static AES_SLOT: RequestSlot<AesExchange, (), InPlace> = RequestSlot::new_in_place(), in critical_data "hart_exchange";
    /// A liveness beat was posted for hart 1.
    static BEAT_POSTED: IrqSignal<Hart1Executor> = IrqSignal::new::<FromPeerHart<Hart0Executor, WakeToken>, Hart1Token>(), in critical_data "hart_exchange";
    /// Hart 1 echoed the beat.
    static BEAT_ANSWERED: IrqSignal<Hart0Executor> = IrqSignal::new::<FromPeerHart<Hart1Executor, Hart1Token>, WakeToken>(), in critical_data "hart_exchange";
    /// The liveness supervision's durable seat, claimed once.
    static BEAT_SEAT: RequestSlot<Beat, (), InPlace> = RequestSlot::new_in_place(), in critical_data "hart_exchange";
}

/// The boot hart's watched channels: the exchange and the key unwrap.
const WATCHED: usize = 2;

/// The seats of the boot hart's watched requests, which the liveness
/// supervisor checks: written and read on the boot hart only.
static OBLIGATIONS: Obligations<Hart0Executor, WATCHED> = Obligations::new();

/// The liveness rounds the supervisor completed: written and read on the
/// boot hart only.
static LIVENESS_ROUNDS: AtomicU32 = AtomicU32::new(0);

/// What hart 1 takes at its launch: its executor, which owns `FROM_CPU1`'s
/// line, the line's interrupt part and table token, its executor's timer
/// grant, the AES block and the servers of the boot hart's requests.
struct Hart1Parts {
    executor: Hart1Executor,
    irq: FromCpu1Irq,
    token: Hart1Token,
    alarm: AlarmGrant<Esp32s31, 1, Hart1AlarmToken>,
    aes: AesParts,
    exchange: Server,
    unwrap: AesServer,
    liveness: LivenessResponder,
}

oer_espressif_staged_runtime::entry!(main, Port);
oer_espressif_staged_runtime::secondary_entry!(1, HART1: Port, Hart1Parts => hart1_main);

fn main(handoff: oer_espressif_staged_runtime::Handoff<Port>) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::peek_previous();
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32));
    };
    let (
        (
            runtime_system_timer,
            runtime_software_interrupts,
            runtime_watchdogs,
            mut diagnostics,
            reset_causes,
            hart1,
            aes,
        ),
        _psram,
        mut adopted,
    ) = match oer_espressif_staged_runtime::adoption::run::<oer_esp32s31_compose::Esp32s31, _, _>(
        handoff,
        root.into_runtime(),
        INTERRUPT_TABLE,
        board::BootstrapPsram,
        oer_esp32s31_compose::phy_fault::report_previous,
        |hardware| {
            let reset_causes = oer_esp32s31_compose::reset_causes::read(hardware.reset_cause);
            // The executor is not running yet: the early writer waits for room
            // within its budget and counts what it drops.
            // Every frame of this boot, the runtime's report of a previous
            // fault included, takes the next number of one shared sequence.
            let mut diagnostics = Console::shared(|bytes| {
                if usb_serial_jtag::early_text(bytes).is_whole() {
                    Ok(())
                } else {
                    Err(())
                }
            });
            if let Some(record) = previous {
                let _ = diagnostics::previous_panic(&mut diagnostics, &record);
            }
            (
                hardware.system_timer,
                hardware.software_interrupts,
                hardware.watchdogs,
                diagnostics,
                reset_causes,
                hardware.hart1,
                hardware.aes,
            )
        },
    ) {
        Ok(adopted) => adopted.into_parts(),
        Err(error) => fail(error.name()),
    };
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_diagnostics::name!("interrupt-table"; 32));
    };
    let software = runtime_software_interrupts;
    // Hart 1's executor owns `FROM_CPU1`'s line from here, so the boot
    // hart's raise reaches it once hart 1 routes the line's source.
    let hart1_executor = InterruptExecutor::<Esp32s31, 1>::new(software.line1);
    // The supervisor first: it issues the seat every watched channel needs.
    let Some((supervisor, responder)) =
        liveness::pair(&BEAT_SEAT, &BEAT_POSTED, &BEAT_ANSWERED, LIVENESS, &OBLIGATIONS)
    else {
        fail(oer_diagnostics::name!("liveness-claimed"; 32));
    };
    let (Some(exchange_seat), Some(aes_seat)) = (supervisor.watch(), supervisor.watch()) else {
        fail(oer_diagnostics::name!("obligation-seats"; 32));
    };
    let Some((exchange, server)) =
        oer_espressif_executor::peer::pair(&SLOT, &POSTED, &ANSWERED, exchange_seat)
    else {
        fail(oer_diagnostics::name!("exchange-claimed"; 32));
    };
    let Some((aes_client, aes_server)) =
        oer_espressif_executor::peer::pair(&AES_SLOT, &AES_POSTED, &AES_ANSWERED, aes_seat)
    else {
        fail(oer_diagnostics::name!("aes-claimed"; 32));
    };
    // The boot hart's executor writes the SYSTIMER registers; hart 1's
    // executor gets comparator 1 through a grant it routes on hart 1.
    let timer = runtime_system_timer;
    let mut timers = oer_espressif_executor::init::<Esp32s31, 0, _, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    let hart1_alarm =
        oer_espressif_executor::grant::<Esp32s31, 1, _>(&mut timers, interrupts.hart1_alarm, timer.alarm1);
    // Hart 1 starts before any executor: it installs its own stacks and
    // exception path, then its own interrupt context, timer and executor; a
    // failure after its release resets.
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
            alarm: hart1_alarm,
            aes,
            exchange: server,
            unwrap: aes_server,
            liveness: responder,
        },
    )
    .unwrap_or_else(|failure| failure.reset());
    let _ = diagnostics.event(&SecondaryReady {
        hart: u32::from(hart1.hart()),
        sp: hart1.ready_sp(),
    });
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    let watchdog = runtime_watchdogs.rtc;
    let peers = Peers {
        exchange,
        aes: aes_client,
    };
    match executor
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            (),
            move |spawner| {
                if Tasks::rtc_watchdog(spawner, watchdog).is_err()
                    || Tasks::supervise(spawner, supervisor).is_err()
                    || Tasks::check(spawner, peers).is_err()
                {
                    fail(oer_diagnostics::name!("task-start"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

/// Hart 1's main, on its own stacks with its interrupt stack and exception
/// path installed: make its own interrupt context, route its executor's
/// timer, build the AES engine over its silent interrupt, start its executor
/// on `FROM_CPU1` with its tasks, enable its interrupts and wait for them.
fn hart1_main(handoff: SecondaryHandoff<Port, 1>, parts: Hart1Parts) -> ! {
    let Hart1Parts {
        executor,
        irq,
        token,
        alarm,
        aes,
        exchange,
        unwrap,
        liveness,
    } = parts;
    let mut adopted = match handoff.adopt(INTERRUPT_TABLE) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    let (hart, router) = adopted.parts();
    alarm
        .bind(hart, router)
        .unwrap_or_else(|failure| failure.reset());
    // No entry of the table binds the AES interrupt and hart 1's router
    // silenced its own bank at install: the engine polls.
    let Ok(silent) = router.silent::<AesInterrupt>() else {
        fail(oer_diagnostics::name!("hart1-aes-silent"; 32));
    };
    let engine = oer_esp32s31_compose::crypto::key_unwrap(aes, silent);
    let spawner = executor
        .start::<Hart1Tasks, _, _, 1, _>(hart, router, token, irq, ())
        .unwrap_or_else(|failure| failure.reset());
    let id = u16::from(hart.id());
    if Hart1Tasks::respond(&spawner, exchange, id).is_err()
        || Hart1Tasks::unwrap(&spawner, unwrap, engine, id).is_err()
        || Hart1Tasks::beat(&spawner, liveness).is_err()
    {
        fail(oer_diagnostics::name!("hart1-task-start"; 32));
    }
    match adopted.enable_interrupts() {
        Ok(_router_and_hart) => oer_espressif_staged_runtime::idle(),
        Err(error) => fail(error.name()),
    }
}

/// Hart 1's task: answer each request with the square of its value and the
/// hart it ran on.
async fn respond(mut server: Server, hart: u16) {
    // `serve` returns only when polled outside hart 1's executor.
    let _foreign = server
        .serve(|exchange| {
            let value = u32::from(exchange.value);
            exchange.answer = value.wrapping_mul(value);
            exchange.hart = hart;
        })
        .await;
    panic::reset_fault::<Port>(
        panic::FaultReason::SignalContext,
        [u32::from(hart), 0, 0, 0],
    )
}

/// Hart 1's AES task: take each unwrap request, wait [`HART1_TIMER`] on
/// hart 1's timer, run the request on hart 1's AES engine (whose block waits
/// are hart 1's timers too) and hand the response back in the next exchange.
async fn serve_aes(mut server: AesServer, mut engine: KeyUnwrapEngine, hart: u16) {
    loop {
        let mut request = None;
        if server
            .serve_one(|exchange| request = exchange.request.take())
            .await
            .is_err()
        {
            break;
        }
        let mut response = match request {
            Some(request) => {
                // Hart 1's own timer: the boot hart's writer programs its
                // comparator, its alarm fires on hart 1. Without it the
                // collecting exchange expires. A wait past representable
                // time runs no unwrap: the check finds no response.
                if EmbassyClock.wait_for(HART1_TIMER).await.is_ok() {
                    let done = engine.unwrap_key(request).await;
                    engine = done.engine;
                    Some(done.response)
                } else {
                    None
                }
            }
            None => None,
        };
        if server
            .serve_one(|exchange| exchange.response = response.take())
            .await
            .is_err()
        {
            break;
        }
    }
    panic::reset_fault::<Port>(
        panic::FaultReason::SignalContext,
        [u32::from(hart), 1, 0, 0],
    )
}

/// Hart 1's liveness task: echo every beat.
async fn answer_beats(mut responder: LivenessResponder) {
    let _foreign = responder.serve().await;
    panic::reset_fault::<Port>(panic::FaultReason::SignalContext, [1, 2, 0, 0])
}

/// The boot hart's liveness supervisor of hart 1, for good: a beat every
/// period. An overdue beat records `LivenessOverdue` and resets; any other
/// end of supervision records its own reason and resets.
async fn supervise_hart1(supervisor: LivenessSupervisor) {
    let error = supervisor
        .supervise(&EmbassyClock, |round| {
            LIVENESS_ROUNDS.store(round.sequence, Ordering::Release);
        })
        .await;
    let (reason, words) = error.diagnostic();
    panic::reset_fault::<Port>(reason, words)
}

/// Post `request` to hart 1, due [`EXCHANGE_DEADLINE`] from now, and wait
/// for its answer; a refused post fails the boot as `busy`, and an expiry
/// records `PeerUnanswered` with the request's `kind` and deadline and
/// resets.
async fn ask<Req, const B: usize>(
    client: &mut PeerClient<Req, Hart1Executor, Hart0Executor>,
    request: Req,
    kind: u32,
    busy: oer_diagnostics::Name<B>,
) -> Req {
    let Ok(deadline) = EmbassyClock.deadline_after(EXCHANGE_DEADLINE) else {
        fail(oer_diagnostics::name!("exchange-deadline"; 32));
    };
    match client.post(request, deadline) {
        Ok(()) => {}
        Err(PostError::Foreign { .. }) => {
            panic::reset_fault::<Port>(panic::FaultReason::SignalContext, [0, kind, 0, 1])
        }
        Err(PostError::Occupied(_) | PostError::Expired { .. }) => fail(busy),
    }
    match client.answer(&EmbassyClock).await {
        Ok(reply) => reply,
        Err(PeerError::Expired { deadline }) => {
            let micros = deadline.as_micros();
            panic::reset_fault::<Port>(
                panic::FaultReason::PeerUnanswered,
                [kind, micros as u32, (micros >> 32) as u32, 0],
            )
        }
        Err(PeerError::NotPosted) => {
            panic::reset_fault::<Port>(panic::FaultReason::PeerUnanswered, [kind, 0, 0, 1])
        }
        Err(PeerError::Foreign(_)) => {
            panic::reset_fault::<Port>(panic::FaultReason::SignalContext, [0, kind, 0, 0])
        }
    }
}

/// The boot hart's check of hart 1: one request, one key unwrap on hart 1's
/// AES and the liveness supervisor's first round, each reported, then the
/// verdict.
async fn check_hart1(peers: Peers) {
    let Peers {
        mut exchange,
        aes: mut unwrap,
    } = peers;
    let reply = ask(
        &mut exchange,
        Exchange {
            value: REQUEST,
            answer: 0,
            hart: 0,
        },
        u32::from(REQUEST),
        oer_diagnostics::name!("exchange-busy"; 32),
    )
    .await;
    // The known answer of RFC 3394 §4.1, unwrapped on hart 1.
    let Ok(wrapped) = KeyData::try_copy(&RFC3394_4_1_CIPHERTEXT) else {
        fail(oer_diagnostics::name!("aes-capacity"; 32));
    };
    let request = KeyUnwrapRequest::new(Aes128Key::copy_from(&RFC3394_4_1_KEK), wrapped);
    let _taken = ask(
        &mut unwrap,
        AesExchange {
            request: Some(request),
            response: None,
        },
        AES_KIND,
        oer_diagnostics::name!("aes-busy"; 32),
    )
    .await;
    let collected = ask(
        &mut unwrap,
        AesExchange {
            request: None,
            response: None,
        },
        AES_KIND,
        oer_diagnostics::name!("aes-busy"; 32),
    )
    .await;
    let matched = collected
        .response
        .as_ref()
        .is_some_and(|response| response.plaintext() == Ok(&RFC3394_4_1_KEY_DATA[..]));
    // The supervisor posted its first beat when it started: by its deadline
    // the round completed, or its expiry reset the image.
    let mut rounds = LIVENESS_ROUNDS.load(Ordering::Acquire);
    if rounds == 0 {
        let Ok(bound) = EmbassyClock.deadline_after(FIRST_ROUND) else {
            fail(oer_diagnostics::name!("liveness-deadline"; 32));
        };
        EmbassyClock.wait_until(bound).await;
        rounds = LIVENESS_ROUNDS.load(Ordering::Acquire);
    }
    // boot-smoke runs no console driver: its last records go through the
    // early writer, within its bounded budget per record, and continue the
    // boot's shared sequence, which the host checks for loss and order.
    let mut console = Console::shared(|bytes| {
        if usb_serial_jtag::early_text(bytes).is_whole() {
            Ok(())
        } else {
            Err(())
        }
    });
    let _ = console.event(&HartExchange {
        hart: reply.hart,
        request: reply.value,
        answer: reply.answer,
    });
    let _ = console.event(&HartAes {
        hart: reply.hart,
        matched,
    });
    if !matched {
        fail(oer_diagnostics::name!("hart1-aes"; 32));
    }
    let _ = console.event(&HartLiveness {
        hart: reply.hart,
        rounds,
    });
    if rounds == 0 {
        fail(oer_diagnostics::name!("liveness-round"; 32));
    }
    let _ = console.event(&BootSmokePassed);
}

/// The request kind the key unwrap's expiry records.
const AES_KIND: u32 = 0x0ae5;

/// Feed the RTC watchdog for good: the image resets when this executor
/// stops polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Retain and report a failed boot step before explicitly resetting the image.
fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    oer_espressif_staged_runtime::fatal::boot_failure::<Port>(
        reason.as_str(),
        || {
            let _note = usb_serial_jtag::fatal_note(&oer_diagnostics::Note::runtime_fail(reason));
        },
    )
}

oer_espressif_executor::time_driver!(Esp32s31);

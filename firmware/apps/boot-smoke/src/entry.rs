use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_hal::peripherals::usb_serial_jtag;
use oer_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_esp32s31_hal::peripherals::{
    software_interrupt::{FromCpu0Irq, FromCpu1Irq},
    system_timer::SystimerAlarm0,
};
use oer_esp32s31_hal::cores::Cores;
use oer_esp32s31_hal::platform::Port;
use oer_esp32s31_hal::root::Root;
use oer_espressif_executor::peer::{PeerClient, PeerError, PeerServer};
use oer_espressif_executor::{Executor, InterruptExecutor};
use oer_espressif_interrupt_table::signal::{FromPeerHart, IrqSignal};
use oer_espressif_staged_runtime::diagnostics::{
    self, BootSmokePassed, Console, HartExchange, SecondaryReady,
};
use oer_espressif_staged_runtime::{SecondaryHandoff, panic};
use oer_memory::{InPlace, RequestSlot};
use oer_time::Clock as _;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; hart1_executor: 2; }
    /// The boot hart's executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 16, context app_wake, hart 0, Hardware;
    /// The boot hart's executor's timer alarm; it only acknowledges and pends.
    alarm: AlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<Tasks, 0, SystimerAlarm0>, line 17, context app_wake, hart 0, Hardware;
    /// Hart 1's interrupt-mode executor on `FROM_CPU1`, routed to hart 1's bank.
    hart1: Hart1Token = FromCpu1Irq => oer_espressif_executor::interrupt_handler::<Hart1Tasks, FromCpu1Irq>, line 16, context hart1_executor, hart 1, Executor;
}

/// The boot hart's thread-mode executor on `FROM_CPU0`.
type Hart0Executor = Executor<Esp32s31, 0>;
/// Hart 1's interrupt-mode executor on `FROM_CPU1`.
type Hart1Executor = InterruptExecutor<Esp32s31, 1>;

oer_espressif_executor::tasks! {
    /// The boot hart's tasks.
    Tasks: Executor<Esp32s31, 0> {
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
        exchange(client: Client, sequence: u32) = exchange_with_hart1;
    }
}

oer_espressif_executor::tasks! {
    /// Hart 1's task.
    Hart1Tasks: InterruptExecutor<Esp32s31, 1> {
        respond(server: Server, hart: u16) = respond;
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
/// How long the boot hart waits for hart 1's answer before it records and
/// resets: far above one software interrupt to hart 1 and back.
const EXCHANGE_DEADLINE: oer_time::Duration = oer_time::Duration::from_millis(100);

/// The boot hart's end of the exchange.
type Client = PeerClient<Exchange, Hart1Executor, Hart0Executor>;
/// Hart 1's end of the exchange.
type Server = PeerServer<Exchange, Hart1Executor, Hart0Executor>;

oer_espressif_staged_runtime::placed_static! {
    /// A request was posted for hart 1's task: raised by the boot hart's task.
    static POSTED: IrqSignal<Hart1Executor> = IrqSignal::new::<FromPeerHart<Hart0Executor, WakeToken>, Hart1Token>(), in critical_data "hart_exchange";
    /// The request was answered: raised by hart 1's task.
    static ANSWERED: IrqSignal<Hart0Executor> = IrqSignal::new::<FromPeerHart<Hart1Executor, Hart1Token>, WakeToken>(), in critical_data "hart_exchange";
    /// The one exchange between the two harts.
    static SLOT: RequestSlot<Exchange, (), InPlace> = RequestSlot::new_in_place(), in critical_data "hart_exchange";
}

/// What hart 1 takes at its launch: its executor, which owns `FROM_CPU1`'s
/// line, the line's interrupt part and table token, and the exchange's
/// server.
struct Hart1Parts {
    executor: Hart1Executor,
    irq: FromCpu1Irq,
    token: Hart1Token,
    server: Server,
}

oer_espressif_staged_runtime::entry!(main, Port);
oer_espressif_staged_runtime::secondary_entry!(1, HART1: Port, Hart1Parts => hart1_main);

fn main(handoff: oer_espressif_staged_runtime::Handoff<Port>) -> ! {
    // Before anything can panic and overwrite it.
    let previous = panic::peek_previous::<Port>();
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
            let mut diagnostics = Console::new(|bytes| {
                if usb_serial_jtag::early_text(bytes) {
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
    let Some((client, server)) = oer_espressif_executor::peer::pair(&SLOT, &POSTED, &ANSWERED)
    else {
        fail(oer_diagnostics::name!("exchange-claimed"; 32));
    };
    // Hart 1 starts before any executor: it installs its own stacks and
    // exception path, then its own interrupt context and executor; a failure
    // after its release resets.
    let Ok(Some(mut cores)) = Cores::take() else {
        fail(oer_diagnostics::name!("cores-taken"; 32));
    };
    let mut timer = runtime_system_timer;
    let (hart1, counter) = oer_esp32s31_compose::secondary::launch_hart1(
        &mut cores,
        hart1,
        reset_causes,
        timer.counter,
        &HART1,
        Hart1Parts {
            executor: hart1_executor,
            irq: software.irq1,
            token: interrupts.hart1,
            server,
        },
    )
    .unwrap_or_else(|failure| failure.reset());
    timer.counter = counter;
    let _ = diagnostics.event(&SecondaryReady {
        hart: u32::from(hart1.hart()),
        sp: hart1.ready_sp(),
    });
    // The exchange task's records continue this boot's frame sequence.
    let sequence = diagnostics.next_sequence();
    let _timer_ready = oer_espressif_executor::init::<Esp32s31, 0, _, _>(
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
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    let watchdog = runtime_watchdogs.rtc;
    match executor
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            move |spawner| {
                if Tasks::rtc_watchdog(spawner, watchdog).is_err()
                    || Tasks::exchange(spawner, client, sequence).is_err()
                {
                    fail(oer_diagnostics::name!("task-start"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

/// Hart 1's main, on its own stacks with its interrupt stack and exception
/// path installed: make its own interrupt context, start its executor on
/// `FROM_CPU1` with its one task, enable its interrupts and wait for them.
fn hart1_main(handoff: SecondaryHandoff<Port, 1>, parts: Hart1Parts) -> ! {
    let Hart1Parts {
        executor,
        irq,
        token,
        server,
    } = parts;
    let mut adopted = match handoff.adopt(INTERRUPT_TABLE) {
        Ok(adopted) => adopted,
        Err(error) => fail(error.name()),
    };
    let (hart, router) = adopted.parts();
    let spawner = executor
        .start::<Hart1Tasks, _, _, 1, _>(hart, router, token, irq, ())
        .unwrap_or_else(|failure| failure.reset());
    if Hart1Tasks::respond(&spawner, server, u16::from(hart.id())).is_err() {
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

/// The boot hart's task: send hart 1 one request and wait for its answer
/// under the executor's timer, report both and pass. No answer by the
/// deadline records and resets: completion supervision beyond one deadline
/// is not part of this image.
async fn exchange_with_hart1(mut client: Client, sequence: u32) {
    if client
        .post(Exchange {
            value: REQUEST,
            answer: 0,
            hart: 0,
        })
        .is_err()
    {
        fail(oer_diagnostics::name!("exchange-busy"; 32));
    }
    let Ok(deadline) = EmbassyClock.deadline_after(EXCHANGE_DEADLINE) else {
        fail(oer_diagnostics::name!("exchange-deadline"; 32));
    };
    let reply = match client.answer(&EmbassyClock, deadline).await {
        Ok(reply) => reply,
        Err(PeerError::Expired) => {
            let micros = deadline.as_micros();
            panic::reset_fault::<Port>(
                panic::FaultReason::PeerUnanswered,
                [
                    u32::from(REQUEST),
                    micros as u32,
                    (micros >> 32) as u32,
                    0,
                ],
            )
        }
        Err(PeerError::Foreign(_)) => {
            panic::reset_fault::<Port>(panic::FaultReason::SignalContext, [0; 4])
        }
    };
    // boot-smoke runs no console driver: its last two records go through
    // the early writer, within its bounded budget per record, and continue
    // the boot console's sequence, which the host checks for loss and order.
    let mut console = Console::starting_at(|bytes| {
        if usb_serial_jtag::early_text(bytes) {
            Ok(())
        } else {
            Err(())
        }
    }, sequence);
    let _ = console.event(&HartExchange {
        hart: reply.hart,
        request: reply.value,
        answer: reply.answer,
    });
    let _ = console.event(&BootSmokePassed);
}

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

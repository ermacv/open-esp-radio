// The image declares the trace class in its manifest; `cargo fw` sets the cfg.
#[cfg(not(oer_trace))]
compile_error!(
    "w3 declares the trace class: build it with `cargo fw build w3`, check it with RUSTFLAGS=\"--cfg oer_trace\""
);

mod future;
mod irq;
mod network;
mod radio;
mod usb;
use oer_esp32s31_compose::Esp32s31;
use oer_firmware_apps_w3::report;

use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::peripherals::usb_serial_jtag::ConsoleInterrupt;
use oer_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_esp32s31_hal::peripherals::{
    efuse::{FactoryIdentity, Interface},
    software_interrupt::{FromCpu0Irq, FromCpu2Irq, FromCpu3Irq},
    system_timer::{SystimerAlarm0, SystimerAlarm1},
};
use oer_esp32s31_hal::root::Root;
use oer_esp32s31_wifi::{self as wifi, InterruptTypes};
use oer_espressif_executor::{Doorbell, Executor, InterruptExecutor};
use oer_memory::OnceValue;
use static_cell::StaticCell;

#[derive(Clone, Copy)]
pub struct W3Config {
    pub channel: u8,
    pub stand: [u8; 6],
    pub ssid: &'static [u8],
    pub country: [u8; 2],
    pub cap_dbm: i8,
}
include!(concat!(env!("OUT_DIR"), "/w3_config.rs"));

mod phy_pilot;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; radio_executor: 2; radio: 3; }
    /// The application executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 40, context app_wake, hart 0, Hardware;
    // SOURCE: family.toml [interrupts] highest-line; HW23_S31_P2_ARBITRATION_20261027
    // measures only this P2 pair on S31: FROM_CPU2 line17 before TIMG0 line16.
    phy: PhyToken = oer_esp32s31_hal::peripherals::timg0_alarm::Timg0AlarmInterrupt => phy_pilot::interrupt, line 16, context radio_executor, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<RadioTasks, FromCpu2Irq>, line 17, context radio_executor, hart 0, Executor;
    /// The radio handler (P3): both Wi-Fi MAC banks and the doorbell.
    wifi: WifiToken = irq::WifiPart => crate::irq::on_wifi, line 18, context radio, hart 0, Hardware;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<Esp32s31, 0, SystimerAlarm0>, line 19, context radio, hart 0, Hardware;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<Esp32s31, 1, SystimerAlarm1>, line 41, context app_wake, hart 0, Hardware;
    /// The console's USB Serial/JTAG interrupt; it raises the endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => crate::usb::interrupt, line 42, context app_wake, hart 0, Hardware;
}

use usb::Console;

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    RadioTasks: InterruptExecutor<Esp32s31, 2> {
        radio_actor(actor: &'static mut Actor) = radio_actor;
        driver(driver: wifi::Driver<radio::binding::Binding>) = radio::drive;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    AppTasks: Executor<Esp32s31, 0> {
        reporting(console: Console, pads: oer_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads) = reporting;
        station_task(
            runner: wifi::StationRunner<radio::binding::Binding>,
            control: wifi::Control<'static>,
            net: wifi::Network<'static>,
            config: W3Config,
            address: [u8; 6],
            seed: u64,
        ) = radio::serve;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}
static REPORT: OnceValue<report::Report> = OnceValue::new();
static ACTOR: StaticCell<Actor> = StaticCell::new();

type Actor = wifi::RadioActor<crate::phy_pilot::Client>;

/// The image's entropy owner.
pub(crate) type Rng = oer_esp32s31_hal::peripherals::entropy::Entropy<EmbassyClock>;

oer_esp32s31_hal::phy_fault_reporter!(oer_esp32s31_compose::phy_fault::reset);
oer_espressif_staged_runtime::entry!(main, oer_esp32s31_hal::platform::Port);
fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    let previous =
        oer_espressif_staged_runtime::panic::peek_previous::<oer_esp32s31_hal::platform::Port>();
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32));
    };
    let (runtime, _psram, mut adopted) = match oer_espressif_staged_runtime::adoption::run::<oer_esp32s31_compose::Esp32s31, _, _>(
        handoff,
        root.into_runtime(),
        INTERRUPT_TABLE,
        board::BootstrapPsram,
        oer_esp32s31_compose::phy_fault::report_previous,
        |hardware| {
            oer_esp32s31_compose::wifi_fault::report_previous();
            hardware
        },
    ) {
        Ok(adopted) => adopted.into_parts(),
        Err(error) => fail(error.name()),
    };
    if let Some(record) = previous {
        usb::early_previous_panic(&record);
    }
    let config = CONFIG;
    let Some(interrupts) = Interrupts::take() else {
        park()
    };
    let factory = FactoryIdentity::read(runtime.efuse);
    let station = factory.address(Interface::Station);
    let ap = factory.address(Interface::AccessPoint);
    let radio_resources = wifi::RadioResources::new::<radio::binding::Binding>(
        runtime.radio,
        runtime.clocks,
        &factory,
    );
    let phy_pilot = crate::phy_pilot::init(runtime.timg0_alarm, adopted.router(), interrupts.phy);
    let (actor, hardware) = radio_resources.start(phy_pilot);
    let actor = ACTOR.init(actor);
    // The radio executor writes the system timer; the application executor
    // publishes its deadlines to it.
    let timer = runtime.system_timer;
    oer_espressif_executor::init::<Esp32s31, 2, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.radio_alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    oer_espressif_executor::bind::<Esp32s31, 0, _, _>(
        adopted.router(),
        interrupts.app_alarm,
        timer.alarm1,
    )
    .unwrap_or_else(|failure| failure.reset());
    let software = runtime.software_interrupts;
    // The radio handler's part holds both banks and the doorbell's lowering
    // half; its binding makes the doorbell's ringing half.
    let (line, lower) = oer_espressif_executor::DoorbellLine::<Esp32s31, 3>::split(software.line3);
    let bound = adopted
        .router()
        .bind(
            interrupts.wifi,
            wifi::Interrupts::part(runtime.wifi_interrupts, software.irq3, wifi::Lower(lower)),
        )
        .unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<
                oer_esp32s31_hal::platform::Port,
                _,
            >(error)
        });
    let (bell, urgent) = Doorbell::new::<FromCpu3Irq>(line, &bound);
    let bell = wifi::Bell(bell);
    let (hart, router) = adopted.parts();
    let radio = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, urgent)
        .unwrap_or_else(|failure| failure.reset());
    let mut rng = Rng::new(runtime.entropy, EmbassyClock);
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let entropy = rng.random();
    let setup = wifi::Config::new(
        config.channel,
        config.country,
        config.cap_dbm,
        Some(config.stand),
        false,
    )
    .unwrap_or_else(|error| radio::failure(error));
    let (control, net, runners) = wifi::station(
        wifi::Resources {
            events: radio::ProductEvents,
            storage: radio::binding::storage(),
            cell: hardware.cell,
            wifi: hardware.wifi,
            rng,
            address: station,
            ap,
            bound,
            bell,
            entropy,
            interrupts: &irq::LINE,
            mac: &irq::MAC_SIGNAL,
            power: &irq::POWER_SIGNAL,
            rx: irq::RX
                .take()
                .unwrap_or_else(|| radio::failure(wifi::InitializationError::Rx)),
            tx: [
                irq::TX0
                    .take()
                    .unwrap_or_else(|| radio::failure(wifi::InitializationError::Tx)),
                irq::TX1
                    .take()
                    .unwrap_or_else(|| radio::failure(wifi::InitializationError::Tx)),
                irq::TX2
                    .take()
                    .unwrap_or_else(|| radio::failure(wifi::InitializationError::Tx)),
                irq::TX3
                    .take()
                    .unwrap_or_else(|| radio::failure(wifi::InitializationError::Tx)),
                irq::TX4
                    .take()
                    .unwrap_or_else(|| radio::failure(wifi::InitializationError::Tx)),
                irq::TX5
                    .take()
                    .unwrap_or_else(|| radio::failure(wifi::InitializationError::Tx)),
                irq::TX6
                    .take()
                    .unwrap_or_else(|| radio::failure(wifi::InitializationError::Tx)),
                irq::TX7
                    .take()
                    .unwrap_or_else(|| radio::failure(wifi::InitializationError::Tx)),
                irq::TX8
                    .take()
                    .unwrap_or_else(|| radio::failure(wifi::InitializationError::Tx)),
            ],
        },
        setup,
    );
    if RadioTasks::radio_actor(&radio, actor).is_err()
        || RadioTasks::driver(&radio, runners.driver).is_err()
    {
        fail(oer_diagnostics::name!("task-start";32));
    }
    let Ok((console, pads)) = usb::open(adopted.router(), interrupts.console_usb) else {
        fail(oer_diagnostics::name!("console"; 32));
    };
    let executor = Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(router) => router,
        Err(error) => fail(error.name()),
    };
    let watchdog = runtime.watchdogs.rtc;
    match executor
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            urgent,
            move |spawner| {
                if AppTasks::reporting(spawner, console, pads).is_err()
                    || AppTasks::station_task(
                        spawner,
                        runners.station,
                        control,
                        net,
                        config,
                        station,
                        seed,
                    )
                    .is_err()
                    || AppTasks::rtc_watchdog(spawner, watchdog).is_err()
                {
                    fail(oer_diagnostics::name!("task-start"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

/// Feed the RTC watchdog for good: the image resets when this executor stops
/// polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Halt the image on a failed boot step, its reason on the console while
/// no one holds the block.
fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    oer_espressif_staged_runtime::fatal_halt::<oer_esp32s31_hal::platform::Port>(|_, _path| {
        let _note = oer_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
            &oer_diagnostics::Note::runtime_fail(reason),
        );
    })
}

async fn radio_actor(actor: &'static mut Actor) {
    let mut run = core::pin::pin!(actor.run());
    match run.as_mut().await {}
}
async fn reporting(
    mut console: Console,
    _pads: oer_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
) {
    let result = loop {
        if let Some(report) = REPORT.get() {
            break report;
        }
        use oer_time::Timer;
        let _ = EmbassyClock
            .wait_for(oer_time::Duration::from_millis(10))
            .await;
    };
    let _ = result
        .notes(async |note| oer_espressif_usb_serial_jtag::write_note(&mut console, note).await)
        .await;
}
fn park() -> ! {
    oer_espressif_staged_runtime::idle()
}

pub fn print_report(report: report::Report) {
    let _ = REPORT.set(report);
}

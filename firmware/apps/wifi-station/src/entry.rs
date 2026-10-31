mod irq;
mod phy_pilot;
mod profile;
mod station;

use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_hal::peripherals::watchdog::{
    ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters,
};
use oer_esp32s31_hal::peripherals::{
    software_interrupt::{FromCpu0Irq, FromCpu2Irq, FromCpu3Irq},
    system_timer::{SystimerAlarm0, SystimerAlarm1},
};
use oer_esp32s31_hal::root::Root;
use oer_esp32s31_wifi::{self as wifi, InterruptTypes};
use oer_espressif_executor::{Doorbell, Executor, InterruptExecutor};
use static_cell::StaticCell;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; radio_executor: 2; radio: 3; }
    /// The application executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 40, context app_wake, hart 0, Hardware;
    /// The application executor's alarm (system timer comparator 1).
    alarm: AppAlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<AppTasks, 1, SystimerAlarm1>, line 41, context app_wake, hart 0, Hardware;
    /// The radio executor's alarm (system timer comparator 0).
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<RadioTasks, 0, SystimerAlarm0>, line 19, context radio, hart 0, Hardware;
    // SOURCE: family.toml [interrupts] highest-line; HW23_S31_P2_ARBITRATION_20261027
    // measures only this P2 pair on S31: FROM_CPU2 line17 before TIMG0 line16.
    phy: PhyToken = oer_esp32s31_hal::peripherals::timg0_alarm::Timg0AlarmInterrupt => phy_pilot::interrupt, line 16, context radio_executor, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<RadioTasks, FromCpu2Irq>, line 17, context radio_executor, hart 0, Executor;
    /// The radio handler (P3): both Wi-Fi MAC banks and the doorbell.
    wifi: WifiToken = irq::WifiPart => crate::irq::on_wifi, line 21, context radio, hart 0, Hardware;
}

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2).
    RadioTasks: InterruptExecutor<Esp32s31, 2> {
        radio_actor(actor: &'static mut Actor) = radio_actor;
        driver(driver: wifi::Driver<station::binding::Binding>) = station::drive;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks (P1).
    AppTasks: Executor<Esp32s31, 0> {
        station_task(
            runner: wifi::StationRunner<station::binding::Binding>,
            control: wifi::Control<'static>,
            network: wifi::Network<'static>,
            config: wifi::Config,
            address: [u8; 6],
            nonces: &'static mut wifi::NonceSource,
        ) = station::serve;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}

static ACTOR: StaticCell<Actor> = StaticCell::new();
static NONCES: StaticCell<wifi::NonceSource> = StaticCell::new();
type Actor = wifi::RadioActor<crate::phy_pilot::Client>;

/// Retry a lost association every second, from the first connection on.
const RECONNECT: wifi::ReconnectPolicy =
    match wifi::ReconnectPolicy::every(oer_time::Duration::from_secs(1)) {
        Ok(policy) => policy,
        Err(_) => panic!("the fixed reconnect interval is nonzero"),
    };

oer_esp32s31_hal::phy_fault_reporter!(oer_esp32s31_compose::phy_fault::reset);
oer_espressif_staged_runtime::entry!(main, oer_esp32s31_hal::platform::Port);

fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    let previous =
        oer_espressif_staged_runtime::panic::peek_previous();
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32));
    };
    let (
        (
            runtime_aes,
            runtime_efuse,
            runtime_radio,
            runtime_clocks,
            runtime_timg0_alarm,
            runtime_system_timer,
            runtime_software_interrupts,
            runtime_wifi_interrupts,
            runtime_entropy,
            runtime_watchdogs,
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
            let _reset_causes = oer_esp32s31_compose::reset_causes::read(hardware.reset_cause);
            oer_esp32s31_compose::wifi_fault::report_previous();
            if let Some(record) = previous {
                let _ = oer_espressif_staged_runtime::diagnostics::emit_previous_panic(
                    |bytes| {
                        if oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes).is_whole() {
                            Ok(())
                        } else {
                            Err(())
                        }
                    },
                    &record,
                );
            }
            (
                hardware.aes,
                hardware.efuse,
                hardware.radio,
                hardware.clocks,
                hardware.timg0_alarm,
                hardware.system_timer,
                hardware.software_interrupts,
                hardware.wifi_interrupts,
                hardware.entropy,
                hardware.watchdogs,
            )
        },
    ) {
        Ok(adopted) => adopted.into_parts(),
        Err(error) => fail(error.name()),
    };
    let profile = profile::PROFILE;
    // The profile's domain, channel and power limit are checked before any
    // radio owner starts.
    let config = wifi::Config::new(profile.channel, profile.country, profile.cap_dbm, false)
        .unwrap_or_else(|error| station::failure(error))
        .with_reconnect(RECONNECT);
    let Some(silent) = adopted
        .router()
        .silent::<oer_esp32s31_hal::peripherals::aes::AesInterrupt>()
        .ok()
    else {
        fail(oer_diagnostics::name!("interrupt-table"; 32));
    };
    let engine = oer_esp32s31_compose::crypto::key_unwrap(runtime_aes, silent);
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_diagnostics::name!("interrupt-table"; 32));
    };
    let factory = oer_esp32s31_compose::platform::DeviceIdentity::read(runtime_efuse);
    let address = factory.station_address();
    let ap = factory.ap_address();
    let radio_resources = wifi::RadioResources::new::<station::binding::Binding>(
        runtime_radio,
        runtime_clocks,
        &factory,
    );
    let phy_pilot = crate::phy_pilot::init(runtime_timg0_alarm, adopted.router(), interrupts.phy);
    let (actor, hardware) = radio_resources.start(phy_pilot);
    let Some(actor) = ACTOR.try_init(actor) else {
        fail(oer_diagnostics::name!("actor-storage"; 32));
    };
    // The radio executor writes the system timer; the application executor
    // publishes its deadlines to it.
    oer_esp32s31_compose::platform::timers(
        runtime_system_timer,
        adopted.router(),
        interrupts.alarm,
        interrupts.radio_alarm,
    );
    let software = runtime_software_interrupts;
    // The radio handler's part holds both banks and the doorbell's lowering
    // half; its binding makes the doorbell's ringing half.
    let (line, lower) = oer_espressif_executor::DoorbellLine::<Esp32s31, 3>::split(software.line3);
    let bound = adopted
        .router()
        .bind(
            interrupts.wifi,
            wifi::Interrupts::part(runtime_wifi_interrupts, software.irq3, wifi::Lower(lower)),
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
    let mut rng = oer_esp32s31_compose::platform::Entropy::new(runtime_entropy, EmbassyClock);
    if !rng.is_healthy() {
        fail(oer_diagnostics::name!("entropy"; 32));
    }
    let Some(nonces) = NONCES.try_init(wifi::NonceSource::from_entropy(|seed| rng.fill(seed)))
    else {
        fail(oer_diagnostics::name!("nonce-storage"; 32));
    };
    let mut tx_bytes = [0; 4];
    rng.fill(&mut tx_bytes);
    let entropy = u32::from_le_bytes(tx_bytes);
    // No IP stack holds the station's frames: its stopper goes at once.
    let (control, network, _, runners) = wifi::station(
        wifi::Resources {
            engine,
            events: station::ProductEvents,
            storage: station::binding::storage(),
            hardware,
            rng,
            address,
            ap,
            bound,
            bell,
            entropy,
            interrupts: &irq::LINE,
            mac: &irq::MAC_SIGNAL,
            power: &irq::POWER_SIGNAL,
            dma: wifi::DmaResources::claim()
                .unwrap_or_else(|error| station::failure(wifi::InitializationError::Dma(error))),
        },
        config,
    );
    if RadioTasks::radio_actor(&radio, actor).is_err()
        || RadioTasks::driver(&radio, runners.driver).is_err()
    {
        fail(oer_diagnostics::name!("task-start"; 32));
    }
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
            urgent,
            move |spawner| {
                if AppTasks::station_task(
                    spawner,
                    runners.station,
                    control,
                    network,
                    config,
                    address,
                    nonces,
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

async fn radio_actor(actor: &'static mut Actor) {
    match actor.run().await {}
}

/// Feed the RTC watchdog for good: the image resets when this executor stops.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Retain and report a failed step before explicitly resetting the image.
fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    oer_espressif_staged_runtime::fatal::boot_failure::<oer_esp32s31_hal::platform::Port>(
        reason.as_str(),
        || {
            let _note = oer_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
                &oer_diagnostics::Note::runtime_fail(reason),
            );
        },
    )
}

oer_espressif_executor::time_driver!(Esp32s31);

mod irq;
mod report;
mod scan;
mod usb;

use oer_esp32s31_compose::Esp32s31;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_board as board;
use oer_esp32s31_hal::peripherals::{efuse::{FactoryIdentity, Interface}, software_interrupt::{FromCpu0Irq, FromCpu2Irq, FromCpu3Irq}, system_timer::{SystimerAlarm0, SystimerAlarm1}};
use oer_esp32s31_hal::power::ChipClocks;
use oer_esp32s31_hal::root::Root;
use oer_esp32s31_hal::peripherals::usb_serial_jtag::ConsoleInterrupt;
use oer_esp32s31_hal::peripherals::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters};
use oer_esp32s31_ieee80211_driver::irq::WifiIrqPart;
use oer_esp32s31_phy::{
    NoopPhyTargetObserver, PhyCalibrationIdentity, PhyRegisterConfig, concurrent::ConcurrentPhy,
    phy_get_rf_cal_version,
};
use oer_esp32s31_radio_client::{LpClock, RcSlow, claim};
use oer_esp32s31_radio_system::{Esp32s31Radio, RadioSystem};
use oer_espressif_executor::{Doorbell, Executor, InterruptExecutor};
use oer_memory::{AffineSpscQueue, AffineSpscReceiver};
use static_cell::StaticCell;

#[derive(Clone, Copy)]
pub struct W2Config {
    pub channel: u8,
    pub stand: [u8; 6],
    pub ssid: &'static [u8],
    pub country: [u8; 2],
    pub cap_dbm: i8,
}
include!(concat!(env!("OUT_DIR"), "/w2_config.rs"));

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
    /// The radio handler (P3): both MAC banks and the doorbell.
    wifi: WifiToken = WifiIrqPart<irq::Lower> => crate::irq::on_wifi, line 18, context radio, hart 0, Hardware;
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
        scanner(boot: scan::Boot) = scan::scanner;
    }
}

oer_espressif_executor::tasks! {
    /// The application executor's tasks.
    AppTasks: Executor<Esp32s31, 0> {
        reporting(
            console: Console,
            pads: oer_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
            receiver: AffineSpscReceiver<'static, report::Report, 1>,
        ) = reporting;
        rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_rtc_watchdog;
    }
}
static REPORT: StaticCell<AffineSpscQueue<report::Report, 1>> = StaticCell::new();
static ACTOR: StaticCell<Actor> = StaticCell::new();

type Actor = RadioSystem<
    oer_esp32s31_radio_client::wifi::Wifi,
    RcSlow,
    Esp32s31Radio<
        ChipClocks,
        oer_espressif_executor::Cooperative<Esp32s31, EmbassyClock>,
        NoopPhyTargetObserver,
        crate::phy_pilot::Client,
    >,
    EmbassyClock,
>;

/// The image's entropy owner.
pub(crate) type Rng = oer_esp32s31_hal::peripherals::entropy::Entropy<EmbassyClock>;

oer_esp32s31_hal::phy_fault_reporter!(oer_esp32s31_compose::phy_fault::reset);
oer_espressif_staged_runtime::entry!(main, oer_esp32s31_hal::platform::Port);
fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    let previous =
        oer_espressif_staged_runtime::panic::peek_previous::<oer_esp32s31_hal::platform::Port>();
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root-taken"; 32))
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
        fail(oer_diagnostics::name!("interrupt-table"; 32))
    };
    let factory = FactoryIdentity::read(runtime.efuse);
    let station = factory.address(Interface::Station);
    let ap = factory.address(Interface::AccessPoint);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let (domain, partitions) = runtime
        .radio
        .split::<ChipClocks, _>(
            ConcurrentPhy::new(),
            oer_esp32s31_compose::phy_fault::identity(),
        )
        .unwrap_or_else(|failure| {
            let fault = failure.fault();
            let _owners = failure;
            oer_esp32s31_compose::phy_fault::reset(fault)
        });
    let (Some((end, cells)), Some(lp)) = (claim(), LpClock::<RcSlow>::claim()) else {
        fail(oer_diagnostics::name!("claim"; 32))
    };
    let phy_pilot = crate::phy_pilot::init(runtime.timg0_alarm, adopted.router(), interrupts.phy);
    let backend = Esp32s31Radio::new(
        domain,
        partitions.wifi_coex,
        runtime.clocks,
        oer_espressif_executor::Cooperative::<Esp32s31, _>::new(EmbassyClock),
        NoopPhyTargetObserver,
        PhyRegisterConfig::new(identity),
        phy_pilot,
    );
    let actor = ACTOR.init_with(|| RadioSystem::new(end, backend, EmbassyClock, lp));
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
    oer_espressif_executor::bind::<Esp32s31, 0, _, _>(adopted.router(), interrupts.app_alarm, timer.alarm1)
        .unwrap_or_else(|failure| failure.reset());
    let software = runtime.software_interrupts;
    // The radio handler's part holds both MAC banks and the doorbell's
    // lowering half; its binding makes the doorbell's ringing half.
    let (line, ack) = oer_espressif_executor::DoorbellLine::<Esp32s31, 3>::split(software.line3);
    let bound = adopted
        .router()
        .bind(
            interrupts.wifi,
            WifiIrqPart::new(runtime.wifi_interrupts, software.irq3, irq::Lower(ack)),
        )
        .unwrap_or_else(|error| {
            oer_espressif_staged_runtime::interrupts::reset_binding::<
                oer_esp32s31_hal::platform::Port,
                _,
            >(error)
        });
    let (bell, urgent) = Doorbell::new::<FromCpu3Irq>(line, &bound);
    let bell = irq::ImageDoorbell(bell);
    let (hart, router) = adopted.parts();
    let radio = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, urgent)
        .unwrap_or_else(|failure| failure.reset());
    let (sender, receiver) = REPORT.init(AffineSpscQueue::new()).split();
    if RadioTasks::radio_actor(&radio, actor).is_err() {
        fail(oer_diagnostics::name!("radio-actor-start"; 32));
    }
    let scanner = RadioTasks::scanner(
        &radio,
        scan::Boot {
            cell: cells.wifi,
            wifi: partitions.wifi,
            rng: Rng::new(runtime.entropy, EmbassyClock),
            station,
            ap,
            config,
            bound,
            bell,
            sender,
        },
    );
    if scanner.is_err() {
        fail(oer_diagnostics::name!("scanner-start"; 32));
    }
    let (console, pads) = usb::open(adopted.router(), interrupts.console_usb);
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
                if AppTasks::reporting(spawner, console, pads, receiver).is_err()
                    || AppTasks::rtc_watchdog(spawner, watchdog).is_err()
                {
                    fail(oer_diagnostics::name!("task-start"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

/// Feed the RTC watchdog for good: the image resets when this executor
/// stops polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

async fn radio_actor(actor: &'static mut Actor) {
    let mut run = core::pin::pin!(actor.run());
    match run.as_mut().await {}
}
async fn reporting(
    mut console: Console,
    _pads: oer_esp32s31_hal::peripherals::usb_serial_jtag::UsbPads,
    receiver: AffineSpscReceiver<'static, report::Report, 1>,
) {
    let mut result = loop {
        if let Ok(report) = receiver.try_receive() {
            break report;
        }
        use oer_time::Timer;
        let _ = EmbassyClock
            .wait_for(oer_time::Duration::from_millis(10))
            .await;
    };
    result.irq_free =
        oer_espressif_staged_runtime::stacks::current_hart_interrupt_stack_free_bytes();
    if result.irq_free < 4096 {
        result.errors += 1;
    }
    let _ = result
        .notes(async |note| oer_espressif_usb_serial_jtag::write_note(&mut console, note).await)
        .await;
}
/// Report a failed boot step on the early writer and wait: no task feeds
/// the RTC watchdog, which resets the chip.
fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    usb::early_text(b"OER-RESULT: fail w2 ");
    usb::early_text(reason.as_str().as_bytes());
    usb::early_text(b"\r\n");
    oer_espressif_staged_runtime::idle()
}

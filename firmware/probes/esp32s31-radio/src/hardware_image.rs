//! Staged, no-traffic executor for the comparison session's silicon sheets.
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_hal::{
    peripherals::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters},
    peripherals::{
        efuse::FactoryIdentity,
        software_interrupt::{FromCpu0Irq, FromCpu2Irq},
        system_timer::{SystimerAlarm0, SystimerAlarm1},
    },
    power::ChipClocks,
    root::{RadioHardware, Root},
};
use oer_esp32s31_phy::{PhyCalibrationIdentity, phy_get_rf_cal_version};
use oer_espressif_executor::{Executor, InterruptExecutor};
use oer_time::{Clock, Duration, Timer};

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app: 1; radio_exec: 2; radio_irq: 3; }
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<Esp32s31, 0, FromCpu0Irq>, line 16, context app, hart 0, Hardware;
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<RadioTasks, FromCpu2Irq>, line 17, context radio_exec, hart 0, Executor;
    radio_alarm: RadioAlarm = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<RadioTasks, 0, SystimerAlarm0>, line 18, context radio_irq, hart 0, Hardware;
    app_alarm: AppAlarm = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<AppTasks, 1, SystimerAlarm1>, line 20, context app, hart 0, Hardware;
}
oer_espressif_executor::tasks! {
    RadioTasks: InterruptExecutor<Esp32s31, 2> { sheets(boot: Boot) = sheets; }
}
oer_espressif_executor::tasks! {
    AppTasks: Executor<Esp32s31, 0> { watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) = feed_watchdog; }
}
struct Boot {
    hardware: RadioHardware,
    clocks: ChipClocks,
    identity: PhyCalibrationIdentity,
}

// StaticPlace checks the actual future's size/alignment at monomorphization;
// construction writes its final place instead of moving a >4-KiB frame.
static CALIBRATION: static_cell::ConstStaticCell<oer_memory::StaticPlace<[u128; 512]>> =
    static_cell::ConstStaticCell::new(oer_memory::StaticPlace::new());

oer_esp32s31_hal::phy_fault_reporter!(oer_esp32s31_compose::phy_fault::reset);
oer_espressif_staged_runtime::entry!(main, oer_esp32s31_hal::platform::Port);
fn main(handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>) -> ! {
    let Some(root) = Root::take() else {
        fail(oer_diagnostics::name!("root"; 32));
    };
    let (
        (
            runtime_efuse,
            runtime_system_timer,
            runtime_software_interrupts,
            runtime_radio,
            runtime_clocks,
            runtime_watchdogs,
        ),
        _psram,
        mut adopted,
    ) = match oer_espressif_staged_runtime::adoption::run::<oer_esp32s31_compose::Esp32s31, _, _>(
        handoff,
        root.into_runtime(),
        INTERRUPT_TABLE,
        oer_esp32s31_board::BootstrapPsram,
        oer_esp32s31_compose::phy_fault::report_previous,
        |hardware| {
            let _reset_causes = oer_esp32s31_compose::reset_causes::read(hardware.reset_cause);
            (
                hardware.efuse,
                hardware.system_timer,
                hardware.software_interrupts,
                hardware.radio,
                hardware.clocks,
                hardware.watchdogs,
            )
        },
    ) {
        Ok(value) => value.into_parts(),
        Err(error) => fail(error.name()),
    };
    let Some(interrupts) = Interrupts::take() else {
        fail(oer_diagnostics::name!("interrupts"; 32));
    };
    let factory = FactoryIdentity::read(runtime_efuse);
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let timer = runtime_system_timer;
    let mut timer_ready = oer_espressif_executor::init::<Esp32s31, 2, _, _>(
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
        &mut timer_ready,
        adopted.router(),
        interrupts.app_alarm,
        timer.alarm1,
    )
    .unwrap_or_else(|failure| failure.reset());
    let software = runtime_software_interrupts;
    let (hart, router) = adopted.parts();
    let radio = InterruptExecutor::<Esp32s31, 2>::new(software.line2)
        .start(hart, router, interrupts.radio, software.irq2, ())
        .unwrap_or_else(|failure| failure.reset());
    let (mut router, hart) = match adopted.enable_interrupts() {
        Ok(value) => value,
        Err(error) => fail(error.name()),
    };
    let boot = Boot {
        hardware: runtime_radio,
        clocks: runtime_clocks,
        identity,
    };
    if RadioTasks::sheets(&radio, boot).is_err() {
        fail(oer_diagnostics::name!("radio-task"; 32));
    }
    match Executor::<Esp32s31, 0>::new(software.line0)
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            move |spawner| {
                if AppTasks::watchdog(spawner, runtime_watchdogs.rtc).is_err() {
                    fail(oer_diagnostics::name!("watchdog-task"; 32));
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}
fn fail<const B: usize>(reason: oer_diagnostics::Name<B>) -> ! {
    oer_espressif_staged_runtime::fatal::boot_failure::<oer_esp32s31_hal::platform::Port>(
        reason.as_str(),
        || {
            let _ = oer_esp32s31_hal::peripherals::usb_serial_jtag::fatal_note(
                &oer_diagnostics::Note::runtime_fail(reason),
            );
        },
    )
}
async fn feed_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    match watchdog
        .feed_every(
            &EmbassyClock,
            Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS),
        )
        .await {}
}
async fn sheets(boot: Boot) {
    // Wait for selection 150 before any radio operation: a reset never repeats calibration.
    crate::est_freq::status(0x1950_0000);
    while crate::est_freq::take_request() != 150 {
        EmbassyClock
            .wait_until(EmbassyClock.now().saturating_add(Duration::from_millis(1)))
            .await;
    }
    crate::est_freq::clear();
    crate::est_freq::emit(150, oer_riscv::cycle32(), 0, 0);
    crate::est_freq::status(0x1950_0002);
    let timer = oer_espressif_executor::Cooperative::<Esp32s31, _>::new(EmbassyClock);
    let started = oer_riscv::cycle32();
    let Some(place) = CALIBRATION.try_take() else {
        fail(oer_diagnostics::name!("calibration-place"; 32));
    };
    let reason = place
        .future(|| {
            crate::session::calibrated(
                boot.hardware,
                boot.clocks,
                boot.identity,
                &timer,
                oer_esp32s31_compose::phy_fault::identity(),
                |fault| oer_esp32s31_compose::phy_fault::reset(fault),
            )
        })
        .await;
    if reason != 0 {
        crate::est_freq::emit(150, reason, oer_riscv::cycle32().wrapping_sub(started), 0);
        crate::est_freq::status(0x1950_bad0);
        fail(oer_diagnostics::name!("calibration"; 32));
    }
    crate::est_freq::emit(150, 0, oer_riscv::cycle32().wrapping_sub(started), 1);
    crate::est_freq::status(0x1950_0001);
    loop {
        let request = crate::est_freq::take_request();
        if request != u32::MAX {
            let _ = crate::est_freq::open_phy_est_freq_sheet(request);
        }
        EmbassyClock
            .wait_until(EmbassyClock.now().saturating_add(Duration::from_millis(1)))
            .await;
    }
}

oer_espressif_executor::time_driver!(Esp32s31);

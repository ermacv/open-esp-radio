// B0's statically placed radio tasks and owned USB console.
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::platform::{FromCpu0Irq, FromCpu2Irq, SystimerAlarm0, SystimerAlarm1};
use oer_esp32s31_hal::usb_serial_jtag::ConsoleInterrupt;
use oer_esp32s31_hal::watchdog::{ArmedRtcWatchdog, RTC_WATCHDOG_FEED_PERIOD_MS, RwdtRegisters};
use oer_espressif_executor::{Executor, InterruptExecutor};
use static_cell::StaticCell;

#[derive(Clone, Copy)]
pub struct Profile {
    pub country: [u8; 2],
    pub cap_dbm: i8,
}
include!(concat!(env!("OUT_DIR"), "/profile.rs"));
mod bridge;
mod gate_fault;
mod host;
mod irq;
mod radio;
mod usb;

mod phy_pilot;

oer_espressif_staged_runtime::interrupt_table! {
    contexts { app_wake: 1; radio_executor: 2; radio: 3; }
    /// The application executor's wake (thread mode).
    wake: WakeToken = FromCpu0Irq => oer_espressif_executor::wake_handler::<0, FromCpu0Irq>, line 40, context app_wake, hart 0, Hardware;
    // SOURCE: family.toml [interrupts] highest-line; HW23_S31_P2_ARBITRATION_20261027
    // measures only this P2 pair on S31: FROM_CPU2 line17 before TIMG0 line16.
    phy: PhyToken = oer_esp32s31_hal::platform::timg0_alarm::Timg0AlarmInterrupt => phy_pilot::interrupt, line 16, context radio_executor, hart 0, Exempt(ExecutorLevel, "SAR/PWDET steps between radio executor rounds");
    /// The radio executor (P2), polled in its handler.
    radio: RadioToken = FromCpu2Irq => oer_espressif_executor::interrupt_handler::<radio::RadioTasks, FromCpu2Irq>, line 17, context radio_executor, hart 0, Executor;
    /// The radio executor's timer alarm; it only acknowledges and pends.
    radio_alarm: RadioAlarmToken = SystimerAlarm0 => oer_espressif_executor::alarm_interrupt::<0, SystimerAlarm0>, line 18, context radio, hart 0, Hardware;
    /// The application executor's timer alarm.
    app_alarm: AppAlarmToken = SystimerAlarm1 => oer_espressif_executor::alarm_interrupt::<1, SystimerAlarm1>, line 41, context app_wake, hart 0, Hardware;
    /// The Bluetooth bank, the modem low-power timer and the radio doorbell.
    bt: BtToken = irq::Interrupts => irq::bt, line 20, context radio, hart 0, Hardware;
    /// The console's USB Serial/JTAG interrupt; it raises the endpoint's signal.
    console_usb: ConsoleUsb = ConsoleInterrupt => usb::interrupt, line 42, context app_wake, hart 0, Hardware;
}
type X = InterruptExecutor<2>;
oer_esp32s31_hal::phy_fault_reporter!(oer_espressif_staged_runtime::phy_fault::reset);
oer_espressif_staged_runtime::entry!(main);
fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    radio::boot(handoff)
}

/// Feed the RTC watchdog for good: the image resets when the application
/// executor stops polling.
async fn feed_rtc_watchdog(watchdog: ArmedRtcWatchdog<RwdtRegisters>) {
    let period = oer_time::Duration::from_millis(RTC_WATCHDOG_FEED_PERIOD_MS);
    match watchdog.feed_every(&EmbassyClock, period).await {}
}

/// Report a failed boot step on the early writer and wait: no task feeds
/// the RTC watchdog, which resets the chip.
fn fail(reason: &'static str) -> ! {
    usb::early_text(b"OER-B0-BRINGUP: fail ");
    usb::early_text(reason.as_bytes());
    usb::early_text(b"\r\n");
    oer_espressif_staged_runtime::idle()
}

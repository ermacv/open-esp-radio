// B0's statically placed radio tasks and owned USB console.
use esp_hal::interrupt::software::SoftwareInterrupt;
use oer_embassy_time::EmbassyClock;
use oer_espressif_executor::{Executor, InterruptExecutor, LevelOrder};
use static_cell::StaticCell;

#[derive(Clone, Copy)]
pub struct Profile {
    pub country: [u8; 2],
    pub cap_dbm: i8,
}
include!(concat!(env!("OUT_DIR"), "/profile.rs"));
mod bridge;
mod host;
mod irq;
mod radio;
mod usb;

oer_espressif_staged_runtime::interrupt_table! {
    wake: WakeToken = FROM_CPU_INTR0 => oer_espressif_executor::wake_handler::<0>, Priority1, ProCpu, Hardware;
    radio: RadioToken = FROM_CPU_INTR2 => oer_espressif_executor::interrupt_handler::<radio::RadioTasks>, Priority2, ProCpu, Executor;
    radio_alarm: RadioAlarmToken = SYSTIMER_TARGET0 => oer_espressif_executor::alarm_interrupt::<0>, Priority3, ProCpu, Hardware;
    app_alarm: AppAlarmToken = SYSTIMER_TARGET1 => oer_espressif_executor::alarm_interrupt::<1>, Priority1, ProCpu, Hardware;
    doorbell: DoorbellToken = FROM_CPU_INTR3 => irq::doorbell, Priority3, ProCpu, Hardware;
    primary: PrimaryToken = MODEM_BT_MAC => irq::primary, Priority3, ProCpu, Hardware;
    nrt: NrtToken = MODEM_BT_MAC_INT1 => irq::nrt, Priority3, ProCpu, Hardware;
    timer: TimerToken = MODEM_LP_TIMER => irq::timer, Priority3, ProCpu, Hardware;
    console_usb: ConsoleUsb = USB_DEVICE => usb::interrupt, Priority1, ProCpu, Hardware;
}
type X = InterruptExecutor<2>;
const LEVELS: LevelOrder<PrimaryToken, RadioToken, WakeToken> = LevelOrder::new();
oer_espressif_staged_runtime::entry!(main);
fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    radio::boot(handoff)
}

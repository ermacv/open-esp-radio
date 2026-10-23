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
mod irq;
mod radio;

oer_espressif_staged_runtime::interrupt_table! {
    wake: WakeToken = FROM_CPU_INTR0 => oer_espressif_executor::wake_handler::<0>, Priority1, ProCpu;
    radio: RadioToken = FROM_CPU_INTR2 => oer_espressif_executor::interrupt_handler::<2>, Priority2, ProCpu;
    radio_alarm: RadioAlarmToken = SYSTIMER_TARGET0 => oer_espressif_executor::alarm_interrupt::<0>, Priority3, ProCpu;
    app_alarm: AppAlarmToken = SYSTIMER_TARGET1 => oer_espressif_executor::alarm_interrupt::<1>, Priority1, ProCpu;
    doorbell: DoorbellToken = FROM_CPU_INTR3 => irq::doorbell, Priority3, ProCpu;
}
type X = InterruptExecutor<2>;
static EXECUTOR: StaticCell<Executor<0>> = StaticCell::new();
static RADIO_EXECUTOR: StaticCell<X> = StaticCell::new();
const LEVELS: LevelOrder<RadioAlarmToken, RadioToken, WakeToken> = LevelOrder::new();
oer_espressif_staged_runtime::entry!(main);
fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    radio::boot(handoff)
}

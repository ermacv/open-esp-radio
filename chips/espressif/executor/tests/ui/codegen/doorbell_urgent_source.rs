use oer_espressif_interrupt_table::{Bound, InterruptSource};
oer_espressif_interrupt_table::__fake_parts!();
include!("support/doorbell.rs");

pub struct FromCpu1Irq;
impl InterruptSource for FromCpu1Irq {
    const SOURCE: u16 = 1;
}
pub struct FromCpu3Irq;
impl InterruptSource for FromCpu3Irq {
    const SOURCE: u16 = 3;
}
pub fn on_doorbell(_: &mut FromCpu3Irq) {}
pub fn on_executor(_: &mut RadioIrq) {}

oer_espressif_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [];
    contexts { executor: 2; radio: 3; }
    executor: ExecutorToken = RadioIrq => on_executor, line 17, context executor, hart 0, Executor;
    irq: IrqToken = FromCpu3Irq => on_doorbell, line 18, context radio, hart 0, Hardware;
}

pub fn make(bound: &Bound<IrqToken>) {
    let (line, _ack) = doorbell::DoorbellLine::<3>::split(chip::platform::FromCpu3);
    let pair = doorbell::Doorbell::<3, _, ExecutorToken>::new::<FromCpu3Irq>(line, bound);
    #[cfg(not(ui_reject))]
    let (_bell, _urgent): (_, urgent_sources::UrgentSource<FromCpu3Irq>) = pair;
    #[cfg(ui_reject)]
    let (_bell, _urgent): (_, urgent_sources::UrgentSource<FromCpu1Irq>) = pair;
}

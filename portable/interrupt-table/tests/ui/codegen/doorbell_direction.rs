use oer_interrupt_table::Bound;
oer_interrupt_table::__fake_parts!();
include!("support/doorbell.rs");

pub fn on_timer(_: &mut TimerIrq) {}
pub fn on_radio(_: &mut RadioIrq) {}
macro_rules! table {
    ($requester_level:literal) => {
        oer_interrupt_table::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { radio: 3; executor: $requester_level; }
            priority: low_to_high;
            executor: ExecutorToken = RadioIrq => on_radio, line 17, context executor, hart 0, Executor;
            irq: IrqToken = TimerIrq => on_timer, line 18, context radio, hart 0, Hardware;
        }
    };
}
#[cfg(ui_reject)]
table!(4);
#[cfg(not(ui_reject))]
table!(2);

pub fn make(bound: &Bound<IrqToken>) {
    let (line, _ack) = doorbell::Doorbell::<3>::split(chip::platform::FromCpu3);
    let _bell = doorbell::Doorbell::new::<TimerIrq, _, ExecutorToken>(line, bound); // ui-error@outside: E0080: evaluation panicked: a request handler must be above its executor on the same hart
}

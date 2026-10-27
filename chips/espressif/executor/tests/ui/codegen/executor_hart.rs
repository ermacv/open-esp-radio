#[path = "__HART_BINDING__"]
mod hart_binding;

oer_interrupt_table::__fake_parts!();
pub fn on_wake(_: &mut RadioIrq) {}

macro_rules! table {
    ($hart:literal) => {
        oer_interrupt_table::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { executor: 1; }
            wake: Wake = RadioIrq => on_wake, line 17, context executor, hart $hart, Executor;
        }
    };
}
#[cfg(not(ui_reject))]
table!(0);
#[cfg(ui_reject)]
table!(1);

pub fn startup() {
    hart_binding::check::<0, Wake>();
}

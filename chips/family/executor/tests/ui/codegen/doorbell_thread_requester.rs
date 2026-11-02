use oer_chips_family_interrupt_table::{Bound, signal::host::Foreign};
oer_chips_family_interrupt_table::__fake_parts!();
include!("support/doorbell.rs");

pub fn on_timer(_: &mut TimerIrq) {}
pub fn on_wake(_: &mut AbsentIrq) {}
// A thread-mode executor rings the doorbell through its wake entry, whose
// tasks run at level zero; its executor entry would poll no thread tasks.
macro_rules! table {
    ($handler:path, $class:ident) => {
        oer_chips_family_interrupt_table::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { radio: 3; app_wake: 1; }
            wake: WakeToken = AbsentIrq => $handler, line 16, context app_wake, hart 0, $class;
            irq: IrqToken = TimerIrq => on_timer, line 18, context radio, hart 0, Hardware;
        }
    };
}
#[cfg(ui_reject)]
table!(OnExecutor, Executor);
#[cfg(not(ui_reject))]
table!(on_wake, Hardware);

pub fn make(bound: &Bound<IrqToken>) {
    let (line, _ack) = doorbell::DoorbellLine::<chip::Fake, 3>::split(chip::platform::FromCpu3);
    let _bell = doorbell::Doorbell::<chip::Fake, 3, _, Foreign, WakeToken>::new::<TimerIrq>(line, bound); // ui-error@outside: E0080: evaluation panicked: a thread executor needs its wake entry
}

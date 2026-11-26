//@revisions: accept reject
//@[accept] check-pass
//@[reject] compile-flags: --cfg ui_reject
oer_chips_family_platform_interrupt_table::__fake_parts!();
pub fn timer(_: &mut TimerIrq) {}

macro_rules! table {
    ($context:ident) => {
        oer_chips_family_platform_interrupt_table::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { radio: 3; }
            irq: Irq = TimerIrq => timer, line 17, context $context, hart 0, Hardware;
        }
    };
}
#[cfg(ui_reject)]
table!(unknown); //~[reject] E0425
#[cfg(not(ui_reject))]
table!(radio);

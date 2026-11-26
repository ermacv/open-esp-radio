//@revisions: accept reject
//@[accept] check-pass
//@[reject] compile-flags: --cfg ui_reject
oer_chips_family_platform_interrupt_table::__fake_parts!();
pub fn timer(_: &mut TimerIrq) {}

// A context below the first level a line holds.
macro_rules! table {
    ($level:literal) => {
        oer_chips_family_platform_interrupt_table::interrupt_table! {
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { radio: 3; idle: $level; }
            timer: TimerToken = TimerIrq => timer, line 17, context radio, hart 0, Hardware;
        }
    };
}
#[cfg(ui_reject)]
table!(0); //~[reject] ERROR: evaluation panicked: a context's level is no level a line can hold
#[cfg(not(ui_reject))]
table!(1);

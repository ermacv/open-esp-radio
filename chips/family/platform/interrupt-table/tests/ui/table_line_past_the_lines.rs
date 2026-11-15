oer_chips_family_platform_interrupt_table::__fake_parts!();
pub fn timer(_: &mut TimerIrq) {}

// FAKE_LINES has lines 16 to 47: line 48 is past them.
macro_rules! table {
    ($line:literal) => {
        oer_chips_family_platform_interrupt_table::interrupt_table! { // ui-error: E0080: evaluation panicked: an entry's line is no line a peripheral source can be routed to
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { radio: 3; }
            timer: TimerToken = TimerIrq => timer, line $line, context radio, hart 0, Hardware;
        }
    };
}
#[cfg(ui_reject)]
table!(48);
#[cfg(not(ui_reject))]
table!(47);

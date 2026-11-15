oer_chips_family_platform_interrupt_table::__fake_parts!();
pub fn timer(_: &mut TimerIrq) {}
pub fn radio(_: &mut RadioIrq) {}

// Two entries of one hart on one line, at two levels.
macro_rules! table {
    ($second:literal) => {
        oer_chips_family_platform_interrupt_table::interrupt_table! { // ui-error: E0080: evaluation panicked: two entries share a line and hart
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { low: 1; radio: 3; }
            timer: TimerToken = TimerIrq => timer, line 17, context low, hart 0, Hardware;
            radio: RadioToken = RadioIrq => radio, line $second, context radio, hart 0, Hardware;
        }
    };
}
#[cfg(ui_reject)]
table!(17);
#[cfg(not(ui_reject))]
table!(18);

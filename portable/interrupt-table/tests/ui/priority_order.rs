oer_interrupt_table::__fake_parts!();
pub fn timer(_: &mut TimerIrq) {}
pub fn radio(_: &mut RadioIrq) {}

// Use literal lines as required by the vector symbol ABI.
macro_rules! table {
    ($second:literal) => {
        oer_interrupt_table::interrupt_table! { // ui-error: E0080
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { radio: 3; }
            first: First = TimerIrq => timer, line 17, context radio, hart 0, Hardware;
            second: Second = RadioIrq => radio, line $second, context radio, hart 0, Hardware;
        }
    };
}
#[cfg(ui_reject)]
table!(16);
#[cfg(not(ui_reject))]
table!(18);

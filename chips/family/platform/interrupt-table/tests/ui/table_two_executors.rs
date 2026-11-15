oer_chips_family_platform_interrupt_table::__fake_parts!();

// Two Executor entries of one hart, on one level or on two.
macro_rules! table {
    ($second:ident) => {
        oer_chips_family_platform_interrupt_table::interrupt_table! { // ui-error: E0080: evaluation panicked: two Executor entries share a level and hart
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { first: 1; second: 2; }
            radio: RadioToken = RadioIrq => OnExecutor, line 17, context first, hart 0, Executor;
            timer: TimerToken = TimerIrq => OnExecutor, line 18, context $second, hart 0, Executor;
        }
    };
}
#[cfg(ui_reject)]
table!(first);
#[cfg(not(ui_reject))]
table!(second);

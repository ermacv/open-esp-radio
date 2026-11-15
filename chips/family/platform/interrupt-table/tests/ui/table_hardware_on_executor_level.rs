oer_chips_family_platform_interrupt_table::__fake_parts!();
pub fn timer(_: &mut TimerIrq) {}

// A Hardware entry on its hart's Executor's level, or above it.
macro_rules! table {
    ($level:ident) => {
        oer_chips_family_platform_interrupt_table::interrupt_table! { // ui-error: E0080: evaluation panicked: a Hardware entry shares the level of its hart's Executor
            lines: FAKE_LINES,
            handler_attributes: [];
            contexts { executor: 2; radio: 3; }
            radio: RadioToken = RadioIrq => OnExecutor, line 17, context executor, hart 0, Executor;
            timer: TimerToken = TimerIrq => timer, line 18, context $level, hart 0, Hardware;
        }
    };
}
#[cfg(ui_reject)]
table!(executor);
#[cfg(not(ui_reject))]
table!(radio);

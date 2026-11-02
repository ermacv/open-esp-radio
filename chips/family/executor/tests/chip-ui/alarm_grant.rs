#![no_std]
use oer_espressif_executor::{AlarmGrant, Binding, TimeDriverReady};
use oer_espressif_interrupt_table::Entry;

/// What a launch mailbox requires of the parts it carries to another hart.
fn to_another_hart<T: Send>(_: T) {}

/// The grant of executor 1's comparator moves to the alarm's hart; the
/// writer's authority and a second copy of the grant do not exist there.
pub fn launch<B: Binding, A: Entry + Send>(grant: AlarmGrant<B, 1, A>, ready: TimeDriverReady<B>)
where
    A::Part: Send,
{
    #[cfg(not(ui_reject))]
    to_another_hart(grant);
    #[cfg(ui_reject)]
    let _copy = grant.clone(); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _local: TimeDriverReady<B> = ready;
    #[cfg(ui_reject)]
    to_another_hart(ready); // ui-error: E0277
}

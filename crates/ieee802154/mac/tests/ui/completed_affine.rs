#![no_std]
use oer_ieee802154::owner::Completed;
pub fn complete(completed: Completed) -> Completed {
    #[cfg(ui_reject)]
    let _second = completed.clone(); // ui-error: E0599
    completed
}

#![no_std]
use oer_ieee802154::owner::Completed;
pub fn complete(completed: Completed) -> Completed {
    #[cfg(ui_reject)]
    {
        Completed {
            interface: completed.interface(), // ui-error: E0451
            id: completed.id(),
        }
    }
    #[cfg(not(ui_reject))]
    {
        completed
    }
}

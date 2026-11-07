#![no_std]
use oer_chips_family_platform_executor::{Binding, TimeDriverReady};
pub fn startup<B: Binding>(ready: TimeDriverReady<B>) {
    #[cfg(not(ui_reject))]
    let _: TimeDriverReady<B> = ready;
    #[cfg(ui_reject)]
    let _: TimeDriverReady<B> = (); // ui-error: E0308
}

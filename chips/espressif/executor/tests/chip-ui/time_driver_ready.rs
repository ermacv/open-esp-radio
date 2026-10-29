use oer_espressif_executor::{Binding, TimeDriverReady};
pub fn startup<B: Binding>(ready: TimeDriverReady<B>) {
    #[cfg(not(ui_reject))]
    let _: TimeDriverReady<B> = ready;
    #[cfg(ui_reject)]
    let _: TimeDriverReady<B> = (); // ui-error: E0308
}

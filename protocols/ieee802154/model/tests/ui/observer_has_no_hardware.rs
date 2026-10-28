use oer_ieee802154_model::ModelHandle;

pub fn observe(handle: &ModelHandle) {
    #[cfg(ui_reject)]
    let _ = &handle.0; // ui-error: E0616
    #[cfg(not(ui_reject))]
    let _ = handle.enabled();
}

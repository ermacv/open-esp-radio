#![no_std]

#[cfg(not(ui_reject))]
const TEXT: &str = "éééééééééééééééééééééééééééééééé";
#[cfg(ui_reject)]
const TEXT: &str = "ééééééééééééééééééééééééééééééééx";

const REASON: oer_portable_diagnostics::Name<{ TEXT.len() }> = oer_portable_diagnostics::name!(TEXT); // ui-error@outside: E0080: evaluation panicked: a diagnostic name fits its budget and MAX_NAME
pub fn diagnostic() {
    let _note = oer_portable_diagnostics::Note::runtime_fail(REASON);
}

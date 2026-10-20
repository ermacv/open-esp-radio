#![no_std]
use oer_radio_port::{Abandon, Quarantine};
pub fn confirm(call: Quarantine) -> Abandon {
    #[cfg(ui_reject)]
    {
        Abandon::Stopped(oer_radio_port::HardwareStopped { _private: () }) // ui-error: E0451
    }
    #[cfg(not(ui_reject))]
    {
        Abandon::Stopped(call.stopped())
    }
}

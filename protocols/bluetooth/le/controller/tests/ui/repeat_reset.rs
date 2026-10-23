#![no_std]
use oer_bluetooth_le_controller::dtm::Reset;
use oer_bluetooth_le_port::{Idle, Session};
use oer_radio_port::PoolPath;
pub fn complete<P: PoolPath>(reset: Reset<P>, fresh: &Session<'_, P, Idle>) {
    let _ = reset.after_install(fresh);
    #[cfg(ui_reject)]
    let _ = reset.after_install(fresh); // ui-error: E0382
}

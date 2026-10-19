#![no_std]
use oer_radio_port::{DriverExit, PoisonProof};
pub fn finish(poisoned: PoisonProof) -> DriverExit<(), ()> {
    #[cfg(ui_reject)]
    let _ = poisoned.clone(); // ui-error: E0599
    DriverExit::Faulted {
        state: (),
        cause: (),
        poisoned,
    }
}

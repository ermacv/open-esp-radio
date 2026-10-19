#![no_std]
use oer_radio_port::{DriverExit, DriverLoan, PublishedPoison, PoisonState};
pub fn fault(driver: DriverLoan<'_, ()>, state: &mut PoisonState<u8>) -> DriverExit<(), u8> {
    #[cfg(ui_reject)]
    let published = PublishedPoison { private: () }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let (_, published) = state.publish(7);
    DriverExit::Faulted {
        state: (),
        cause: 7,
        poisoned: driver.fail_stop(published),
    }
}

#![no_std]
use oer_radio_port::{DriverExit, DriverLoan, Retire, RetireProof};
pub async fn retire<'p>(retire: Retire<'p>, driver: DriverLoan<'p, ()>) -> DriverExit<(), ()> {
    #[cfg(ui_reject)]
    let proof = RetireProof { private: () }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let proof = retire.requested(driver).await;
    DriverExit::Retired { state: (), proof }
}

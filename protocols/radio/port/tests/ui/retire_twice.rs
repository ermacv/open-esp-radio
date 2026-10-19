#![no_std]
use oer_radio_port::{DriverLoan, Retire, RetireProof};
pub async fn retire<'p>(retire: Retire<'p>, first_driver: DriverLoan<'p, ()>, second_driver: DriverLoan<'p, ()>) -> RetireProof {
    let first = retire.requested(first_driver);
    #[cfg(ui_reject)]
    let _ = retire.requested(second_driver); // ui-error: E0382
    first.await
}

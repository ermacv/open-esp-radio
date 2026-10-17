#![no_std]
use oer_radio_port::{DriverLoan, Retire, RetireProof};
pub async fn finish<'p, D>(driver: DriverLoan<'p, D>, retire: Retire<'p>) -> RetireProof {
    let request = retire.requested(driver);
    #[cfg(ui_reject)]
    let _still_driving = &*driver; // ui-error: E0382
    request.await
}

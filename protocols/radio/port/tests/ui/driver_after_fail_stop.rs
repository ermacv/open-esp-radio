#![no_std]
use oer_radio_port::{DriverLoan, PoisonProof, PublishedPoison};
pub fn finish<D>(driver: DriverLoan<'_, D>, published: PublishedPoison) -> PoisonProof {
    let proof = driver.fail_stop(published);
    #[cfg(ui_reject)]
    let _still_driving = &*driver; // ui-error: E0382
    proof
}

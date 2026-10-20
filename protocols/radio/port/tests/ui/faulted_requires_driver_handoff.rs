#![no_std]
use oer_radio_port::{DriverLoan, PoisonProof, PublishedPoison};
pub fn finish<D>(driver: DriverLoan<'_, D>, published: PublishedPoison) -> PoisonProof {
    #[cfg(ui_reject)]
    let proof = PoisonProof { private: () }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    let proof = driver.fail_stop(published);
    proof
}

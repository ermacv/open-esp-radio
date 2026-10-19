#![no_std]
use esp_hal::{interrupt::software::SoftwareInterrupt, peripherals::RNG, rng::RngOwner};
use oer_esp32c5_esp_hal_soc::{
    radio::{RadioSingletons, take_radio},
    watchdog::{DeadlineBudget, DeadlineWatchdog},
};
use oer_espressif_executor::Executor;

pub fn executor(line: SoftwareInterrupt<'static, 0>) {
    let _first = Executor::<0>::new(line);
    #[cfg(ui_reject)]
    let _second = Executor::<0>::new(line); // ui-error: E0382
}
pub fn rng(rng: RNG<'static>) {
    let mut owner = RngOwner::new(rng);
    let _word = owner.random();
    #[cfg(ui_reject)]
    let _second = RngOwner::new(rng); // ui-error: E0382
}
pub fn watchdog(owner: DeadlineWatchdog, budget: DeadlineBudget) {
    let _lease = owner.arm(budget);
    #[cfg(ui_reject)]
    let _second = owner.arm(budget); // ui-error: E0382
}
pub fn handoff(singletons: RadioSingletons) {
    let _root = take_radio(singletons).into_partitions();
    #[cfg(ui_reject)]
    let _second = take_radio(singletons); // ui-error: E0382
}

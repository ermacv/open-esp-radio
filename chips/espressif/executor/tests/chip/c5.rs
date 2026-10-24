#![no_std]
use esp_hal::{
    interrupt::software::SoftwareInterrupt,
    peripherals::{LP_CLKRST, LP_PERI, RNG},
};
use oer_esp32c5_esp_hal_soc::{
    handoff::{take_entropy, take_reset_cause},
    radio::{RadioSingletons, take_radio},
    watchdog::{DeadlineBudget, DeadlineWatchdog},
};
use oer_espressif_executor::Executor;

pub fn executor(line: SoftwareInterrupt<'static, 0>) {
    let _first = Executor::<0>::new(line);
    #[cfg(ui_reject)]
    let _second = Executor::<0>::new(line); // ui-error: E0382
}
pub fn entropy(rng: RNG<'static>, lp_peri: LP_PERI<'static>) {
    let mut owner = take_entropy(rng, lp_peri);
    owner.enable();
    let _word = owner.random_word();
    #[cfg(ui_reject)]
    let _second = take_entropy(
        rng,     // ui-error: E0382
        lp_peri, // ui-error: E0382
    );
}
pub fn reset_cause(lp_clkrst: LP_CLKRST<'static>) {
    let _owner = take_reset_cause(lp_clkrst);
    #[cfg(ui_reject)]
    let _second = take_reset_cause(lp_clkrst); // ui-error: E0382
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

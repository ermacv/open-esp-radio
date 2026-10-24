#![no_std]
use esp_hal::{
    interrupt::software::SoftwareInterrupt,
    peripherals::{LP_CLKRST, LP_PERI, LP_WDT, RNG, TIMG0, TIMG1},
};
use oer_esp32c5_esp_hal_soc::{
    handoff::{take_entropy, take_reset_cause, take_watchdogs},
    radio::{RadioSingletons, take_radio},
};
use oer_esp32c5_hal::watchdog::{DeadlineBudget, DeadlineWatchdog};
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
pub fn watchdog<C: oer_time::Clock>(owner: DeadlineWatchdog<C>, budget: DeadlineBudget) {
    let _lease = owner.arm(budget);
    #[cfg(ui_reject)]
    let _second = owner.arm(budget); // ui-error: E0382
}
pub fn watchdogs(timg0: TIMG0<'static>, timg1: TIMG1<'static>, lp_wdt: LP_WDT<'static>) {
    let mut owners = take_watchdogs(timg0, timg1, lp_wdt);
    owners.super_watchdog.feed_automatically();
    #[cfg(ui_reject)]
    let _second = take_watchdogs(
        timg0,  // ui-error: E0382
        timg1,  // ui-error: E0382
        lp_wdt, // ui-error: E0382
    );
}
pub fn handoff(singletons: RadioSingletons) {
    let _root = take_radio(singletons).into_partitions();
    #[cfg(ui_reject)]
    let _second = take_radio(singletons); // ui-error: E0382
}

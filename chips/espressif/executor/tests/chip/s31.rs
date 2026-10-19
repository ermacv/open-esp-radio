#![no_std]
use esp_hal::{
    interrupt::{InterruptRoutes, software::SoftwareInterrupt},
    peripherals::{PSRAM, RNG, SYSTIMER},
    timer::systimer::AlarmBank,
    usb::usb_serial_jtag::UsbPadControl,
};
use oer_espressif_esp_hal_soc::{
    entropy::Entropy,
    watchdog::{DeadlineBudget, DeadlineWatchdog},
};
use oer_espressif_executor::{DoorbellAcknowledger, Executor};

pub fn executor(line: SoftwareInterrupt<'static, 2>) {
    let _first = Executor::<2>::new(line);
    #[cfg(ui_reject)]
    let _second = Executor::<2>::new(line); // ui-error: E0382
}
pub fn doorbell(mut owner: DoorbellAcknowledger<3>) {
    #[cfg(ui_reject)]
    let _moved = owner;
    owner.acknowledge(); // ui-error: E0382
}
pub fn no_free_acknowledgement() {
    #[cfg(ui_reject)]
    oer_espressif_executor::acknowledge::<3>(); // ui-error: E0425
}
pub fn timer(timer: SYSTIMER<'static>) {
    let (mut bank, mut zero, mut one, mut two) = AlarmBank::adopt(timer);
    bank.bind(&mut zero);
    bank.bind(&mut one);
    bank.bind(&mut two);
    bank.program(0, Some(100));
    zero.acknowledge();
    #[cfg(ui_reject)]
    let _second = AlarmBank::adopt(timer); // ui-error: E0382
}
pub fn entropy(rng: RNG<'static>) {
    let mut owner = Entropy::new(rng);
    let _health = owner.source_status();
    let _bytes = owner.random_bytes::<8>();
    #[cfg(ui_reject)]
    let _second = Entropy::new(rng); // ui-error: E0382
}
pub fn usb(pads: UsbPadControl<'static>) {
    pads.disconnect();
    #[cfg(ui_reject)]
    pads.disconnect(); // ui-error: E0382
}
pub fn psram(mut psram: PSRAM<'static>) {
    let _supply = esp_hal::psram::existing_supply_configuration(&mut psram);
    #[cfg(ui_reject)]
    let _unwitnessed = esp_hal::psram::existing_supply_configuration(); // ui-error: E0061
}
pub fn matrix(routes: InterruptRoutes) {
    let _banks = routes.split();
    #[cfg(ui_reject)]
    let _second = routes.split(); // ui-error: E0382
}
pub fn no_free_matrix() {
    #[cfg(ui_reject)]
    let _matrix = oer_espressif_esp_hal_irq::EspHalMatrix; // ui-error: E0423
}
pub fn watchdog(owner: DeadlineWatchdog, budget: DeadlineBudget) {
    let _lease = owner.arm(budget);
    #[cfg(ui_reject)]
    let _second = owner.arm(budget); // ui-error: E0382
}

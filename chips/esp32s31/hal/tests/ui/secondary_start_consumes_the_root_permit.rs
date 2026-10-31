use oer_esp32s31_hal::{
    cores::{Cores, LaunchPermit, SecondaryEntry, SecondaryHart},
    peripherals::reset_cause::ResetCauses,
    platform::core_control::CoreControlRegisters,
};

pub fn start(cores: &mut Cores, permit: LaunchPermit, causes: ResetCauses, entry: SecondaryEntry) {
    let _ = cores.start_secondary(permit, causes, entry);
    #[cfg(ui_reject)]
    let _ = cores.start_secondary(permit, causes, entry); // ui-error: E0382
}

#[cfg(ui_reject)]
pub fn forge(peripherals: oer_esp32s31_pac::ownership::SecondaryLaunchPeripherals) -> LaunchPermit {
    LaunchPermit::new(peripherals) // ui-error: E0624
}

pub fn containment(registers: &mut CoreControlRegisters, hart: SecondaryHart) {
    #[cfg(ui_reject)]
    registers.release(hart); // ui-error: E0599
    #[cfg(not(ui_reject))]
    registers.hold(hart);
}

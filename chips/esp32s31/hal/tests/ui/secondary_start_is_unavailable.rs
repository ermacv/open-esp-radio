use oer_esp32s31_hal::{
    cores::{Cores, SecondaryHart},
    platform::core_control::CoreControlRegisters,
};

pub fn image(_cores: &mut Cores, _hart: SecondaryHart, _entry: extern "C" fn() -> !) {
    #[cfg(ui_reject)]
    let _ = _cores.start(_hart, _entry); // ui-error: E0599
    #[cfg(not(ui_reject))]
    let _ = _cores.status(_hart);
}

pub fn containment(_registers: &mut CoreControlRegisters, _hart: SecondaryHart) {
    #[cfg(ui_reject)]
    _registers.release(_hart); // ui-error: E0599
    #[cfg(not(ui_reject))]
    _registers.hold(_hart);
}

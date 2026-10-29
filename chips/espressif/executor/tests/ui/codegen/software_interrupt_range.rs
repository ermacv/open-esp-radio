oer_espressif_interrupt_table::__fake_parts!();
include!("support/doorbell.rs");
#[cfg(ui_reject)]
const INDEX: u8 = 4;
#[cfg(not(ui_reject))]
const INDEX: u8 = 3;
pub fn identity() -> SoftwareInterrupt {
    SoftwareInterrupt::constant::<INDEX>()
}

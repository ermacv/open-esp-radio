//! The interrupt-table handlers of the Bluetooth Controller's three sources,
//! in SRAM: each dispatches its source to the bound Controller epoch.

use crate::EspHalBluetoothInterruptSource;
use crate::bluetooth_interrupt::dispatch_bound_source;

/// The interrupt-table handler of `MODEM_BT_MAC`.
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub fn bluetooth_primary_interrupt_handler() {
    dispatch_bound_source(EspHalBluetoothInterruptSource::Primary);
}

/// The interrupt-table handler of `MODEM_LP_TIMER`.
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub fn bluetooth_modem_lp_timer_interrupt_handler() {
    dispatch_bound_source(EspHalBluetoothInterruptSource::ModemLpTimer);
}

/// The interrupt-table handler of `MODEM_BT_MAC_INT1`.
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub fn bluetooth_nrt_default_interrupt_handler() {
    dispatch_bound_source(EspHalBluetoothInterruptSource::NrtDefault);
}

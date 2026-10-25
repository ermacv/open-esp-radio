//! Channel-disjoint capabilities over the modem event-task matrix.
//!
//! IEEE 802.15.4 programs channels zero and one, the Bluetooth PHY
//! initialization channel two and the Bluetooth runtime channels four through
//! seven. The channel-enable set and clear words are
//! write-trigger — writing 1 acts on one channel and writing 0 has no effect,
//! as in the ESP32-S31 SoC ETM of identical layout; this is not yet verified
//! on the modem ETM hardware. The generated division gives each capability
//! its own channels' event and task words and only its own bits of the
//! trigger words, checked at compile time, so the two protocols need no lock,
//! and IEEE 802.15.4 may use its channels from its interrupt handler.

#![forbid(unsafe_code)]

use crate::generated::modem_etm::{ModemEtmBluetooth, ModemEtmBluetoothPhy, ModemEtmIeee802154};

/// Divide the unique modem ETM owner into the three channel capabilities,
/// which are never reunited. Every part is the generated one: the HAL drives
/// IEEE 802.15.4's and the Bluetooth ones.
pub(crate) fn split(
    peripherals: crate::generated::peripheral_ownership::ModemEtmPeripherals,
) -> (
    ModemEtmIeee802154,
    ModemEtmBluetoothPhy,
    ModemEtmBluetooth,
) {
    let crate::generated::peripheral_ownership::ModemEtmPeripherals { modem_etm } = peripherals;
    let (ieee802154, phy, bluetooth) = modem_etm.divide();
    (
        ieee802154,
        phy,
        bluetooth,
    )
}

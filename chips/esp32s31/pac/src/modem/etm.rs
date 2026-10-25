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

/// ETM channels zero and one, owned by IEEE 802.15.4.
#[must_use = "dropping the ETM channels loses their register authority"]
pub struct Ieee802154EtmChannels {
    pub(crate) registers: ModemEtmIeee802154,
}

/// ETM channel two, owned by the Bluetooth controller.
///
/// The pinned BLE PHY initialization routes modem event 8 to task 20 on a
/// channel it reserves. The vendor library uses channel zero, which
/// IEEE 802.15.4 owns here; the same route runs on this Bluetooth channel,
/// and Direct Test Mode disables and restores it as the vendor does its
/// channel zero.
#[must_use = "dropping the ETM channel loses its register authority"]
pub struct BluetoothPhyEtmChannel {
    registers: ModemEtmBluetoothPhy,
}

impl BluetoothPhyEtmChannel {
    /// Route event 8 to task 20 on channel two and enable the channel.
    pub(crate) fn route_and_enable(&mut self) {
        self.registers.route_bluetooth_phy_etm_channel2();
        self.registers.enable_bluetooth_phy_etm_channel2();
    }

    /// Disable channel two, as a Direct Test Mode event without CTE
    /// disables the vendor channel carrying this route.
    pub(crate) fn disable(&mut self) {
        self.registers.disable_bluetooth_phy_etm_channel2();
    }
}

/// ETM channels four through seven, owned by the Bluetooth runtime.
#[must_use = "dropping the ETM channels loses their register authority"]
pub struct BluetoothEtmChannels {
    pub(crate) registers: ModemEtmBluetooth,
}

/// Divide the unique modem ETM owner into the three channel capabilities,
/// which are never reunited.
pub(crate) fn split(
    peripherals: crate::generated::peripheral_ownership::ModemEtmPeripherals,
) -> (
    Ieee802154EtmChannels,
    BluetoothPhyEtmChannel,
    BluetoothEtmChannels,
) {
    let crate::generated::peripheral_ownership::ModemEtmPeripherals { modem_etm } = peripherals;
    let (ieee802154, phy, bluetooth) = modem_etm.divide();
    (
        Ieee802154EtmChannels {
            registers: ieee802154,
        },
        BluetoothPhyEtmChannel { registers: phy },
        BluetoothEtmChannels {
            registers: bluetooth,
        },
    )
}

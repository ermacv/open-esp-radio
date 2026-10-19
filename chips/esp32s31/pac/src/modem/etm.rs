//! Channel-disjoint capabilities over the modem event-task matrix.
//!
//! IEEE 802.15.4 programs channels zero and one, the Bluetooth PHY
//! initialization channel two and the Bluetooth runtime channels four through
//! seven. The channel-enable set and clear words are
//! write-trigger — writing 1 acts on one channel and writing 0 has no effect,
//! as in the ESP32-S31 SoC ETM of identical layout; this is not yet verified
//! on the modem ETM hardware. Each capability therefore writes only its own
//! bits and never reads a trigger word, so the two protocols need no lock,
//! and IEEE 802.15.4 may use its channels from its interrupt handler.

use crate::svd;

/// ETM channels zero and one, owned by IEEE 802.15.4.
#[must_use = "dropping the ETM channels loses their register authority"]
pub struct Ieee802154EtmChannels {
    pub(crate) registers: svd::ModemEtm,
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
    registers: svd::ModemEtm,
}

impl BluetoothPhyEtmChannel {
    /// Route event 8 to task 20 on channel two and enable the channel.
    pub(crate) fn route_and_enable(&self) {
        crate::svd::fixed_register_sequence::route_bluetooth_phy_etm_channel2(&self.registers);
        crate::svd::fixed_register_image::enable_bluetooth_phy_etm_channel2(&self.registers);
    }

    /// Disable channel two, as a Direct Test Mode event without CTE
    /// disables the vendor channel carrying this route.
    pub(crate) fn disable(&self) {
        crate::svd::fixed_register_image::disable_bluetooth_phy_etm_channel2(&self.registers);
    }
}

/// ETM channels four through seven, owned by the Bluetooth runtime.
#[must_use = "dropping the ETM channels loses their register authority"]
pub struct BluetoothEtmChannels {
    pub(crate) registers: svd::ModemEtm,
}

/// Split the unique modem ETM owner into the three channel capabilities.
///
/// # Safety invariant
///
/// The duplicated handles live in [`BluetoothPhyEtmChannel`], whose
/// operations write channel two's event, task and enable-set and clear bits, and in
/// [`BluetoothEtmChannels`], whose only operation is the generated channel
/// four-to-seven sequence; [`Ieee802154EtmChannels`] writes only channel zero
/// and one words and bits. The capabilities are never reunited, so the
/// complete block is never recovered while they exist.
#[allow(
    unsafe_code,
    reason = "consuming the singleton creates the channel-disjoint capabilities"
)]
pub(crate) fn split(
    peripherals: svd::peripheral_ownership::ModemEtmPeripherals,
) -> (
    Ieee802154EtmChannels,
    BluetoothPhyEtmChannel,
    BluetoothEtmChannels,
) {
    let svd::peripheral_ownership::ModemEtmPeripherals { modem_etm } = peripherals;
    // SAFETY: `modem_etm` was consumed above; see the invariant.
    let (phy, bluetooth) = unsafe { (svd::ModemEtm::steal(), svd::ModemEtm::steal()) };
    (
        Ieee802154EtmChannels {
            registers: modem_etm,
        },
        BluetoothPhyEtmChannel { registers: phy },
        BluetoothEtmChannels {
            registers: bluetooth,
        },
    )
}

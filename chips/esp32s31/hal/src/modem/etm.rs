//! Channel-disjoint owners of the modem event/task matrix.
#![forbid(unsafe_code)]
use oer_esp32s31_pac::{
    instances,
    modem_etm::{ModemEtmBluetooth, ModemEtmBluetoothPhy, ModemEtmIeee802154},
    ownership::ModemEtmPeripherals,
};
use oer_mmio::{Fixed, Placement};
type EtmPlacement = Fixed<{ instances::modem_etm::BASE }>;

#[must_use = "dropping the ETM channel loses its register authority"]
pub struct BluetoothPhyEtmChannel<P: Placement = EtmPlacement> {
    registers: ModemEtmBluetoothPhy<P>,
}
impl<P: Placement> BluetoothPhyEtmChannel<P> {
    pub fn new(registers: ModemEtmBluetoothPhy<P>) -> Self {
        Self { registers }
    }
    /// SOURCE: BLOB_LIBBLE_PHY_INIT_REGISTERS. Channel two belongs to Bluetooth.
    pub(crate) fn route_and_enable(&mut self) {
        self.registers.modify_ch2_event(|w| {
            w.set_id(8);
        });
        self.registers.modify_ch2_task(|w| {
            w.set_id(20);
        });
        self.registers.trigger_channel_enable_set_ch2();
    }
    pub(crate) fn disable(&mut self) {
        self.registers.trigger_channel_enable_clear_ch2();
    }
}
#[must_use = "dropping the ETM channels loses their register authority"]
pub struct BluetoothEtmChannels<P: Placement = EtmPlacement> {
    pub(crate) registers: ModemEtmBluetooth<P>,
}
/// Consume the generated division; it has already proved that the channels are disjoint.
pub fn split(
    parts: ModemEtmPeripherals,
) -> (
    ModemEtmIeee802154,
    BluetoothPhyEtmChannel,
    BluetoothEtmChannels,
) {
    (
        parts.modem_etm_ieee802154,
        BluetoothPhyEtmChannel::new(parts.modem_etm_bluetooth_phy),
        BluetoothEtmChannels {
            registers: parts.modem_etm_bluetooth,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_esp32s31_pac::modem_etm::modem_etm_host;
    use std::vec;
    #[test]
    fn bluetooth_phy_routes_only_channel_two_and_strobes_only_its_enable_bits() {
        for fill in [0, u32::MAX] {
            let mut memory = vec![fill; instances::modem_etm::SIZE.div_ceil(4)];
            let mut expected = memory.clone();
            let (_, phy, _) = modem_etm_host(&mut memory).expect("matrix fits");
            let mut channel = BluetoothPhyEtmChannel::new(phy);
            channel.route_and_enable();
            channel.disable();
            drop(channel);
            expected[0x1c / 4] = 8;
            expected[0x20 / 4] = 20;
            expected[0x4 / 4] = 4;
            expected[0x8 / 4] = 4;
            assert_eq!(memory, expected);
        }
    }
}

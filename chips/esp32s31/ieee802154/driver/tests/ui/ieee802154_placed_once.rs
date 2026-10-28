#![no_std]
use oer_esp32s31_hal::{dma_static::Placed, layouts::DmaWindowAddress};
use oer_esp32s31_ieee802154_driver::engine::Ieee802154Engine;
use oer_espressif_ieee802154_engine::{
    ll::Ieee802154LowLevel,
    pib::{Ieee802154Permit, Ieee802154PibDefaults},
};
use oer_espressif_ieee802154_mac_values::Ieee802154EngineBuffers;
use oer_espressif_ieee802154_mac_values::Ieee802154TxPowerLevels;

pub fn bind<Mac: Ieee802154LowLevel<Address = DmaWindowAddress>>(
    mac: Mac,
    buffers: Placed<Ieee802154EngineBuffers>,
    levels: Ieee802154TxPowerLevels<'static>,
    defaults: Ieee802154PibDefaults,
    start: Ieee802154Permit,
) {
    let _engine = Ieee802154Engine::place(mac, buffers, levels, defaults, start);
    #[cfg(ui_reject)]
    let _storage = buffers.into_inner(); // ui-error: E0382
    #[cfg(ui_reject)]
    let _mac = mac; // ui-error: E0382
}

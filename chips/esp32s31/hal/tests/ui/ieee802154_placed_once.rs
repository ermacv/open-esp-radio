#![no_std]
use oer_esp32s31_hal::{dma_static::Placed, ieee802154::{engine::Ieee802154Engine, ll::Ieee802154LowLevel}, layouts::DmaWindowAddress};
use oer_espressif_ieee802154_engine::{
    engine::Ieee802154EngineBuffers,
    pib::{Ieee802154Permit, Ieee802154PibDefaults},
};
use oer_espressif_ieee802154_mac_values::Ieee802154TxPowerLevels;

pub fn bind<Mac: Ieee802154LowLevel<Address = DmaWindowAddress>>(mac: Mac,
    buffers: Placed<Ieee802154EngineBuffers>, levels: Ieee802154TxPowerLevels<'static>,
    defaults: Ieee802154PibDefaults, start: Ieee802154Permit) {
    let _engine = Ieee802154Engine::place(mac, buffers, levels, defaults, start);
    #[cfg(ui_reject)]
    let _storage = buffers.into_inner(); // ui-error: E0382
    #[cfg(ui_reject)]
    let _mac = mac; // ui-error: E0382
}

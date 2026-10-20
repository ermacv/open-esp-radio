//! The IEEE 802.15.4 MAC and its placed DMA buffers, owned together.

use super::ll::Ieee802154MacOwners;
use crate::{dma_static::Placed, layouts::DmaWindowAddress};
use core::ops::{Deref, DerefMut};
use oer_espressif_ieee802154_engine::{
    engine::{
        Ieee802154DmaAddresses, Ieee802154Engine as SharedEngine, Ieee802154EngineBuffers,
        Ieee802154Interfaces, owner::Ieee802154EngineOwner,
    },
    ll::Ieee802154LowLevel,
    pib::{Ieee802154Permit, Ieee802154PibDefaults},
    tx_power::Ieee802154TxPowerLevels,
};

crate::dma_static! {
    #[allow(unsafe_code, reason = "DMA storage is placed through the HAL macro")]
    static BUFFERS: Ieee802154EngineBuffers = zeroed in ".dma.bss.ieee802154";
}

/// Claim the HAL's frame storage once, before constructing its engine.
pub fn claim_buffers() -> Option<Placed<Ieee802154EngineBuffers>> {
    BUFFERS.take()
}

/// The MAC token and placed frame storage of one IEEE 802.15.4 engine.
///
/// Its operations borrow its own MAC; callers see frames and slots, never
/// DMA addresses or an independently reusable register owner.
#[must_use = "retain the MAC with its placed storage for the hardware epoch"]
pub struct Ieee802154Engine<
    Ll: Ieee802154LowLevel<Address = DmaWindowAddress> = Ieee802154MacOwners,
> {
    inner: Ieee802154EngineOwner<'static, Ll>,
}

impl<Ll: Ieee802154LowLevel<Address = DmaWindowAddress>> Ieee802154Engine<Ll> {
    /// Own `mac` with the uniquely placed buffers. No register is touched.
    pub fn place(
        mac: Ll,
        buffers: Placed<Ieee802154EngineBuffers>,
        levels: Ieee802154TxPowerLevels<'static>,
        defaults: Ieee802154PibDefaults,
        start: Ieee802154Permit,
    ) -> Self {
        Self::bind(mac, buffers, levels, defaults, start, None)
    }

    /// Own `mac` and the placed buffers with multi-PAN enabled.
    pub fn place_multipan(
        mac: Ll,
        buffers: Placed<Ieee802154EngineBuffers>,
        levels: Ieee802154TxPowerLevels<'static>,
        defaults: Ieee802154PibDefaults,
        interfaces: Ieee802154Interfaces,
        start: Ieee802154Permit,
    ) -> Self {
        Self::bind(mac, buffers, levels, defaults, start, Some(interfaces))
    }

    #[allow(
        unsafe_code,
        reason = "the owner consumes and retains the placed frame storage"
    )]
    fn bind(
        mac: Ll,
        buffers: Placed<Ieee802154EngineBuffers>,
        levels: Ieee802154TxPowerLevels<'static>,
        defaults: Ieee802154PibDefaults,
        start: Ieee802154Permit,
        interfaces: Option<Ieee802154Interfaces>,
    ) -> Self {
        let buffers = buffers.into_inner();
        // SAFETY: every frame is a transparent, four-byte-aligned DMA byte image
        // inside the consumed Placed allocation. The inner engine retains its
        // unique static borrow for every hardware epoch; native models record
        // these identities without dereferencing them as pointers.
        let addresses = unsafe {
            let address = |frame: &oer_espressif_ieee802154_engine::engine::DmaFrame| {
                crate::dma_address::placed(core::ptr::from_ref(frame).addr() as u32)
            };
            Ieee802154DmaAddresses {
                rx: buffers.rx_frames().each_ref().map(address),
                tx: address(buffers.tx_frame()),
                enhanced_ack: address(buffers.enhanced_ack_frame()),
            }
        };
        let engine = match interfaces {
            Some(interfaces) => {
                SharedEngine::new_multipan(buffers, addresses, levels, defaults, interfaces, start)
            }
            None => SharedEngine::new(buffers, addresses, levels, defaults, start),
        };
        Self {
            inner: Ieee802154EngineOwner::new(engine, mac),
        }
    }
}

impl<Ll: Ieee802154LowLevel<Address = DmaWindowAddress>> Deref for Ieee802154Engine<Ll> {
    type Target = Ieee802154EngineOwner<'static, Ll>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<Ll: Ieee802154LowLevel<Address = DmaWindowAddress>> DerefMut for Ieee802154Engine<Ll> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests;

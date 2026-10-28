//! The IEEE 802.15.4 MAC and its placed DMA buffers, owned together.

use super::ll::Ieee802154MacOwners;
use crate::{dma_static::Placed, layouts::DmaWindowAddress};
use core::ops::{Deref, DerefMut};
use oer_espressif_ieee802154_engine::{
    engine::{
        Ieee802154DmaAddresses, Ieee802154Engine as SharedEngine, Ieee802154EngineBuffers,
        Ieee802154Interfaces,
        owner::{DetachedIeee802154Engine as DetachedEngine, Ieee802154EngineOwner},
    },
    ll::Ieee802154LowLevel,
    pib::{Ieee802154Permit, Ieee802154PibDefaults},
};
use oer_espressif_ieee802154_mac_values::Ieee802154TxPowerLevels;
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

/// Quiescent engine retaining the HAL's placed DMA storage between MAC epochs.
#[must_use = "retain the detached engine to reuse the same placed storage"]
pub struct DetachedIeee802154Engine {
    inner: DetachedEngine<'static, DmaWindowAddress>,
}

/// Rejected placement returns both unchanged affine owners.
pub struct PlacementRefusal<Ll> {
    pub error: crate::dma_address::ProjectionError,
    pub mac: Ll,
    pub buffers: Placed<Ieee802154EngineBuffers>,
}

impl<Ll> core::fmt::Debug for PlacementRefusal<Ll> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PlacementRefusal")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl DetachedIeee802154Engine {
    /// Rebind the retained, already placed storage to the returned MAC owners.
    /// No new storage claim, address construction or register access occurs.
    pub fn rebind<Ll: Ieee802154LowLevel<Address = DmaWindowAddress>>(
        self,
        mac: Ll,
    ) -> Ieee802154Engine<Ll> {
        Ieee802154Engine {
            inner: self.inner.rebind(mac),
        }
    }
}

impl<Ll: Ieee802154LowLevel<Address = DmaWindowAddress>> Ieee802154Engine<Ll> {
    /// Service events, returning the owner only on success.
    ///
    /// # Errors
    /// Unsupported events retain the MAC and placed buffers in quarantine.
    /// The image records the fault and resets without releasing either owner.
    #[allow(
        clippy::result_large_err,
        reason = "quarantine retains the complete affine owner"
    )]
    pub fn isr<E: oer_espressif_ieee802154_engine::engine::Ieee802154Environment + ?Sized>(
        self,
        env: &mut E,
    ) -> Result<Self, oer_espressif_ieee802154_engine::engine::owner::Quarantined<'static, Ll>>
    {
        self.inner.isr(env).map(|inner| Self { inner })
    }

    /// Detach a quiescent MAC, retaining the DMA placement proof for rebind.
    /// Stop the operation and return delivered slots first. A software-only
    /// disable cannot close an active operation epoch.
    ///
    /// # Errors
    /// Returns the unchanged owner while an operation or DMA epoch is active.
    #[allow(
        clippy::result_large_err,
        reason = "failure retains the affine engine owner"
    )]
    pub fn detach(self) -> Result<(DetachedIeee802154Engine, Ll), Self> {
        self.inner
            .detach()
            .map(|(inner, mac)| (DetachedIeee802154Engine { inner }, mac))
            .map_err(|inner| Self { inner })
    }

    /// Own `mac` with the uniquely placed buffers. No register is touched.
    ///
    /// # Errors
    /// Returns both unchanged owners if a frame's DMA projection is refused.
    pub fn place(
        mac: Ll,
        buffers: Placed<Ieee802154EngineBuffers>,
        levels: Ieee802154TxPowerLevels<'static>,
        defaults: Ieee802154PibDefaults,
        start: Ieee802154Permit,
    ) -> Result<Self, PlacementRefusal<Ll>> {
        Self::bind(mac, buffers, levels, defaults, start, None)
    }

    /// Own `mac` and the placed buffers with multi-PAN enabled.
    ///
    /// # Errors
    /// Returns both unchanged owners if a frame's DMA projection is refused.
    pub fn place_multipan(
        mac: Ll,
        buffers: Placed<Ieee802154EngineBuffers>,
        levels: Ieee802154TxPowerLevels<'static>,
        defaults: Ieee802154PibDefaults,
        interfaces: Ieee802154Interfaces,
        start: Ieee802154Permit,
    ) -> Result<Self, PlacementRefusal<Ll>> {
        Self::bind(mac, buffers, levels, defaults, start, Some(interfaces))
    }

    fn bind(
        mac: Ll,
        buffers: Placed<Ieee802154EngineBuffers>,
        levels: Ieee802154TxPowerLevels<'static>,
        defaults: Ieee802154PibDefaults,
        start: Ieee802154Permit,
        interfaces: Option<Ieee802154Interfaces>,
    ) -> Result<Self, PlacementRefusal<Ll>> {
        let projected = crate::dma_address::PlacementProjection::try_with::<
            crate::ieee80211::dma::space::NativeDma,
            _,
            _,
        >(buffers, |storage, projection| {
            let tx = projection.address(storage.tx_frame())?;
            let enhanced_ack = projection.address(storage.enhanced_ack_frame())?;
            let mut rx = [tx; oer_espressif_ieee802154_engine::engine::RX_BUFFER_COUNT + 1];
            for (address, frame) in rx.iter_mut().zip(storage.rx_frames()) {
                *address = projection.address(frame)?;
            }
            Ok(Ieee802154DmaAddresses {
                rx,
                tx,
                enhanced_ack,
            })
        });
        let (buffers, addresses) = match projected {
            Ok(parts) => parts,
            Err((error, buffers)) => {
                return Err(PlacementRefusal {
                    error,
                    mac,
                    buffers,
                });
            }
        };
        let engine = match interfaces {
            Some(interfaces) => {
                SharedEngine::new_multipan(buffers, addresses, levels, defaults, interfaces, start)
            }
            None => SharedEngine::new(buffers, addresses, levels, defaults, start),
        };
        Ok(Self {
            inner: Ieee802154EngineOwner::new(engine, mac),
        })
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

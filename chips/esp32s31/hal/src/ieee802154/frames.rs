//! Permanently placed IEEE 802.15.4 DMA frames and their concrete projection.

use crate::{dma_static::Placed, layouts::DmaWindowAddress};
use oer_espressif_ieee802154_mac_values::{
    Ieee802154DmaAddresses, Ieee802154EngineBuffers, RX_BUFFER_COUNT,
};

crate::dma_static! {
    #[allow(unsafe_code, reason = "DMA storage is placed through the HAL macro")]
    static BUFFERS: Ieee802154EngineBuffers = zeroed in ".dma.bss.ieee802154";
}

/// Claim the HAL's frame storage once, before constructing its engine.
pub fn claim_buffers() -> Option<Placed<Ieee802154EngineBuffers>> {
    BUFFERS.take()
}

/// A refused projection retains the unchanged, uniquely placed storage.
pub struct ProjectionRefusal {
    pub error: crate::dma_address::ProjectionError,
    pub buffers: Placed<Ieee802154EngineBuffers>,
}

/// Project each frame while retaining its permanent placement proof.
/// No register access or DMA publication occurs.
///
/// # Errors
/// Returns the unchanged storage if any frame lies outside the DMA window.
pub fn project(
    buffers: Placed<Ieee802154EngineBuffers>,
) -> Result<
    (
        &'static mut Ieee802154EngineBuffers,
        Ieee802154DmaAddresses<DmaWindowAddress>,
    ),
    ProjectionRefusal,
> {
    let projected = crate::dma_address::PlacementProjection::try_with::<
        crate::ieee80211::dma::space::NativeDma,
        _,
        _,
    >(buffers, |storage, projection| {
        let tx = projection.address(storage.tx_frame())?;
        let enhanced_ack = projection.address(storage.enhanced_ack_frame())?;
        let mut rx = [tx; RX_BUFFER_COUNT + 1];
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
            return Err(ProjectionRefusal { error, buffers });
        }
    };
    Ok((buffers, addresses))
}

#[cfg(test)]
mod tests;

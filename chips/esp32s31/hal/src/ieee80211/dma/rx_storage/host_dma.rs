//! Host model of the DMA engine's memory port (test support only).
//!
//! The Wi-Fi DMA engine writes received bytes at the buffer address a
//! descriptor publishes. In the [`HostDma`] space that address is
//! [`HOST_ARENA_BASE`](crate::ieee80211::dma::space::HOST_ARENA_BASE) plus the buffer's offset
//! in its arena, so a host test can play the engine's part through these
//! methods. They exist only on a `HostDma` arena: a chip arena has no such
//! port.

use super::{ExternalRxReturnState, RxDmaBuffer, RxDmaStorage, RxRingLive};
use crate::ieee80211::dma::space::HostDma;

/// Why the modelled DMA engine refused an access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostDmaAccessError {
    /// The ring is not the live ring of this arena.
    ForeignRing,
    /// The range does not lie inside one buffer of this arena.
    OutsideBuffers,
    /// The buffer is detached to an upper owner, not in the DMA ring.
    NotInRing,
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>
    RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, HostDma>
{
    /// Model the DMA engine writing `bytes` at the published `address`.
    ///
    /// The range must lie inside one buffer's `STORAGE_SIZE` bytes, which
    /// include the trailing recycle guard; anything else is refused and
    /// nothing is written. `ring` is this arena's live ring: borrowing it
    /// shared proves that no completed-descriptor view of a buffer exists.
    #[allow(
        unsafe_code,
        reason = "the modelled DMA engine writes a buffer the live ring published"
    )]
    pub fn dma_write(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        address: u32,
        bytes: &[u8],
    ) -> Result<(), HostDmaAccessError> {
        let (buffer, offset) = self.dma_range(ring, address, bytes.len())?;
        // SAFETY: `dma_range` bounded `offset + bytes.len()` by this buffer's
        // `STORAGE_SIZE` bytes, which live in its `UnsafeCell`. CPU views of
        // them exist only inside completed-descriptor tokens, which borrow the
        // live ring mutably while `ring` is borrowed shared here, and inside
        // detached buffers, which `dma_range` refused. The ring is neither
        // `Send` nor `Sync`, so no other thread can use it meanwhile.
        unsafe {
            buffer
                .0
                .get()
                .cast::<u8>()
                .add(offset)
                .copy_from_nonoverlapping(bytes.as_ptr(), bytes.len());
        }
        Ok(())
    }

    /// Model the DMA engine reading `output.len()` bytes at the published
    /// `address`, under the same conditions as [`Self::dma_write`].
    #[allow(
        unsafe_code,
        reason = "the modelled DMA engine reads a buffer the live ring published"
    )]
    pub fn dma_read(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        address: u32,
        output: &mut [u8],
    ) -> Result<(), HostDmaAccessError> {
        let (buffer, offset) = self.dma_range(ring, address, output.len())?;
        // SAFETY: as in `dma_write`: the range lies inside this buffer's
        // `UnsafeCell`, and no CPU writer of it can run while `ring` is
        // borrowed shared.
        unsafe {
            buffer
                .0
                .get()
                .cast::<u8>()
                .add(offset)
                .copy_to_nonoverlapping(output.as_mut_ptr(), output.len());
        }
        Ok(())
    }

    /// The published buffer (a ring buffer or the reserved descriptor's)
    /// holding `address .. address + len` and the offset of `address` in it.
    fn dma_range(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        address: u32,
        len: usize,
    ) -> Result<(&RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>, usize), HostDmaAccessError> {
        if !ring.belongs_to_arena(&self.arena) {
            return Err(HostDmaAccessError::ForeignRing);
        }
        for buffer in self.buffers.iter().chain([&self.reserved_buffer]) {
            let start = self
                .buffer_dma_address(buffer)
                .map_err(|_| HostDmaAccessError::OutsideBuffers)?;
            let Some(offset) = address
                .checked_sub(start)
                .and_then(|offset| usize::try_from(offset).ok())
                .filter(|&offset| offset < STORAGE_SIZE)
            else {
                continue;
            };
            if len > STORAGE_SIZE - offset {
                return Err(HostDmaAccessError::OutsideBuffers);
            }
            if buffer.state() != ExternalRxReturnState::Attached {
                return Err(HostDmaAccessError::NotInRing);
            }
            return Ok((buffer, offset));
        }
        Err(HostDmaAccessError::OutsideBuffers)
    }
}

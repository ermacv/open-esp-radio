//! Host model of the DMA engine's memory port (test support only).
//!
//! The Wi-Fi DMA engine writes received bytes at the buffer address a
//! descriptor publishes. In the [`HostDma`] space that address is
//! [`HOST_ARENA_BASE`](crate::radio::ieee80211::dma::space::HOST_ARENA_BASE) plus the buffer's offset
//! in its arena, so a host test can play the engine's part through these
//! methods. They exist only on a `HostDma` arena: a chip arena has no such
//! port.

use super::{ExternalRxReturnState, RxDmaBuffer, RxDmaStorage, RxRingLive};
use crate::radio::ieee80211::dma::{
    rx_dma::model::{RxWalkerLanding, RxWalkerModel},
    space::HostDma,
};

/// Why the modelled DMA engine refused an access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HostDmaAccessError {
    /// The ring is not the live ring of this arena.
    ForeignRing,
    /// The range does not lie inside one buffer of this arena.
    OutsideBuffers,
    /// The buffer is detached to an upper owner, not in the DMA ring.
    NotInRing,
    /// The CPU may hold a view of this buffer: a ring buffer whose
    /// descriptor the ring handed out, or R's buffer while its frame may be
    /// lent. OWNER stays set after a completion and a descriptor can name
    /// any buffer, so no descriptor word says so: the check goes by the
    /// ring's own record and the buffer's binding.
    HeldByCpu,
}

impl<const COUNT: usize, const BUFFER_SIZE: usize, const STORAGE_SIZE: usize>
    RxDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE, HostDma>
{
    /// Model the DMA engine writing `bytes` at the published `address`.
    ///
    /// The range must lie inside one buffer's `STORAGE_SIZE` bytes, which
    /// include the leading recycle guard; anything else is refused and
    /// nothing is written. `ring` is this arena's live ring. A buffer the CPU
    /// may hold a view of is refused ([`HostDmaAccessError::HeldByCpu`]).
    pub fn dma_write(
        &self,
        ring: &RxRingLive<'_, COUNT>,
        address: u32,
        bytes: &[u8],
    ) -> Result<(), HostDmaAccessError> {
        if !ring.belongs_to_arena(&self.arena) {
            return Err(HostDmaAccessError::ForeignRing);
        }
        self.write_published(address, bytes)
    }

    /// Model the DMA engine landing one whole received unit in the
    /// descriptor `walker`'s NEXT names: the bytes first, then the word that
    /// completes it, as [`RxWalkerModel::land`] rules. This needs no ring: the
    /// ring shows the arena what the CPU holds, which ties a landing to the
    /// buffer's owner. A walker that lands nothing writes nothing and says
    /// why.
    ///
    /// # Errors
    ///
    /// `bytes` do not fit the armed buffer, or the CPU holds that descriptor
    /// ([`HostDmaAccessError::HeldByCpu`]); nothing is written.
    pub fn land_unit(
        &self,
        walker: &core::cell::RefCell<RxWalkerModel>,
        bytes: &[u8],
    ) -> Result<RxWalkerLanding, HostDmaAccessError> {
        let table = self
            .walker_table()
            .ok_or(HostDmaAccessError::OutsideBuffers)?;
        let length = u32::try_from(bytes.len()).map_err(|_| HostDmaAccessError::OutsideBuffers)?;
        let mut walker = walker.borrow_mut();
        if let Some(descriptor) = walker.armed_next(table) {
            self.write_published(descriptor.buffer_address(), bytes)?;
        }
        Ok(walker.land(table, length, true))
    }

    /// Whether a CPU view of the buffer at `address` can be alive. The
    /// address comes from a descriptor, which can name any buffer, so what
    /// counts is the buffer written: a ring buffer against each taken
    /// descriptor's own binding, R's buffer against the record of its lent
    /// frame.
    fn held_by_cpu(&self, address: u32, len: usize) -> Result<bool, HostDmaAccessError> {
        let (buffer, _) = self.published_range(address, len)?;
        let Some(written) = self
            .buffers
            .iter()
            .position(|bound| core::ptr::eq(bound, buffer))
        else {
            // R's buffer: `reserved_frame` lends it, and only an exclusive
            // ring loan, which ends that view, returns it.
            return Ok(self.arena.host_reserved_lent());
        };
        Ok((0..COUNT).any(|descriptor| {
            self.arena.host_taken(descriptor)
                && self.descriptor_buffer_id(descriptor) == Some(written)
        }))
    }

    /// Write `bytes` at `address` of a buffer in the ring, unless a CPU view
    /// of it can be alive. Every write of the modelled engine passes here.
    #[allow(
        unsafe_code,
        reason = "the modelled DMA engine writes a buffer the ring published"
    )]
    fn write_published(&self, address: u32, bytes: &[u8]) -> Result<(), HostDmaAccessError> {
        if self.held_by_cpu(address, bytes.len())? {
            return Err(HostDmaAccessError::HeldByCpu);
        }
        let (buffer, offset) = self.published_range(address, bytes.len())?;
        // SAFETY: `published_range` bounded `offset + bytes.len()` by this
        // buffer's `STORAGE_SIZE` bytes, which live in its `UnsafeCell`, and
        // refused a buffer that is not attached, so no detached owner holds
        // it. A CPU view of an attached buffer comes only from the ring: a
        // completed descriptor or unit, whose descriptors the ring's mask
        // records and shows the arena on every change until a recycle
        // republishes them, or R's frame, which `reserved_frame` records as
        // lent before handing it out and which only an exclusive ring loan
        // (ending that view) clears. `held_by_cpu` refused the buffer when
        // either record names it, mapping descriptors to buffers by the
        // trusted binding, never by descriptor words. A view that only copies
        // out ends inside its call, and the arena is used on the thread of
        // its `!Sync` ring, so no view is alive across this write.
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
        if !ring.belongs_to_arena(&self.arena) {
            return Err(HostDmaAccessError::ForeignRing);
        }
        let (buffer, offset) = self.published_range(address, output.len())?;
        // SAFETY: as in `write_published`: the range lies inside this
        // buffer's `UnsafeCell`, and no CPU writer of it can run while `ring`
        // is borrowed shared.
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
    fn published_range(
        &self,
        address: u32,
        len: usize,
    ) -> Result<(&RxDmaBuffer<BUFFER_SIZE, STORAGE_SIZE>, usize), HostDmaAccessError> {
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

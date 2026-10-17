//! Explicit cached-PSRAM ownership handoff to non-coherent peripheral readers.

const CACHE_LINE_SIZE: usize = 64;
const PSRAM_LOW: usize = 0x5000_0000;
const PSRAM_HIGH: usize = 0x5400_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PsramCacheWritebackError {
    Empty,
    AddressOverflow,
    OutsidePsram,
}

fn cache_aligned_psram_range(
    address: usize,
    size: usize,
) -> Result<(u32, u32), PsramCacheWritebackError> {
    if size == 0 {
        return Err(PsramCacheWritebackError::Empty);
    }
    let end = address
        .checked_add(size)
        .ok_or(PsramCacheWritebackError::AddressOverflow)?;
    if address < PSRAM_LOW || end > PSRAM_HIGH {
        return Err(PsramCacheWritebackError::OutsidePsram);
    }
    let start = address & !(CACHE_LINE_SIZE - 1);
    let aligned_end = end
        .checked_add(CACHE_LINE_SIZE - 1)
        .ok_or(PsramCacheWritebackError::AddressOverflow)?
        & !(CACHE_LINE_SIZE - 1);
    Ok((start as u32, (aligned_end - start) as u32))
}

/// Write CPU-dirty cached PSRAM lines back before a peripheral reads them.
///
/// The mutable borrow is the software-ownership proof for the affected data.
/// Callers must additionally ensure that the complete cache lines containing
/// `storage` do not contain data concurrently owned by another actor. After
/// this function returns, the caller must not mutate `storage` until the
/// peripheral has completed. ESP32-S31 requires the register-level writeback
/// operation to be issued twice; this is the exact source-side operation used
/// by ESP-IDF's async memcpy path. Source data is deliberately not invalidated.
#[allow(
    unsafe_code,
    reason = "the validated PSRAM span and exclusive data borrow satisfy the HAL cache-maintenance contract"
)]
#[inline(never)]
#[unsafe(link_section = ".rwtext.cache_maintenance")]
pub fn writeback_psram_for_dma_read(storage: &mut [u8]) -> Result<(), PsramCacheWritebackError> {
    let (start, size) = cache_aligned_psram_range(storage.as_ptr() as usize, storage.len())?;

    // SAFETY: validation bounds the complete cache lines to PSRAM, and the
    // caller retains exclusive ownership until the peripheral completes.
    // HAL serializes this operation with its DMA and executable-code cache
    // maintenance; a separate adapter lock would not protect the shared engine.
    unsafe { esp_hal::psram::writeback_for_dma(start as *const u8, size as usize) };
    Ok(())
}

#[cfg(test)]
mod tests;

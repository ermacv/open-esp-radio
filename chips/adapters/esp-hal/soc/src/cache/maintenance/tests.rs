use super::*;

#[test]
fn psram_range_expands_to_complete_cache_lines() {
    assert_eq!(
        cache_aligned_psram_range(PSRAM_LOW + 65, 64),
        Ok(((PSRAM_LOW + 64) as u32, 128))
    );
}

#[test]
fn invalid_or_wrapping_ranges_fail_closed() {
    assert_eq!(
        cache_aligned_psram_range(PSRAM_LOW, 0),
        Err(PsramCacheWritebackError::Empty)
    );
    assert_eq!(
        cache_aligned_psram_range(PSRAM_LOW - 1, 1),
        Err(PsramCacheWritebackError::OutsidePsram)
    );
    assert_eq!(
        cache_aligned_psram_range(usize::MAX, 2),
        Err(PsramCacheWritebackError::AddressOverflow)
    );
}

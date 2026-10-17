use super::*;

/// The window the ESP32-S31 MAC DMA reaches, as an example chip window.
const WINDOW: core::ops::Range<usize> = 0x2f00_0000..0x2f08_0000;

/// A block is DMA-visible only when it lies wholly in the window; PSRAM
/// and a block crossing the window's end are not.
#[test]
fn only_the_dma_window_is_dma_visible() {
    let size = core::mem::size_of::<Ieee802154EngineBuffers>();
    assert!(dma_window_contains(&WINDOW, WINDOW.start, size));
    assert!(dma_window_contains(&WINDOW, WINDOW.end - size, size));
    assert!(!dma_window_contains(&WINDOW, WINDOW.end - size + 1, size));
    assert!(!dma_window_contains(&WINDOW, WINDOW.start - 4, size));
    // Where the PSRAM data profile placed the buffers on hardware.
    assert!(!dma_window_contains(&WINDOW, 0x500f_142c, size));
    assert!(!dma_window_contains(&WINDOW, usize::MAX - 2, size));
}

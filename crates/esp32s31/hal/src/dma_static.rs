//! Statics placed in the DMA window (`docs/hardware-memory.md`).
//!
//! [`dma_static!`](crate::dma_static!) declares a zero-initialised static in
//! a `.dma.bss.<block>` section. The runtime's linker script gathers every
//! such section into its `.dma.bss` output and asserts that the whole DMA
//! range lies inside the DMA window and holds no address whose low DMA
//! address bits are zero (`dma-window.x`, generated from the register model).
//! A placed static is claimed once as a [`Placed`]: the proof a storage needs
//! before it publishes its own addresses to DMA hardware.

use oer_memory::zeroed::{Zeroable, ZeroedStatic};

/// The section prefix every DMA static uses.
pub const DMA_BSS_PREFIX: &str = ".dma.bss.";

/// Whether `section` is a `.dma.bss.<block>` input section.
pub const fn is_dma_section(section: &str) -> bool {
    let (section, prefix) = (section.as_bytes(), DMA_BSS_PREFIX.as_bytes());
    if section.len() <= prefix.len() {
        return false;
    }
    let mut index = 0;
    while index < prefix.len() {
        if section[index] != prefix[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// A zero-initialised static placed in the DMA window, claimed once.
pub struct DmaStatic<T>(ZeroedStatic<T>);

impl<T: Zeroable> DmaStatic<T> {
    /// The unclaimed static.
    ///
    /// # Safety
    ///
    /// The value is a `static` in a `.dma.bss.<block>` section of a runtime
    /// image linked with the runtime's linker script: only
    /// [`dma_static!`](crate::dma_static!) calls this.
    #[doc(hidden)]
    #[allow(unsafe_code, reason = "the caller promises the placement")]
    pub const unsafe fn placed() -> Self {
        Self(ZeroedStatic::new())
    }
}

impl<T> DmaStatic<T> {
    /// The placed value, the first time; `None` once claimed.
    pub fn take(&'static self) -> Option<Placed<T>> {
        self.0.try_take().map(|value| Placed { value })
    }
}

/// A value placed in the DMA window, claimed once from its [`DmaStatic`]:
/// every address inside it lies in the window, off an address with zero low
/// DMA address bits.
pub struct Placed<T: 'static> {
    value: &'static mut T,
}

impl<T: 'static> Placed<T> {
    /// The placed value.
    pub fn into_inner(self) -> &'static mut T {
        self.value
    }

    /// A host model's value, as if placed: host builds have no DMA window,
    /// and their storages address DMA through their synthetic space.
    #[cfg(any(test, feature = "test-support"))]
    pub fn host(value: &'static mut T) -> Self {
        Self { value }
    }
}

/// Declare a zero-initialised static in the DMA window: a
/// [`DmaStatic`](crate::dma_static::DmaStatic) in the `.dma.bss.<block>`
/// section named.
///
/// ```ignore
/// oer_esp32s31_hal::dma_static! {
///     /// The receive epoch's arena.
///     static RX_ARENA: Arena = zeroed in ".dma.bss.wifi_rx";
/// }
/// ```
#[macro_export]
macro_rules! dma_static {
    () => {};
    (
        $(#[$meta:meta])*
        $vis:vis static $name:ident: $ty:ty = zeroed in $section:literal;
        $($rest:tt)*
    ) => {
        const _: () = assert!(
            $crate::dma_static::is_dma_section($section),
            "a DMA static lives in a `.dma.bss.<block>` section"
        );
        $(#[$meta])*
        #[allow(unsafe_code, reason = "a DMA static's section placement")]
        #[unsafe(link_section = $section)]
        $vis static $name: $crate::dma_static::DmaStatic<$ty> =
            // SAFETY: the static is in the `.dma.bss.<block>` section checked
            // above, which the runtime linker script places in `.dma.bss` and
            // asserts inside the DMA window.
            unsafe { $crate::dma_static::DmaStatic::placed() };
        $crate::dma_static! { $($rest)* }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_named_dma_bss_section_is_a_dma_section() {
        assert!(is_dma_section(".dma.bss.wifi_rx"));
        for section in [
            ".dma.bss.",
            ".dma.bss",
            ".dma.data.wifi",
            ".bss.wifi",
            ".dma.bsss.x",
        ] {
            assert!(!is_dma_section(section), "{section}");
        }
    }

    #[test]
    fn a_placed_static_is_claimed_once() {
        crate::dma_static! {
            static CELL: [u32; 4] = zeroed in ".dma.bss.test";
        }
        let placed = CELL.take().unwrap();
        assert_eq!(*placed.into_inner(), [0; 4]);
        assert!(CELL.take().is_none());
    }
}

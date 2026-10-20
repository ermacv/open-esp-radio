//! Statics placed in the ESP32-C5 DMA window (`docs/hardware-memory.md`).
//!
//! [`dma_static!`](crate::dma_static!) declares zeroed storage in a
//! `.dma.bss.<block>` section. The runtime linker gathers these sections
//! and proves their placement with the generated `dma-window.x` assertions.

use oer_memory::zeroed::{Zeroable, ZeroedStatic};

/// The section prefix every DMA static uses.
pub const DMA_BSS_PREFIX: &str = ".dma.bss.";

/// Whether `section` is a `.dma.bss.<block>` input section.
pub const fn is_dma_section(section: &str) -> bool {
    let (mut section, mut prefix) = (section.as_bytes(), DMA_BSS_PREFIX.as_bytes());
    while let Some((expected, rest)) = prefix.split_first() {
        let Some((actual, tail)) = section.split_first() else {
            return false;
        };
        if *actual != *expected {
            return false;
        }
        prefix = rest;
        section = tail;
    }
    !section.is_empty()
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
/// every address inside it lies in the window.
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

impl<A: 'static, B: 'static> Placed<(A, B)> {
    /// The two values of a pair placed together: each lies inside the
    /// placed static, so each is placed.
    pub fn split(self) -> (Placed<A>, Placed<B>) {
        let (first, second) = self.value;
        (Placed { value: first }, Placed { value: second })
    }
}

/// Declare a zero-initialised static in the DMA window: a
/// [`DmaStatic`](crate::dma_static::DmaStatic) in the `.dma.bss.<block>`
/// section named.
///
/// ```ignore
/// oer_esp32c5_hal::dma_static! {
///     /// The receive epoch's arena.
///     static RX_ARENA: Arena = zeroed in ".dma.bss.ieee802154";
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
    fn a_placed_pair_splits_into_its_own_two_values() {
        let pair: &'static mut (u32, [u8; 4]) =
            std::boxed::Box::leak(std::boxed::Box::new((7, [1; 4])));
        let (first_address, second_address) = (
            core::ptr::addr_of!(pair.0).addr(),
            core::ptr::addr_of!(pair.1).addr(),
        );
        let (first, second) = Placed::host(pair).split();
        let (first, second) = (first.into_inner(), second.into_inner());
        assert_eq!((*first, *second), (7, [1; 4]));
        assert_eq!(
            (
                core::ptr::from_mut(first).addr(),
                core::ptr::from_mut(second).addr()
            ),
            (first_address, second_address)
        );
    }

    #[test]
    fn only_a_named_dma_bss_section_is_a_dma_section() {
        assert!(is_dma_section(".dma.bss.ieee802154"));
        for section in [
            ".dma.bss.",
            ".dma.bss",
            ".dma.data.ieee802154",
            ".bss.ieee802154",
            ".dma.bsss.x",
        ] {
            assert!(!is_dma_section(section), "{section}");
        }
    }
}

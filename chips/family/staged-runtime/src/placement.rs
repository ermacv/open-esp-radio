//! Statics in SRAM and reset-retained RTC fast memory, and the common
//! exception-handler binding selected by [`entry!`](crate::entry). Data macros name only sections the
//! runtime linker script maps. The image linker places strict code in SRAM
//! from its analysis; functions need no manual section attributes.

/// Statics in SRAM: `critical_data` (`.critical.data.<name>`), `isr_rodata`
/// (`.isr.rodata.<name>`) or `dma_data` (`.dma.data.<name>`).
///
/// ```ignore
/// oer_espressif_staged_runtime::placed_static! {
///     /// Handler entries without an installed line.
///     pub static UNSERVICED: AtomicU32 = AtomicU32::new(0), in critical_data "w1";
/// }
/// ```
#[macro_export]
macro_rules! placed_static {
    () => {};
    ($(#[$attribute:meta])* $vis:vis static $name:ident: $ty:ty = $init:expr, in critical_data $section:literal; $($rest:tt)*) => {
        $(#[$attribute])*
        #[unsafe(link_section = concat!(".critical.data.", $section))]
        $vis static $name: $ty = $init;
        $crate::placed_static! { $($rest)* }
    };
    ($(#[$attribute:meta])* $vis:vis static $name:ident: $ty:ty = $init:expr, in isr_rodata $section:literal; $($rest:tt)*) => {
        $(#[$attribute])*
        #[unsafe(link_section = concat!(".isr.rodata.", $section))]
        $vis static $name: $ty = $init;
        $crate::placed_static! { $($rest)* }
    };
    ($(#[$attribute:meta])* $vis:vis static $name:ident: $ty:ty = $init:expr, in dma_data $section:literal; $($rest:tt)*) => {
        $(#[$attribute])*
        #[unsafe(link_section = concat!(".dma.data.", $section))]
        $vis static $name: $ty = $init;
        $crate::placed_static! { $($rest)* }
    };
}

/// A record in reset-retained RTC fast memory (`.rtc_fast.persistent`),
/// behind an [`oer_memory::retained::RetainedCell`] whose claim is in
/// the SRAM critical BSS, which the runtime entry zeroes: a fatal path reaches
/// both without a cache. The record's type is `bytemuck::AnyBitPattern`.
///
/// ```ignore
/// oer_espressif_staged_runtime::retained_static! {
///     static FAULT: Slot;
/// }
/// ```
#[macro_export]
macro_rules! retained_static {
    ($(#[$attribute:meta])* $vis:vis static $name:ident: $ty:ty;) => {
        $crate::__memory::__retained_static! {
            $(#[$attribute])* $vis static $name: $ty,
            in ".rtc_fast.persistent", held in ".critical.bss.retained";
        }
    };
}

/// Platform binding of the common exception path. Used by [`crate::entry!`].
#[doc(hidden)]
#[macro_export]
macro_rules! __exception_handler {
    ($platform:ty, $hook:expr) => {
        // SAFETY: the staged trap entry passes its initialized, aligned SRAM frame;
        // the handler never returns and a second binding is a link error.
        #[unsafe(export_name = "oer_exception_handler")]
        extern "C" fn __oer_exception_handler(context: &$crate::trap::TrapFrame) -> ! {
            $crate::trap::handle::<$platform>(context, $hook)
        }
    };
}

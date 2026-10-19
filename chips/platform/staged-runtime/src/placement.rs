//! Statics and functions in the sections the runtime linker script places:
//! SRAM data that interrupt handlers reach without a PSRAM access, SRAM
//! text for fatal paths that run with caches unavailable, and records in
//! reset-retained RTC fast memory. Each macro names only sections the script
//! maps, so placing an item needs no unsafe code in the image.

/// Statics in SRAM: `critical_data` (`.critical.data.<name>`), `isr_rodata`
/// (`.isr.rodata.<name>`) or `dma_data` (`.dma.data.<name>`).
///
/// ```ignore
/// oer_platform_staged_runtime::placed_static! {
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

/// A function in SRAM text: `rwtext` (`.rwtext.<name>`) or `critical_text`
/// (`.flash.critical.text.<name>`, which the script keeps), for a fatal path
/// that runs with caches unavailable.
///
/// ```ignore
/// oer_platform_staged_runtime::sram_fn! {
///     in rwtext "fatal";
///     pub(crate) fn record_exception(ra: u32) -> ! { … }
/// }
/// ```
#[macro_export]
macro_rules! sram_fn {
    (in rwtext $section:literal; $(#[$attribute:meta])* $vis:vis fn $($item:tt)*) => {
        $(#[$attribute])*
        #[unsafe(link_section = concat!(".rwtext.", $section))]
        $vis fn $($item)*
    };
    (in critical_text $section:literal; $(#[$attribute:meta])* $vis:vis fn $($item:tt)*) => {
        $(#[$attribute])*
        #[unsafe(link_section = concat!(".flash.critical.text.", $section))]
        $vis fn $($item)*
    };
}

/// A record in reset-retained RTC fast memory (`.rtc_fast.persistent`),
/// behind an [`oer_base_memory::retained::RetainedCell`] whose claim is in
/// the SRAM critical BSS, which the runtime entry zeroes: a fatal path reaches
/// both without a cache. The record's type is `bytemuck::AnyBitPattern`.
///
/// ```ignore
/// oer_platform_staged_runtime::retained_static! {
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

/// The image's panic hook (feature `panic-hook`): the panic entry calls
/// `$hook` with the platform's record after its own, before it resets the
/// chip. `$hook` formats nothing and stays in SRAM text.
#[macro_export]
macro_rules! panic_hook {
    ($hook:path) => {
        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".flash.critical.text.panic_hook")]
        fn oer_platform_panic_hook(details: &$crate::panic::PanicDetails<'_>) {
            $hook(details)
        }
    };
}

/// The image's fatal-exception handler: the trap entry calls `$handler` on
/// an SRAM stack with the saved registers, also when the interrupted task
/// ran from PSRAM. It does not return.
#[macro_export]
macro_rules! exception_handler {
    ($handler:path) => {
        #[unsafe(export_name = "ExceptionHandler")]
        #[unsafe(link_section = ".rwtext.exception")]
        extern "C" fn __oer_exception_handler(context: &$crate::trap::TrapFrame) -> ! {
            $handler(context)
        }
    };
}

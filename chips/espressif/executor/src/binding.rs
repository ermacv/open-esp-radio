//! The executor's chip binding: the platform port it runs over and the one
//! static storage of the port's owned blocks.
//!
//! Rust has no generic statics, so the executor's code is generic over a
//! binding `B` and the chip's composition crate declares the concrete
//! statics once, with [`bind!`](crate::bind): the storage, the one waker
//! vtable. The image declares the Embassy time driver and its symbols.

use core::task::RawWakerVTable;

use oer_espressif_platform_port::{ExecutorPlatform, Platform};
use oer_memory::{HandlerCell, OnceValue, StaticRef};
use static_cell::StaticCell;

use crate::chip::{Line, SOFTWARE_INTERRUPT_COUNT};
use crate::systimer::ComparatorWriter;

/// A chip binding: the platform port the executor runs over, its storage
/// and its one waker vtable. Only [`bind!`](crate::bind) implements it.
pub trait Binding: Sized + 'static {
    /// The chip HAL's implementation of the platform contracts.
    type Port: ExecutorPlatform<Counter: oer_espressif_boot::drivers::system_timer::CounterLatch>
        + Platform;
    /// The binding's one storage of the port's owned blocks.
    fn storage() -> &'static Storage<Self::Port>;
    /// The binding's one waker vtable.
    fn waker_vtable() -> &'static RawWakerVTable;
}

/// The blocks the executor owns, published once each: the counter latch
/// every context reads, the four software interrupt lines and the
/// comparator owner its time driver's writer holds.
pub struct Storage<P: ExecutorPlatform> {
    /// The shared counter latch every context reads, published once by the
    /// time driver's handoff.
    pub(crate) counter: StaticRef<P::Counter>,
    pub(crate) counter_storage: StaticCell<P::Counter>,
    pub(crate) line0: StaticCell<P::Line0>,
    pub(crate) line1: StaticCell<P::Line1>,
    pub(crate) line2: StaticCell<P::Line2>,
    pub(crate) line3: StaticCell<P::Line3>,
    /// Each executor's line, published once for wakers; only the line's
    /// handler lowers it. Boot lowers the line before it is published.
    pub(crate) lines: [OnceValue<Line<P>>; SOFTWARE_INTERRUPT_COUNT],
    /// The boot context hands the bank to its one writer executor. Each
    /// alarm handler holds its own comparator's alarm part and never
    /// borrows the bank.
    pub(crate) bank_storage: StaticCell<ComparatorWriter<P>>,
    pub(crate) bank: HandlerCell<ComparatorWriter<P>>,
}

impl<P: ExecutorPlatform> Storage<P> {
    /// The empty storage, which [`bind!`](crate::bind) declares once.
    #[allow(
        clippy::new_without_default,
        reason = "the one storage is a static, built in a const context"
    )]
    pub const fn new() -> Self {
        Self {
            counter: StaticRef::new(),
            counter_storage: StaticCell::new(),
            line0: StaticCell::new(),
            line1: StaticCell::new(),
            line2: StaticCell::new(),
            line3: StaticCell::new(),
            lines: [const { OnceValue::new() }; SOFTWARE_INTERRUPT_COUNT],
            bank_storage: StaticCell::new(),
            bank: HandlerCell::new(),
        }
    }
}

/// The Embassy time driver of binding `B`, with image-selected observer `O`.
/// The image declares its one exported instance with [`time_driver!`](crate::time_driver).
pub struct TimeDriver<B, O> {
    #[cfg(oer_trace)]
    pub(crate) observer: O,
    binding: core::marker::PhantomData<fn() -> B>,
    observer_type: core::marker::PhantomData<fn() -> O>,
}

impl<B, O> TimeDriver<B, O> {
    /// The driver, which the image declares once.
    #[allow(
        clippy::new_without_default,
        reason = "the one driver is a static, built in a const context"
    )]
    pub const fn new(#[cfg(oer_trace)] observer: O) -> Self {
        Self {
            #[cfg(oer_trace)]
            observer,
            binding: core::marker::PhantomData,
            observer_type: core::marker::PhantomData,
        }
    }
}

/// Bind the executor to a chip: implement [`Binding`] for the marker type
/// `$binding` over the platform port `$port` and declare the binding's
/// storage and one waker vtable once in the calling crate. An image links
/// one binding and declares its time driver separately.
///
/// ```ignore
/// /// The ESP32-S31's executor binding.
/// pub struct Esp32s31 { _private: () }
/// oer_espressif_executor::bind!(Esp32s31, oer_esp32s31_hal::platform::Port);
/// ```
#[macro_export]
macro_rules! bind {
    ($binding:ident, $port:ty) => {
        impl $crate::Binding for $binding {
            type Port = $port;
            #[inline(always)]
            fn storage() -> &'static $crate::Storage<$port> {
                // Every hart reads the lines and the counter latch.
                $crate::__bind::placed_static! {
                    static STORAGE: $crate::Storage<$port> = $crate::Storage::new(), in critical_data "executor_binding";
                }
                &STORAGE
            }
            #[inline(always)]
            fn waker_vtable() -> &'static ::core::task::RawWakerVTable {
                &WAKER_VTABLE
            }
        }

        /// The executor's one waker vtable.
        static WAKER_VTABLE: ::core::task::RawWakerVTable =
            $crate::task::waker_vtable::<$binding>();
    };
}

/// Declare the image's Embassy time driver, with `Noop` by default. An
/// optional unit-struct observer is instantiated only under `oer_trace`.
#[macro_export]
macro_rules! time_driver {
    ($binding:ty) => {
        $crate::time_driver!($binding, $crate::__trace::Noop);
    };
    ($binding:ty, $observer:path) => {
        #[cfg(oer_trace)]
        type ImageTimeDriver = $crate::TimeDriver<$binding, $observer>;
        #[cfg(not(oer_trace))]
        type ImageTimeDriver = $crate::TimeDriver<$binding, $crate::__trace::Noop>;
        #[used]
        #[unsafe(link_section = ".critical.data.executor_time")]
        static EMBASSY_TIME_DRIVER: ImageTimeDriver = {
            #[cfg(oer_trace)]
            {
                $crate::TimeDriver::new($observer)
            }
            #[cfg(not(oer_trace))]
            {
                $crate::TimeDriver::new()
            }
        };
        #[unsafe(no_mangle)]
        fn _embassy_time_now() -> u64 {
            <ImageTimeDriver as $crate::__bind::Driver>::now(&EMBASSY_TIME_DRIVER)
        }
        #[unsafe(no_mangle)]
        fn _embassy_time_schedule_wake(at: u64, waker: &::core::task::Waker) {
            <ImageTimeDriver as $crate::__bind::Driver>::schedule_wake(
                &EMBASSY_TIME_DRIVER,
                at,
                waker,
            );
        }
    };
}

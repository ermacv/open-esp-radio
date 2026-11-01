#![cfg_attr(not(test), no_std)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]
//! The Espressif family's application entry.
//!
//! An application selects its chip by feature (`esp32s31`), takes the
//! peripherals it needs once and builds drivers and services from them with
//! defaults. The library places their interrupts, executors and service
//! tasks and applies the record-and-reset policy on the image's behalf.
//!
//! - [`aes`]: the shared AES service, one engine owner and many clients;
//! - [`net`]: waiting on the IP service for an address or a link loss.

pub mod aes;
pub mod net;
mod print;

#[cfg(all(target_arch = "riscv32", not(feature = "esp32s31")))]
compile_error!("select the application's chip: the `esp32s31` feature");

#[cfg(all(feature = "esp32s31", target_arch = "riscv32"))]
mod esp32s31;
#[cfg(all(feature = "esp32s31", target_arch = "riscv32"))]
pub use esp32s31::*;

/// The image's entry: `boot` runs once before any executor, with the
/// peripherals and the boot context; it builds drivers and services and
/// returns the application's future, which the library runs beside its
/// service tasks. Every error records and resets.
///
/// ```ignore
/// oer_espressif::main!(boot);
///
/// fn boot(p: Peripherals, boot: &mut Boot) -> Result<Harts<impl Future<Output = AppResult>>, Error> {
///     let aes = Aes::start(p.aes, boot)?;
///     Ok(Harts::one(app(aes)))
/// }
/// ```
#[macro_export]
macro_rules! main {
    ($boot:path) => {
        type __OerApp = impl ::core::future::Future<Output = $crate::AppResult>;

        #[define_opaque(__OerApp)]
        fn __oer_boot(
            peripherals: $crate::Peripherals,
            boot: &mut $crate::Boot,
        ) -> ::core::result::Result<$crate::Harts<__OerApp>, $crate::Error> {
            $boot(peripherals, boot)
        }

        $crate::__private::tasks! {
            /// The image's task on the application executor, after the
            /// library's service tasks.
            __OerAppTasks: Executor<$crate::Esp32s31, 0> extends $crate::SdkTasks {
                app(future: __OerApp) = $crate::__private::run_app;
            }
        }

        $crate::__private::entry!(__oer_main, $crate::__private::Port);

        fn __oer_main(handoff: $crate::__private::Handoff<$crate::__private::Port>) -> ! {
            $crate::__private::start::<__OerAppTasks, __OerApp>(
                handoff,
                __oer_boot,
                __OerAppTasks::app,
            )
        }
    };
}

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
//! - [`channel`]: a typed channel between the application's future and the
//!   second boot's;
//! - [`net`]: waiting on the IP service for an address or a link loss.

pub mod aes;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
mod boot;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
pub mod channel;
pub mod net;
pub mod print;

#[cfg(all(target_arch = "riscv32", not(feature = "esp32s31")))]
compile_error!("select the application's chip: the `esp32s31` feature");

#[cfg(all(feature = "esp32s31", target_arch = "riscv32"))]
mod esp32s31;
#[cfg(all(feature = "esp32s31", target_arch = "riscv32"))]
pub use esp32s31::*;

/// The image's entry: `boot` runs once before any executor, with the
/// peripherals and the boot context; it builds drivers and services and ends
/// with [`Boot::run`] and the application's future, which the library runs
/// beside its service tasks. Every error records and resets.
///
/// `hart1 = boot1` names the second boot: `boot1` runs once the boot ended,
/// with what the boot handed it ([`Hart1Peripherals`]: the station's
/// network device), starts the IP service ([`wifi::Net::start`]) and ends
/// with [`Hart1Boot::run`] and the future that uses its sockets. That
/// future and the IP service run on hart 0, the application's executor.
///
/// ```ignore
/// oer_espressif::entry!(boot, hart1 = network);
///
/// fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
///     let aes = Aes::start(p.aes, &mut boot)?;
///     let (station, status) = Station::start(p.wifi, &aes, config, &mut boot)?;
///     Ok(boot.run(app(station, status)))
/// }
///
/// fn network(p: Hart1Peripherals, boot: Hart1Boot) -> Result<impl Hart1Started, Error> {
///     let device = p.device.ok_or(Error::NoDevice)?;
///     let (net, boot) = Net::start(device, NetConfig::Dhcpv4, boot)?;
///     Ok(boot.run(sockets(net)))
/// }
/// ```
#[macro_export]
macro_rules! entry {
    ($boot:path) => {
        $crate::entry!($boot, hart1 = $crate::__private::no_hart1);
    };
    ($boot:path, hart1 = $hart1:path) => {
        $crate::__private::entry!(
            __oer_main,
            $crate::__private::Port,
            exception = $crate::__private::exception
        );

        fn __oer_main(handoff: $crate::__private::Handoff<$crate::__private::Port>) -> ! {
            $crate::__private::start(handoff, $boot, $hart1)
        }
    };
}

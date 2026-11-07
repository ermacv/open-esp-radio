//! The product AES engine: the one hardware AES owner, which the Wi-Fi
//! station, the library's user or boot-smoke's hart-1 task takes by value and
//! lends to each synchronous operation. Each block waits synchronously on the
//! accelerator's `STATE` flag.
use oer_chips_esp32s31_hal::peripherals::aes::{AesInterrupt, AesParts, AesRegisters};
use oer_chips_family_platform_interrupt_table::Silent;
use oer_portable_embassy_time::EmbassyClock;

/// The accelerator as a synchronous AES-128 engine, its block settles
/// measured on the executors' clock.
pub type AesEngine = oer_chips_esp32s31_peripherals_crypto::AesBlocks<AesRegisters, EmbassyClock>;

/// Transfer AES by value to its one engine. IRQ55 stays silent.
pub fn aes_engine(parts: AesParts, silent: Silent<AesInterrupt>) -> AesEngine {
    oer_chips_esp32s31_peripherals_crypto::AesBlocks::new(parts.into_polled(silent), EmbassyClock)
}

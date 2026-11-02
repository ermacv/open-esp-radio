//! The product AES engine: the one hardware AES owner, which the Wi-Fi
//! station, the library's AES service or boot-smoke's hart-1 task takes by
//! value. Each block waits synchronously on the accelerator's `STATE` flag.
use oer_crypto::block::BlockKeyUnwrap;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::peripherals::aes::{AesInterrupt, AesParts, AesRegisters};
use oer_espressif_interrupt_table::Silent;

/// The accelerator as a synchronous AES-128 engine, its block settles
/// measured on the executors' clock.
pub type AesEngine = oer_esp32s31_crypto::AesBlocks<AesRegisters, EmbassyClock>;

/// The key-unwrap engine over it: its operation is ready on its first poll.
pub type KeyUnwrapEngine = BlockKeyUnwrap<AesEngine>;

/// Transfer AES by value to its one engine. IRQ55 stays silent.
pub fn aes_engine(parts: AesParts, silent: Silent<AesInterrupt>) -> AesEngine {
    oer_esp32s31_crypto::AesBlocks::new(parts.into_polled(silent), EmbassyClock)
}

/// [`aes_engine`] as a key-unwrap engine.
pub fn key_unwrap(parts: AesParts, silent: Silent<AesInterrupt>) -> KeyUnwrapEngine {
    BlockKeyUnwrap::new(aes_engine(parts, silent))
}

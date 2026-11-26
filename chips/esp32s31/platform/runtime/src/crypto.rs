//! The product AES engine: the one hardware AES owner, which the Wi-Fi
//! station, the library's user or boot-smoke's hart-1 task takes by value and
//! lends to each synchronous operation. Each block waits synchronously on the
//! accelerator's `STATE` flag.
use oer_chips_esp32s31_hal::peripherals::aes::{AesInterrupt, AesParts, AesRegisters};
use oer_chips_esp32s31_hal::system::cycles::CycleCounter;
use oer_chips_family_platform_interrupt_table::Silent;

/// The accelerator as a synchronous AES-128 engine, its block settles
/// counted on the adopted CPU rate's counter.
pub type AesEngine = oer_chips_esp32s31_drivers_aes::AesBlocks<AesRegisters, CycleCounter>;

/// What the engine's last stop observed, its driver's record
/// ([`AesEngine`]'s `last_stop`).
pub use oer_chips_esp32s31_drivers_aes::AesStop;

/// Transfer AES by value to its one engine. IRQ55 stays silent.
pub fn aes_engine(
    parts: AesParts,
    silent: Silent<AesInterrupt>,
    counter: CycleCounter,
) -> AesEngine {
    oer_chips_esp32s31_drivers_aes::AesBlocks::new(parts.into_polled(silent), counter)
}

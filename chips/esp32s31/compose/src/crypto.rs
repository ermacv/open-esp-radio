//! The product key-unwrap engine supplied to the Wi-Fi facade.
use oer_embassy_time::{EmbassyClock, WaitUntil};
use oer_esp32s31_hal::peripherals::aes::{AesInterrupt, AesParts, AesRegisters};
use oer_espressif_interrupt_table::Silent;
use oer_time::Instant;

pub type WifiKeyUnwrap = oer_esp32s31_crypto::AesKeyUnwrap<AesRegisters, EmbassyClock, AesWait>;

/// Static timer factory: no function pointer in the station's engine.
pub struct AesWait;
impl oer_esp32s31_crypto::WaitFactory for AesWait {
    type Wait = WaitUntil;
    fn wait_until(&self, at: Instant) -> WaitUntil {
        WaitUntil::new(at)
    }
}

/// Transfer AES by value to the station's one engine slot. IRQ55 stays silent.
pub fn wifi_key_unwrap(parts: AesParts, silent: Silent<AesInterrupt>) -> WifiKeyUnwrap {
    oer_esp32s31_crypto::AesKeyUnwrap::new(parts.into_polled(silent), EmbassyClock, AesWait)
}

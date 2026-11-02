//! The S31 AES accelerator as the image's one synchronous AES-128 engine.

use oer_esp32s31_hal::peripherals::aes::AesInterrupt;

use super::{AesPeripheral, Boot, Error};

/// The accelerator as a synchronous AES-128 engine
/// (`oer_crypto::block::Aes128Engine`): each block waits on its `STATE`
/// flag within its settle, and the AES interrupt stays silent. An operation
/// borrows it for the length of its call (`oer_crypto::block::unwrap_now`),
/// so it is free again on return.
pub type AesEngine = oer_esp32s31_compose::crypto::AesEngine;

impl AesPeripheral {
    /// The accelerator as the image's AES engine, owned by the caller from
    /// here.
    ///
    /// # Errors
    /// [`Error::InterruptRouted`] when the table routes the AES interrupt.
    pub fn engine(self, boot: &mut Boot) -> Result<AesEngine, Error> {
        let silent = boot
            .router()
            .silent::<AesInterrupt>()
            .map_err(|_| Error::InterruptRouted)?;
        Ok(oer_esp32s31_compose::crypto::aes_engine(self.0, silent))
    }
}

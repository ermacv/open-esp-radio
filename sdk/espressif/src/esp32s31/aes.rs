//! The S31 AES accelerator as the shared service of the application
//! executor's context.

use oer_esp32s31_hal::peripherals::aes::AesInterrupt;
use oer_espressif_executor::ThreadContext;

use super::{AesPeripheral, Boot, Error};
use crate::aes::AesService;

/// The engine the service owns: the polled CPU AES, IRQ55 held silent.
pub(crate) type Engine = oer_esp32s31_compose::crypto::KeyUnwrapEngine;

/// Key data capacity of every request: the Wi-Fi station's RSN key data.
pub const CAPACITY: usize = oer_ieee80211_core_roles_sta::machine::KEY_DATA_CAPACITY;

/// Client seats of the service.
pub const CLIENTS: usize = 4;

type Service = AesService<ThreadContext<0>, CAPACITY, CLIENTS>;

/// One client of the shared AES service.
pub type AesClient = crate::aes::AesClient<'static, ThreadContext<0>, CAPACITY, CLIENTS>;

static SERVICE: Service = Service::new();

/// The started AES service: it issues clients.
#[derive(Clone, Copy)]
pub struct Aes {
    service: &'static Service,
}

impl Aes {
    /// Make the accelerator the shared service; its task starts with the
    /// executor.
    ///
    /// # Errors
    /// [`Error::InterruptRouted`] when the table routes IRQ55.
    pub fn start(aes: AesPeripheral, boot: &mut Boot) -> Result<Self, Error> {
        let silent = boot
            .router()
            .silent::<AesInterrupt>()
            .map_err(|_| Error::InterruptRouted)?;
        if boot.pending.aes.is_some() {
            return Err(Error::AlreadyStarted);
        }
        boot.pending.aes = Some(oer_esp32s31_compose::crypto::key_unwrap(aes.0, silent));
        Ok(Self { service: &SERVICE })
    }

    /// A client on a free seat, in the boot or in a task.
    ///
    /// # Errors
    /// [`Error::AesClients`] when every seat has a client.
    pub fn client(&self) -> Result<AesClient, Error> {
        self.service
            .client()
            .map_err(|crate::aes::Exhausted| Error::AesClients)
    }
}

/// The service task: it owns the engine for good.
pub(crate) async fn serve(engine: Engine) {
    let error = SERVICE.serve(engine).await;
    super::fail(match error {
        crate::aes::AesError::Foreign => oer_diagnostics::name!("sdk-aes-context"; 32),
    });
}

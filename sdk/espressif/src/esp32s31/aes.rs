//! The S31 AES accelerator as the shared service of the application
//! executor's context.

use oer_esp32s31_hal::peripherals::aes::AesInterrupt;
use oer_espressif_executor::ThreadContext;

use static_cell::StaticCell;

use super::{AesPeripheral, Boot, Error};
use crate::aes::AesService;

/// The engine the service owns: the polled CPU AES, IRQ55 held silent.
pub(crate) type Engine = oer_esp32s31_compose::crypto::KeyUnwrapEngine;

/// Key data capacity of every request: the largest RSN key data.
pub const CAPACITY: usize = oer_ieee80211_core_security_cipher::aes::RSN_KEY_DATA_CAPACITY;

/// Client seats of the service [`Aes::start`] makes.
pub const CLIENTS: usize = 4;

type Context = ThreadContext<0>;
type Service = AesService<Context, CAPACITY>;

/// One client of the shared AES service.
pub type AesClient = crate::aes::AesClient<'static, Context, CAPACITY>;

/// The client seats of an AES service: `C` of them, 1 to 32, each holding
/// one request of [`CAPACITY`]. An image that needs another count than
/// [`CLIENTS`] keeps a static of its own and passes it to
/// [`Aes::start_with`].
pub type AesSeats<const C: usize> = crate::aes::AesSeats<Context, CAPACITY, C>;

static SEATS: AesSeats<CLIENTS> = AesSeats::new();
static SERVICE: StaticCell<Service> = StaticCell::new();

/// The started AES service: it issues clients.
#[derive(Clone, Copy)]
pub struct Aes {
    service: &'static Service,
}

/// What the service task takes: the engine and the service it serves.
pub(crate) struct AesStart {
    engine: Engine,
    service: &'static Service,
}

impl Aes {
    /// Make the accelerator the shared service with [`CLIENTS`] seats; its
    /// task starts with the executor.
    ///
    /// # Errors
    /// [`Error::InterruptRouted`] when the table routes IRQ55,
    /// [`Error::AlreadyStarted`] for a second service.
    pub fn start(aes: AesPeripheral, boot: &mut Boot) -> Result<Self, Error> {
        Self::start_with(aes, &SEATS, boot)
    }

    /// As [`Self::start`], with the image's own `seats`.
    ///
    /// # Errors
    /// As [`Self::start`].
    pub fn start_with<const C: usize>(
        aes: AesPeripheral,
        seats: &'static AesSeats<C>,
        boot: &mut Boot,
    ) -> Result<Self, Error> {
        if boot.pending.aes.is_some() {
            return Err(Error::AlreadyStarted);
        }
        let silent = boot
            .router()
            .silent::<AesInterrupt>()
            .map_err(|_| Error::InterruptRouted)?;
        let service: &'static Service = SERVICE
            .try_init(Service::new(seats))
            .ok_or(Error::AlreadyStarted)?;
        boot.pending.aes = Some(AesStart {
            engine: oer_esp32s31_compose::crypto::key_unwrap(aes.0, silent),
            service,
        });
        Ok(Self { service })
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
pub(crate) async fn serve(start: AesStart) {
    let AesStart { engine, service } = start;
    let error = observed!(Aes, service.serve(engine)).await;
    super::fail(match error {
        crate::aes::AesError::Foreign => oer_diagnostics::name!("sdk-aes-context"; 32),
    });
}

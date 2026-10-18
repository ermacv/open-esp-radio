//! ESP-HAL CPU-route ownership and time for the ESP32-S31 IEEE 802.15.4 MAC.
//!
//! The typed esp-hal interrupt identifies the IEEE 802.15.4 MAC route. This
//! adapter keeps its priority, bound core and process-wide claim behind one
//! affine owner, and supplies the microsecond clock the MAC engine reads and
//! the random words of CSMA-CA backoffs.
//! The MAC owners live in the IEEE 802.15.4 runtime: the handler bound here
//! calls the runtime's interrupt entry.
//!
//! The image's interrupt table names the handler of source 132; the image
//! hands this adapter the source's route once at boot ([`install`]).
//!
//! Bring-up order: activate the HAL interrupt owner, install the runtime,
//! then `bind`. Teardown reverses it: `BoundEspHalIInterruptRoute::quiesce`,
//! uninstall the runtime, then deactivate the HAL interrupt owner. This is
//! ESP-IDF's order: `i_mac_init` allocates the route last with
//! `esp_intr_alloc(132, 0)`, the lowest free level of 1 to 3 on the calling
//! core, and `i_mac_deinit` frees it first.

#![no_std]
#![cfg(feature = "esp32s31")]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

use core::cell::{Cell, RefCell};

use critical_section::Mutex;
use esp_hal::{interrupt::Priority, peripherals::Interrupt, rng::Rng, system::Cpu, time::Instant};
use oer_espressif_interrupt_table_esp_hal::{self as interrupt_table, Route};
use oer_interrupt_table::Entry;

const SOURCE: Interrupt = Interrupt::MODEM_ZB_MAC;
const ROUTE_PRIORITY: Priority = Priority::Priority1;

/// ESP-HAL's one-microsecond monotonic clock (`esp_timer_get_time`).
///
/// `esp_hal::init` must have run before the clock is sampled.
pub fn now_micros() -> u64 {
    Instant::now().duration_since_epoch().as_micros()
}

/// A random word from the hardware generator, for CSMA-CA backoffs where
/// ESP-IDF's OpenThread draws from its non-cryptographic generator.
pub fn random() -> u32 {
    Rng::new().random()
}

static ROUTE_CLAIMED: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));

/// Source 132's route in the image's interrupt table, once [`install`]ed.
static ROUTE: Mutex<RefCell<Option<Route>>> = Mutex::new(RefCell::new(None));

/// The route of modem source 132 at the vendor's `Priority1`, from its token
/// in the image's interrupt table: another source's or another level's token
/// does not compile.
pub struct EspHalIeee802154Source(Route);

impl EspHalIeee802154Source {
    /// The route of `token`'s source.
    pub fn new<T>(token: T) -> Self
    where
        T: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    {
        const {
            assert!(
                T::SOURCE as u16 == SOURCE as u16,
                "the token is not MODEM_ZB_MAC's"
            );
            assert!(
                T::LEVEL as u8 == ROUTE_PRIORITY as u8,
                "the vendor routes source 132 at priority one"
            );
        };
        Self(Route::new(token))
    }
}

/// Keep source 132's route for every epoch of the boot.
///
/// # Errors
///
/// The boot installed a route before; `source` returns unchanged.
pub fn install(source: EspHalIeee802154Source) -> Result<(), EspHalIeee802154Source> {
    critical_section::with(|critical_section| {
        let mut route = ROUTE.borrow_ref_mut(critical_section);
        if route.is_some() {
            return Err(source);
        }
        *route = Some(source.0);
        Ok(())
    })
}

/// Failure to create or quiesce the unique IEEE 802.15.4 CPU route.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalIeee802154InterruptRouteError {
    /// Another live route owner already controls modem source 132.
    AlreadyActive,
    /// The image never [`install`]ed the route of source 132.
    NotInstalled,
    /// The route was bound, or torn down, on a CPU other than its table's.
    WrongCore,
}

/// Active ESP-HAL route for modem source 132.
///
/// It must be disabled on the binding core before the runtime returns the
/// MAC owners or a later epoch binds the source again.
#[must_use = "source 132 must be disabled before the MAC owners are recovered"]
pub struct BoundEspHalIeee802154InterruptRoute {
    core: Cpu,
}

/// Route modem source 132 to the handler its interrupt-table entry names.
///
/// # Errors
///
/// No route is installed, a route is already bound, or the caller runs on
/// another core than the table's.
pub fn bind() -> Result<BoundEspHalIeee802154InterruptRoute, EspHalIeee802154InterruptRouteError> {
    critical_section::with(|critical_section| {
        let route = ROUTE.borrow_ref(critical_section);
        let route = route
            .as_ref()
            .ok_or(EspHalIeee802154InterruptRouteError::NotInstalled)?;
        let claimed = ROUTE_CLAIMED.borrow(critical_section);
        if claimed.get() {
            return Err(EspHalIeee802154InterruptRouteError::AlreadyActive);
        }
        interrupt_table::enable_route(route)
            .map_err(|_| EspHalIeee802154InterruptRouteError::WrongCore)?;
        claimed.set(true);
        Ok(BoundEspHalIeee802154InterruptRoute {
            core: Cpu::current(),
        })
    })
}

impl BoundEspHalIeee802154InterruptRoute {
    /// Disable source 132 on the binding core and release the claim.
    ///
    /// # Errors
    ///
    /// On another core nothing changes and the route owner is returned.
    pub fn quiesce(self) -> Result<(), (EspHalIeee802154InterruptRouteError, Self)> {
        if Cpu::current() != self.core {
            return Err((EspHalIeee802154InterruptRouteError::WrongCore, self));
        }
        critical_section::with(|critical_section| {
            if let Some(route) = ROUTE.borrow_ref(critical_section).as_ref() {
                interrupt_table::disable_route(route);
            }
            ROUTE_CLAIMED.borrow(critical_section).set(false);
        });
        Ok(())
    }
}

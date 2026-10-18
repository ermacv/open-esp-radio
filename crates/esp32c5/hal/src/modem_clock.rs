//! ESP32-C5 shared modem clocks and the IEEE 802.15.4 MAC reset.
//!
//! [`ModemClocks`] owns the PAC's modem clock registers and a managed
//! [`oer_espressif_modem_clock`] planner over the ESP32-C5 vendor device order. A
//! module is enabled by acquiring its dependency set; each dependency's
//! physical action runs only at its zero-to-one or one-to-zero edge.
//!
//! Only the modules whose every device action the PAC publishes are offered:
//! IEEE 802.15.4, coexistence, the modem ETM and the Bluetooth APB. The table
//! still names every vendor device, so later modules keep the vendor order.
//!
//! The vendor also ORs a per-domain clock-gating map into MODEM_SYSCON and
//! MODEM_LPCON on every module enable (`modem_clock_module_icg_map_init_all`).
//! That map belongs to platform power management and is left to the platform;
//! in particular the MODEM state must not be added to the analog I2C master
//! map (see the ESP32-C5 rev 1.0 USB Serial/JTAG reset erratum in
//! `docs/hardware-errata.md`).
//!
//! SOURCE: reviewed evidence `ESP_IDF_4D59230D_C5_MODEM_CLOCK`
//! (`modem_clock_impl.c` `*_CLOCK_DEPS` and `modem_clock_get_module_deps`,
//! the I2C-master refcount exception in `modem_clock_device_context`, the
//! `MODEM_STATUS_WIFI_INITED` guards of the Wi-Fi configure actions, the
//! device order of `modem_clock_impl.h` under the ESP32-C5 `soc_caps.h`, and
//! `modem_clock_module_mac_reset` in `modem_clock.c`). The vendor `DATADUMP`
//! device has no module dependency and is omitted.

use oer_esp32c5_pac::{ModemClockDevice, ModemClockRegisters};
use oer_espressif_modem_clock::{
    DependencySet, ModemClockAcquirePreparationError, ModemClockReleasePreparationError,
    execute_acquire, execute_release,
};

pub use oer_espressif_modem_clock::ModemClockPlannerIdentity;

/// The ESP32-C5 modem clock devices, in the vendor device order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ModemClockDependency {
    ModemAdcCommonFe,
    ModemPrivateFe,
    Coexistence,
    /// The analog I2C master clock; the platform owner keeps its refcount.
    AnalogI2cMaster,
    WifiApb,
    WifiBb44m,
    WifiMac,
    WifiBb,
    Etm,
    BtMac,
    BtPeripheral,
    /// Bluetooth APB and modem-security APB clocks.
    BtApb,
    /// The baseband clock shared by Bluetooth LE and IEEE 802.15.4.
    BtIeee802154CommonBaseband,
    /// IEEE 802.15.4 APB and MAC clocks.
    Ieee802154Mac,
}

impl oer_espressif_modem_clock::ModemClockDependency for ModemClockDependency {
    const LOW_BIT_FIRST: &'static [Self] = &[
        Self::ModemAdcCommonFe,
        Self::ModemPrivateFe,
        Self::Coexistence,
        Self::AnalogI2cMaster,
        Self::WifiApb,
        Self::WifiBb44m,
        Self::WifiMac,
        Self::WifiBb,
        Self::Etm,
        Self::BtMac,
        Self::BtPeripheral,
        Self::BtApb,
        Self::BtIeee802154CommonBaseband,
        Self::Ieee802154Mac,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn refcounted(self) -> bool {
        !matches!(self, Self::AnalogI2cMaster)
    }

    fn retained_while_wifi_initialized(self) -> bool {
        matches!(
            self,
            Self::WifiApb | Self::WifiBb44m | Self::WifiMac | Self::WifiBb
        )
    }
}

/// A radio module that requests modem clocks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemClockModule {
    Ieee802154,
    Coexistence,
    ModemEtm,
    BluetoothApb,
}

impl ModemClockModule {
    /// The module's exact vendor dependency set.
    fn dependencies(self) -> DependencySet<ModemClockDependency> {
        use ModemClockDependency::*;
        DependencySet::of(match self {
            Self::Ieee802154 => &[
                Ieee802154Mac,
                BtIeee802154CommonBaseband,
                Etm,
                Coexistence,
                BtApb,
            ],
            Self::Coexistence => &[Coexistence],
            Self::ModemEtm => &[Etm],
            Self::BluetoothApb => &[BtApb, Etm],
        })
    }
}

/// The PAC device one dependency's action drives, if the PAC publishes it.
const fn device(dependency: ModemClockDependency) -> Option<ModemClockDevice> {
    Some(match dependency {
        ModemClockDependency::Coexistence => ModemClockDevice::Coexistence,
        ModemClockDependency::Etm => ModemClockDevice::Etm,
        ModemClockDependency::BtApb => ModemClockDevice::BluetoothApb,
        ModemClockDependency::BtIeee802154CommonBaseband => {
            ModemClockDevice::BluetoothIeee802154CommonBaseband
        }
        ModemClockDependency::Ieee802154Mac => ModemClockDevice::Ieee802154Mac,
        _ => return None,
    })
}

/// A dependency has no published device action.
#[derive(Clone, Copy, Debug)]
struct UnpublishedDevice;

/// The modem clock gates the planner's edges drive.
pub(crate) trait ModemClockPort {
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool);
    fn pulse_ieee802154_mac_reset(&mut self);
}

impl ModemClockPort for ModemClockRegisters {
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool) {
        self.configure_modem_clock_device(device, enable);
    }

    fn pulse_ieee802154_mac_reset(&mut self) {
        ModemClockRegisters::pulse_ieee802154_mac_reset(self);
    }
}

fn perform(
    port: &mut impl ModemClockPort,
    dependency: ModemClockDependency,
    enable: bool,
) -> Result<(), UnpublishedDevice> {
    let device = device(dependency).ok_or(UnpublishedDevice)?;
    port.configure_device(device, enable);
    Ok(())
}

type Planner<'identity> =
    oer_espressif_modem_clock::ModemClockPlanner<'identity, ModemClockDependency>;
type Lease<'identity> = oer_espressif_modem_clock::ModemClockLease<'identity, ModemClockDependency>;
type PoisonedAcquire<'identity> =
    oer_espressif_modem_clock::PoisonedModemClockAcquire<'identity, ModemClockDependency>;
type PoisonedRelease<'identity> = oer_espressif_modem_clock::PoisonedModemClockRelease<
    'identity,
    'identity,
    ModemClockDependency,
>;

/// One enabled module's hold on its modem clocks.
#[must_use = "an enabled module's clocks stay on until the grant is returned"]
pub struct ModemClockGrant<'identity> {
    module: ModemClockModule,
    lease: Lease<'identity>,
}

impl ModemClockGrant<'_> {
    pub const fn module(&self) -> ModemClockModule {
        self.module
    }
}

/// Why a modem clock request did not change the clocks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemClockError {
    /// The planner rejected the acquisition before any edge.
    AcquireRejected(ModemClockAcquirePreparationError<ModemClockDependency>),
    /// The planner rejected the release before any edge; the grant is
    /// returned with the error.
    ReleaseRejected(ModemClockReleasePreparationError<ModemClockDependency>),
    /// A transaction failed at a physical edge, now or earlier; the clocks
    /// are frozen.
    Poisoned,
    /// The grant is not the one the operation needs.
    WrongModule,
}

/// Why [`ModemClocks::disable`] did not return a module's clocks.
#[must_use = "a rejected release returns the grant it could not consume"]
pub enum ModemClockDisableError<'identity> {
    /// The planner rejected the grant before any edge; it is returned.
    Rejected(
        ModemClockGrant<'identity>,
        ModemClockReleasePreparationError<ModemClockDependency>,
    ),
    /// A transaction failed at a physical edge, now or earlier; the clocks
    /// are frozen and the grant was retained with them.
    Poisoned,
}

impl core::fmt::Debug for ModemClockDisableError<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Rejected(grant, error) => formatter
                .debug_tuple("Rejected")
                .field(&grant.module)
                .field(error)
                .finish(),
            Self::Poisoned => formatter.write_str("Poisoned"),
        }
    }
}

#[allow(
    clippy::large_enum_variant,
    reason = "the owner keeps the allocation-free planner or its poisoned transaction inline"
)]
enum State<'identity> {
    Ready(Planner<'identity>),
    /// A transaction failed at a physical edge. The failed transaction is
    /// retained so that no further clock change is planned.
    Poisoned {
        _acquire: Option<PoisonedAcquire<'identity>>,
        _release: Option<PoisonedRelease<'identity>>,
    },
    /// Transient state while one transaction owns the planner.
    InFlight,
}

/// Unique owner of the ESP32-C5 modem clocks and the IEEE 802.15.4 MAC
/// reset.
#[must_use = "dropping the modem clock owner loses the modem clock registers"]
pub struct ModemClocks<'identity>(Core<'identity, ModemClockRegisters>);

impl<'identity> ModemClocks<'identity> {
    /// Take ownership of the modem clock registers with a managed planner:
    /// this owner is the only one that changes the modem clock gates.
    pub fn new(
        registers: ModemClockRegisters,
        identity: &'identity ModemClockPlannerIdentity,
    ) -> Self {
        Self(Core::with_port(registers, identity))
    }

    /// Enable one module's clocks, performing the edges it needs.
    ///
    /// # Errors
    ///
    /// The planner rejected the request, or the owner is poisoned.
    pub fn enable(
        &mut self,
        module: ModemClockModule,
    ) -> Result<ModemClockGrant<'identity>, ModemClockError> {
        self.0.enable(module)
    }

    /// Return one module's clocks, performing the edges its release needs.
    ///
    /// # Errors
    ///
    /// The planner rejected the grant, which is returned unchanged, or the
    /// owner is poisoned.
    pub fn disable(
        &mut self,
        grant: ModemClockGrant<'identity>,
    ) -> Result<(), ModemClockDisableError<'identity>> {
        self.0.disable(grant)
    }

    /// Pulse the IEEE 802.15.4 MAC and APB resets, as the vendor
    /// `ieee802154_mac_init` does through `modem_clock_module_mac_reset`.
    /// The IEEE 802.15.4 grant proves the MAC clocks are on.
    ///
    /// # Errors
    ///
    /// The grant is not an IEEE 802.15.4 grant; nothing is written.
    pub fn reset_ieee802154_mac(
        &mut self,
        grant: &ModemClockGrant<'identity>,
    ) -> Result<(), ModemClockError> {
        self.0.reset_ieee802154_mac(grant)
    }

    /// Recover the registers once no module holds a grant.
    ///
    /// # Errors
    ///
    /// A grant is outstanding or the owner is poisoned; the owner is returned.
    #[allow(
        clippy::result_large_err,
        reason = "the refused owner keeps the allocation-free planner inline"
    )]
    pub fn into_registers(self) -> Result<ModemClockRegisters, Self> {
        self.0.into_port().map_err(Self)
    }
}

/// The owner over any modem clock port, for host tests.
pub(crate) struct Core<'identity, Port> {
    port: Port,
    state: State<'identity>,
    held: u8,
}

impl<'identity, Port: ModemClockPort> Core<'identity, Port> {
    pub(crate) fn with_port(port: Port, identity: &'identity ModemClockPlannerIdentity) -> Self {
        Self {
            port,
            state: State::Ready(Planner::managed(identity)),
            held: 0,
        }
    }

    /// Enable one module's clocks, performing the edges it needs.
    ///
    /// # Errors
    ///
    /// The planner rejected the request, or the owner is poisoned.
    pub(crate) fn enable(
        &mut self,
        module: ModemClockModule,
    ) -> Result<ModemClockGrant<'identity>, ModemClockError> {
        let planner = match core::mem::replace(&mut self.state, State::InFlight) {
            State::Ready(planner) => planner,
            other => {
                self.state = other;
                return Err(ModemClockError::Poisoned);
            }
        };
        let prepared = match planner.prepare_acquire(module.dependencies()) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                self.state = State::Ready(failure.into_planner());
                return Err(ModemClockError::AcquireRejected(error));
            }
        };
        let port = &mut self.port;
        match execute_acquire(prepared, |dependency| perform(port, dependency, true)) {
            Ok((planner, lease)) => {
                self.state = State::Ready(planner);
                self.held += 1;
                Ok(ModemClockGrant { module, lease })
            }
            Err(poisoned) => {
                self.state = State::Poisoned {
                    _acquire: Some(poisoned),
                    _release: None,
                };
                Err(ModemClockError::Poisoned)
            }
        }
    }

    /// Return one module's clocks, performing the edges its release needs.
    ///
    /// # Errors
    ///
    /// The planner rejected the grant, which is returned unchanged, or the
    /// owner is poisoned.
    pub(crate) fn disable(
        &mut self,
        grant: ModemClockGrant<'identity>,
    ) -> Result<(), ModemClockDisableError<'identity>> {
        let planner = match core::mem::replace(&mut self.state, State::InFlight) {
            State::Ready(planner) => planner,
            other => {
                self.state = other;
                // The grant retires with the poisoned owner.
                return Err(ModemClockDisableError::Poisoned);
            }
        };
        let ModemClockGrant { module, lease } = grant;
        let prepared = match planner.prepare_release(lease) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let error = failure.error();
                let (planner, lease) = failure.into_owners();
                self.state = State::Ready(planner);
                return Err(ModemClockDisableError::Rejected(
                    ModemClockGrant { module, lease },
                    error,
                ));
            }
        };
        let port = &mut self.port;
        match execute_release(prepared, |dependency| perform(port, dependency, false)) {
            Ok(planner) => {
                self.state = State::Ready(planner);
                self.held -= 1;
                Ok(())
            }
            Err(poisoned) => {
                self.state = State::Poisoned {
                    _acquire: None,
                    _release: Some(poisoned),
                };
                Err(ModemClockDisableError::Poisoned)
            }
        }
    }

    /// Pulse the IEEE 802.15.4 MAC and APB resets, as the vendor
    /// `ieee802154_mac_init` does through `modem_clock_module_mac_reset`.
    /// The IEEE 802.15.4 grant proves the MAC clocks are on.
    ///
    /// # Errors
    ///
    /// The grant is not an IEEE 802.15.4 grant; nothing is written.
    pub(crate) fn reset_ieee802154_mac(
        &mut self,
        grant: &ModemClockGrant<'identity>,
    ) -> Result<(), ModemClockError> {
        if grant.module != ModemClockModule::Ieee802154 {
            return Err(ModemClockError::WrongModule);
        }
        self.port.pulse_ieee802154_mac_reset();
        Ok(())
    }

    /// Recover the registers once no module holds a grant.
    ///
    /// # Errors
    ///
    /// A grant is outstanding or the owner is poisoned; the owner is returned.
    #[allow(
        clippy::result_large_err,
        reason = "the refused owner keeps the allocation-free planner inline"
    )]
    pub(crate) fn into_port(self) -> Result<Port, Self> {
        if self.held == 0 && matches!(self.state, State::Ready(_)) {
            Ok(self.port)
        } else {
            Err(self)
        }
    }
}

#[cfg(test)]
mod tests;

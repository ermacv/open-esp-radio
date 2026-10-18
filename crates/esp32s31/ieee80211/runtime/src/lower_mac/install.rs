//! The ESP32-S31 Wi-Fi port bound to the radio's lifetime.
//!
//! [`install_esp32s31_wifi_port`] takes the stopped Wi-Fi a radio start
//! produced: it publishes the MAC register owner in the radio's arena, wraps
//! it in [`CooperativeRadioHardware`] and installs the lower-MAC core over
//! it, returning the backend the composition runs its port body over
//! ([`run_port`](oer_ieee80211_port::run_port)). The port then owns the MAC
//! for as long as the radio runs: no role materializes or returns the
//! hardware in between. [`uninstall_esp32s31_wifi_port`] reverses it at
//! radio stop, with the core and registers `run_port` returned: it reclaims
//! the register owner and reassembles the stopped Wi-Fi the radio stop
//! releases. The core comes back as the parts a later
//! install rebuilds it from ([`LowerMacCore::dismantle`]). The stopped
//! Wi-Fi carries the channel the PHY is tuned to, which the installed core
//! learns ([`LowerMacCore::radio_tuned_to`]), so its next `Enable` retunes
//! when it is configured for another.
//!
//! Clocks and the PHY stay with the radio system: the port never holds a
//! `PlatformClockGuard`, so it releases nothing here, and the radio stop's
//! own release ([`WifiStopped::release`]) ends them explicitly. The
//! composition's membership `Membership` (its part in coexistence) passes
//! through beside the port unchanged, in the held owner.

use oer_esp32s31_hal::{
    ieee80211::arena::{RadioAccess, RadioOwnerArena, RadioOwnerArenaError},
    owner::MacInterruptSetup,
};
use oer_esp32s31_ieee80211::{
    cooperative_hardware::CooperativeRadioHardware,
    lower_mac::{LowerMacCore, LowerMacCoreParts},
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile},
    runtime::{WifiRoleOwner, WifiStopped, materialize_esp32s31_wifi_role},
};
use oer_ieee80211_mac::channel::WifiChannel;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_ieee80211_lower_mac::{IndividualAddress, SupportedChannel};

use super::{
    Esp32s31Backend, Esp32s31Caps, Esp32s31LowerMac, Esp32s31LowerMacParts, LowerMacRetune,
    LowerMacRxUnit,
};

/// The product retune of an installed port: one channel switch under the
/// shared radio's lease, through the published register owner the runner
/// lends it for the switch.
pub struct Esp32s31PortRetune<'a, P, C, D, O> {
    radio: &'a oer_esp32s31_radio_runtime::RadioSystem<P, C, D>,
    observer: O,
}

impl<'a, P, C, D, O> Esp32s31PortRetune<'a, P, C, D, O> {
    /// A retune leasing `radio` for each switch; `observer` sees the PHY's
    /// steps.
    pub const fn new(
        radio: &'a oer_esp32s31_radio_runtime::RadioSystem<P, C, D>,
        observer: O,
    ) -> Self {
        Self { radio, observer }
    }
}

impl<'arena, P, C, D, O> LowerMacRetune<CooperativeRadioHardware<'arena>>
    for Esp32s31PortRetune<'_, P, C, D, O>
where
    C: oer_esp32s31_hal::shared_radio::PlatformClockProvider,
    D: oer_time::Timer,
    O: oer_esp32s31_phy::PhyTargetObserver,
{
    async fn retune(
        &mut self,
        hardware: &mut CooperativeRadioHardware<'arena>,
        channel: WifiChannel,
    ) -> bool {
        let registers: RadioAccess<'arena> = hardware.register_access();
        let mut guard = self.radio.lock().await;
        let (lease, platform, _) = guard.parts();
        oer_esp32s31_ieee80211::switch_published_esp32s31_wifi_channel::<
            oer_esp32s31_phy::RomShortDelay,
            _,
            _,
        >(
            self.radio.timer(),
            lease,
            platform,
            &registers,
            channel,
            &mut self.observer,
        )
        .await
        .is_ok()
    }
}

/// What the installed port holds aside for the radio stop: the Wi-Fi owner
/// the stopped radio gave up. The interrupt setup is the composition's
/// while the port runs: it routes the MAC and power interrupts to the
/// runtime and returns the setup at uninstall.
#[must_use = "the radio stop needs the held Wi-Fi owner back"]
pub struct Esp32s31WifiPortHeld<W, Membership> {
    owner: WifiRoleOwner<W, Membership>,
}

/// An installed port: the backend to run, the owner held for the radio stop
/// and the interrupt setup for the composition's route.
pub struct Esp32s31WifiPortInstalled<B, W, Membership> {
    pub backend: B,
    pub held: Esp32s31WifiPortHeld<W, Membership>,
    pub interrupt_setup: MacInterruptSetup,
}

/// Why the port could not be installed, with every owner it was given.
pub struct Esp32s31WifiPortInstallFailure<W, Membership, C, R> {
    pub error: Esp32s31WifiPortInstallError,
    pub wifi: WifiStopped<W, Membership>,
    pub core: C,
    pub retune: R,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Esp32s31WifiPortInstallError {
    /// The core is not disabled and idle, as a built or rebuilt core is.
    CoreNotDisabled,
    /// The arena refused the register owner.
    Registers(RadioOwnerArenaError),
    /// A core is installed already.
    Installed,
}

/// The stopped Wi-Fi and the core's parts back from an uninstalled port.
pub struct Esp32s31WifiPortUninstalled<W, Membership, Parts> {
    pub wifi: WifiStopped<W, Membership>,
    pub core: Parts,
}

/// A register transaction is still borrowed: the hardware, the disabled
/// core, the held owner and the setup are back, for a retry of the reclaim.
pub struct Esp32s31WifiPortUninstallFailure<'arena, W, Membership, Core> {
    pub error: RadioOwnerArenaError,
    pub hardware: CooperativeRadioHardware<'arena>,
    pub core: Core,
    pub held: Esp32s31WifiPortHeld<W, Membership>,
    pub interrupt_setup: MacInterruptSetup,
}

/// Install the lower-MAC port over the stopped Wi-Fi a radio start produced:
/// publish its register owner in `arena`, wrap it in
/// [`CooperativeRadioHardware`] and install `core` with `retune` into
/// `runtime`, on `channel`, with its access point at `access_point`. The
/// port starts disabled with an empty stream.
///
/// # Errors
///
/// Every owner comes back unchanged when the arena refuses the register
/// owner or a core is installed already.
#[allow(
    clippy::result_large_err,
    clippy::type_complexity,
    clippy::too_many_arguments,
    reason = "the no-alloc runtime returns every unconsumed owner by value"
)]
pub fn install_esp32s31_wifi_port<
    'r,
    'arena,
    'slot,
    M: RawMutex,
    P,
    E,
    T,
    R,
    W,
    Membership,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
    const EVENTS: usize,
    U: LowerMacRxUnit,
    O,
>(
    runtime: &'r Esp32s31LowerMac<
        'slot,
        M,
        P,
        E,
        T,
        CooperativeRadioHardware<'arena>,
        R,
        BUFFER_SIZE,
        TX_BUFFERS,
        EVENTS,
        U,
        O,
    >,
    wifi: WifiStopped<W, Membership>,
    arena: &'arena RadioOwnerArena,
    core: LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>,
    retune: R,
    channel: SupportedChannel<Esp32s31Caps<BUFFER_SIZE>>,
    access_point: IndividualAddress,
) -> Result<
    Esp32s31WifiPortInstalled<
        Esp32s31Backend<
            'r,
            'slot,
            M,
            P,
            E,
            T,
            CooperativeRadioHardware<'arena>,
            R,
            BUFFER_SIZE,
            TX_BUFFERS,
            EVENTS,
            U,
            O,
        >,
        W,
        Membership,
    >,
    Esp32s31WifiPortInstallFailure<
        W,
        Membership,
        LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>,
        R,
    >,
>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + crate::mac_clock::ReceptionClock,
    R: LowerMacRetune<CooperativeRadioHardware<'arena>>,
    O: oer_ieee80211_lower_mac::TxBody,
{
    // The radio start left the PHY on the stopped Wi-Fi's channel; the
    // core retunes at `Enable` when it is configured for another.
    let mut core = core;
    if !core.radio_tuned_to(wifi.current_channel()) {
        return Err(Esp32s31WifiPortInstallFailure {
            error: Esp32s31WifiPortInstallError::CoreNotDisabled,
            wifi,
            core,
            retune,
        });
    }
    let materialized = materialize_esp32s31_wifi_role(wifi, ());
    let (owner, registers, interrupt_setup) = (
        materialized.owner,
        materialized.registers,
        materialized.interrupt_setup,
    );
    let published = match arena.publish(registers) {
        Ok(published) => published,
        Err(failure) => {
            return Err(Esp32s31WifiPortInstallFailure {
                error: Esp32s31WifiPortInstallError::Registers(failure.error),
                wifi: owner.into_stopped(failure.owner, interrupt_setup, ()).wifi,
                core,
                retune,
            });
        }
    };
    let hardware = CooperativeRadioHardware::new(published);
    match runtime.install(
        Esp32s31LowerMacParts {
            core,
            hardware,
            retune,
        },
        channel,
        access_point,
    ) {
        Ok(backend) => Ok(Esp32s31WifiPortInstalled {
            backend,
            held: Esp32s31WifiPortHeld { owner },
            interrupt_setup,
        }),
        Err(parts) => {
            let registers = match parts.hardware.try_into_registers() {
                Ok(registers) => registers,
                // Nothing borrowed the fresh publication.
                Err((_, error)) => unreachable!("a fresh publication reclaims: {error:?}"),
            };
            Err(Esp32s31WifiPortInstallFailure {
                error: Esp32s31WifiPortInstallError::Installed,
                wifi: owner.into_stopped(registers, interrupt_setup, ()).wifi,
                core: parts.core,
                retune: parts.retune,
            })
        }
    }
}

/// Take the port apart at radio stop, with the core and registers
/// `run_port` returned after the body's uninstall: reclaim the register
/// owner from the arena and reassemble the stopped Wi-Fi from it, the held
/// owner and the interrupt setup the composition's route returned. The
/// stopped Wi-Fi records the channel the core is tuned to.
///
/// # Errors
///
/// A still borrowed register transaction returns every owner, as
/// [`Esp32s31WifiPortUninstallFailure`] says.
#[allow(
    clippy::result_large_err,
    clippy::type_complexity,
    reason = "the no-alloc runtime returns every unconsumed owner by value"
)]
pub fn uninstall_esp32s31_wifi_port<
    'arena,
    'slot,
    P,
    E,
    T,
    W,
    Membership,
    const BUFFER_SIZE: usize,
    const TX_BUFFERS: usize,
>(
    uninstalled: (
        LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>,
        CooperativeRadioHardware<'arena>,
    ),
    held: Esp32s31WifiPortHeld<W, Membership>,
    interrupt_setup: MacInterruptSetup,
) -> Result<
    Esp32s31WifiPortUninstalled<
        W,
        Membership,
        LowerMacCoreParts<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>,
    >,
    Esp32s31WifiPortUninstallFailure<
        'arena,
        W,
        Membership,
        LowerMacCore<'slot, P, E, T, BUFFER_SIZE, TX_BUFFERS>,
    >,
>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    let (core, hardware) = uninstalled;
    let registers = match hardware.try_into_registers() {
        Ok(registers) => registers,
        Err((hardware, error)) => {
            return Err(Esp32s31WifiPortUninstallFailure {
                error,
                hardware,
                core,
                held,
                interrupt_setup,
            });
        }
    };
    let mut owner = held.owner;
    owner.set_current_channel(core.tuned_channel());
    let wifi = owner.into_stopped(registers, interrupt_setup, ()).wifi;
    let Ok(core) = core.dismantle() else {
        unreachable!("the runtime gives back only a disabled, idle core");
    };
    Ok(Esp32s31WifiPortUninstalled { wifi, core })
}

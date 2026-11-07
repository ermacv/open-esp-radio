//! S31 binding of family lifecycle and consuming interrupt transactions, and
//! the owners of an operational MAC epoch with their PHY membership.
use core::fmt;
use oer_chips_esp32s31_hal::{
    power::PlatformClockBackend,
    radio::domain::RadioDomain,
    radio::ieee802154::{
        Ieee802154Clocked, Ieee802154Operational,
        mac::{
            Ieee802154ArmedTaskOwner, Ieee802154InterruptOwner, Ieee802154InterruptSetupOwner,
            Ieee802154StartupPort, Ieee802154TaskOwner, Ieee802154TeardownPort,
        },
    },
};
use oer_chips_esp32s31_radio_phy::ieee802154_client::Ieee802154PhyMembership;
use oer_chips_family_platform_interrupt_table::{Bound, Entry};
use oer_chips_family_radio_ieee802154_engine::{
    interrupt,
    lifecycle::{
        Ieee802154FoundationFailure, Ieee802154FoundationReturn, Ieee802154Lifecycle,
        Ieee802154ResetFailure, state,
    },
};
use oer_portable_mmio::Placement;

/// Pulse the private MAC/APB reset through the shared radio's exclusive loan.
pub fn reset_mac<B: PlatformClockBackend, T>(
    clocked: Ieee802154Clocked,
    radio: &mut RadioDomain<B, T>,
) -> Result<
    Ieee802154Lifecycle<Ieee802154Clocked, state::Reset>,
    Ieee802154ResetFailure<Ieee802154Clocked>,
> {
    let mut port = clocked.reset_port(radio);
    Ieee802154Lifecycle::clocked(clocked).reset_mac(&mut port)
}
/// Complete masked pre-bind discard before releasing the bindable IRQ owner.
pub fn arm<P: Placement, E: Placement, B: Placement>(
    setup: Ieee802154InterruptSetupOwner<P>,
    task: Ieee802154TaskOwner<P, E, B>,
) -> (
    Ieee802154ArmedTaskOwner<P, E, B>,
    Ieee802154InterruptOwner<P>,
) {
    interrupt::arm::<Ieee802154StartupPort<P, E, B>>(setup, task)
}
/// Activate the same runtime event set as every subsequent bound session.
pub fn activate<
    P: Placement,
    E: Placement,
    B: Placement,
    I: Entry<Part = Ieee802154InterruptOwner<P>>,
>(
    startup: Ieee802154ArmedTaskOwner<P, E, B>,
    bound: &Bound<I>,
) -> Ieee802154TaskOwner<P, E, B> {
    startup.activate(bound, interrupt::RUNTIME_EVENTS)
}
/// Publish the same ten runtime events for a later bound session.
pub fn enable_events<
    P: Placement,
    E: Placement,
    B: Placement,
    I: Entry<Part = Ieee802154InterruptOwner<P>>,
>(
    task: &mut Ieee802154TaskOwner<P, E, B>,
    bound: &Bound<I>,
) {
    task.enable_events(bound, interrupt::RUNTIME_EVENTS);
}
/// Stop and return an unbound polling epoch, consuming the saved runtime sample.
pub fn deactivate<'a, P: Placement, E: Placement, B: Placement>(
    interrupt: Ieee802154InterruptOwner<P>,
    task: &'a mut Ieee802154TaskOwner<P, E, B>,
) -> Ieee802154InterruptSetupOwner<P> {
    interrupt::deactivate::<Ieee802154TeardownPort<'a, P, E, B>>(interrupt, task)
}

/// The owners of one operational IEEE 802.15.4 MAC epoch together with its
/// PHY membership: the MAC is installed only with the membership a join
/// issued.
///
/// ```
/// use oer_chips_esp32s31_hal::radio::ieee802154::Ieee802154Clocked;
/// use oer_chips_family_radio_ieee802154_engine::lifecycle::{Ieee802154Lifecycle, state};
/// use oer_chips_esp32s31_radio_ieee802154_driver::lifecycle::RegisteredIeee802154Operational;
/// use oer_chips_esp32s31_radio_phy::ieee802154_client::Ieee802154PhyMembership;
/// fn install(
///     foundation: Ieee802154Lifecycle<Ieee802154Clocked, state::FoundationConfigured>,
///     membership: Ieee802154PhyMembership,
/// ) -> RegisteredIeee802154Operational {
///     RegisteredIeee802154Operational::new(foundation, membership)
/// }
/// ```
///
/// Not without it:
///
/// ```compile_fail,E0061
/// use oer_chips_esp32s31_hal::radio::ieee802154::Ieee802154Clocked;
/// use oer_chips_family_radio_ieee802154_engine::lifecycle::{Ieee802154Lifecycle, state};
/// use oer_chips_esp32s31_radio_ieee802154_driver::lifecycle::RegisteredIeee802154Operational;
/// fn install(foundation: Ieee802154Lifecycle<Ieee802154Clocked, state::FoundationConfigured>) -> RegisteredIeee802154Operational {
///     RegisteredIeee802154Operational::new(foundation)
/// }
/// ```
#[must_use = "the operational owners must return to their registered route"]
pub struct RegisteredIeee802154Operational {
    /// Task-side MAC registers.
    pub task: Ieee802154TaskOwner,
    /// Inactive interrupt ownership for the platform CPU route.
    pub interrupts: Ieee802154InterruptSetupOwner,
    /// The PHY membership retained while the MAC is operational.
    pub route: RegisteredIeee802154OperationalRoute,
}

impl RegisteredIeee802154Operational {
    /// Hand the foundation owners to the operational MAC under `membership`.
    /// This performs no MMIO.
    pub fn new(
        foundation: Ieee802154Lifecycle<Ieee802154Clocked, state::FoundationConfigured>,
        membership: Ieee802154PhyMembership,
    ) -> Self {
        let (clocked, foundation_return) = foundation.into_operational();
        let Ieee802154Operational { task, interrupts } = clocked.into_operational();
        Self {
            task,
            interrupts,
            route: RegisteredIeee802154OperationalRoute {
                membership,
                foundation_return,
            },
        }
    }
}

/// PHY membership retained while the MAC is operational. The membership
/// comes back only with the foundation owners
/// ([`RegisteredIeee802154OperationalRoute::into_foundation`]), so IEEE 802.15.4
/// leaves the PHY only after its operational MAC was taken apart:
///
/// ```
/// use oer_chips_esp32s31_hal::radio::ieee802154::mac::{Ieee802154InterruptSetupOwner, Ieee802154TaskOwner};
/// use oer_chips_esp32s31_radio_ieee802154_driver::lifecycle::RegisteredIeee802154OperationalRoute;
/// fn uninstall(
///     route: RegisteredIeee802154OperationalRoute,
///     task: Ieee802154TaskOwner,
///     interrupts: Ieee802154InterruptSetupOwner,
/// ) {
///     if let Ok((_foundation, _membership)) = route.into_foundation(task, interrupts) {}
/// }
/// ```
///
/// ```compile_fail,E0616
/// use oer_chips_esp32s31_radio_ieee802154_driver::lifecycle::RegisteredIeee802154OperationalRoute;
/// use oer_chips_esp32s31_radio_phy::ieee802154_client::Ieee802154PhyMembership;
/// fn leave_early(route: RegisteredIeee802154OperationalRoute) -> Ieee802154PhyMembership {
///     route.membership
/// }
/// ```
#[must_use = "the registered operational route must reunite with its owners"]
pub struct RegisteredIeee802154OperationalRoute {
    membership: Ieee802154PhyMembership,
    foundation_return: Ieee802154FoundationReturn<Ieee802154Clocked>,
}

impl RegisteredIeee802154OperationalRoute {
    /// Reunite the quiescent operational owners and prove the MAC
    /// foundation again.
    ///
    /// The operational epoch rewrote the MAC PIB, so the return proves the
    /// foundation, not a static policy.
    ///
    /// # Errors
    ///
    /// A foundation field did not read back; the failure keeps the reset
    /// owner and the membership.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains the owner and the membership"
    )]
    pub fn into_foundation(
        self,
        task: Ieee802154TaskOwner,
        interrupts: Ieee802154InterruptSetupOwner,
    ) -> Result<
        (
            Ieee802154Lifecycle<Ieee802154Clocked, state::FoundationConfigured>,
            Ieee802154PhyMembership,
        ),
        RegisteredIeee802154ReturnFailure,
    > {
        match Ieee802154Lifecycle::resume(
            Ieee802154Clocked::from_operational(Ieee802154Operational { task, interrupts }),
            self.foundation_return,
        ) {
            Ok(foundation) => Ok((foundation, self.membership)),
            Err(failure) => Err(RegisteredIeee802154ReturnFailure {
                failure,
                membership: self.membership,
            }),
        }
    }
}

/// Failed foundation proof on return, retaining the reset owner and the
/// membership.
#[must_use = "a failed return still owns the partition and the PHY membership"]
pub struct RegisteredIeee802154ReturnFailure<M = Ieee802154PhyMembership> {
    failure: Ieee802154FoundationFailure<Ieee802154Clocked>,
    membership: M,
}

impl<M> RegisteredIeee802154ReturnFailure<M> {
    /// Borrow the foundation readback failure.
    pub const fn failure(&self) -> &Ieee802154FoundationFailure<Ieee802154Clocked> {
        &self.failure
    }

    /// Recover the reset owner and the membership.
    pub fn into_parts(self) -> (Ieee802154FoundationFailure<Ieee802154Clocked>, M) {
        (self.failure, self.membership)
    }
}

impl<M> fmt::Debug for RegisteredIeee802154ReturnFailure<M> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisteredIeee802154ReturnFailure")
            .field("failure", &self.failure.error())
            .finish_non_exhaustive()
    }
}

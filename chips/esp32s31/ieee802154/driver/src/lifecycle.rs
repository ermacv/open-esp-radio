//! S31 binding of family lifecycle and consuming interrupt transactions.
use oer_esp32s31_hal::{
    domain::RadioDomain,
    ieee802154::{
        Ieee802154Clocked,
        mac::{
            Ieee802154ArmedTaskOwner, Ieee802154InterruptOwner, Ieee802154InterruptSetupOwner,
            Ieee802154StartupPort, Ieee802154TaskOwner, Ieee802154TeardownPort,
        },
    },
    power::PlatformClockBackend,
};
use oer_espressif_ieee802154_engine::{
    interrupt,
    lifecycle::{Ieee802154Lifecycle, Ieee802154ResetFailure, state},
};
use oer_espressif_interrupt_table::{Bound, Entry};
use oer_mmio::Placement;

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

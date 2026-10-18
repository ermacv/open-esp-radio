#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]
use oer_esp32s31_ieee80211_form::scope::{
    ScopeReturnLatch, DrainBound, DriverCallbacks, DriverEvents, DriverScope, DriverStatus,
    DriverStep, IrqControl, ScopeLease, ScopeRoute, StopOutcome,
};
use oer_esp32s31_hal::owner::RadioRuntimeOwner;
use oer_esp32s31_ieee80211_mac::irq::epoch::{ActiveEpoch, Moderated};

fn step(_: &mut RadioRuntimeOwner, _: &mut IrqControl<'_>, _: DriverEvents, _: DriverStatus) -> DriverStep {
    DriverStep::Retired
}
fn stop(_: &mut RadioRuntimeOwner, _: &mut IrqControl<'_>) -> StopOutcome {
    StopOutcome::Unconfirmed
}

/// The caller must establish the scope's routing and callback premises.
pub unsafe fn bind<'a>(
    lease: ScopeLease<RadioRuntimeOwner, ActiveEpoch<Moderated>, (), ()>,
    radio: RadioRuntimeOwner,
    interrupts: ActiveEpoch<Moderated>,
    route: ScopeRoute<(), ()>,
    latch: &'a mut ScopeReturnLatch<RadioRuntimeOwner, ActiveEpoch<Moderated>, (), ()>,
) -> DriverScope<'a, RadioRuntimeOwner, ActiveEpoch<Moderated>, (), ()> {
    #[cfg(ui_reject)]
    return DriverScope::new(
        lease,
        radio,
        interrupts,
        DriverCallbacks::new(step, stop),
        route,
        DrainBound::from_maxima::<100, 20, 30>(),
        latch,
    ); // ui-error: E0133
    #[cfg(not(ui_reject))]
    // SAFETY: this UI twin explicitly transfers its caller's whole-binding
    // premise; a route alone does not authorize these later-chosen callbacks.
    unsafe {
        DriverScope::new(
            lease,
            radio,
            interrupts,
            DriverCallbacks::new(step, stop),
            route,
            DrainBound::from_maxima::<100, 20, 30>(),
            latch,
        )
    }
}

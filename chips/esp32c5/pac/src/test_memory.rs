//! Tests of generated field readers over initialized host memory.

#[test]
#[allow(
    unsafe_code,
    reason = "synthetic register block backed by initialized host memory"
)]
fn reserved_route_bits_do_not_change_named_reset_classification() {
    use crate::Ieee802154RouteState;
    let word = 0xfffffec0u32;
    // SAFETY: the register block is exactly one u32 register. This local
    // initialized word supplies its size and alignment and outlives the
    // read; no hardware address or mutable alias is involved.
    let registers = unsafe {
        &*core::ptr::from_ref(&word).cast::<crate::svd::ieee802154_interrupt_route::RegisterBlock>()
    };
    let route = registers.core0_route().read();
    assert_eq!(
        Ieee802154RouteState::from_observation(
            route.map().bits() != 0,
            route.pass_in_sec().bit_is_set()
        ),
        Ieee802154RouteState::ResetDetached
    );
    let word = 0xffffffc0u32;
    // SAFETY: the same initialized, aligned single-register host storage.
    let registers = unsafe {
        &*core::ptr::from_ref(&word).cast::<crate::svd::ieee802154_interrupt_route::RegisterBlock>()
    };
    let route = registers.core0_route().read();
    assert_eq!(
        Ieee802154RouteState::from_observation(
            route.map().bits() != 0,
            route.pass_in_sec().bit_is_set()
        ),
        Ieee802154RouteState::PassLevelConfigured
    );
}

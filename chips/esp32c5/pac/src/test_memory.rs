//! Tests of generated field readers over host memory.

use crate::generated::ieee802154_interrupt_route::Ieee802154InterruptRoute;

/// The route classification of one interrupt-matrix word image.
fn classify(word: u32) -> crate::Ieee802154RouteState {
    let mut memory = [word];
    let route = Ieee802154InterruptRoute::host(&mut memory)
        .expect("one word holds the route block")
        .read_core0_route();
    crate::Ieee802154RouteState::from_observation(route.map().get() != 0, route.pass_in_sec())
}

#[test]
fn reserved_route_bits_do_not_change_named_reset_classification() {
    use crate::Ieee802154RouteState;
    assert_eq!(classify(0xffff_fec0), Ieee802154RouteState::ResetDetached);
    assert_eq!(
        classify(0xffff_ffc0),
        Ieee802154RouteState::PassLevelConfigured
    );
}

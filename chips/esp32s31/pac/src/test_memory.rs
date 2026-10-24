//! Synthetic layout references for transaction recorders; never dereferenced.

use crate::generated::ieee802154_interrupt_route::{self, Ieee802154InterruptRoute};
use crate::layouts::{DmaLayout, DmaRef, DmaWindowAddress};

#[allow(
    unsafe_code,
    clippy::disallowed_methods,
    reason = "host transaction recorders use synthetic addresses without accessing memory"
)]
pub(crate) const fn reference<L: DmaLayout>(address: u32) -> DmaRef<L> {
    // SAFETY: the host models record pointer identities only. They never
    // dereference these synthetic references or publish them to hardware.
    unsafe { DmaRef::new_unchecked(DmaWindowAddress::new_unchecked(address)) }
}

/// The route classification of both cores' interrupt-matrix word images.
fn classify(core0: u32, core1: u32) -> crate::Ieee802154RouteState {
    let mut memory = [0u32; ieee802154_interrupt_route::SIZE / 4];
    memory[0] = core0;
    memory[0x800 / 4] = core1;
    let route = Ieee802154InterruptRoute::host(&mut memory).expect("the route block fits");
    let (core0, core1) = (route.read_core0_route(), route.read_core1_route());
    crate::Ieee802154RouteState::from_observation(
        core0.map().get() != 0 || core1.map().get() != 0,
        core0.pass_level().get() != 0 || core1.pass_level().get() != 0,
    )
}

#[test]
fn reserved_route_bits_do_not_change_named_reset_classification() {
    use crate::Ieee802154RouteState;
    for bits in [0xffff_fcc0u32, 0x0000_00c0, 0xffff_fc00] {
        assert_eq!(classify(bits, bits), Ieee802154RouteState::ResetDetached);
    }
    assert_eq!(
        classify(0xffff_fcc1, 0),
        Ieee802154RouteState::DestinationAssigned
    );
    assert_eq!(
        classify(0, 0xffff_fcc1),
        Ieee802154RouteState::DestinationAssigned
    );
}

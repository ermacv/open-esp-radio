//! Synthetic layout references for transaction recorders; never dereferenced.

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

#[test]
#[allow(
    unsafe_code,
    reason = "synthetic register block backed by initialized host memory"
)]
fn reserved_route_bits_do_not_change_named_reset_classification() {
    use crate::Ieee802154RouteState;
    for bits in [0xfffffcc0u32, 0x000000c0, 0xfffffc00] {
        let words = [bits; 0x804 / 4];
        // SAFETY: this read-only register block contains u32 registers at
        // offsets 0 and 0x800. The initialized u32 array supplies their full
        // size and alignment, lives through the read, and is never MMIO.
        let registers = unsafe {
            &*words
                .as_ptr()
                .cast::<crate::svd::ieee802154_interrupt_route::RegisterBlock>()
        };
        let route = registers.core0_route().read();
        assert_eq!(
            Ieee802154RouteState::from_observation(
                route.map().bits() != 0,
                route.pass_level().bits() != 0
            ),
            Ieee802154RouteState::ResetDetached,
        );
    }
    let words = [0xfffffcc1u32; 0x804 / 4];
    // SAFETY: the same initialized, aligned and sufficiently sized host
    // storage as above; no hardware address is dereferenced.
    let registers = unsafe {
        &*words
            .as_ptr()
            .cast::<crate::svd::ieee802154_interrupt_route::RegisterBlock>()
    };
    let route = registers.core0_route().read();
    assert_eq!(
        Ieee802154RouteState::from_observation(
            route.map().bits() != 0,
            route.pass_level().bits() != 0
        ),
        Ieee802154RouteState::DestinationAssigned
    );
}

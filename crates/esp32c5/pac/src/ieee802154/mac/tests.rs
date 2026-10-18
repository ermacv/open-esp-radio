#[test]
fn dedicated_route_lends_the_same_narrow_mac_surface() {
    let (mut task, interrupts) = crate::ownership::test_support::ieee802154_task();
    let mut lease = task.ieee802154_register_lease();

    // The host reaches only the architecture-neutral fence. The lease is
    // backed by the dedicated register set rather than by a second raw
    // singleton.
    lease.order_device_accesses();
    let _partition = task.into_partition(interrupts);
}

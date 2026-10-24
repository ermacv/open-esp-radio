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

#[test]
fn each_multipan_context_names_its_own_register_index() {
    for context in 0..super::Ieee802154MultipanIndex::COUNT {
        let index = super::Ieee802154MultipanIndex::new(context).expect("one of four contexts");
        assert_eq!(
            super::multipan_register_index(index).get(),
            usize::from(context)
        );
    }
}

#[test]
fn bounded_field_values_reach_their_writers_unchanged() {
    let code = super::Ieee802154FrequencyCode::new(0x7f).expect("seven bits");
    assert_eq!(code.bits().get(), 0x7f);
    assert_eq!(code.value(), 0x7f);
    assert!(super::Ieee802154FrequencyCode::new(0x80).is_none());
    let pti = super::Ieee802154Pti::new(0xf).expect("four bits");
    assert_eq!(pti.bits().get(), 0xf);
    assert_eq!(pti.value(), 0xf);
    assert!(super::Ieee802154Pti::new(0x10).is_none());
}

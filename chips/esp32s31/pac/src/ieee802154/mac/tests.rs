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

/// Each power-sequencing field reaches the accessor named after it.
#[test]
fn power_sequence_fields_keep_their_identity() {
    let sequence =
        super::Ieee802154PowerSequence::from_raw(crate::ieee802154::ownership::RawPowerSequence {
            paon_delay: 0x3a5,
            txon_delay: 0x2d,
            txen_stop_delay: 0x21,
            txoff_delay: 0x05,
            rxon_delay: 0x532,
            txrx_switch_delay: 0x75,
            cont_rx_delay: 0x12,
            dcdc_pre_up_delay: 0xc3,
            dcdc_down_delay: 0x5a,
            dcdc_ctrl_enabled: true,
            tx_dcdc_up: false,
        });
    assert_eq!(sequence.pa_on_delay(), 0x3a5);
    assert_eq!(sequence.tx_on_delay(), 0x2d);
    assert_eq!(sequence.tx_enable_stop_delay(), 0x21);
    assert_eq!(sequence.tx_off_delay(), 0x05);
    assert_eq!(sequence.rx_on_delay(), 0x532);
    assert_eq!(sequence.txrx_switch_delay(), 0x75);
    assert_eq!(sequence.continuous_rx_delay(), 0x12);
    assert_eq!(sequence.dcdc_pre_raise_delay(), 0xc3);
    assert_eq!(sequence.dcdc_drop_delay(), 0x5a);
    assert!(sequence.dcdc_control_enabled());
    assert!(!sequence.dcdc_raise_for_tx());
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
    let code = super::Ieee802154FrequencyCode::new(0xff);
    assert_eq!(code.bits(), 0xff);
    assert_eq!(code.value(), 0xff);
    let pti = super::Ieee802154Pti::new(0x1f).expect("five bits");
    assert_eq!(pti.bits().get(), 0x1f);
    assert_eq!(pti.value(), 0x1f);
    assert!(super::Ieee802154Pti::new(0x20).is_none());
}

use super::*;
use oer_esp32s31_hal::dma_static::Placed;
use std::boxed::Box;

// Exercise the same wire decode and typed hardware policy as AdvertisingImageBuilder.
fn decode_advertising_address(
    pdu: &[u8],
) -> Result<LegacyAdvertisingOwnAddress, LegacyAdvertisingPduError> {
    let prefix = oer_bluetooth_le_wire::advertising_header::LegacyAdvertisingPduPrefix::decode(pdu)
        .map_err(|_| LegacyAdvertisingPduError::MissingAdvertiserAddress)?;
    super::legacy_advertising_event_image::own_address_for_advertisement(prefix)
}

#[test]
fn a_role_recipe_cannot_overwrite_an_already_prepared_event() {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<DtmStorage, 1>::new()));
    let numbers = SchedulerAllocationConfig::new(0, 0, 0)
        .expect("numbers fit")
        .direct_test_mode();
    with_pool(|pool| {
        with_epoch(|epoch| {
            let mut pool = pool
                .place::<HostDma<0x2f00_1000>, _, 1>(epoch, Placed::host(storage), numbers)
                .expect("one DTM allocation");
            let lease = pool.acquire().expect("one slot");
            let initial = pool.words(&lease).expect("CPU value seed");
            let builder = DtmImageBuilder::receiver(
                DtmEventPhase::Initial,
                DtmSchedulerReceiverPhy::Le1M,
                LeTxPower::from_dbm(0).expect("supported power"),
                0,
                DtmSchedulerFrequency::from_controller_image(24).expect("valid frequency"),
                SchedulerRawWindow::from_projected_scheduler_window(100, 200)
                    .expect("nonempty window"),
                100,
            );
            builder
                .prepare(&mut pool, &lease)
                .expect("CPU graph accepts its own bound seed");
            let committed = pool.words(&lease).expect("detached value snapshot");
            assert_ne!(committed, initial);
            assert_eq!(
                builder.prepare(&mut pool, &lease),
                Err(DtmPrepareError::State)
            );
            assert_eq!(pool.words(&lease), Some(committed));
            pool.release(lease)
                .expect("unpublished graph remains CPU owned");
        })
    });
}

#[test]
fn advertising_policy_orders_selected_channels_and_marks_only_followers_flexible() {
    assert_eq!(
        LegacyAdvertisingPrimaryChannelPlan::new(false, false, false),
        None
    );
    let plan = LegacyAdvertisingPrimaryChannelPlan::new(true, false, true).expect("two channels");
    let image = plan.image(u32::MAX - 4, 10, 7);
    let items = image.items();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].channel, LegacyAdvertisingPrimaryChannel::Channel37);
    assert_eq!(items[1].channel, LegacyAdvertisingPrimaryChannel::Channel39);
    assert!(!items[0].flexible_start);
    assert!(items[1].flexible_start);
    assert_eq!((items[0].start, items[0].end), (u32::MAX - 4, 5));
    assert_eq!((items[1].start, items[1].end), (5, 15));
    assert!(items.iter().all(|item| item.sequence_lead == 7));
}

#[test]
fn advertising_pdu_policy_selects_the_host_address_and_rejects_unsupported_headers() {
    assert_eq!(
        decode_advertising_address(&[2, 0]),
        Err(LegacyAdvertisingPduError::MissingAdvertiserAddress)
    );
    assert_eq!(
        decode_advertising_address(&[0, 6, 1, 2, 3, 4, 5, 6]),
        Err(LegacyAdvertisingPduError::UnsupportedPduType)
    );
    assert_eq!(
        decode_advertising_address(&[0x12, 6, 1, 2, 3, 4, 5, 6]),
        Err(LegacyAdvertisingPduError::UnsupportedHeaderFlags)
    );
    assert_eq!(
        decode_advertising_address(&[2, 6, 1, 2, 3, 4, 5, 6]),
        Ok(LegacyAdvertisingOwnAddress::Public)
    );
    assert_eq!(
        decode_advertising_address(&[0x42, 6, 1, 2, 3, 4, 5, 6]),
        Ok(LegacyAdvertisingOwnAddress::Random([1, 2, 3, 4, 5, 6]))
    );
}

#[test]
fn advertising_address_decoding_preserves_all_header_and_truncation_decisions() {
    for header in u8::MIN..=u8::MAX {
        let pdu = [header, 6, 1, 2, 3, 4, 5, 6];
        for length in 0..pdu.len() {
            assert_eq!(
                decode_advertising_address(&pdu[..length]),
                Err(LegacyAdvertisingPduError::MissingAdvertiserAddress)
            );
        }
        let expected = match header {
            0x02 => Ok(LegacyAdvertisingOwnAddress::Public),
            0x42 => Ok(LegacyAdvertisingOwnAddress::Random([1, 2, 3, 4, 5, 6])),
            0x12 | 0x22 | 0x32 | 0x52 | 0x62 | 0x72 | 0x82 | 0x92 | 0xa2 | 0xb2 | 0xc2 | 0xd2
            | 0xe2 | 0xf2 => Err(LegacyAdvertisingPduError::UnsupportedHeaderFlags),
            _ => Err(LegacyAdvertisingPduError::UnsupportedPduType),
        };
        assert_eq!(decode_advertising_address(&pdu), expected);
    }
    // Packet extent validation belongs to prepare_packet, after address selection.
    assert_eq!(
        decode_advertising_address(&[0x42, 0, 1, 2, 3, 4, 5, 6, 9]),
        Ok(LegacyAdvertisingOwnAddress::Random([1, 2, 3, 4, 5, 6]))
    );
}

#[test]
fn scanner_policy_keeps_active_filter_and_start_choices_independent() {
    let power = LeTxPower::from_dbm(9).expect("provider level");
    for (scan_type, kind) in [
        (LegacyScanType::Passive, ScanItemKind::Listen),
        (LegacyScanType::Active, ScanItemKind::RequestResponse),
    ] {
        for (filter, accept_list) in [
            (LegacyScanFilterPolicy::AcceptAll, false),
            (LegacyScanFilterPolicy::AcceptListOnly, true),
        ] {
            let image = LegacyScanResetConfig::le_1m_public(power, scan_type, filter).image();
            assert_eq!(
                (image.power, image.kind, image.accept_list),
                (power, kind, accept_list)
            );
        }
    }
    assert!(
        !LegacyScanStartSelection::Requested
            .image(LegacyScanPrimaryChannel::Channel38)
            .flexible_start
    );
    assert!(
        LegacyScanStartSelection::EarliestAvailable
            .image(LegacyScanPrimaryChannel::Channel38)
            .flexible_start
    );
}

#[test]
fn only_recurring_dtm_events_copy_the_link_power_into_the_item() {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<DtmStorage, 1>::new()));
    let numbers = SchedulerAllocationConfig::new(0, 0, 0)
        .expect("numbers fit")
        .direct_test_mode();
    with_pool(|scope| {
        with_epoch(|epoch| {
            let mut pool = scope
                .place::<HostDma<0x2f00_1000>, _, 1>(epoch, Placed::host(storage), numbers)
                .expect("one pool");
            let lease = pool.acquire().expect("one slot");
            let power = LeTxPower::from_dbm(9).expect("provider level");
            let make = |phase, power| {
                DtmImageBuilder::transmitter(
                    phase,
                    DtmSchedulerTransmitterPhy::Le1M,
                    power,
                    0,
                    DtmSchedulerFrequency::from_controller_image(24).expect("frequency fits"),
                    SchedulerRawWindow::from_projected_scheduler_window(100, 200)
                        .expect("window fits"),
                    101,
                )
            };
            make(DtmEventPhase::Initial, power)
                .prepare(&mut pool, &lease)
                .expect("initial image");
            let initial = pool.words(&lease).expect("CPU snapshot");
            assert_eq!(initial.link_state().power(), Some(power));
            assert_eq!(
                initial.scheduler_item().power(),
                LeTxPower::from_dbm(LeTxPower::LOWEST_DBM)
            );
            assert_eq!(initial.scheduler_item().sequence_start(), 101);
            assert_eq!(initial.scheduler_item().raw_window(), (100, 200));
            pool.finish_event(&lease, false)
                .expect("unpublished event recycles");
            // Each event restores its authoritative policy rather than
            // inheriting stale fields left by the previous transmission.
            let recurring_power = LeTxPower::from_dbm(-6).expect("provider level");
            make(DtmEventPhase::Recurring, recurring_power)
                .prepare(&mut pool, &lease)
                .expect("recurring image");
            assert_eq!(
                pool.words(&lease)
                    .expect("CPU snapshot")
                    .link_state()
                    .power(),
                Some(recurring_power)
            );
            assert_eq!(
                pool.words(&lease)
                    .expect("CPU snapshot")
                    .scheduler_item()
                    .power(),
                Some(recurring_power)
            );
            pool.release(lease).expect("nothing was listed");
        })
    });
}

#[test]
fn receiver_dtm_phases_choose_their_distinct_wait_profiles_and_valid_phy() {
    let priority = SchedulerEventPriority::new(3).expect("priority fits");
    for (phy, code) in [
        (DtmSchedulerReceiverPhy::Le1M, 0),
        (DtmSchedulerReceiverPhy::Le2M, 1),
        (DtmSchedulerReceiverPhy::LeCoded, 3),
    ] {
        for (phase, extension) in [
            (DtmReceiverEventPhase::Initial, 15),
            (DtmReceiverEventPhase::Recurring, 0),
        ] {
            let actual = DtmSchedulerItemEventType::Receiver { phy, phase }.configuration();
            let expected = DtmItemConfiguration::new(
                false,
                code,
                Some(DtmReceiveWait::new(1, extension).expect("timing fits")),
                priority,
            )
            .expect("PHY fits");
            assert_eq!(actual, expected);
        }
    }
}

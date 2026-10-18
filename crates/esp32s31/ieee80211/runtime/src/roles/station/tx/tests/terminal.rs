use super::*;

#[test]
fn exhausted_aggregate_emits_one_terminal_receipt() {
    let (mut device, network) = make_network();
    for marker in 1..=3 {
        send_frame(&mut device, marker);
    }
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let observer = RecordingAggregateTxObserver::default();
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap()
    .with_observer(&observer);
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );

    hardware.aggregate_completion = Some(aggregate_completion(7, 0b001));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(hardware.ht_publications, 2);
    assert_eq!(network.tx_consumer().promotion_capacity(), 0);
    assert!(device.transmit(&mut context()).is_some());

    // The missing MPDUs age out: the aggregate ends instead of retrying.
    embassy_futures::block_on(
        tx.ordinary
            .wait_until(oer_time::Instant::from_micros(u64::from(
                VENDOR_AMPDU_MSDU_LIFETIME_MICROS,
            ))),
    );
    hardware.aggregate_completion = Some(aggregate_completion(8, 0b00));
    assert_eq!(
        tx.service(
            &mut hardware,
            WifiTxWake::Interrupt {
                events: EVENT_TX_COMPLETE,
            },
        ),
        Ok(WifiTxProgress::Complete)
    );
    let work = tx.aggregate_work();
    let status = tx.take_last_aggregate_status().unwrap();
    assert_eq!(*observer.terminal.lock().unwrap(), [status]);
    assert_eq!(tx.take_last_aggregate_status(), None);
    assert_eq!(work.publications, 2);
    assert_eq!(work.mpdus, 5);
    assert!(work.psdu_bytes > 0);
    assert!(work.nominal_data_micros > 0);
    assert_eq!(work.unestimated_publications, 0);
    assert_eq!(
        Some(status),
        Some(MacAmpduTxStatus {
            result: MacAmpduTxResult::Incomplete,
            original_subframes: 3,
            aggregate_attempts: 2,
            aggregate_rate: TxPhyRate::Ht(TEST_RATE),
            block_acknowledged_subframes: 1,
            individual_retries: MacIndividualRetries::NONE,
            ack_snr_db: Some(96),
        })
    );
    send_frame(&mut device, 4);
    send_frame(&mut device, 5);
    send_frame(&mut device, 6);
    assert!(device.transmit(&mut context()).is_none());
    assert_eq!(network.tx_queue_len(), TEST_QUEUE_DEPTH);
    for _ in 0..TEST_QUEUE_DEPTH {
        drop(network.try_receive_tx_direct().unwrap());
    }
}

#[test]
fn unavailable_or_changed_radio_clock_retains_the_published_aggregate_and_completion() {
    let (mut device, network) = make_network();
    for marker in 1..=2 {
        send_frame(&mut device, marker);
    }
    let first = network.try_receive_tx_direct().unwrap();
    let mut hardware = Hardware::default();
    let mut slot = core::pin::pin!(TxSlot::<TEST_BUFFER_SIZE>::new_model());
    let ordinary = make_ordinary(slot.as_mut(), &mut hardware);
    let mut ampdu = core::pin::pin!(HtAmpduTxStorage::<TEST_SLOTS, 0>::new());
    let mut tx = ConnectedTx::new_for_test(
        ordinary,
        AggregateTxResources::single(
            HtAmpduTxResources::new_model(ampdu.as_mut()).unwrap(),
            std::boxed::Box::leak(std::boxed::Box::new(RetainedAmpduDmaStorage::new())),
        ),
        AggregateTxConfig {
            rate: TxPhyRate::Ht(TEST_RATE),
            frame_limit: TEST_SLOTS as u8,
            completion_timeout: oer_time::Duration::from_micros(250_000),
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .unwrap();
    tx.set_block_ack_window(0, Some(TEST_SLOTS as u16));
    assert_eq!(
        tx.start_network(&mut hardware, first, &network.tx_consumer()),
        Ok(WifiTxProgress::Pending)
    );
    assert_eq!(tx.radio_stamp().unwrap().at.as_micros(), 5_000);
    assert_eq!(
        tx.ordinary.now().as_micros(),
        0,
        "radio and monotonic epochs differ"
    );
    hardware.aggregate_completion = Some(aggregate_completion(7, 0b11));
    tx.ordinary.timer().available.set(false);
    let wake = WifiTxWake::Interrupt {
        events: EVENT_TX_COMPLETE,
    };
    assert_eq!(
        tx.service(&mut hardware, wake),
        Err(AggregateTxError::RadioClockUnavailable)
    );
    tx.ordinary.timer().available.set(true);
    tx.ordinary.timer().generation.set(2);
    assert_eq!(
        tx.service(&mut hardware, wake),
        Err(AggregateTxError::Retry(
            AmpduRetryError::ClockGenerationChanged {
                expected: crate::clock_generation(1),
                observed: crate::clock_generation(2)
            }
        ))
    );
    assert!(tx.active());
    assert_eq!(tx.aggregate_held_backings(), 2);
    assert!(hardware.aggregate_completion.is_some());
    assert_eq!(hardware.ht_publications, 1);
    assert_eq!(hardware.timeout_detaches, 0);
    assert!(
        tx.try_into_parts().is_err(),
        "failed timing does not release a hardware-owned descriptor"
    );
}

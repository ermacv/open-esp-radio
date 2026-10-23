#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use super::*;
use oer_time::Clock as _;
use oer_time_virtual::VirtualClock;

fn identity() -> PhyCalibrationIdentity {
    PhyCalibrationIdentity {
        rf_cal_version: 102,
        base_mac_address: [1, 2, 3, 4, 5, 6],
        mac_extension: 7,
    }
}
fn config() -> PrepareCapture {
    PrepareCapture {
        channel: 1,
        radio: CaptureRadio::Wifi,
    }
}
fn record() -> codec::CalibrationRecord {
    let mut record = codec::CalibrationRecord::decode(
        codec::S31_SEMANTIC_V1,
        include_bytes!("../../../../../chips/esp32s31/knowledge/src/calibration_cache/golden.bin"),
    )
    .unwrap();
    record.identity = codec::CalibrationIdentity {
        rf_cal_version: 102,
        base_mac_address: [1, 2, 3, 4, 5, 6],
        mac_extension: 7,
    };
    record.common.rc_calibrated = true;
    record.wifi.baseband_calibrated = true;
    record.wifi.pwdet_calibrated = true;
    record.wifi.tx_power_calibrated = true;
    record.wifi.tx_iq_calibrated = true;
    record.wifi.rx_gain_dc_calibrated = true;
    record.wifi.rx_gain_tables_initialized = true;
    record.bluetooth.tx_dc_calibrated = true;
    record.bluetooth.tx_power_calibrated = true;
    // A structurally valid record with a deliberately invalid native RX shape.
    record.wifi.wifi_rx_table_last_index = 255;
    record.wifi.shared_rx_table_last_index = 255;
    record
}
fn offer(bytes: &[u8]) -> CalibrationCacheMetadata {
    CalibrationCacheMetadata {
        format: CalibrationCacheFormat(codec::S31_SEMANTIC_V1),
        schema: codec::SNAPSHOT_SCHEMA,
        algorithm_revision: codec::CALIBRATION_ALGORITHM_REVISION,
        profile: codec::CALIBRATION_PROFILE_ID,
        length: bytes.len() as u16,
        crc32c: calibration_cache_crc32c(bytes),
    }
}
fn begin(cache: &mut Cache, clock: &impl oer_time::Clock) {
    cache
        .begin(
            StartCalibration {
                capture: config(),
                source: CalibrationSource::HostCache,
            },
            identity(),
            clock.now(),
        )
        .unwrap();
}
fn upload(cache: &mut Cache, bytes: &[u8], now: Instant) {
    for (index, bytes) in bytes.chunks(CALIBRATION_CACHE_PAGE).enumerate() {
        cache
            .write(
                WriteCalibrationCachePage {
                    ticket: TICKET,
                    first: (index * CALIBRATION_CACHE_PAGE) as u16,
                    bytes: bytes.try_into().unwrap(),
                },
                now,
            )
            .unwrap();
    }
}
fn complete(
    cache: &mut Cache,
    metadata: CalibrationCacheMetadata,
    now: Instant,
) -> CalibrationCacheDecision {
    cache
        .complete(
            CompleteCalibrationCache {
                ticket: TICKET,
                offer: CalibrationCacheOffer::Snapshot(metadata),
            },
            identity(),
            now,
        )
        .unwrap()
        .decision
}
fn ready(full: bool) -> CaptureReady {
    CaptureReady {
        epoch: 1,
        lifecycle: CaptureLifecycle::Cold,
        full_calibration_performed: full,
        path: if full {
            CalibrationPath::FullForCache
        } else {
            CalibrationPath::PartialFromCache
        },
        projection_schema: 1,
        calibration_words: 92,
        registers: 1,
        analog_bytes: 1,
        channel: 1,
        radio: CaptureRadio::Wifi,
    }
}

#[test]
fn a_start_never_holds_the_slot_while_waiting_and_registration_waits_for_published_reply() {
    let clock = VirtualClock::<8>::new();
    let mut storage = core::mem::MaybeUninit::uninit();
    let mut cache = Cache::place(&mut storage);
    assert_eq!(cache.identity(identity()).active_ticket, None);
    let started = cache
        .begin(
            StartCalibration {
                capture: config(),
                source: CalibrationSource::Full,
            },
            identity(),
            clock.now(),
        )
        .unwrap();
    assert!(matches!(
        started.state,
        CalibrationStartState::Registering {
            decision: CalibrationCacheDecision::FullRequested
        }
    ));
    assert!(!cache.pending());
    assert!(cache.take_registration().is_err());
    cache.release();
    assert!(cache.pending());
    assert!(cache.take_registration().unwrap().1.is_none());
    assert!(!cache.pending());
    assert!(cache.take_registration().is_err());
}
#[test]
fn timeout_wins_at_500_ms_and_pages_and_status_do_not_extend_the_window() {
    for elapsed in [499_999, 500_000, 500_001] {
        let clock = VirtualClock::<8>::new();
        let mut storage = core::mem::MaybeUninit::uninit();
        let mut cache = Cache::place(&mut storage);
        begin(&mut cache, &clock);
        let deadline = cache.deadline().unwrap();
        clock.advance(Duration::from_micros(elapsed)).unwrap();
        let page = WriteCalibrationCachePage {
            ticket: TICKET,
            first: 0,
            bytes: [1].as_slice().try_into().unwrap(),
        };
        assert_eq!(cache.write(page, clock.now()).is_ok(), elapsed < 500_000);
        assert_eq!(cache.deadline().is_some(), elapsed < 500_000);
        if elapsed < 500_000 {
            assert_eq!(cache.deadline(), Some(deadline));
        } else {
            assert!(cache.pending());
            let (_, candidate) = cache.take_registration().unwrap();
            assert!(candidate.is_none());
            assert!(matches!(
                cache.status(TICKET, identity(), clock.now()).unwrap().state,
                CalibrationStartState::Registering {
                    decision: CalibrationCacheDecision::TimedOut
                }
            ));
            assert!(
                cache
                    .complete(
                        CompleteCalibrationCache {
                            ticket: TICKET,
                            offer: CalibrationCacheOffer::NoCache(CalibrationCacheMiss::Missing)
                        },
                        identity(),
                        clock.now()
                    )
                    .is_err()
            );
        }
    }
}
#[test]
fn duplicate_gap_foreign_ticket_and_overflow_do_not_change_the_inbox() {
    let clock = VirtualClock::<8>::new();
    let mut storage = core::mem::MaybeUninit::uninit();
    let mut cache = Cache::place(&mut storage);
    begin(&mut cache, &clock);
    let page = WriteCalibrationCachePage {
        ticket: TICKET,
        first: 0,
        bytes: [1, 2].as_slice().try_into().unwrap(),
    };
    cache.write(page.clone(), clock.now()).unwrap();
    for (ticket, first) in [
        (TICKET, 0),
        (TICKET, 1),
        (TICKET, 3),
        (0, 2),
        (2, 2),
        (TICKET, u16::MAX),
    ] {
        assert!(
            cache
                .write(
                    WriteCalibrationCachePage {
                        ticket,
                        first,
                        ..page.clone()
                    },
                    clock.now()
                )
                .is_err()
        );
        assert_eq!(cache.next, 2);
        assert_eq!(&cache.bytes[..2], &[1, 2]);
    }
    assert!(
        cache
            .complete(
                CompleteCalibrationCache {
                    ticket: 2,
                    offer: CalibrationCacheOffer::NoCache(CalibrationCacheMiss::Missing)
                },
                identity(),
                clock.now()
            )
            .is_err()
    );
    assert_eq!(cache.next, 2);
}
#[test]
fn each_envelope_failure_is_a_caller_rejection_without_a_native_candidate() {
    let clock = VirtualClock::<8>::new();
    let bytes = record().encode().unwrap();
    for reason in [
        CalibrationCacheRejection::Format,
        CalibrationCacheRejection::Schema,
        CalibrationCacheRejection::Algorithm,
        CalibrationCacheRejection::Profile,
        CalibrationCacheRejection::Length,
        CalibrationCacheRejection::Checksum,
        CalibrationCacheRejection::Decode,
    ] {
        let mut storage = core::mem::MaybeUninit::uninit();
        let mut cache = Cache::place(&mut storage);
        begin(&mut cache, &clock);
        let mut bytes = bytes;
        if reason == CalibrationCacheRejection::Decode {
            bytes[32] = 2;
        }
        upload(&mut cache, &bytes, clock.now());
        let mut metadata = offer(&bytes);
        match reason {
            CalibrationCacheRejection::Format => metadata.format.0 += 1,
            CalibrationCacheRejection::Schema => metadata.schema += 1,
            CalibrationCacheRejection::Algorithm => metadata.algorithm_revision += 1,
            CalibrationCacheRejection::Profile => metadata.profile += 1,
            CalibrationCacheRejection::Length => metadata.length -= 1,
            CalibrationCacheRejection::Checksum => metadata.crc32c ^= 1,
            _ => {}
        }
        assert_eq!(
            complete(&mut cache, metadata, clock.now()),
            CalibrationCacheDecision::Rejected(reason)
        );
        cache.release();
        assert!(cache.take_registration().unwrap().1.is_none());
    }
}
#[test]
fn structural_decode_preserves_foreign_identity_and_native_guard_failures() {
    let clock = VirtualClock::<8>::new();
    for changed in 0..3 {
        let mut record = record();
        match changed {
            0 => record.identity.mac_extension += 1,
            1 => record.wifi.tx_iq_calibrated = false,
            _ => {}
        }
        let bytes = record.encode().unwrap();
        let mut storage = core::mem::MaybeUninit::uninit();
        let mut cache = Cache::place(&mut storage);
        begin(&mut cache, &clock);
        upload(&mut cache, &bytes, clock.now());
        assert_eq!(
            complete(&mut cache, offer(&bytes), clock.now()),
            CalibrationCacheDecision::CandidateSupplied
        );
        cache.release();
        let candidate = cache.take_registration().unwrap().1.unwrap();
        use oer_esp32s31_phy::state::PhyCalibrationCacheError as Error;
        let expected = match changed {
            0 => Error::IdentityMismatch,
            1 => Error::IncompleteCalibration,
            _ => Error::InvalidRxGainTable,
        };
        assert_eq!(candidate.validate_for_replay(identity()), Err(expected));
        assert_eq!(adapter::record(candidate.into_snapshot()), record);
    }
}
#[test]
fn full_export_is_immutable_and_partial_or_failed_registration_cannot_export() {
    let clock = VirtualClock::<8>::new();
    for full in [true, false] {
        let mut storage = core::mem::MaybeUninit::uninit();
        let mut cache = Cache::place(&mut storage);
        cache
            .begin(
                StartCalibration {
                    capture: config(),
                    source: CalibrationSource::Full,
                },
                identity(),
                clock.now(),
            )
            .unwrap();
        cache.release();
        cache.take_registration().unwrap();
        cache.freeze(full, adapter::native(record()), 123).unwrap();
        cache.ready(ready(full)).unwrap();
        let request = ReadCalibrationSnapshot {
            ticket: TICKET,
            first: 0,
            count: 32,
        };
        assert_eq!(cache.read(request).is_ok(), full);
        if full {
            let before = cache.read(request).unwrap();
            clock.advance(Duration::from_secs(100)).unwrap();
            assert_eq!(cache.read(request).unwrap(), before);
            let status = cache.status(TICKET, identity(), clock.now()).unwrap();
            assert!(matches!(
                status.state,
                CalibrationStartState::Ready {
                    registration_us: 123,
                    ..
                }
            ));
        }
        cache.fail();
        assert!(cache.read(request).is_err());
        assert!(matches!(
            cache.status(TICKET, identity(), clock.now()).unwrap().state,
            CalibrationStartState::Failed { .. }
        ));
        assert!(
            cache
                .begin(
                    StartCalibration {
                        capture: config(),
                        source: CalibrationSource::Full
                    },
                    identity(),
                    clock.now()
                )
                .is_err()
        );
    }
}
#[test]
fn new_boot_has_no_ticket_upload_or_export_from_the_previous_owner() {
    let clock = VirtualClock::<8>::new();
    let mut storage = core::mem::MaybeUninit::uninit();
    let mut cache = Cache::place(&mut storage);
    assert!(
        cache
            .write(
                WriteCalibrationCachePage {
                    ticket: TICKET,
                    first: 0,
                    bytes: [1].as_slice().try_into().unwrap()
                },
                clock.now()
            )
            .is_err()
    );
    assert!(cache.status(TICKET, identity(), clock.now()).is_err());
    assert!(
        cache
            .read(ReadCalibrationSnapshot {
                ticket: TICKET,
                first: 0,
                count: 1
            })
            .is_err()
    );
}
#[test]
fn native_mapping_preserves_every_semantic_golden_field() {
    let golden =
        include_bytes!("../../../../../chips/esp32s31/knowledge/src/calibration_cache/golden.bin");
    let record = codec::CalibrationRecord::decode(codec::S31_SEMANTIC_V1, golden).unwrap();
    assert_eq!(
        adapter::record(adapter::native(record))
            .encode()
            .unwrap()
            .as_slice(),
        golden
    );
}

#[test]
fn a_sealed_decision_reaches_registration_even_if_reply_publication_stalls() {
    let clock = VirtualClock::<8>::new();
    let mut storage = core::mem::MaybeUninit::uninit();
    let mut cache = Cache::place(&mut storage);
    begin(&mut cache, &clock);
    cache
        .complete(
            CompleteCalibrationCache {
                ticket: TICKET,
                offer: CalibrationCacheOffer::NoCache(CalibrationCacheMiss::Missing),
            },
            identity(),
            clock.now(),
        )
        .unwrap();
    assert!(!cache.pending());
    clock.advance(Duration::from_millis(500)).unwrap();
    cache.expire(clock.now());
    assert!(cache.pending());
    assert!(cache.take_registration().unwrap().1.is_none());
    assert!(matches!(
        cache.status(TICKET, identity(), clock.now()).unwrap().state,
        CalibrationStartState::Registering {
            decision: CalibrationCacheDecision::NoCache(CalibrationCacheMiss::Missing)
        }
    ));
}

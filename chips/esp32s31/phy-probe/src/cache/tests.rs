use super::*;

fn bytes<const N: usize>(source: &[u8]) -> heapless::Vec<u8, N> {
    heapless::Vec::from_slice(source).expect("fixture fits")
}

#[test]
fn pages_reject_foreign_zero_duplicate_overlapping_gapped_and_overflowing_offsets() {
    let page = WriteCalibrationCachePage {
        ticket: 7,
        first: 32,
        bytes: bytes(&[1; 32]),
    };
    assert_eq!(page.range(7, 32), Some(32..64));
    for (ticket, next) in [(0, 32), (6, 32), (7, 0), (7, 31), (7, 33), (7, 64)] {
        assert_eq!(page.range(ticket, next), None);
    }
    for first in [241, 272, u16::MAX] {
        assert_eq!(
            WriteCalibrationCachePage {
                first,
                ..page.clone()
            }
            .range(7, first),
            None
        );
    }
    assert_eq!(
        WriteCalibrationCachePage {
            bytes: bytes(&[]),
            ..page
        }
        .range(7, 32),
        None
    );
    let last = WriteCalibrationCachePage {
        ticket: 7,
        first: 256,
        bytes: bytes(&[9; 5]),
    };
    assert_eq!(last.range(7, 256), Some(256..261));
    let read = ReadCalibrationSnapshot {
        ticket: 7,
        first: 256,
        count: 5,
    };
    assert_eq!(read.range(7, 261), Some(256..261));
    for (ticket, total) in [(0, 261), (8, 261), (7, 260), (7, 273)] {
        assert_eq!(read.range(ticket, total), None);
    }
    for count in [0, 6, 33, u8::MAX] {
        assert_eq!(
            ReadCalibrationSnapshot { count, ..read }.range(7, 261),
            None
        );
    }
}

#[test]
fn length_and_crc_cover_exactly_the_opaque_bytes() {
    let mut metadata = CalibrationCacheMetadata {
        format: CalibrationCacheFormat(0x98765432),
        schema: 3,
        algorithm_revision: 8,
        profile: 9,
        length: 1,
        crc32c: 0,
    };
    for length in 0..=u16::MAX {
        metadata.length = length;
        assert_eq!(metadata.valid_length(), (1..=272).contains(&length));
    }
    assert_eq!(calibration_cache_crc32c(b"123456789"), 0xe3069283);
}

fn round_trip<M: oer_hil_protocol::Message + Clone + core::fmt::Debug + PartialEq>(body: M) {
    let envelope = oer_hil_protocol::Envelope::new(1, 2, 0, 3, body.clone());
    let mut encoder = oer_hil_protocol::FrameEncoder::new();
    let encoded = encoder
        .encode(&envelope)
        .expect("bounded message fits existing frame");
    let mut decoder = oer_hil_protocol::FrameDecoder::new();
    let mut actual = None;
    decoder.feed(M::WIRE_KIND, encoded, |frame| {
        actual = Some(
            frame
                .expect("valid frame")
                .decode::<M>()
                .expect("payload")
                .expect("matching key")
                .body,
        );
    });
    assert_eq!(actual, Some(body));
}

#[test]
fn every_cache_message_fits_existing_framing_with_maximum_pages_and_identifiers() {
    let target = CalibrationCacheTarget {
        identity: bytes(&[255; 32]),
        format: CalibrationCacheFormat(u32::MAX),
        schema: u16::MAX,
        algorithm_revision: u32::MAX,
        profile: u32::MAX,
    };
    let request = CalibrationCacheRequest {
        target: target.clone(),
        remaining_ms: 500,
    };
    let metadata = CalibrationCacheMetadata {
        format: target.format,
        schema: target.schema,
        algorithm_revision: target.algorithm_revision,
        profile: target.profile,
        length: 272,
        crc32c: u32::MAX,
    };
    round_trip(GetCalibrationCacheIdentity);
    round_trip(CalibrationCacheIdentity {
        target,
        active_ticket: Some(u32::MAX),
    });
    round_trip(StartCalibration {
        capture: super::super::PrepareCapture {
            channel: 14,
            radio: super::super::CaptureRadio::Wifi,
        },
        source: CalibrationSource::HostCache,
    });
    round_trip(CalibrationStarted {
        ticket: u32::MAX,
        state: CalibrationStartState::AwaitingCache(request.clone()),
    });
    round_trip(CalibrationCacheRequested {
        ticket: u32::MAX,
        request,
    });
    round_trip(WriteCalibrationCachePage {
        ticket: u32::MAX,
        first: u16::MAX,
        bytes: bytes(&[255; 32]),
    });
    round_trip(CalibrationCachePageWritten {
        ticket: u32::MAX,
        next: u16::MAX,
    });
    round_trip(CompleteCalibrationCache {
        ticket: u32::MAX,
        offer: CalibrationCacheOffer::Snapshot(metadata),
    });
    round_trip(CompleteCalibrationCache {
        ticket: 1,
        offer: CalibrationCacheOffer::NoCache(CalibrationCacheMiss::Missing),
    });
    round_trip(CalibrationCacheCompleted {
        ticket: u32::MAX,
        decision: CalibrationCacheDecision::CandidateSupplied,
    });
    round_trip(GetCalibrationStartStatus { ticket: u32::MAX });
    for (decision, path, full) in [
        (
            CalibrationCacheDecision::FullRequested,
            super::super::CalibrationPath::FullForCache,
            true,
        ),
        (
            CalibrationCacheDecision::TimedOut,
            super::super::CalibrationPath::FullForCache,
            true,
        ),
        (
            CalibrationCacheDecision::NoCache(CalibrationCacheMiss::Missing),
            super::super::CalibrationPath::FullForCache,
            true,
        ),
        (
            CalibrationCacheDecision::Rejected(CalibrationCacheRejection::Decode),
            super::super::CalibrationPath::FullForCache,
            true,
        ),
        (
            CalibrationCacheDecision::CandidateSupplied,
            super::super::CalibrationPath::FullAfterRejectedCache,
            true,
        ),
        (
            CalibrationCacheDecision::CandidateSupplied,
            super::super::CalibrationPath::PartialFromCache,
            false,
        ),
    ] {
        round_trip(CalibrationStartStatus {
            ticket: u32::MAX,
            state: CalibrationStartState::Registering { decision },
        });
        round_trip(CalibrationStartStatus {
            ticket: u32::MAX,
            state: CalibrationStartState::Failed { decision },
        });
        round_trip(CalibrationStartStatus {
            ticket: u32::MAX,
            state: CalibrationStartState::Ready {
                decision,
                capture: super::super::CaptureReady {
                    epoch: u32::MAX,
                    lifecycle: super::super::CaptureLifecycle::Cold,
                    full_calibration_performed: full,
                    path,
                    projection_schema: u16::MAX,
                    calibration_words: u16::MAX,
                    registers: u16::MAX,
                    analog_bytes: u16::MAX,
                    channel: 14,
                    radio: super::super::CaptureRadio::Wifi,
                },
                registration_us: u32::MAX,
                snapshot: full.then_some(metadata),
            },
        });
    }
    round_trip(ReadCalibrationSnapshot {
        ticket: u32::MAX,
        first: u16::MAX,
        count: 32,
    });
    round_trip(CalibrationSnapshot {
        temperature: CalibrationTemperature {
            value: i16::MIN,
            scale: CalibrationTemperatureScale::RawSensor { profile: u32::MAX },
        },
        ticket: u32::MAX,
        first: u16::MAX,
        metadata,
        bytes: bytes(&[255; 32]),
    });
}

#[test]
fn postcard_rejects_oversized_pages() {
    // Serialize the Vec length as postcard does, then the oversized payload.
    let mut buffer = [0; 64];
    let encoded = postcard::to_slice(&(7u32, 0u16, &[0u8; 33][..]), &mut buffer).expect("fixture");
    assert!(postcard::from_bytes::<WriteCalibrationCachePage>(encoded).is_err());
}

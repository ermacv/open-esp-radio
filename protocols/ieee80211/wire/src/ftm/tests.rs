use super::*;
use crate::element::ElementId;

fn request_parameters() -> FtmRequestParameters {
    FtmRequestParameters::new(
        0,
        FtmBurstDuration::Millis8,
        2,
        None,
        FtmAsap::AsSoonAsPossible,
        8,
        FtmFormatAndBandwidth::HtMixed20Mhz,
        0,
    )
    .unwrap()
}

fn response_parameters() -> FtmResponseParameters {
    FtmResponseParameters {
        status: FtmResponseStatus::Success,
        number_of_bursts_exponent: 0,
        burst_duration: FtmBurstDuration::Millis8,
        min_delta_ftm_100us: 3,
        partial_tsf_timer: 0x1234,
        asap_capable: FtmAsapCapability::Capable,
        asap: FtmAsap::AsSoonAsPossible,
        ftms_per_burst: 8,
        format_and_bandwidth: FtmFormatAndBandwidth::HtMixed20Mhz,
        burst_period_100ms: 0,
    }
}

#[test]
fn initial_request_round_trips_exact_parameter_element() {
    let body = encode_initial_request(request_parameters());
    assert_eq!(&body[..3], &[4, 32, 1]);
    assert_eq!(body[3], 206);
    assert_eq!(body[4], 9);

    let decoded = FtmRequest::decode_body(&body).unwrap();
    assert_eq!(decoded.trigger, FtmTrigger::StartOrContinue);
    assert_eq!(decoded.parameters, Some(request_parameters()));
}

#[test]
fn reserved_request_fields_fail_closed() {
    let mut element = request_parameters().encode_element();
    element[2] = 1;
    assert_eq!(
        FtmRequestParameters::decode_element(&element),
        Err(FtmParameterError::ReservedStatus)
    );
    element = request_parameters().encode_element();
    element[8] |= 1;
    assert_eq!(
        FtmRequestParameters::decode_element(&element),
        Err(FtmParameterError::ReservedBitsSet)
    );
    element = request_parameters().encode_element();
    element[5] = 1;
    assert_eq!(
        FtmRequestParameters::decode_element(&element),
        Err(FtmParameterError::ReservedBitsSet)
    );
}

#[test]
fn response_exponent_fifteen_is_a_finite_allocation_not_no_preference() {
    let mut response = response_parameters();
    response.number_of_bursts_exponent = 15;
    response.burst_period_100ms = 1;
    let element = response.encode_element().unwrap();
    assert_eq!(
        FtmResponseParameters::decode_element(&element),
        Ok(response)
    );

    let mut reserved = response_parameters().encode_element().unwrap();
    reserved[7] |= 1;
    assert_eq!(
        FtmResponseParameters::decode_element(&reserved),
        Err(FtmParameterError::ReservedBitsSet)
    );
}

#[test]
fn measurement_uses_six_octet_picosecond_timestamps() {
    let fields = FtmMeasurementFields {
        dialog_token: DialogToken::new(2),
        follow_up_dialog_token: DialogToken::new(1),
        tod: FtmTimestampPs::new(0x0102_0304_0506).unwrap(),
        toa: FtmTimestampPs::new(0x0a0b_0c0d_0e0f).unwrap(),
        tod_error: FtmTodError {
            max_error_exponent: 4,
            not_continuous: false,
        },
        toa_error: FtmToaError {
            max_error_exponent: 5,
        },
    };
    let mut body = [0_u8; FTM_MEASUREMENT_PREFIX_LEN];
    assert_eq!(
        encode_measurement(fields, &[], &mut body),
        Ok(FTM_MEASUREMENT_PREFIX_LEN)
    );
    assert_eq!(&body[4..10], &[6, 5, 4, 3, 2, 1]);
    assert_eq!(FtmMeasurement::decode_body(&body).unwrap().fields, fields);
}

#[test]
fn initial_measurement_reserves_timestamp_fields_and_decodes_parameters() {
    let fields = FtmMeasurementFields {
        dialog_token: DialogToken::new(1),
        follow_up_dialog_token: DialogToken::new(0),
        tod: FtmTimestampPs::ZERO,
        toa: FtmTimestampPs::ZERO,
        tod_error: FtmTodError {
            max_error_exponent: 0,
            not_continuous: false,
        },
        toa_error: FtmToaError {
            max_error_exponent: 0,
        },
    };
    let parameters = response_parameters().encode_element().unwrap();
    let mut body = [0_u8; FTM_MEASUREMENT_PREFIX_LEN + FTM_PARAMETERS_ELEMENT_LEN];
    encode_measurement(fields, &parameters, &mut body).unwrap();
    let decoded = FtmMeasurement::decode_body(&body).unwrap();
    assert_eq!(decoded.parameters, Some(response_parameters()));

    body[4] = 1;
    assert_eq!(
        FtmMeasurement::decode_body(&body),
        Err(FtmWireError::ReservedTimestampFields)
    );
}

#[test]
fn duplicate_or_truncated_parameter_elements_are_rejected() {
    let parameters = response_parameters().encode_element().unwrap();
    let mut body = [0_u8; FTM_MEASUREMENT_PREFIX_LEN + 2 * FTM_PARAMETERS_ELEMENT_LEN];
    let fields = FtmMeasurementFields {
        dialog_token: DialogToken::new(1),
        follow_up_dialog_token: DialogToken::new(0),
        tod: FtmTimestampPs::ZERO,
        toa: FtmTimestampPs::ZERO,
        tod_error: FtmTodError {
            max_error_exponent: 0,
            not_continuous: false,
        },
        toa_error: FtmToaError {
            max_error_exponent: 0,
        },
    };
    let mut information_elements = [0_u8; 2 * FTM_PARAMETERS_ELEMENT_LEN];
    information_elements[..FTM_PARAMETERS_ELEMENT_LEN].copy_from_slice(&parameters);
    information_elements[FTM_PARAMETERS_ELEMENT_LEN..].copy_from_slice(&parameters);
    assert_eq!(
        encode_measurement(fields, &information_elements, &mut body),
        Err(FtmWireError::DuplicateParametersElement)
    );
    body[..FTM_MEASUREMENT_PREFIX_LEN].fill(0);
    body[0] = ActionCategory::PUBLIC.value();
    body[1] = PublicAction::FTM.value();
    body[2] = fields.dialog_token.value();
    body[FTM_MEASUREMENT_PREFIX_LEN..].copy_from_slice(&information_elements);
    assert_eq!(
        FtmMeasurement::decode_body(&body),
        Err(FtmWireError::DuplicateParametersElement)
    );
    assert_eq!(
        FtmRequest::decode_body(&[4, 32, 1, 206, 9, 0]),
        Err(FtmWireError::MalformedInformationElement)
    );
}

#[test]
fn trigger_requests_are_complete_without_parameters() {
    for trigger in [FtmTrigger::Stop, FtmTrigger::StartOrContinue] {
        let body = encode_request_trigger(trigger);
        let decoded = FtmRequest::decode_body(&body).unwrap();
        assert_eq!(decoded.trigger, trigger);
        assert!(decoded.information_elements.is_empty());
        assert_eq!(decoded.parameters, None);
    }
}

#[test]
fn every_prefix_of_a_body_is_handled() {
    let mut request = [0_u8; FTM_INITIAL_REQUEST_BODY_LEN];
    request.copy_from_slice(&encode_initial_request(request_parameters()));
    let mut measurement = [0_u8; FTM_MEASUREMENT_PREFIX_LEN + FTM_PARAMETERS_ELEMENT_LEN];
    measurement[0] = ActionCategory::PUBLIC.value();
    measurement[1] = PublicAction::FTM.value();
    measurement[FTM_MEASUREMENT_PREFIX_LEN] = ElementId::FTM_PARAMETERS.value();
    measurement[FTM_MEASUREMENT_PREFIX_LEN + 1] = FTM_PARAMETERS_FIELD_LEN as u8;
    for len in 0..=measurement.len() {
        let _ = FtmRequest::decode_body(request.get(..len).unwrap_or(&request));
        let _ = FtmMeasurement::decode_body(&measurement[..len]);
        let mut output = [0_u8; FTM_MEASUREMENT_PREFIX_LEN];
        let _ = encode_measurement(
            FtmMeasurementFields {
                dialog_token: DialogToken::new(1),
                follow_up_dialog_token: DialogToken::new(0),
                tod: FtmTimestampPs::ZERO,
                toa: FtmTimestampPs::ZERO,
                tod_error: FtmTodError {
                    max_error_exponent: 0,
                    not_continuous: false,
                },
                toa_error: FtmToaError {
                    max_error_exponent: 0,
                },
            },
            &measurement[FTM_MEASUREMENT_PREFIX_LEN..len.max(FTM_MEASUREMENT_PREFIX_LEN)],
            &mut output[..len.min(FTM_MEASUREMENT_PREFIX_LEN)],
        );
    }
}

#[test]
fn a_response_element_has_the_bytes_of_its_fields() {
    assert_eq!(
        response_parameters().encode_element(),
        Ok([
            0xce, 0x09, 0x01, 0x70, 0x03, 0x34, 0x12, 0x46, 0x24, 0x00, 0x00
        ])
    );
}

#[test]
fn clear_asap_bits_are_an_incapable_responder_at_the_partial_tsf() {
    let mut response = response_parameters();
    response.asap_capable = FtmAsapCapability::Incapable;
    response.asap = FtmAsap::AtPartialTsf;
    let element = response.encode_element().unwrap();
    assert_eq!(
        FtmResponseParameters::decode_element(&element),
        Ok(response)
    );
}

/// A request's Status Indication and Value each refuse it alone; their
/// reserved bit is refused for every element (IEEE Std 802.11-2020,
/// 9.4.2.167: zero in a request).
#[test]
fn each_status_bit_refuses_a_request() {
    for status in [0x01, 0x04] {
        let mut element = request_parameters().encode_element();
        element[2] = status;
        assert_eq!(
            FtmRequestParameters::decode_element(&element),
            Err(FtmParameterError::ReservedStatus),
            "{status:#04x}"
        );
    }
}

/// The TOD and TOA Error fields' reserved bits and the TOA's continuity
/// bit each refuse a measurement; in an initial one (follow-up token 0) a
/// TOA, a TOD Error or a TOA Error each refuses it alone.
#[test]
fn each_reserved_timestamp_bit_refuses_a_measurement() {
    let fields = FtmMeasurementFields {
        dialog_token: DialogToken::new(2),
        follow_up_dialog_token: DialogToken::new(1),
        tod: FtmTimestampPs::new(0x0102_0304_0506).unwrap(),
        toa: FtmTimestampPs::new(0x0a0b_0c0d_0e0f).unwrap(),
        tod_error: FtmTodError {
            max_error_exponent: 4,
            not_continuous: false,
        },
        toa_error: FtmToaError {
            max_error_exponent: 5,
        },
    };
    let mut body = [0_u8; FTM_MEASUREMENT_PREFIX_LEN];
    encode_measurement(fields, &[], &mut body).unwrap();
    assert!(FtmMeasurement::decode_body(&body).is_ok());
    // TOD Error at 16..18, TOA Error at 18..20: bit 5 reserved, bit 15
    // the TOA's not-continuous bit.
    for (octet, bit) in [(16, 0x20), (18, 0x20), (19, 0x80)] {
        let mut refused = body;
        refused[octet] ^= bit;
        assert_eq!(
            FtmMeasurement::decode_body(&refused),
            Err(FtmWireError::ReservedTimestampErrorBits),
            "octet {octet} bit {bit:#04x}"
        );
    }

    let initial = FtmMeasurementFields {
        follow_up_dialog_token: DialogToken::new(0),
        tod: FtmTimestampPs::ZERO,
        toa: FtmTimestampPs::ZERO,
        tod_error: FtmTodError {
            max_error_exponent: 0,
            not_continuous: false,
        },
        toa_error: FtmToaError {
            max_error_exponent: 0,
        },
        ..fields
    };
    let parameters = response_parameters().encode_element().unwrap();
    let mut body = [0_u8; FTM_MEASUREMENT_PREFIX_LEN + FTM_PARAMETERS_ELEMENT_LEN];
    encode_measurement(initial, &parameters, &mut body).unwrap();
    assert!(FtmMeasurement::decode_body(&body).is_ok());
    // TOA, TOD Error, TOA Error.
    for octet in [10, 16, 18] {
        let mut refused = body;
        refused[octet] = 1;
        assert_eq!(
            FtmMeasurement::decode_body(&refused),
            Err(FtmWireError::ReservedTimestampFields),
            "octet {octet}"
        );
    }
}

/// Every Burst Duration and Format And Bandwidth name has its standard
/// code, and the code reads back as the name; the timestamp holds 48 bits.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.167 (FTM Parameters element: the
/// Burst Duration and Format And Bandwidth field encodings).
#[test]
fn the_codes_and_the_timestamp_fill_their_fields() {
    use FtmBurstDuration as D;
    use FtmFormatAndBandwidth as F;
    let durations = [
        (D::Micros250, 2),
        (D::Micros500, 3),
        (D::Millis1, 4),
        (D::Millis2, 5),
        (D::Millis4, 6),
        (D::Millis8, 7),
        (D::Millis16, 8),
        (D::Millis32, 9),
        (D::Millis64, 10),
        (D::Millis128, 11),
        (D::NoPreference, 15),
    ];
    let formats = [
        (F::NoPreference, 0),
        (F::NonHt5Mhz, 4),
        (F::NonHt10Mhz, 6),
        (F::NonHt20Mhz, 8),
        (F::HtMixed20Mhz, 9),
        (F::Vht20Mhz, 10),
        (F::HtMixed40Mhz, 11),
        (F::Vht40Mhz, 12),
        (F::Vht80Mhz, 13),
        (F::Vht80Plus80Mhz, 14),
        (F::Vht160MhzSeparateLos, 15),
        (F::Vht160MhzSingleLo, 16),
        (F::Dmg2160Mhz, 31),
    ];
    // A name added to either enum and missing from its table stops this
    // test from building.
    const fn listed_duration(duration: D) {
        match duration {
            D::Micros250
            | D::Micros500
            | D::Millis1
            | D::Millis2
            | D::Millis4
            | D::Millis8
            | D::Millis16
            | D::Millis32
            | D::Millis64
            | D::Millis128
            | D::NoPreference => {}
        }
    }
    const fn listed_format(format: F) {
        match format {
            F::NoPreference
            | F::NonHt5Mhz
            | F::NonHt10Mhz
            | F::NonHt20Mhz
            | F::HtMixed20Mhz
            | F::Vht20Mhz
            | F::HtMixed40Mhz
            | F::Vht40Mhz
            | F::Vht80Mhz
            | F::Vht80Plus80Mhz
            | F::Vht160MhzSeparateLos
            | F::Vht160MhzSingleLo
            | F::Dmg2160Mhz => {}
        }
    }
    for (duration, code) in durations {
        listed_duration(duration);
        assert_eq!(duration.wire().get(), code, "{duration:?}");
        assert_eq!(FtmBurstDuration::from_wire(code), Ok(duration));
    }
    for (format, code) in formats {
        listed_format(format);
        assert_eq!(format.wire().get(), code, "{format:?}");
        assert_eq!(FtmFormatAndBandwidth::from_wire(code), Ok(format));
    }
    let highest = FtmTimestampPs::new(FTM_TIMESTAMP_MASK).unwrap();
    assert_eq!(FtmTimestampPs::decode(highest.encode()), highest);
    assert_eq!(highest.get(), FTM_TIMESTAMP_MASK);
    assert_eq!(highest.encode(), [0xff; 6]);
    assert_eq!(
        FtmTimestampPs::new(FTM_TIMESTAMP_MASK + 1),
        Err(FtmWireError::TimestampOutOfRange)
    );
}

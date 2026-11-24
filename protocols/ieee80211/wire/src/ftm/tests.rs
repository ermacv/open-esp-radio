use super::*;

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
        dialog_token: 2,
        follow_up_dialog_token: 1,
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
        dialog_token: 1,
        follow_up_dialog_token: 0,
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
        dialog_token: 1,
        follow_up_dialog_token: 0,
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
    body[0] = PUBLIC_ACTION_CATEGORY;
    body[1] = FTM_MEASUREMENT_PUBLIC_ACTION;
    body[2] = fields.dialog_token;
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
    measurement[0] = PUBLIC_ACTION_CATEGORY;
    measurement[1] = FTM_MEASUREMENT_PUBLIC_ACTION;
    measurement[FTM_MEASUREMENT_PREFIX_LEN] = FTM_PARAMETERS_ELEMENT_ID;
    measurement[FTM_MEASUREMENT_PREFIX_LEN + 1] = FTM_PARAMETERS_FIELD_LEN as u8;
    for len in 0..=measurement.len() {
        let _ = FtmRequest::decode_body(request.get(..len).unwrap_or(&request));
        let _ = FtmMeasurement::decode_body(&measurement[..len]);
        let mut output = [0_u8; FTM_MEASUREMENT_PREFIX_LEN];
        let _ = encode_measurement(
            FtmMeasurementFields {
                dialog_token: 1,
                follow_up_dialog_token: 0,
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
        dialog_token: 2,
        follow_up_dialog_token: 1,
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
        follow_up_dialog_token: 0,
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

/// Every Burst Duration and Format And Bandwidth code reads back as
/// itself through its field; the timestamp holds 48 bits.
#[test]
fn the_codes_and_the_timestamp_fill_their_fields() {
    for duration in [
        FtmBurstDuration::Micros250,
        FtmBurstDuration::Millis128,
        FtmBurstDuration::NoPreference,
    ] {
        assert_eq!(
            FtmBurstDuration::from_wire(duration.wire().get()),
            Ok(duration)
        );
    }
    for format in [
        FtmFormatAndBandwidth::NoPreference,
        FtmFormatAndBandwidth::Vht160MhzSingleLo,
        FtmFormatAndBandwidth::Dmg2160Mhz,
    ] {
        assert_eq!(
            FtmFormatAndBandwidth::from_wire(format.wire().get()),
            Ok(format)
        );
    }
    let highest = FtmTimestampPs::new(FTM_TIMESTAMP_MASK).unwrap();
    assert_eq!(FtmTimestampPs::decode(highest.encode()), highest);
    assert_eq!(highest.encode(), [0xff; 6]);
    assert_eq!(
        FtmTimestampPs::new(FTM_TIMESTAMP_MASK + 1),
        Err(FtmWireError::TimestampOutOfRange)
    );
}

use super::*;

fn request_parameters() -> FtmRequestParameters {
    FtmRequestParameters::new(
        0,
        FtmBurstDuration::Millis8,
        2,
        None,
        true,
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
        asap_capable: true,
        asap: true,
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

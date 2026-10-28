use super::*;

fn length(octets: u16, time: u16) -> LeDataLength {
    LeDataLength::new(octets, time).unwrap()
}

#[test]
fn lengths_outside_the_specified_ranges_are_refused() {
    assert!(LeDataLength::new(26, 328).is_none());
    assert!(LeDataLength::new(252, 2120).is_none());
    assert!(LeDataLength::new(27, 327).is_none());
    assert!(LeDataLength::new(251, 17_041).is_none());
    assert_eq!(
        length(251, 17_040).supported(),
        LeDataLength::SUPPORTED_MAXIMUM
    );
}

#[test]
fn air_time_limits_the_1m_payload_and_counts_the_mic() {
    assert_eq!(LeDataLength::MINIMUM.payload_octets(0).unwrap(), 27);
    assert_eq!(LeDataLength::MINIMUM.payload_octets(4).unwrap(), 27);
    assert_eq!(
        LeDataLength::SUPPORTED_MAXIMUM.payload_octets(4).unwrap(),
        251
    );
    // 1000 us fit 125 octets on air: 115 without a MIC, 111 with one.
    assert_eq!(length(251, 1_000).payload_octets(0).unwrap(), 115);
    assert_eq!(length(251, 1_000).payload_octets(4).unwrap(), 111);
}

#[test]
fn overflowing_mic_overhead_is_an_error_and_large_valid_overhead_fits_no_payload() {
    use super::LeDataLengthError;
    assert_eq!(
        LeDataLength::MINIMUM.payload_octets(u16::MAX),
        Err(LeDataLengthError::MicLengthOverflow {
            mic_octets: u16::MAX
        })
    );
    assert_eq!(
        LeDataLength::MINIMUM.payload_octets(u16::MAX - 9),
        Err(LeDataLengthError::MicLengthOverflow {
            mic_octets: u16::MAX - 9
        })
    );
    assert_eq!(LeDataLength::MINIMUM.payload_octets(u16::MAX - 10), Ok(0));
}

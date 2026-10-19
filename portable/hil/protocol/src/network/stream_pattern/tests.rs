use super::*;

#[test]
fn chunks_are_independent_of_read_boundaries() {
    let mut whole = [0; 257];
    fill_stream_pattern(&mut whole, 0);
    assert!(stream_pattern_matches(&whole[..113], 0));
    assert!(stream_pattern_matches(&whole[113..], 113));
    whole[200] ^= 1;
    assert!(!stream_pattern_matches(&whole[113..], 113));
}

#[test]
fn bulk_pattern_crosses_multiple_periods_at_an_arbitrary_phase() {
    let mut buffer = [0; 1_025];
    fill_stream_pattern(&mut buffer, 173);
    assert!(stream_pattern_matches(&buffer, 173));
    assert_eq!(buffer[0], stream_pattern_byte(173));
    assert_eq!(buffer[1_024], stream_pattern_byte(1_197));
}

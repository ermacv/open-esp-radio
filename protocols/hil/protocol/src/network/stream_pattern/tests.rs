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

#[test]
fn stream_offsets_wrap_through_the_end_of_the_u64_domain() {
    for offset in [0, 255, 256, u64::MAX - 2, u64::MAX] {
        let mut buffer = [0; 1_025];
        fill_stream_pattern(&mut buffer, offset);
        for (index, byte) in buffer.iter().enumerate() {
            assert_eq!(
                *byte,
                stream_pattern_byte(offset.wrapping_add(index as u64))
            );
        }
        assert!(stream_pattern_matches(&buffer, offset));
        buffer[256] ^= 1;
        assert!(!stream_pattern_matches(&buffer, offset));
        assert!(stream_pattern_matches(&[], offset));
    }
}

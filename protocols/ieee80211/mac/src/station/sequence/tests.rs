use super::*;
use crate::sequence::seq;

#[test]
fn sta_sequence_counter_is_monotonic_across_twelve_bit_wrap() {
    let mut sequence = StaSequenceCounter::new(seq(0x0ffe));
    assert_eq!(sequence.take(), seq(0x0ffe));
    assert_eq!(sequence.take(), seq(0x0fff));
    assert_eq!(sequence.take(), seq(0x0000));
    assert_eq!(sequence.peek(), seq(0x0001));
}

#[test]
fn sta_tx_sequence_spaces_do_not_advance_each_other() {
    let mut sequences = StaTxSequenceCounters::new(seq(25));

    assert_eq!(sequences.take_non_qos(), seq(25));
    assert_eq!(sequences.take_non_qos(), seq(26));
    assert_eq!(sequences.peek_qos(0), Some(seq(25)));
    assert_eq!(sequences.peek_qos(5), Some(seq(25)));
    assert_eq!(sequences.peek_qos(7), Some(seq(25)));

    assert_eq!(sequences.take_qos(0), Some(seq(25)));
    assert_eq!(sequences.peek_qos(0), Some(seq(26)));
    assert_eq!(sequences.peek_qos(5), Some(seq(25)));
    assert_eq!(sequences.peek_qos(7), Some(seq(25)));
    assert_eq!(sequences.peek_non_qos(), seq(27));
}

#[test]
fn sta_tx_sequence_space_rejects_invalid_tid_and_wraps_independently() {
    let mut sequences = StaTxSequenceCounters::new(seq(0x0fff));

    assert_eq!(sequences.take_data(Some(15)), Some(seq(0x0fff)));
    assert_eq!(sequences.peek_qos(15), Some(seq(0)));
    assert_eq!(sequences.take_data(None), Some(seq(0x0fff)));
    assert_eq!(sequences.peek_non_qos(), seq(0));
    assert_eq!(sequences.take_data(Some(16)), None);
}

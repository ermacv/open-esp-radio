use super::*;

fn report(start: u16, bitmap: u64) -> BlockAckReport {
    BlockAckReport::new(SequenceNumber::new(start).unwrap(), bitmap)
}

fn sequence(value: u16) -> SequenceNumber {
    SequenceNumber::new(value).unwrap()
}

#[test]
fn the_bitmap_acknowledges_across_the_sequence_wrap() {
    let ack = report(0x0ffe, 0b1101);
    assert!(acknowledges(ack, sequence(0x0ffe)));
    assert!(!acknowledges(ack, sequence(0x0fff)));
    assert!(acknowledges(ack, sequence(0)));
    assert!(acknowledges(ack, sequence(1)));
    assert!(!acknowledges(ack, sequence(2)));
}

#[test]
fn only_a_bounded_predecessor_of_an_advanced_start_is_delivered() {
    let ack = report(105, 0b1);
    assert!(acknowledges(ack, sequence(105)));
    assert!(acknowledges(ack, sequence(104)));
    assert!(acknowledges(ack, sequence(41)));
    assert!(!acknowledges(ack, sequence(40)));
    assert!(!acknowledges(ack, sequence(106)));
}

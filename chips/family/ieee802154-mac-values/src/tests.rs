// SPDX-License-Identifier: Apache-2.0
#[test]
fn semantic_event_union_never_loses_ed_done_or_rx_abort_presence() {
    use crate::Ieee802154ObservedEventState as State;

    assert_eq!(
        State::EdDoneAndRxAbortWithOther.union(State::RxAbortOnly),
        State::EdDoneAndRxAbortWithOther
    );
    assert_eq!(
        State::RxAbortWithOther.union(State::EdDoneWithOther),
        State::EdDoneAndRxAbortWithOther
    );
}

#[test]
fn a_security_payload_offset_is_made_only_inside_seven_bits() {
    use super::Ieee802154SecurityPayloadOffset as Offset;

    assert_eq!(Offset::new(Offset::MAX).map(Offset::value), Some(0x7f));
    assert_eq!(Offset::new(0x80), None);
    assert_eq!(Offset::new(u8::MAX), None);
}

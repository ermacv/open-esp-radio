/// The masked event clear maps every raw field back to the event it came from.
#[test]
fn every_raw_event_maps_back_to_its_event() {
    use super::super::event_of;
    use super::{Ieee802154Event, raw_event};
    for event in [
        Ieee802154Event::TxDone,
        Ieee802154Event::RxDone,
        Ieee802154Event::AckTxDone,
        Ieee802154Event::AckRxDone,
        Ieee802154Event::RxAbort,
        Ieee802154Event::TxAbort,
        Ieee802154Event::EdDone,
        Ieee802154Event::Timer0Overflow,
        Ieee802154Event::Timer1Overflow,
        Ieee802154Event::ClockCountMatch,
        Ieee802154Event::TxSfdDone,
        Ieee802154Event::RxSfdDone,
    ] {
        assert_eq!(event_of(raw_event(event)), event);
    }
}

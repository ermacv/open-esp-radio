use super::*;
use oer_protocols_network_port::Class;

struct Fixed(u32);

impl FrameCost for Fixed {
    fn least(&self, frame_octets: usize) -> NonZeroU32 {
        NonZeroU32::new(u32::try_from(frame_octets).unwrap()).unwrap()
    }
    fn ceiling(&self, frame_octets: usize) -> NonZeroU32 {
        NonZeroU32::new(self.0 + u32::try_from(frame_octets).unwrap()).unwrap()
    }
}

fn ready(length: usize) -> Ready {
    Ready {
        class: Class::Data,
        length,
        peer: [2, 0, 0, 0, 0, 1],
        flow: Some(0),
        dscp: Default::default(),
    }
}

const fn up(epoch: u64) -> LinkSnapshot {
    LinkSnapshot {
        state: LinkState::Up,
        epoch,
    }
}

#[test]
fn an_association_is_one_account_keyed_by_its_epoch_and_a_down_link_serves_none() {
    let link = NetworkLink::new();
    let air = StationAir::new(&link, Fixed(1_000));
    assert_eq!(air.candidate(&ready(1514)), None, "down");
    link.publish(up(3));
    let first = air.candidate(&ready(1514)).unwrap();
    let small = air.candidate(&ready(64)).unwrap();
    assert_eq!((first.key, small.key), (3, 3), "every peer, one account");
    assert_eq!(
        (first.minimum_micros.get(), small.minimum_micros.get()),
        (1514, 64),
        "each frame's least grant"
    );
    link.publish(up(4));
    assert_eq!(air.candidate(&ready(1514)).unwrap().key, 4);
    assert_eq!(air.ceiling(&ready(1514)).get(), 2_514);
}

#[test]
fn a_buffer_comes_back_only_with_its_end_and_a_new_epoch_ends_the_old_account_first() {
    let mut ends = Completions::<&str, u8, 2>::new();
    assert!(!ends.waiting(up(1)));
    assert_eq!(ends.take_notice(up(1)).map(|(notice, _)| notice), None);
    ends.returned("a", 7, TxEstimate::Estimated { micros: 300 })
        .unwrap();
    ends.returned("b", 8, TxEstimate::NONE).unwrap();
    assert_eq!(ends.returned("c", 9, TxEstimate::NONE), Err("c"), "full");
    assert!(ends.waiting(up(1)));
    assert_eq!(
        ends.take_notice(up(1)),
        Some((
            RadioNotice::Ended(Exchange {
                ticket: 7,
                charged: NonZeroU32::new(300)
            }),
            Some("a")
        ))
    );
    // A new association: its predecessor's account ends before the next
    // frame's end, which still settles on that old account.
    assert!(ends.waiting(up(2)));
    assert_eq!(ends.take_notice(up(2)), Some((RadioNotice::Left(1), None)));
    assert_eq!(
        ends.take_notice(up(2)),
        Some((
            RadioNotice::Ended(Exchange {
                ticket: 8,
                charged: Some(NonZeroU32::MIN)
            }),
            Some("b")
        )),
        "a frame that never reached the air is charged the least"
    );
    assert!(!ends.waiting(up(2)));
}

#[test]
fn a_lost_frame_settles_with_no_buffer_and_an_unknown_cost_is_charged_the_ceiling() {
    let mut ends = Completions::<&str, u8, 2>::new();
    ends.lost(5).unwrap();
    ends.returned("a", 6, TxEstimate::Unknown).unwrap();
    assert_eq!(ends.take_notice(up(1)), Some((RadioNotice::Lost(5), None)));
    assert_eq!(
        ends.take_notice(up(1)),
        Some((
            RadioNotice::Ended(Exchange {
                ticket: 6,
                charged: None
            }),
            Some("a")
        ))
    );
}

use super::*;
use core::num::NonZeroU16;
use oer_protocols_ieee80211_wire::phy::LegacyRate;
use std::vec::Vec;

fn positions(mask: FrameMask) -> Vec<u8> {
    mask.iter().map(FrameIndex::get).collect()
}

#[test]
fn arena_positions_are_the_subframe_mask_width() {
    assert_eq!(usize::from(ARENA_FRAMES), AMPDU_SUBFRAME_MASK_BITS);
    assert!(u32::from(ARENA_FRAMES) <= u64::BITS);
}

#[test]
fn masks_refuse_positions_beyond_the_arena() {
    assert_eq!(FrameMask::first(0), Some(FrameMask::EMPTY));
    assert_eq!(FrameMask::first(3).map(FrameMask::bits), Some(0b111));
    let full = FrameMask::first(ARENA_FRAMES).unwrap();
    assert_eq!(full.count(), ARENA_FRAMES);
    assert_eq!(FrameMask::first(ARENA_FRAMES + 1), None);
    assert_eq!(FrameMask::from_bits(full.bits()), Some(full));
    assert_eq!(FrameMask::from_bits(full.bits() + 1), None);
    assert_eq!(FrameMask::from_bits(u64::MAX).is_some(), ARENA_FRAMES == 64);
}

#[test]
fn masks_iterate_positions_in_ascending_order() {
    let mask = FrameMask::from_bits(0b1010_0101).unwrap();
    assert_eq!(positions(mask), [0, 2, 5, 7]);
    assert_eq!(mask.count(), 4);
    let last = FrameMask::first(ARENA_FRAMES)
        .unwrap()
        .iter()
        .last()
        .unwrap();
    assert_eq!(last.get(), ARENA_FRAMES - 1);
    assert_eq!(positions(FrameMask::EMPTY), []);
}

#[test]
fn mask_operations_keep_their_positions() {
    let mut indices = FrameMask::first(4).unwrap().iter();
    let zero = indices.next().unwrap();
    let two = indices.nth(1).unwrap();
    let mut mask = FrameMask::EMPTY.with(two);
    assert!(mask.contains(two) && !mask.contains(zero));
    mask.insert(zero);
    assert_eq!(positions(mask), [0, 2]);
    assert_eq!(positions(mask.without(zero)), [2]);
    mask.remove(two);
    assert_eq!(positions(mask), [0]);
    let a = FrameMask::from_bits(0b0110).unwrap();
    let b = FrameMask::from_bits(0b0011).unwrap();
    assert_eq!(a.union(b).bits(), 0b0111);
    assert_eq!(a.intersection(b).bits(), 0b0010);
    assert_eq!(a.difference(b).bits(), 0b0100);
    assert!(a.is_within(3) && !a.is_within(2));
    assert!(FrameMask::EMPTY.is_within(0) && FrameMask::EMPTY.is_empty());
}

#[test]
fn arena_ids_name_the_pair() {
    assert_eq!(ArenaId::First.other(), ArenaId::Second);
    assert_eq!(ArenaId::Second.other(), ArenaId::First);
    assert_eq!([ArenaId::First.index(), ArenaId::Second.index()], [0, 1]);
}

#[test]
fn only_an_unknown_or_established_transmission_sets_the_retry_bit() {
    let air = Air {
        transmissions: NonZeroU16::MIN,
        rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
    };
    let report = BlockAckReport::new(
        oer_protocols_ieee80211_wire::sequence::SequenceNumber::from_low_bits(0),
        1,
    );
    let reached: [AttemptEnd<(), ()>; 6] = [
        AttemptEnd::Answered {
            air,
            report,
            response: (),
        },
        AttemptEnd::Acked { air, response: () },
        AttemptEnd::NoResponse(air),
        AttemptEnd::ProtectionFailure(Some(air)),
        AttemptEnd::Collision(Some(air)),
        AttemptEnd::Aborted,
    ];
    for end in reached {
        assert!(end.may_have_reached_air(), "{end:?}");
    }
    let unsent: [AttemptEnd<(), ()>; 3] = [
        AttemptEnd::ProtectionFailure(None),
        AttemptEnd::Collision(None),
        AttemptEnd::AirDenied {
            denial: (),
            air: None,
        },
    ];
    for end in unsent {
        assert!(!end.may_have_reached_air(), "{end:?}");
        assert_eq!(end.air(), None);
    }
    assert_eq!(reached[0].block_ack(), Some(&report));
    assert_eq!(reached[1].block_ack(), None);
    assert_eq!(reached[5].air(), None);
}

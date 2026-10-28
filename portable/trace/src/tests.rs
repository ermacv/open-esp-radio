extern crate std;

use crate::{Domain, Kind, Record};

#[test]
fn domain_channel_ranges_partition_the_mask() {
    let domains = [
        Domain::Platform,
        Domain::Phy,
        Domain::Ieee80211,
        Domain::Bluetooth,
        Domain::Ieee802154,
    ];
    let mut next = 0;
    for domain in domains {
        let (first, count) = domain.channels();
        assert_eq!(first, next, "{domain:?}");
        next = first + count;
    }
    assert_eq!(next, 64);
}

#[derive(Debug, PartialEq)]
struct Wake(u32);

impl crate::Event for Wake {
    const KIND: Kind = Kind::new(Domain::Phy, 4);
    const CHANNEL: crate::Channel = const {
        match crate::Channel::new(Domain::Phy, 0) {
            Ok(channel) => channel,
            Err(_) => panic!("invalid trace channel"),
        }
    };

    fn encode(&self) -> [u32; 2] {
        [self.0, 0]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self(words[0]))
    }
}

impl core::fmt::Display for Wake {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "wake step {}", self.0)
    }
}

crate::event_set!(PhySet: Wake);

#[test]
fn records_are_described_by_their_set_or_generically() {
    use crate::{Described, EventSet as _};
    use std::string::ToString;
    let wake = Record {
        tag: 1,
        kind: Kind::new(Domain::Phy, 4).raw(),
        t_us: 0,
        words: [7, 0],
    };
    let unknown = Record {
        kind: Kind::new(Domain::Ieee80211, 9).raw(),
        ..wake
    };
    let sets: &[crate::Describer] = &[PhySet::describe];
    assert_eq!(
        Described {
            record: &wake,
            sets
        }
        .to_string(),
        "wake step 7"
    );
    assert_eq!(
        Described {
            record: &unknown,
            sets
        }
        .to_string(),
        "ieee80211.9 [0x00000007, 0x00000000]"
    );
}

#[test]
fn channel_construction_returns_the_rejected_input_instead_of_panicking() {
    use crate::{Channel, InvalidChannel};
    for domain in [
        Domain::Platform,
        Domain::Phy,
        Domain::Ieee80211,
        Domain::Bluetooth,
        Domain::Ieee802154,
    ] {
        let (first, count) = domain.channels();
        for index in 0..=u8::MAX {
            let channel = Channel::new(domain, index);
            if index < count {
                let channel = channel.unwrap();
                assert_eq!(channel.bit_index(), first + index);
                assert_eq!(channel.mask(), 1u64 << (first + index));
            } else {
                assert_eq!(channel, Err(InvalidChannel { domain, index }));
            }
        }
    }
}

#[test]
fn caller_owned_record_buffers_keep_the_legacy_cyclic_order() {
    use crate::oldest_first;
    fn record(tag: u16) -> Record {
        Record {
            tag,
            kind: 0,
            t_us: 0,
            words: [0, 0],
        }
    }
    // Cover empty/singleton buffers and gap selection across zero, maximum
    // tags, duplicate tags and ties. The reference is the former algorithm.
    oldest_first(&mut []);
    let mut single = [record(65535)];
    oldest_first(&mut single);
    assert_eq!(single, [record(65535)]);
    for a in [0, 1, 2, 32767, 32768, 65534, 65535] {
        for b in [0, 1, 2, 32767, 32768, 65534, 65535] {
            for c in [0, 1, 2, 32767, 32768, 65534, 65535] {
                let mut actual = [record(a), record(b), record(c)];
                let mut expected = actual;
                expected.sort_unstable_by_key(|record| record.tag);
                let mut widest = u32::from(expected[0].tag) + 65535 - u32::from(expected[2].tag);
                let mut start = 0;
                for (index, pair) in expected.windows(2).enumerate() {
                    let gap = u32::from(pair[1].tag - pair[0].tag);
                    if gap > widest {
                        widest = gap;
                        start = index + 1;
                    }
                }
                expected.rotate_left(start);
                oldest_first(&mut actual);
                assert_eq!(actual, expected);
            }
        }
    }
}

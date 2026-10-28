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

#[derive(Clone, Copy, Debug, PartialEq)]
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

/// Keeps how many observations it received and the words of the last one.
#[derive(Default)]
struct Keeping {
    count: core::cell::Cell<u32>,
    last: core::cell::Cell<Option<(Kind, [u32; 2])>>,
}

impl crate::Observer for Keeping {
    fn event<E: crate::Event + Copy + 'static>(&self, event: E) {
        self.count.set(self.count.get() + 1);
        self.last.set(Some((E::KIND, event.encode())));
    }
}

/// An owner that reports one event per step through its observer field.
struct Owner<O: crate::Observer = crate::Noop> {
    observer: O,
    steps: u32,
}

impl<O: crate::Observer> Owner<O> {
    /// Advances the owner; `evaluated` counts evaluations of the event, so
    /// it is read only inside the observation point.
    #[cfg_attr(not(oer_trace), allow(unused_variables))]
    fn step(&mut self, evaluated: &core::cell::Cell<u32>) -> u32 {
        self.steps = self.steps.wrapping_add(1);
        crate::observe!(self.observer, {
            evaluated.set(evaluated.get() + 1);
            Wake(self.steps)
        });
        self.steps
    }
}

/// A child that receives its parent's observer by reference.
struct Child<'a, O: crate::Observer>(&'a O);

impl<O: crate::Observer> Child<'_, O> {
    #[cfg_attr(not(oer_trace), allow(unused_variables))]
    fn report(&self, evaluated: &core::cell::Cell<u32>) {
        crate::observe!(self.0, {
            evaluated.set(evaluated.get() + 1);
            Wake(40)
        });
    }
}

#[test]
fn the_noop_observer_adds_nothing_to_its_owner() {
    use core::mem::{align_of, needs_drop, size_of};
    assert_eq!(size_of::<crate::Noop>(), 0);
    assert_eq!(align_of::<crate::Noop>(), 1);
    assert!(!needs_drop::<crate::Noop>());
    assert_eq!(size_of::<Owner>(), size_of::<u32>());
    assert_eq!(align_of::<Owner>(), align_of::<u32>());
    let evaluated = core::cell::Cell::new(0);
    let mut owner = Owner {
        observer: crate::Noop,
        steps: 0,
    };
    assert_eq!(owner.step(&evaluated), 1);
    Child(&owner.observer).report(&evaluated);
}

#[test]
fn observers_by_reference_forward_to_the_owner_observer() {
    use crate::Observer;
    fn forward<O: Observer>(observer: O) {
        observer.event(Wake(3));
    }
    let keeping = Keeping::default();
    forward(&keeping);
    assert_eq!(keeping.count.get(), 1);
    assert_eq!(
        keeping.last.get(),
        Some((Kind::new(Domain::Phy, 4), [3, 0]))
    );
}

#[cfg(not(oer_trace))]
#[test]
fn a_point_without_the_trace_cfg_evaluates_no_argument_and_reports_nothing() {
    let evaluated = core::cell::Cell::new(0);
    let mut owner = Owner {
        observer: Keeping::default(),
        steps: 0,
    };
    assert_eq!(owner.step(&evaluated), 1);
    assert_eq!(owner.step(&evaluated), 2);
    Child(&owner.observer).report(&evaluated);
    let binding = &owner.observer;
    crate::observe!(binding, {
        evaluated.set(evaluated.get() + 1);
        Wake(50)
    });
    assert_eq!(evaluated.get(), 0);
    assert_eq!(owner.observer.count.get(), 0);
    assert_eq!(owner.observer.last.get(), None);
}

#[cfg(oer_trace)]
#[test]
fn a_point_with_the_trace_cfg_evaluates_its_event_once_and_reports_it() {
    let evaluated = core::cell::Cell::new(0);
    let mut owner = Owner {
        observer: Keeping::default(),
        steps: 0,
    };
    assert_eq!(owner.step(&evaluated), 1);
    assert_eq!(owner.step(&evaluated), 2);
    assert_eq!(evaluated.get(), 2);
    assert_eq!(owner.observer.count.get(), 2);
    assert_eq!(
        owner.observer.last.get(),
        Some((Kind::new(Domain::Phy, 4), [2, 0]))
    );
    Child(&owner.observer).report(&evaluated);
    let binding = &owner.observer;
    crate::observe!(binding, {
        evaluated.set(evaluated.get() + 1);
        Wake(50)
    });
    assert_eq!(evaluated.get(), 4);
    assert_eq!(owner.observer.count.get(), 4);
    assert_eq!(
        owner.observer.last.get(),
        Some((Kind::new(Domain::Phy, 4), [50, 0]))
    );
}

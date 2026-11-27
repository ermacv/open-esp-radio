extern crate std;

use super::*;
use oer_portable_time::{ClockRelation, Duration};

#[test]
fn the_monotonic_clock_counts_microseconds_of_the_image_clock() {
    assert_eq!(
        ClockInfo::<Port>::MONOTONIC_MICROS.monotonic,
        Some(ClockRelation::IDENTICAL)
    );
    assert_eq!(
        ClockInfo::<Port>::MONOTONIC_MICROS.resolution,
        Duration::from_micros(1)
    );
}

/// A test port's domain.
enum Port {}

#[test]
fn a_clocks_resolution_and_its_relation_are_both_compared() {
    let clock = |micros, monotonic| ClockInfo::<Port> {
        resolution: Duration::from_micros(micros),
        monotonic,
    };
    let one = clock(1, Some(ClockRelation::IDENTICAL));
    assert_eq!(one, ClockInfo::MONOTONIC_MICROS);
    assert_ne!(one, clock(2, Some(ClockRelation::IDENTICAL)));
    assert_ne!(one, clock(1, None));
    assert_ne!(one, clock(2, None));
    assert_eq!(
        std::format!("{:?}", clock(1, None)),
        "ClockInfo { resolution: DomainDuration(1), monotonic: None }"
    );
}

#[test]
fn a_declared_layout_issues_each_pools_capacity_without_stored_mint_state() {
    enum Keys {}
    enum Gates {}
    type Layout = (Slots<Keys, 256>, Slots<Gates, 2>);
    assert_eq!(core::mem::size_of::<Mint<'_, Layout>>(), 0);
    assert_eq!(core::mem::size_of::<PoolBuilder<'_>>(), 0);
    assert_eq!(core::mem::size_of::<Pools<'_, Layout>>(), 258);
    let (keys, gates) = Mint::<Layout>::new().into_pools();
    assert_eq!(keys.len(), 256);
    assert_eq!(keys[0].index(), 0);
    assert_eq!(keys[255].index(), 255);
    assert_eq!(gates.map(|gate| gate.index()), [0, 1]);
}

#[test]
fn nested_same_kind_paths_add_no_runtime_storage() {
    type Layout = (Slots<(), 1>, (Slots<(), 1>,));
    let ([first], ([second],)) = Mint::<Layout>::new().into_pools();
    assert_eq!(first.index(), 0);
    assert_eq!(second.index(), 0);
    assert_eq!(core::mem::size_of::<Pools<'_, Layout>>(), 2);
    assert_eq!(core::mem::size_of::<Pools<'_, ()>>(), 0);
    let () = Mint::<()>::new().into_pools();
}

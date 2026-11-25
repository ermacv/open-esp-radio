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
fn a_mint_numbers_a_lines_seats_and_sizes_its_line() {
    let PermitTokens { line, seats, .. } = Mint::<Permits<(), 0, 4>>::new().into_pools();
    assert_eq!(line.places(), 4);
    assert_eq!(seats.map(|seat| seat.index()), [0, 1, 2, 3]);
}

#[test]
fn a_declared_layout_issues_each_pools_capacity_without_stored_mint_state() {
    enum Keys {}
    enum Work {}
    type Layout = (Slots<Keys, 256>, Permits<Work, 2, 3>);
    assert_eq!(core::mem::size_of::<Mint<'_, Layout>>(), 0);
    assert_eq!(core::mem::size_of::<PoolBuilder<'_>>(), 0);
    assert_eq!(core::mem::size_of::<Pools<'_, Layout>>(), 261);
    let (keys, work) = Mint::<Layout>::new().into_pools();
    assert_eq!(keys.len(), 256);
    assert_eq!(keys[0].index(), 0);
    assert_eq!(keys[255].index(), 255);
    assert_eq!(
        work.supply.into_permits().map(|permit| permit.index()),
        [0, 1]
    );
    assert_eq!(work.seats.map(|seat| seat.index()), [0, 1, 2]);
    assert_eq!(work.line.places(), 3);
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

#[test]
fn all_256_permit_and_waiter_indices_are_representable() {
    let tokens = Mint::<Permits<(), 256, 256>>::new().into_pools();
    assert_eq!(tokens.line.places(), 256);
    assert_eq!(tokens.supply.into_permits()[255].index(), 255);
    assert_eq!(tokens.seats[255].index(), 255);
    assert_eq!(
        core::mem::size_of::<Pools<'_, Permits<(), 256, 256>>>(),
        512
    );
}

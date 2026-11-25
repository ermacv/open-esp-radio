extern crate std;

use super::{Duration, Instant};

#[test]
fn instant_arithmetic_is_checked_at_both_ends() {
    let instant = Instant::from_micros(100);
    assert_eq!(
        instant.checked_add(Duration::from_micros(50)),
        Some(Instant::from_micros(150))
    );
    assert_eq!(
        Instant::from_micros(u64::MAX).checked_add(Duration::from_micros(1)),
        None
    );
    assert_eq!(instant.checked_sub(Duration::from_micros(101)), None);
    assert_eq!(
        instant.checked_duration_since(Instant::from_micros(40)),
        Some(Duration::from_micros(60))
    );
    assert_eq!(
        instant.checked_duration_since(Instant::from_micros(101)),
        None
    );
    assert_eq!(
        instant.saturating_duration_since(Instant::from_micros(101)),
        Duration::ZERO
    );
}

#[test]
fn durations_convert_every_unit_to_microseconds() {
    assert_eq!(Duration::from_millis(3).as_micros(), 3_000);
    assert_eq!(Duration::from_secs(2).as_micros(), 2_000_000);
    assert_eq!(
        Duration::from_millis(u32::MAX).as_micros(),
        u64::from(u32::MAX) * 1_000
    );
    assert_eq!(
        Duration::from_secs(u32::MAX).as_micros(),
        u64::from(u32::MAX) * 1_000_000
    );
    assert_eq!(
        Duration::from_micros(u64::MAX).checked_add(Duration::from_micros(1)),
        None
    );
    assert_eq!(
        Duration::from_micros(5).saturating_sub(Duration::from_micros(9)),
        Duration::ZERO
    );
}

#[test]
fn a_saturating_deadline_past_the_range_never_comes() {
    let late = Instant::from_micros(u64::MAX - 1);
    assert_eq!(
        late.saturating_add(Duration::from_secs(1)),
        Instant::from_micros(u64::MAX)
    );
    assert_eq!(
        Instant::from_micros(5).saturating_add(Duration::from_micros(10)),
        Instant::from_micros(15)
    );
}

/// `oer-portable-time` is a dependency-free contract crate, with no feature
/// that could add one.
#[test]
fn the_crate_has_no_dependency_and_no_feature() {
    let manifest: toml::Table = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    for table in ["dependencies", "build-dependencies", "target", "features"] {
        assert!(manifest.get(table).is_none(), "`[{table}]`");
    }
}

use super::*;

#[test]
fn chunk_interval_matches_requested_rate() {
    assert_eq!(
        chunk_interval(14_600, 80_000_000).unwrap(),
        Duration::from_nanos(1_460_000)
    );
}

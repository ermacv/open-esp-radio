use super::*;

#[test]
fn packet_interval_matches_requested_payload_rate() {
    assert_eq!(
        packet_interval(1_200, 80_000_000).unwrap(),
        Duration::from_micros(120)
    );
    assert_eq!(
        packet_interval(1_200, 10_000_000).unwrap(),
        Duration::from_micros(960)
    );
}

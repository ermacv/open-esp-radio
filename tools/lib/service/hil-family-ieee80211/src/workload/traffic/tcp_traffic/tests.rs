use super::*;

#[test]
fn tcp_tx_defaults_separate_offer_from_acceptance_floor() {
    let options = Config::for_direction(Direction::Tx).validate().unwrap();

    assert_eq!(options.chunk_bytes, 32_768);
    assert_eq!(options.tx_rate_bps, Some(60_000_000));
    assert_eq!(options.tx_floor_bps, Some(45_000_000));
    assert_eq!(options.rx_rate_bps, None);
}

#[test]
fn direction_rejects_an_inapplicable_flow() {
    let options = Config {
        tx_rate_bps: Some(1_000_000),
        ..Config::for_direction(Direction::Rx)
    };
    assert!(options.validate().is_err());
}

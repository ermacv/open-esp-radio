use super::{RxRateSampleUpdate, rate_control_route, update_rx_rate_sample};

fn route(flags: u8) -> Option<u8> {
    rate_control_route(flags).map(|route| route.index())
}

#[test]
fn reproduces_every_recovered_route_class() {
    assert_eq!(route(0x10), Some(0));
    assert_eq!(route(0x20), Some(1));
    assert_eq!(route(0x40), Some(2));
    assert_eq!(route(0x00), None);
    assert_eq!(route(0x30), None);
}

#[test]
fn preserves_vendor_bit_precedence() {
    assert_eq!(route(0x50), Some(0));
    assert_eq!(route(0x60), Some(1));
    assert_eq!(route(0x70), None);
}

#[test]
fn ignores_unrelated_rx_control_bits() {
    for unrelated in [0x01, 0x02, 0x04, 0x08, 0x80, 0x8f] {
        assert_eq!(route(unrelated), None);
        assert_eq!(route(unrelated | 0x10), Some(0));
        assert_eq!(route(unrelated | 0x20), Some(1));
        assert_eq!(route(unrelated | 0x40), Some(2));
    }
}

#[test]
fn rate_sample_filter_reproduces_initial_and_running_updates() {
    assert_eq!(
        update_rx_rate_sample(0, 0, 3, 0xf6, 127, 127),
        Some(RxRateSampleUpdate {
            latest: -7,
            smoothed: -7,
        })
    );
    assert_eq!(
        update_rx_rate_sample(0, 0, 0, 0xec, -10, -8),
        Some(RxRateSampleUpdate {
            latest: -20,
            smoothed: -9,
        })
    );
}

#[test]
fn rate_sample_filter_preserves_guards_and_byte_wrapping() {
    assert_eq!(update_rx_rate_sample(0x80, 0, 0, 1, 0, 0), None);
    assert_eq!(update_rx_rate_sample(0, 1, 0, 1, 0, 0), None);
    assert_eq!(
        update_rx_rate_sample(0, 0, 20, 250, 127, 127),
        Some(RxRateSampleUpdate {
            latest: 14,
            smoothed: 14,
        })
    );
}

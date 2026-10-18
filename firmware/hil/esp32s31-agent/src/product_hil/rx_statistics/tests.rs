use super::ObservedRxStatistics;

#[test]
fn hardware_rx_deltas_use_each_counter_domain() {
    let earlier = ObservedRxStatistics {
        fcs_error: u16::MAX - 1,
        brx_error: 0x03fc,
        rx_hang: u8::MAX - 1,
        rx_tx_hang: u32::MAX - 1,
        ..ObservedRxStatistics::default()
    };
    let current = ObservedRxStatistics {
        fcs_error: 1,
        brx_error: 3,
        rx_hang: 1,
        rx_tx_hang: 1,
        ..ObservedRxStatistics::default()
    };

    let delta = current.wrapping_delta_since(earlier);
    assert_eq!(delta.fcs_error, 3);
    assert_eq!(delta.brx_error, 7);
    assert_eq!(delta.rx_hang, 3);
    assert_eq!(delta.rx_tx_hang, 3);
}

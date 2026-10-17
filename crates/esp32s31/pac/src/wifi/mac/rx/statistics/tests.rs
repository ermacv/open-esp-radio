use super::{
    MacRxDecodeErrorStatistics, MacRxDecodeErrorStatisticsDelta, MacRxHangStatistics,
    MacRxHangStatisticsDelta, MacRxPrimaryStatistics, MacRxPrimaryStatisticsDelta,
};

fn primary(mpdu_count: u16, buffer_full: u16) -> MacRxPrimaryStatistics {
    MacRxPrimaryStatistics {
        mpdu_count,
        cfo_scaled_40: -80,
        fcs_error: 2,
        abort: 3,
        abort_fcs_pass: 4,
        power_drop_error: 5,
        he_sig_b_error: 6,
        same_bm_error: 7,
        signal_field: 8,
        end: 9,
        data_success: 10,
        other_unicast: 11,
        buffer_full,
        fifo_overflow: 12,
        tkip_error: 13,
        bt_block_error: 14,
        frequency_hop_error: 15,
        last_unmatched_error: 16,
        ack_interrupt: 17,
        rts_interrupt: 18,
    }
}

#[test]
fn cfo_transform_matches_complete_blob_signed_halfword_arithmetic() {
    let transform = |raw: u16| i32::from(raw as i16) * 40;
    assert_eq!(transform(0), 0);
    assert_eq!(transform(1), 40);
    assert_eq!(transform(0xffff), -40);
    assert_eq!(transform(0x8000), -1_310_720);
    assert_eq!(transform(0x7fff), 1_310_680);
}

#[test]
fn primary_counter_delta_preserves_sixteen_bit_wrap() {
    let earlier = primary(0xfffe, 0xffff);
    let mut current = primary(3, 2);
    current.fcs_error = 9;

    let delta = current.wrapping_delta_since(earlier);
    assert_eq!(
        delta,
        MacRxPrimaryStatisticsDelta {
            mpdu_count: 5,
            fcs_error: 7,
            buffer_full: 3,
            ..MacRxPrimaryStatisticsDelta::default()
        }
    );
}

#[test]
fn decode_counter_delta_wraps_at_ten_bits() {
    let earlier = MacRxDecodeErrorStatistics {
        brx_agc: 0x03fe,
        brx: 4,
        nrx: 0,
        nrx_abort: 0,
        nrx_agc_exit: 0,
        nrx_baseband_off: 0,
        nrx_fdm_watchdog: 0,
        nrx_restart: 0,
        nrx_service: 0,
        nrx_tx_over: 0,
        nrx_unsupported: 0,
        nrx_he_format: 0,
        nrx_ht_sig: 0,
        nrx_he_unsupported: 0,
        nrx_he_sig_a_crc: 0,
    };
    let mut current = earlier;
    current.brx_agc = 3;
    current.brx = 9;

    assert_eq!(
        current.wrapping_delta_since(earlier),
        MacRxDecodeErrorStatisticsDelta {
            brx_agc: 5,
            brx: 5,
            ..MacRxDecodeErrorStatisticsDelta::default()
        }
    );
}

#[test]
fn hang_counter_delta_uses_each_hardware_storage_width() {
    let earlier = MacRxHangStatistics {
        rx: 0xff,
        tx: 2,
        rx_tx_hang: u32::MAX,
        rx_tx_panic: 5,
    };
    let current = MacRxHangStatistics {
        rx: 1,
        tx: 7,
        rx_tx_hang: 2,
        rx_tx_panic: 9,
    };

    assert_eq!(
        current.wrapping_delta_since(earlier),
        MacRxHangStatisticsDelta {
            rx: 2,
            tx: 5,
            rx_tx_hang: 3,
            rx_tx_panic: 4,
        }
    );
}

//! The `mac_txrx_init` transactions over host memory.

use oer_esp32s31_pac::{instances, wifi_mac::SharedRadioInitControl};

use super::MacDelaySlot;
use crate::ieee80211::registers::{HostWords, element};

#[test]
fn delay_slot_keeps_the_vendor_modulo() {
    assert_eq!(MacDelaySlot::from_random(0).value(), 0);
    assert_eq!(MacDelaySlot::from_random(10).value(), 10);
    assert_eq!(MacDelaySlot::from_random(11).value(), 0);
    assert_eq!(
        MacDelaySlot::from_random(u32::MAX).value(),
        (u32::MAX % 11) as u8
    );
}

#[test]
fn prefix_clears_the_queue_defaults_then_sets_queues_zero_and_one() {
    let mut memory = HostWords::filled(u32::MAX);
    memory.owners().registers.initialize_mac_txrx_prefix();

    let owners = memory.owners();
    let prefix = &owners.registers.mac.wifi_mac_txrx_prefix;
    for queue in 0..4 {
        let default = prefix.read_rx_queue_default(element(queue));
        let first_two = queue < 2;
        assert_eq!(default.bits_16_23_opaque(), 0);
        assert_eq!(default.bit_24_opaque(), first_two);
        assert!(!default.bit_25_opaque());
        assert_eq!(default.bit_26_opaque(), first_two);
        assert_eq!(default.bits_27_31_opaque().get(), 0);
        assert_eq!(default.bits_0_15_opaque(), u16::MAX);
    }
    assert!(!prefix.read_mode_control().bit_3_opaque());
    assert_eq!(prefix.read_timing_control().bits_20_27_opaque(), 0x1b);
    assert_eq!(
        prefix
            .read_txrx_prefix_shared_enable_control()
            .bits_0_1_opaque()
            .get(),
        3
    );
}

#[test]
fn callbacks_publish_the_delays_and_response_rate_tables() {
    let mut memory = HostWords::filled(0);
    memory
        .owners()
        .registers
        .initialize_mac_txrx_callbacks(MacDelaySlot::from_random(7));

    let owners = memory.owners();
    let callbacks = &owners.registers.mac.wifi_mac_txrx_callbacks;
    let primary = callbacks.read_delay_primary();
    assert_eq!(primary.rx_cck_delay().get(), 0x3b9);
    assert_eq!(primary.rx_ofdm_delay().get(), 0xf5 + 7);
    assert_eq!(primary.tx_ofdm_delay().get(), 0x5e);
    let secondary = callbacks.read_delay_secondary();
    assert_eq!(secondary.rx_11g_ofdm_delay().get(), 0xfa + 7);
    assert_eq!(secondary.tx_cck_delay().get(), 0x276);
    let ack = callbacks.read_ack_rate_table();
    let cts = callbacks.read_cts_rate_table();
    for table in [
        (
            ack.bpsk_rate(),
            ack.qpsk_rate(),
            ack.qam16_rate(),
            ack.bits_24_31_opaque(),
        ),
        (
            cts.bpsk_rate(),
            cts.qpsk_rate(),
            cts.qam16_rate(),
            cts.bits_24_31_opaque(),
        ),
    ] {
        assert_eq!(table, (0x0b, 0x0a, 0x09, 0));
    }
    let ack_cck = callbacks.read_ack_cck_rate_table();
    let cts_cck = callbacks.read_cts_cck_rate_table();
    for (rate_1m, rate_2m, rate_2m_short) in [
        (
            ack_cck.rate_1m(),
            ack_cck.rate_2m(),
            ack_cck.rate_2m_short_preamble(),
        ),
        (
            cts_cck.rate_1m(),
            cts_cck.rate_2m(),
            cts_cck.rate_2m_short_preamble(),
        ),
    ] {
        assert_eq!((rate_1m, rate_2m, rate_2m_short), (0x00, 0x01, 0x05));
    }
    assert_eq!(
        callbacks.read_bb_rx_hang_control().bits_0_11_opaque().get(),
        0x00f
    );
}

#[test]
fn suffix_stops_the_walker_and_publishes_the_shared_initialization() {
    let mut memory = HostWords::filled(0);
    let mut shared_words = std::vec![0; instances::wifi_mac::SIZE.div_ceil(4)];
    let mut shared =
        SharedRadioInitControl::host(&mut shared_words).expect("the part's host words");
    {
        let mut owners = memory.owners();
        owners.rx_dma.dma.modify_rx_control(|w| {
            w.set_walker_enable(true);
        });
        owners
            .registers
            .initialize_mac_txrx_suffix(&mut shared, &mut owners.rx_dma);
    }
    assert_eq!(
        shared.read_radio_init_control().bits_0_11_opaque().get(),
        0x0f0
    );

    let owners = memory.owners();
    assert!(!owners.rx_dma.dma.read_rx_control().walker_enable());
    let mac = &owners.registers.mac;
    let hang = mac.wifi_mac_txrx_callbacks.read_bb_rx_hang_control();
    assert!(hang.bit_30_opaque() && hang.bit_31_opaque());
    let suffix = &mac.wifi_mac_txrx_suffix;
    assert_eq!(
        suffix.read_default_image_a().bits_0_11_opaque().get(),
        0x0f0
    );
    assert_eq!(suffix.read_field_control().bits_4_7_opaque().get(), 4);
    let gate = suffix.read_gate_control();
    assert_eq!(gate.bits_16_30_opaque().get(), 0x7fff);
    assert!(gate.bit_31_opaque());
    assert!(suffix.read_aux_enable().bit_1_opaque());
}

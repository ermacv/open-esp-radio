//! Stand self-checks: each catalog scenario runs in its own process, and the
//! assertions are independent expectations read from the pinned ESP-IDF
//! source, not snapshots of the stand's own output.

use std::process::Command;

fn trace(scenario: &str) -> Vec<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_oer-esp32s31-ieee802154-vendor-host"))
        .args(["run", scenario])
        .output()
        .expect("the stand binary runs");
    assert!(output.status.success(), "{scenario} failed: {output:?}");
    String::from_utf8(output.stdout)
        .expect("UTF-8 trace")
        .lines()
        .filter(|line| !line.contains("_critical"))
        .map(str::to_owned)
        .collect()
}

/// The records in `expected` appear in `trace` in this order.
fn assert_subsequence(trace: &[String], expected: &[&str]) {
    let mut remaining = trace.iter();
    for record in expected {
        assert!(
            remaining.any(|line| line == record),
            "missing or out of order: {record}\ntrace:\n{}",
            trace.join("\n")
        );
    }
}

#[test]
fn every_scenario_runs_without_a_driver_assertion() {
    let list = Command::new(env!("CARGO_BIN_EXE_oer-esp32s31-ieee802154-vendor-host"))
        .arg("list")
        .output()
        .expect("the stand binary lists its scenarios");
    for scenario in String::from_utf8(list.stdout).unwrap().lines() {
        let trace = trace(scenario);
        assert!(
            !trace
                .iter()
                .any(|line| line.starts_with("event assert_failed")),
            "{scenario} tripped a driver assertion:\n{}",
            trace.join("\n")
        );
    }
}

/// The production engine reproduces every catalog trace.
#[test]
fn every_scenario_matches_the_production_engine() {
    let list = Command::new(env!("CARGO_BIN_EXE_oer-esp32s31-ieee802154-vendor-host"))
        .arg("list")
        .output()
        .expect("the stand binary lists its scenarios");
    for scenario in String::from_utf8(list.stdout).unwrap().lines() {
        let output = Command::new(env!("CARGO_BIN_EXE_oer-esp32s31-ieee802154-vendor-host"))
            .args(["compare", scenario])
            .output()
            .expect("the stand binary compares");
        let verdict = String::from_utf8_lossy(&output.stdout);
        assert_eq!(verdict.trim(), "MATCH", "{scenario}: {verdict}");
    }
}

/// `esp_i_enable` then `i_mac_init` (esp_i.c
/// L35-L41, esp_i_dev.c L897-L956), in the build without software
/// coexistence.
#[cfg(not(feature = "sw-coex"))]
#[test]
fn enable_follows_the_public_order_and_mac_init() {
    assert_eq!(
        trace("enable"),
        [
            "ext modem_clock_module_enable(0x7)",
            "ext esp_phy_enable(0x4)",
            "ext esp_btbb_enable()",
            "ext modem_clock_module_mac_reset(0x7)",
            "ll ieee802154_ll_enable_events(0x3fff)",
            "ll ieee802154_ll_disable_events(0x100)",
            "ll ieee802154_ll_enable_tx_abort_events(0x1868000)",
            "ll ieee802154_ll_enable_rx_abort_events(0x28000)",
            "ll ieee802154_ll_set_ed_sample_mode(0x1)",
            "ll ieee802154_ll_disable_coex()",
            "ext ieee802154_txon_delay_set()",
            "ext esp_intr_alloc(0x84, 0x0)",
            "ext esp_phy_modem_init(0x2)",
            "return esp_ieee802154_enable = 0",
        ]
    );
}

/// With software coexistence `i_mac_init` publishes the ACK at
/// `IEEE802154_MIDDLE` (2) and the idle scene at `IEEE802154_IDLE` (4)
/// instead of disabling coexistence (esp_i_dev.c L924-L928,
/// esp_i_util.c L31-L35); receive, energy detection and CCA publish
/// the `IEEE802154_LOW` (3) scene before their command (L481, L1229,
/// L1246), a timed transmission `IEEE802154_MIDDLE` (L1054).
#[cfg(feature = "sw-coex")]
#[test]
fn software_coexistence_publishes_the_scene_levels() {
    let enable = trace("enable");
    assert_subsequence(
        &enable,
        &[
            "ll ieee802154_ll_set_ed_sample_mode(0x1)",
            "coex esp_coex_ieee802154_ack_pti_set(0x2)",
            "coex esp_coex_ieee802154_txrx_pti_set(0x4)",
            "ext ieee802154_txon_delay_set()",
        ],
    );
    assert!(!enable.iter().any(|line| line.contains("disable_coex")));
    for scenario in ["receive-with-auto-ack", "energy-detect", "cca-busy"] {
        assert!(
            trace(scenario)
                .iter()
                .any(|line| line == "coex esp_coex_ieee802154_txrx_pti_set(0x3)"),
            "{scenario}"
        );
    }
    assert!(
        trace("transmit-at")
            .iter()
            .any(|line| line == "coex esp_coex_ieee802154_txrx_pti_set(0x2)")
    );
}

/// `tx_init` stops the current operation, applies the pending PIB, publishes
/// the frame and the ACK receive buffer, then issues `TX_START`; `TX_DONE` of
/// an ACK-requesting frame starts the 200 ms timer-zero watchdog and
/// `ACK_RX_DONE` stops it before reporting the ACK (esp_i_dev.c
/// L507-L536, L595-L602, L992-L1026).
// The default build's trace; multi-PAN adds identity reads and interface
// indices.
#[cfg(not(feature = "multipan"))]
#[test]
fn transmit_with_ack_arms_and_disarms_the_ack_watchdog() {
    assert_subsequence(
        &trace("transmit-with-ack"),
        &[
            "ll ieee802154_ll_set_cmd(0x45)",
            "ll ieee802154_ll_set_freq(0x3)",
            "ll ieee802154_ll_set_pending_mode(0x0)",
            "ll ieee802154_ll_set_tx_addr(tx#0)",
            "ll ieee802154_ll_set_rx_addr(buf#0)",
            "ll ieee802154_ll_set_cmd(0x41)",
            "ll ieee802154_ll_clear_events(0x1)",
            "ll ieee802154_ll_enable_events(0x100)",
            "ll ieee802154_ll_timer0_set_threshold(0x30d40)",
            "ll ieee802154_ll_set_cmd(0x4c)",
            "ll ieee802154_ll_clear_events(0x8)",
            "ll ieee802154_ll_set_cmd(0x4d)",
            "ll ieee802154_ll_disable_events(0x100)",
            "event transmit_done(0x0, 0x1, 0xb, 0xffffffffffffffc4, 0xc8, 0x0, 0x1, 0x0) \
             [0c6188013412ffff7856aa0000] [05020001c4c8]",
        ],
    );
}

/// A CRC abort is not reported; `next_operation` re-arms receive into the
/// same buffer (esp_i_dev.c L488-L505, L604-L640).
#[test]
fn receive_crc_error_restarts_into_the_same_buffer() {
    let trace = trace("receive-crc-error-restarts");
    assert!(!trace.iter().any(|line| line.starts_with("event ")));
    assert_subsequence(
        &trace,
        &[
            "ll ieee802154_ll_set_rx_addr(buf#0)",
            "ll ieee802154_ll_set_cmd(0x42)",
            "ll ieee802154_ll_clear_events(0x10)",
            "ll ieee802154_ll_get_rx_status()",
            "ll ieee802154_ll_set_rx_addr(buf#0)",
            "ll ieee802154_ll_set_cmd(0x42)",
        ],
    );
}

/// `RX_DONE` of an ACK-requesting 2006 frame with TX auto-ACK sets the pending
/// bit before the ACK leaves; `ACK_TX_DONE` delivers the frame and
/// `next_operation` re-arms receive into the next buffer
/// (esp_i_dev.c L538-L593).
// The default build's trace; multi-PAN adds identity reads and interface
// indices.
#[cfg(not(feature = "multipan"))]
#[test]
fn receive_with_auto_ack_selects_pending_before_the_ack() {
    assert_subsequence(
        &trace("receive-with-auto-ack"),
        &[
            "ll ieee802154_ll_set_rx_addr(buf#0)",
            "ll ieee802154_ll_clear_events(0x2)",
            "ll ieee802154_ll_get_tx_auto_ack()",
            "ll ieee802154_ll_set_pending_bit(0x1)",
            "ll ieee802154_ll_clear_events(0x4)",
            "event receive_done(0x1, 0x1, 0xb, 0xffffffffffffffc9, 0xb4, 0x0, 0x1, 0x0) \
             [0c6188013412ffff7856aac9b4] []",
            "ll ieee802154_ll_set_rx_addr(buf#1)",
            "ll ieee802154_ll_set_cmd(0x42)",
            "return esp_ieee802154_receive_handle_done = 0",
        ],
    );
}

/// ED and standalone CCA share `ED_START`; `ED_DONE` reports the RSS code
/// plus the S31 compensation 0, or the CCA busy bit
/// (esp_i_dev.c L761-L770, L985-L990, L1221-L1252).
#[test]
fn energy_detection_and_cca_report_their_ed_done_sample() {
    assert_subsequence(
        &trace("energy-detect"),
        &[
            "ll ieee802154_ll_enable_events(0x40)",
            "ll ieee802154_ll_set_ed_duration(0x8)",
            "ll ieee802154_ll_set_cmd(0x44)",
            "ll ieee802154_ll_get_ed_rss()",
            "event energy_detect_done(0xffffffffffffffba) [] []",
        ],
    );
    assert_subsequence(
        &trace("cca-busy"),
        &[
            "ll ieee802154_ll_set_cmd(0x44)",
            "ll ieee802154_ll_is_cca_busy()",
            "event cca_done(0x1) [] []",
        ],
    );
}

/// A secured enhanced ACK is armed before the driver publishes it: the
/// OpenThread port reads the extended address, configures transmit security
/// over the ACK, then the driver publishes the ACK and notifies the hardware
/// (esp_openthread_radio.c `enh_ack_set_security_addr_and_key`,
/// esp_i_sec.c L16-L25, esp_i_dev.c L564-L573); the ACK's
/// `TX_DONE` clears the security.
#[test]
fn a_secured_enhanced_ack_is_armed_before_it_is_published() {
    assert_subsequence(
        &trace("receive-with-secured-enhanced-ack"),
        &[
            "ll ieee802154_ll_get_multipan_ext_addr(0x0, [0000000000000000])",
            "ll ieee802154_ll_set_security_addr([0000000000000000])",
            "ll ieee802154_ll_set_security_key([42424242424242424242424242424242])",
            "ll ieee802154_ll_set_security_offset(0xd)",
            "ll ieee802154_ll_set_transmit_security(0x1)",
            "ll ieee802154_ll_set_tx_addr(buf#1)",
            "ll ieee802154_ll_enhack_generate_done_notify()",
            "ll ieee802154_ll_set_transmit_security(0x0)",
        ],
    );
}

/// `esp_i_disable` runs `i_mac_deinit` first, which frees
/// the source-132 interrupt before BTBB, the PHY client and the modem clock
/// are released, and writes no MAC register, even with a reception running
/// (esp_i.c `esp_i_disable`, esp_i_dev.c
/// `i_mac_deinit`). `esp_i_enable` allocates it last in
/// `i_mac_init` (see `enable_follows_the_public_order_and_mac_init`).
/// The composition mirrors both: it binds the route after the runtime is
/// installed and quiesces it first on stop.
#[test]
fn disable_frees_the_route_before_the_shared_resources_without_mac_writes() {
    let trace = trace("enable-disable");
    let received = trace
        .iter()
        .position(|line| line == "return esp_ieee802154_receive = 0")
        .expect("the reception started");
    let disable: Vec<&str> = trace[received + 1..].iter().map(String::as_str).collect();
    assert_eq!(
        disable,
        [
            "ext esp_phy_modem_deinit(0x2)",
            "ext esp_intr_free()",
            "ext esp_btbb_disable()",
            "ext esp_phy_disable(0x4)",
            "ext modem_clock_module_disable(0x7)",
            "return esp_ieee802154_disable = 0",
        ]
    );
}

/// `esp_i_get_recent_rssi` is `(int8_t)(bt_bb_get_cur_rx_info() &
/// 0xff)` (esp_i_dev.c L271-L274): in every driver state it makes
/// exactly one receive-information call and no register-layer access, and
/// returns the image's low byte as a signed value whatever its upper bits.
#[test]
fn recent_rssi_is_the_signed_low_byte_of_one_rx_info_read() {
    let trace = trace("recent-rssi");
    let returns: Vec<usize> = trace
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("return esp_ieee802154_get_recent_rssi"))
        .map(|(index, _)| index)
        .collect();
    // Four images in each of five states.
    assert_eq!(returns.len(), 20, "trace:\n{}", trace.join("\n"));
    for &index in &returns {
        assert_eq!(
            trace[index - 1],
            "ext bt_bb_get_cur_rx_info()",
            "trace:\n{}",
            trace.join("\n")
        );
        assert!(
            index < 2 || !trace[index - 2].starts_with("ext bt_bb_get_cur_rx_info"),
            "one read per call:\n{}",
            trace.join("\n")
        );
    }
    let values: Vec<&str> = returns
        .iter()
        .map(|&index| trace[index].rsplit(" = ").next().unwrap())
        .collect();
    assert_eq!(values, ["-58", "127", "-128", "0"].repeat(5));
}

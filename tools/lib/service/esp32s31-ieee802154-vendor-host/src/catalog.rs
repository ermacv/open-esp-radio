//! Named vendor scenarios.
//!
//! Event bits, abort codes and frame layouts are the pinned ESP-IDF values
//! (`ieee802154_common_ll.h`, `esp_ieee802154.h`): an RX/TX buffer is
//! `[length, psdu...]`, the length counts the two FCS bytes, and a received
//! buffer carries RSSI and LQI in place of the FCS.

use crate::{
    record::Inputs,
    scenario::{Scenario, Step},
};

/// `IEEE802154_EVENT_*` bits.
pub mod event {
    pub const TX_DONE: u16 = 1 << 0;
    pub const RX_DONE: u16 = 1 << 1;
    pub const ACK_TX_DONE: u16 = 1 << 2;
    pub const ACK_RX_DONE: u16 = 1 << 3;
    pub const RX_ABORT: u16 = 1 << 4;
    pub const TX_ABORT: u16 = 1 << 5;
    pub const ED_DONE: u16 = 1 << 6;
    pub const TIMER0_OVERFLOW: u16 = 1 << 8;
    pub const TIMER1_OVERFLOW: u16 = 1 << 9;
}

/// `IEEE802154_RX_ABORT_BY_*` codes used by the catalog.
pub mod rx_abort {
    pub const RX_STOP: u8 = 1;
    pub const SFD_TIMEOUT: u8 = 2;
    pub const CRC_ERROR: u8 = 3;
    pub const INVALID_LEN: u8 = 4;
    pub const FILTER_FAIL: u8 = 5;
    pub const NO_RSS: u8 = 6;
    pub const COEX_BREAK: u8 = 7;
    pub const UNEXPECTED_ACK: u8 = 8;
    pub const RX_RESTART: u8 = 9;
    pub const TX_ACK_TIMEOUT: u8 = 16;
    pub const TX_ACK_STOP: u8 = 17;
    pub const TX_ACK_COEX_BREAK: u8 = 18;
    pub const ENHACK_SECURITY_ERROR: u8 = 19;
    pub const ED_ABORT: u8 = 24;
    pub const ED_STOP: u8 = 25;
    pub const ED_COEX_REJECT: u8 = 26;
}

/// `IEEE802154_TX_ABORT_BY_*` codes used by the catalog.
pub mod tx_abort {
    pub const RX_ACK_STOP: u8 = 1;
    pub const RX_ACK_SFD_TIMEOUT: u8 = 2;
    pub const RX_ACK_CRC_ERROR: u8 = 3;
    pub const RX_ACK_INVALID_LEN: u8 = 4;
    pub const RX_ACK_FILTER_FAIL: u8 = 5;
    pub const RX_ACK_NO_RSS: u8 = 6;
    pub const RX_ACK_COEX_BREAK: u8 = 7;
    pub const RX_ACK_TYPE_NOT_ACK: u8 = 8;
    pub const RX_ACK_RESTART: u8 = 9;
    pub const RX_ACK_TIMEOUT: u8 = 16;
    pub const TX_STOP: u8 = 17;
    pub const TX_COEX_BREAK: u8 = 18;
    pub const TX_SECURITY_ERROR: u8 = 19;
    pub const CCA_FAILED: u8 = 24;
    pub const CCA_BUSY: u8 = 25;
}

/// Data frame, PAN-ID compression, short addresses, 2006 frame version.
/// `acknowledge` sets the ACK-request bit.
fn data_frame(acknowledge: bool) -> Vec<u8> {
    let control = if acknowledge { 0x61 } else { 0x41 };
    // length 12: FCF(2) sequence(1) PAN(2) destination(2) source(2) payload(1) FCS(2)
    vec![
        12, control, 0x88, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa, 0x00, 0x00,
    ]
}

/// Immediate ACK for sequence one, RSSI -60 and LQI 200 in place of the FCS.
fn received_ack() -> Vec<u8> {
    vec![5, 0x02, 0x00, 0x01, (-60i8) as u8, 200]
}

/// Received data frame requesting an ACK, RSSI -55 and LQI 180.
fn received_data_frame() -> Vec<u8> {
    let mut frame = data_frame(true);
    let len = frame.len();
    frame[len - 2] = (-55i8) as u8;
    frame[len - 1] = 180;
    frame
}

/// Received 2015 data frame requesting an ACK, RSSI -55 and LQI 180.
fn received_2015_frame() -> Vec<u8> {
    let mut frame = received_data_frame();
    frame[2] = 0xa8;
    frame
}

fn no_inputs() -> Inputs {
    Inputs::default()
}

/// Enhanced ACK returned by the application generator.
fn enhanced_ack_inputs() -> Inputs {
    Inputs {
        enhanced_ack: Some(vec![5, 0x02, 0x20, 0x01, 0x00, 0x00]),
        ..Inputs::default()
    }
}

fn interrupt(events: u16) -> Step {
    Step::Interrupt {
        events,
        rx_abort: 0,
        tx_abort: 0,
    }
}

fn transmit_ack_timer_expires() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Input("esp_timer_get_time", 1_000),
        Step::Transmit {
            frame: data_frame(true),
            cca: false,
        },
        interrupt(event::TX_DONE),
        interrupt(event::TIMER0_OVERFLOW),
    ]
}

fn transmit_ack_abort_timeout() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Transmit {
            frame: data_frame(true),
            cca: false,
        },
        interrupt(event::TX_DONE),
        Step::Interrupt {
            events: event::TX_ABORT,
            rx_abort: 0,
            tx_abort: tx_abort::RX_ACK_TIMEOUT,
        },
    ]
}

fn receive_with_enhanced_ack() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetRxWhenIdle(true),
        Step::Receive,
        Step::DeliverFrame(received_2015_frame()),
        interrupt(event::RX_DONE),
        interrupt(event::ACK_TX_DONE),
    ]
}

/// Secured enhanced ACK returned by the application generator with its key:
/// ACK, security enabled, 2015, short destination; ENC-MIC-32 with key index
/// 1 after frame counter 0x01020304, then the MIC and FCS placeholders.
fn secured_enhanced_ack_inputs() -> Inputs {
    Inputs {
        enhanced_ack: Some(vec![
            0x13, 0x0a, 0x28, 0x01, 0x34, 0x12, 0x78, 0x56, 0x0d, 0x04, 0x03, 0x02, 0x01, 0x01, 0,
            0, 0, 0, 0, 0,
        ]),
        enhanced_ack_key: Some([0x42; 16]),
        ..Inputs::default()
    }
}

fn receive_2015_without_enhanced_ack() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetRxWhenIdle(true),
        Step::Receive,
        Step::DeliverFrame(received_2015_frame()),
        interrupt(event::RX_DONE),
    ]
}

fn transmit_during_ack_fails() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Receive,
        Step::DeliverFrame(received_data_frame()),
        interrupt(event::RX_DONE),
        Step::Transmit {
            frame: data_frame(false),
            cca: true,
        },
    ]
}

fn transmit_stops_receive() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetRxWhenIdle(true),
        Step::Receive,
        Step::Transmit {
            frame: data_frame(false),
            cca: false,
        },
        interrupt(event::TX_DONE),
    ]
}

fn transmit_restarts_transmit() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Transmit {
            frame: data_frame(true),
            cca: false,
        },
        Step::Transmit {
            frame: data_frame(false),
            cca: true,
        },
    ]
}

fn full_receive_ring_uses_the_stub() -> Vec<Step> {
    let mut steps = vec![Step::Enable, Step::SetRxWhenIdle(true), Step::Receive];
    for _ in 0..21 {
        steps.push(Step::DeliverFrame(data_frame(false)));
        steps.push(interrupt(event::RX_DONE));
    }
    steps.push(Step::DeliverFrame(data_frame(false)));
    steps.push(interrupt(event::RX_DONE));
    steps
}

fn energy_detect_abort() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::EnergyDetect(8),
        Step::Input("ieee802154_ll_get_rx_status", 0x0018_0000),
        Step::Interrupt {
            events: event::RX_ABORT,
            rx_abort: rx_abort::ED_ABORT,
            tx_abort: 0,
        },
    ]
}

fn transmit_at() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Input("esp_timer_get_time", 1_000),
        Step::TransmitAt {
            frame: data_frame(false),
            cca: true,
            time: 5_000,
        },
        interrupt(event::TX_DONE),
    ]
}

fn receive_at_window_closes() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Input("esp_timer_get_time", 1_000),
        Step::ReceiveAt {
            time: 5_000,
            duration: 2_000,
        },
        Step::Input("esp_timer_get_time", 4_854),
        interrupt(event::TIMER1_OVERFLOW),
        Step::Input("esp_timer_get_time", 7_000),
        interrupt(event::TIMER1_OVERFLOW),
    ]
}

fn receive_at_window_closes_mid_frame() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::ReceiveAt {
            time: 5_000,
            duration: 2_000,
        },
        interrupt(event::TIMER1_OVERFLOW),
        Step::Input("ieee802154_ll_is_current_rx_frame", 1),
        interrupt(event::TIMER1_OVERFLOW),
        Step::DeliverFrame(data_frame(false)),
        interrupt(event::RX_DONE),
    ]
}

fn configure_identity() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetPanId(0x1234),
        Step::SetShortAddress(0x5678),
        Step::SetExtendedAddress([1, 2, 3, 4, 5, 6, 7, 8]),
        Step::SetAckTimeout(200),
        Step::GetIdentity,
    ]
}

/// The 2006 data frame with security enabled and a frame-counter-suppressed
/// auxiliary security header in place of its payload byte.
fn secured_frame() -> Vec<u8> {
    let mut frame = data_frame(false);
    frame[1] |= 0x08;
    frame[10] = 0x25;
    frame
}

fn transmit_with_security() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetTransmitSecurity {
            frame: secured_frame(),
            key: [7; 16],
            address: [9; 8],
        },
        Step::Transmit {
            frame: secured_frame(),
            cca: false,
        },
        interrupt(event::TX_DONE),
    ]
}

/// Interface 0 owns PAN 0x1234 / short 0x0001, interface 1 PAN 0xabcd /
/// short 0x1111; interface 1 sets the pending bit for source 0x5678.
#[cfg(feature = "multipan")]
fn multipan_identities() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetMultipanPanId {
            index: 0,
            panid: 0x1234,
        },
        Step::SetMultipanShortAddress {
            index: 0,
            address: 0x0001,
        },
        Step::SetMultipanPanId {
            index: 1,
            panid: 0xabcd,
        },
        Step::SetMultipanShortAddress {
            index: 1,
            address: 0x1111,
        },
        Step::SetMultipanExtendedAddress {
            index: 1,
            address: [1, 2, 3, 4, 5, 6, 7, 8],
        },
        Step::MultipanSetPendingMode { index: 1, mode: 1 },
        Step::MultipanAddPendingAddress {
            index: 1,
            address: vec![0x78, 0x56],
            short: true,
        },
    ]
}

/// A received data frame requesting an ACK to short `destination` in PAN
/// `panid`, RSSI -55 and LQI 180.
#[cfg(feature = "multipan")]
fn received_frame_to(panid: u16, destination: [u8; 2]) -> Vec<u8> {
    let mut frame = received_data_frame();
    frame[4..6].copy_from_slice(&panid.to_le_bytes());
    frame[6..8].copy_from_slice(&destination);
    frame
}

#[cfg(feature = "multipan")]
fn multipan_receive_routes_to_interface() -> Vec<Step> {
    let mut steps = multipan_identities();
    steps.extend([
        Step::MultipanRxWhenIdle {
            index: 1,
            enable: true,
        },
        Step::MultipanReceive(1),
        Step::DeliverFrame(received_frame_to(0xabcd, [0x11, 0x11])),
        interrupt(event::RX_DONE),
        interrupt(event::ACK_TX_DONE),
        Step::DeliverFrame(received_frame_to(0x1234, [0x01, 0x00])),
        interrupt(event::RX_DONE),
        interrupt(event::ACK_TX_DONE),
    ]);
    steps
}

#[cfg(feature = "multipan")]
fn multipan_unmatched_and_broadcast() -> Vec<Step> {
    let mut steps = multipan_identities();
    steps.extend([
        Step::MultipanRxWhenIdle {
            index: 0,
            enable: true,
        },
        Step::MultipanReceive(0),
        Step::DeliverFrame(received_frame_to(0x9999, [0x11, 0x11])),
        interrupt(event::RX_DONE),
        interrupt(event::ACK_TX_DONE),
        Step::DeliverFrame(received_frame_to(0xabcd, [0xff, 0xff])),
        interrupt(event::RX_DONE),
    ]);
    steps
}

#[cfg(feature = "multipan")]
fn multipan_last_interface_sleeps() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetMultipanEnable(0),
        Step::MultipanReceive(0),
        Step::MultipanReceive(1),
        Step::MultipanSleep(0),
        Step::MultipanSleep(1),
        Step::MultipanRxWhenIdle {
            index: 1,
            enable: true,
        },
        Step::MultipanRxWhenIdle {
            index: 1,
            enable: false,
        },
    ]
}

fn sleep_after_receive() -> Vec<Step> {
    vec![Step::Enable, Step::Receive, Step::Sleep, Step::Sleep]
}

/// One transmission without acknowledgement, which publishes the pending PIB
/// and with it the resolved power index.
fn transmit_once() -> [Step; 2] {
    [
        Step::Transmit {
            frame: data_frame(false),
            cca: false,
        },
        Step::Interrupt {
            events: event::TX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
    ]
}

/// Transmit-power resolution against the recovered ESP32-S31 level set
/// (-24 to 21 dBm in 3 dB steps): the default highest level, a request below
/// the lowest level, between two levels, on a level and above the highest,
/// then a per-channel power on the current channel, one on another channel,
/// a change to that channel, and a per-channel power outside the channels.
fn transmit_power_levels() -> Vec<Step> {
    let mut steps = vec![Step::Enable];
    steps.extend(transmit_once());
    for power in [-30, 1, -3, 30] {
        steps.push(Step::SetTxPower(power));
        steps.extend(transmit_once());
    }
    steps.push(Step::SetChannel(20));
    steps.push(Step::SetPowerWithChannel {
        channel: 20,
        power: 5,
    });
    steps.extend(transmit_once());
    steps.push(Step::SetPowerWithChannel {
        channel: 11,
        power: -24,
    });
    steps.extend(transmit_once());
    steps.push(Step::SetChannel(11));
    steps.extend(transmit_once());
    steps.push(Step::SetPowerWithChannel {
        channel: 27,
        power: 0,
    });
    steps.extend(transmit_once());
    steps
}

fn rx_abort_event(reason: u8) -> Step {
    Step::Interrupt {
        events: event::RX_ABORT,
        rx_abort: reason,
        tx_abort: 0,
    }
}

fn tx_abort_event(reason: u8) -> Step {
    Step::Interrupt {
        events: event::TX_ABORT,
        rx_abort: 0,
        tx_abort: reason,
    }
}

/// Every receive-abort reason the driver's receive phase handles, each while
/// it receives with receive-when-idle: the stops and the ACK reasons end
/// without a next operation, the frame failures report and restart.
fn receive_abort_reasons() -> Vec<Step> {
    use rx_abort::*;
    let mut steps = vec![Step::Enable, Step::SetRxWhenIdle(true), Step::Receive];
    for reason in [
        RX_STOP,
        SFD_TIMEOUT,
        CRC_ERROR,
        INVALID_LEN,
        FILTER_FAIL,
        NO_RSS,
        COEX_BREAK,
        UNEXPECTED_ACK,
        RX_RESTART,
        TX_ACK_TIMEOUT,
        TX_ACK_STOP,
        TX_ACK_COEX_BREAK,
        ENHACK_SECURITY_ERROR,
        ED_STOP,
    ] {
        steps.push(rx_abort_event(reason));
    }
    steps
}

/// The receive-abort reasons the hardware raises while the driver transmits
/// the automatic ACK of a received frame. Both ISR phases see every abort,
/// and the receive phase rejects frame failures outside reception, so these
/// are the ACK failures, which deliver the frame and receive again, and the
/// stops, which end the phase silently and leave the driver in it; the
/// application then sleeps and receives again.
fn ack_transmit_abort_reasons() -> Vec<Step> {
    use rx_abort::*;
    let mut steps = vec![
        Step::Enable,
        Step::SetRxWhenIdle(true),
        Step::Receive,
        Step::Input("ieee802154_ll_get_tx_auto_ack", 1),
    ];
    for reason in [
        TX_ACK_TIMEOUT,
        TX_ACK_COEX_BREAK,
        ENHACK_SECURITY_ERROR,
        TX_ACK_STOP,
        RX_STOP,
        ED_STOP,
    ] {
        steps.push(Step::DeliverFrame(received_data_frame()));
        steps.push(interrupt(event::RX_DONE));
        steps.push(rx_abort_event(reason));
        if ![TX_ACK_TIMEOUT, TX_ACK_COEX_BREAK, ENHACK_SECURITY_ERROR].contains(&reason) {
            steps.push(Step::Sleep);
            steps.push(Step::Receive);
        }
    }
    steps
}

/// Every transmit-abort reason in the phase that raises it: while the
/// driver waits for the ACK, while it transmits, and while it assesses the
/// channel first.
fn transmit_abort_reasons() -> Vec<Step> {
    use tx_abort::*;
    let mut steps = vec![Step::Enable];
    for reason in [
        RX_ACK_STOP,
        RX_ACK_SFD_TIMEOUT,
        RX_ACK_CRC_ERROR,
        RX_ACK_INVALID_LEN,
        RX_ACK_FILTER_FAIL,
        RX_ACK_NO_RSS,
        RX_ACK_COEX_BREAK,
        RX_ACK_TYPE_NOT_ACK,
        RX_ACK_RESTART,
        RX_ACK_TIMEOUT,
    ] {
        steps.push(Step::Transmit {
            frame: data_frame(true),
            cca: false,
        });
        steps.push(interrupt(event::TX_DONE));
        steps.push(tx_abort_event(reason));
    }
    for (reason, cca) in [
        (TX_STOP, false),
        (TX_COEX_BREAK, false),
        (TX_SECURITY_ERROR, false),
        (TX_COEX_BREAK, true),
        (TX_SECURITY_ERROR, true),
        (CCA_FAILED, true),
        (CCA_BUSY, true),
    ] {
        steps.push(Step::Transmit {
            frame: data_frame(true),
            cca,
        });
        steps.push(tx_abort_event(reason));
    }
    steps
}

/// The energy-detection aborts of an energy scan and of a clear-channel
/// assessment.
fn energy_detection_abort_reasons() -> Vec<Step> {
    use rx_abort::*;
    let mut steps = vec![Step::Enable];
    for reason in [ED_ABORT, ED_COEX_REJECT] {
        steps.push(Step::EnergyDetect(8));
        steps.push(Step::Input("ieee802154_ll_get_rx_status", 0x0018_0000));
        steps.push(rx_abort_event(reason));
        steps.push(Step::Cca);
        steps.push(rx_abort_event(reason));
    }
    steps
}

fn enable() -> Vec<Step> {
    vec![Step::Enable]
}

/// Baseband receive-information images: the RSSI byte in the low eight bits
/// under nonzero upper bits, at both ends of the signed range.
pub const RECENT_RSSI_IMAGES: [(u64, i8); 4] = [
    (0xa5a5_12c6, -58),
    (0x0000_007f, 127),
    (0xffff_ff80, -128),
    (0x1234_5600, 0),
];

/// `esp_ieee802154_get_recent_rssi` disabled, idle, receiving, awaiting an
/// ACK and asleep, over each receive-information image.
fn recent_rssi() -> Vec<Step> {
    let reads = |steps: &mut Vec<Step>| {
        for (image, _) in RECENT_RSSI_IMAGES {
            steps.push(Step::Input("bt_bb_get_cur_rx_info", image));
            steps.push(Step::GetRecentRssi);
        }
    };
    let mut steps = Vec::new();
    reads(&mut steps);
    steps.push(Step::Enable);
    reads(&mut steps);
    steps.push(Step::Receive);
    reads(&mut steps);
    steps.push(Step::Transmit {
        frame: data_frame(true),
        cca: false,
    });
    steps.push(interrupt(event::TX_DONE));
    reads(&mut steps);
    steps.push(Step::Sleep);
    reads(&mut steps);
    steps
}

fn enable_disable() -> Vec<Step> {
    vec![Step::Enable, Step::Receive, Step::Disable]
}

fn transmit_without_ack() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Transmit {
            frame: data_frame(false),
            cca: false,
        },
        Step::Interrupt {
            events: event::TX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
    ]
}

fn transmit_with_ack() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Transmit {
            frame: data_frame(true),
            cca: false,
        },
        Step::Interrupt {
            events: event::TX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
        Step::DeliverFrame(received_ack()),
        Step::Interrupt {
            events: event::ACK_RX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
    ]
}

fn transmit_with_cca_busy() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Transmit {
            frame: data_frame(false),
            cca: true,
        },
        Step::Interrupt {
            events: event::TX_ABORT,
            rx_abort: 0,
            tx_abort: tx_abort::CCA_BUSY,
        },
    ]
}

fn receive_with_auto_ack() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetRxWhenIdle(true),
        Step::Receive,
        Step::Input("ieee802154_ll_get_tx_auto_ack", 1),
        Step::DeliverFrame(received_data_frame()),
        Step::Interrupt {
            events: event::RX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
        Step::Interrupt {
            events: event::ACK_TX_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
        Step::ReceiveHandleDone,
    ]
}

fn receive_crc_error_restarts() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::SetRxWhenIdle(true),
        Step::Receive,
        Step::Interrupt {
            events: event::RX_ABORT,
            rx_abort: rx_abort::CRC_ERROR,
            tx_abort: 0,
        },
    ]
}

fn energy_detect() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::EnergyDetect(8),
        Step::Input("ieee802154_ll_get_ed_rss", (-70i64) as u64),
        Step::Interrupt {
            events: event::ED_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
    ]
}

fn cca_busy() -> Vec<Step> {
    vec![
        Step::Enable,
        Step::Cca,
        Step::Input("ieee802154_ll_is_cca_busy", 1),
        Step::Interrupt {
            events: event::ED_DONE,
            rx_abort: 0,
            tx_abort: 0,
        },
    ]
}

/// Every catalog scenario.
pub const SCENARIOS: &[Scenario] = &[
    Scenario {
        name: "enable",
        inputs: no_inputs,
        steps: enable,
    },
    Scenario {
        name: "enable-disable",
        inputs: no_inputs,
        steps: enable_disable,
    },
    Scenario {
        name: "recent-rssi",
        inputs: no_inputs,
        steps: recent_rssi,
    },
    Scenario {
        name: "receive-abort-reasons",
        inputs: no_inputs,
        steps: receive_abort_reasons,
    },
    Scenario {
        name: "ack-transmit-abort-reasons",
        inputs: no_inputs,
        steps: ack_transmit_abort_reasons,
    },
    Scenario {
        name: "transmit-abort-reasons",
        inputs: no_inputs,
        steps: transmit_abort_reasons,
    },
    Scenario {
        name: "energy-detection-abort-reasons",
        inputs: no_inputs,
        steps: energy_detection_abort_reasons,
    },
    Scenario {
        name: "transmit-power-levels",
        inputs: no_inputs,
        steps: transmit_power_levels,
    },
    Scenario {
        name: "transmit-without-ack",
        inputs: no_inputs,
        steps: transmit_without_ack,
    },
    Scenario {
        name: "transmit-with-ack",
        inputs: no_inputs,
        steps: transmit_with_ack,
    },
    Scenario {
        name: "transmit-with-cca-busy",
        inputs: no_inputs,
        steps: transmit_with_cca_busy,
    },
    Scenario {
        name: "receive-with-auto-ack",
        inputs: no_inputs,
        steps: receive_with_auto_ack,
    },
    Scenario {
        name: "receive-crc-error-restarts",
        inputs: no_inputs,
        steps: receive_crc_error_restarts,
    },
    Scenario {
        name: "energy-detect",
        inputs: no_inputs,
        steps: energy_detect,
    },
    Scenario {
        name: "cca-busy",
        inputs: no_inputs,
        steps: cca_busy,
    },
    Scenario {
        name: "transmit-ack-timer-expires",
        inputs: no_inputs,
        steps: transmit_ack_timer_expires,
    },
    Scenario {
        name: "transmit-ack-abort-timeout",
        inputs: no_inputs,
        steps: transmit_ack_abort_timeout,
    },
    Scenario {
        name: "receive-with-enhanced-ack",
        inputs: enhanced_ack_inputs,
        steps: receive_with_enhanced_ack,
    },
    Scenario {
        name: "receive-with-secured-enhanced-ack",
        inputs: secured_enhanced_ack_inputs,
        steps: receive_with_enhanced_ack,
    },
    Scenario {
        name: "receive-2015-without-enhanced-ack",
        inputs: no_inputs,
        steps: receive_2015_without_enhanced_ack,
    },
    Scenario {
        name: "transmit-during-ack-fails",
        inputs: no_inputs,
        steps: transmit_during_ack_fails,
    },
    Scenario {
        name: "transmit-stops-receive",
        inputs: no_inputs,
        steps: transmit_stops_receive,
    },
    Scenario {
        name: "transmit-restarts-transmit",
        inputs: no_inputs,
        steps: transmit_restarts_transmit,
    },
    Scenario {
        name: "full-receive-ring-uses-the-stub",
        inputs: no_inputs,
        steps: full_receive_ring_uses_the_stub,
    },
    Scenario {
        name: "energy-detect-abort",
        inputs: no_inputs,
        steps: energy_detect_abort,
    },
    Scenario {
        name: "transmit-at",
        inputs: no_inputs,
        steps: transmit_at,
    },
    Scenario {
        name: "receive-at-window-closes",
        inputs: no_inputs,
        steps: receive_at_window_closes,
    },
    Scenario {
        name: "receive-at-window-closes-mid-frame",
        inputs: no_inputs,
        steps: receive_at_window_closes_mid_frame,
    },
    Scenario {
        name: "configure-identity",
        inputs: no_inputs,
        steps: configure_identity,
    },
    Scenario {
        name: "transmit-with-security",
        inputs: no_inputs,
        steps: transmit_with_security,
    },
    Scenario {
        name: "sleep-after-receive",
        inputs: no_inputs,
        steps: sleep_after_receive,
    },
    #[cfg(feature = "multipan")]
    Scenario {
        name: "multipan-receive-routes-to-interface",
        inputs: no_inputs,
        steps: multipan_receive_routes_to_interface,
    },
    #[cfg(feature = "multipan")]
    Scenario {
        name: "multipan-unmatched-and-broadcast",
        inputs: no_inputs,
        steps: multipan_unmatched_and_broadcast,
    },
    #[cfg(feature = "multipan")]
    Scenario {
        name: "multipan-last-interface-sleeps",
        inputs: no_inputs,
        steps: multipan_last_interface_sleeps,
    },
];

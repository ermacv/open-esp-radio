use super::*;

fn ht(bits: u16, short: bool) -> PpduTiming {
    PpduTiming::BccOfdm {
        data_bits_per_symbol: NonZeroU16::new(bits).unwrap(),
        symbol_nanos: NonZeroU16::new(if short { 3600 } else { 4000 }).unwrap(),
        preamble_micros: 36,
        tail_bits: 6,
        signal_extension_micros: 6,
    }
}
fn basic() -> PpduTiming {
    PpduTiming::BccOfdm {
        data_bits_per_symbol: NonZeroU16::new(24).unwrap(),
        symbol_nanos: NonZeroU16::new(4000).unwrap(),
        preamble_micros: 20,
        tail_bits: 6,
        signal_extension_micros: 6,
    }
}

#[test]
fn ht_counts_preamble_once_and_rounds_whole_symbols() {
    // 1500-byte HT20 MCS7: ceil((16 + 12000 + 6)/260) = 47 symbols.
    assert_eq!(ht(260, false).duration_micros(1500), 230);
    assert_eq!(ht(260, true).duration_micros(1500), 212);
    // One HT40 aggregate: ceil(480022 / 540) = 889 symbols.
    assert_eq!(ht(540, true).duration_micros(60000), 3243);
    assert!(ht(540, true).duration_micros(3000) < 2 * ht(540, true).duration_micros(1500));
}

#[test]
fn ack_block_ack_protection_and_selected_contention_are_separate() {
    let cost = TxCost::estimate(
        1500,
        Some(ht(260, false)),
        10,
        TxResponse::Ack(basic()),
        TxProtection::None,
        Some(TxAccess {
            contention: TxContention {
                aifsn: 3,
                backoff_slots: 7,
            },
            slot_micros: 9,
        }),
    );
    assert_eq!(cost.ppdu_micros, Some(230));
    assert_eq!(cost.response_micros, Some(60)); // SIFS + 6-Mbit/s ACK
    assert_eq!(cost.access_micros, Some(100));
    assert_eq!(cost.exchange_micros(), Some(290));
    assert_eq!(cost.service_micros(), Some(390));
    let protected = TxCost::estimate(
        1500,
        Some(ht(260, false)),
        10,
        TxResponse::BlockAck {
            timing: basic(),
            psdu_bytes: NonZeroU16::new(32).unwrap(),
        },
        TxProtection::RtsCts {
            rts: basic(),
            cts: basic(),
        },
        None,
    );
    assert_eq!(protected.response_micros, Some(84));
    assert_eq!(protected.protection_micros, Some(128));
    assert_eq!(protected.exchange_micros(), Some(442));
    assert_eq!(protected.service_micros(), None);
}

#[test]
fn unknown_is_not_zero_and_overflow_cannot_reduce_cost() {
    let unknown = TxCost::estimate(100, None, 10, TxResponse::Unknown, TxProtection::None, None);
    assert_eq!(unknown.ppdu_micros, None);
    assert_eq!(unknown.exchange_micros(), None);
    let overflow = TxCost {
        ppdu_micros: Some(u32::MAX),
        response_micros: Some(1),
        protection_micros: Some(0),
        access_micros: Some(0),
    };
    assert_eq!(overflow.exchange_micros(), None);
    let access = TxCost::estimate(
        100,
        Some(basic()),
        u16::MAX,
        TxResponse::None,
        TxProtection::None,
        Some(TxAccess {
            contention: TxContention {
                aifsn: u8::MAX,
                backoff_slots: u16::MAX,
            },
            slot_micros: u16::MAX,
        }),
    );
    assert_eq!(access.access_micros, None);
}

#[test]
fn group_frame_has_no_reply_and_timeout_is_an_explicit_wait_budget() {
    let phy = PpduTiming::Dsss {
        bitrate_kbps: NonZeroU32::new(1000).unwrap(),
        preamble_micros: 192,
    };
    assert_eq!(phy.duration_micros(100), 992);
    let group = TxCost::estimate(
        100,
        Some(phy),
        10,
        TxResponse::None,
        TxProtection::None,
        None,
    );
    assert_eq!(group.exchange_micros(), Some(992));
    let failed = TxCost::estimate(
        100,
        Some(phy),
        10,
        TxResponse::Timeout { micros: 500 },
        TxProtection::None,
        None,
    );
    assert_eq!(failed.exchange_micros(), Some(1492));
}

mod budget;

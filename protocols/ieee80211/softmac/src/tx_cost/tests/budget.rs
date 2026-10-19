use super::*;

#[test]
fn inversion_fits_and_the_next_byte_exceeds_each_finite_budget() {
    for timing in [
        basic(),
        ht(26, false),
        ht(260, true),
        ht(540, true),
        PpduTiming::Dsss {
            bitrate_kbps: NonZeroU32::new(5500).unwrap(),
            preamble_micros: 96,
        },
    ] {
        for micros in 0..6000 {
            match timing.maximum_psdu_bytes(micros) {
                Some(bytes) => {
                    assert!(timing.duration_micros(bytes.get()) <= micros);
                    if let Some(next) = bytes.get().checked_add(1) {
                        assert!(timing.duration_micros(next) > micros);
                    }
                }
                None => assert!(timing.duration_micros(1) > micros),
            }
        }
        assert_eq!(timing.maximum_psdu_bytes(u32::MAX).unwrap().get(), u16::MAX);
    }
}

#[test]
fn extreme_timing_inputs_do_not_overflow_the_inverse() {
    for timing in [
        PpduTiming::Dsss {
            bitrate_kbps: NonZeroU32::new(u32::MAX).unwrap(),
            preamble_micros: u16::MAX,
        },
        PpduTiming::BccOfdm {
            data_bits_per_symbol: NonZeroU16::new(u16::MAX).unwrap(),
            symbol_nanos: NonZeroU16::new(1).unwrap(),
            preamble_micros: u16::MAX,
            tail_bits: u8::MAX,
            signal_extension_micros: u8::MAX,
        },
    ] {
        assert_eq!(timing.maximum_psdu_bytes(0), None);
        assert_eq!(timing.maximum_psdu_bytes(u32::MAX).unwrap().get(), u16::MAX);
    }
}

#[test]
fn exchange_reserves_reply_and_protection_before_assigning_payload() {
    let phy = ht(540, true);
    let ba = TxResponse::BlockAck {
        timing: basic(),
        psdu_bytes: NonZeroU16::new(32).unwrap(),
    };
    for (reply, protection) in [
        (TxResponse::None, TxProtection::None),
        (TxResponse::Ack(basic()), TxProtection::None),
        (ba, TxProtection::None),
        (
            ba,
            TxProtection::RtsCts {
                rts: basic(),
                cts: basic(),
            },
        ),
        (TxResponse::Timeout { micros: 100 }, TxProtection::None),
    ] {
        let bytes = TxCost::maximum_psdu_bytes(1000, Some(phy), 10, reply, protection)
            .unwrap()
            .get();
        let cost = |len| {
            TxCost::estimate(len, Some(phy), 10, reply, protection, None)
                .exchange_micros()
                .unwrap()
        };
        assert!(cost(bytes) <= 1000);
        assert!(cost(bytes + 1) > 1000);
    }
}

#[test]
fn incomplete_or_exhausted_budget_cannot_admit_a_packet() {
    for (phy, reply, protection) in [
        (None, TxResponse::None, TxProtection::None),
        (Some(basic()), TxResponse::Unknown, TxProtection::None),
        (Some(basic()), TxResponse::None, TxProtection::Unknown),
        (
            Some(basic()),
            TxResponse::Timeout { micros: u32::MAX },
            TxProtection::None,
        ),
    ] {
        assert_eq!(
            TxCost::maximum_psdu_bytes(u32::MAX, phy, 10, reply, protection),
            None
        );
    }
    assert_eq!(
        TxCost::maximum_psdu_bytes(
            60,
            Some(basic()),
            10,
            TxResponse::Ack(basic()),
            TxProtection::None
        ),
        None
    );
}

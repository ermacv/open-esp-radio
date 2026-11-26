//! The cost model's figures, by hand from the standard's TXTIME.

use super::*;
use crate::model::installed;
use core::num::{NonZeroU8, NonZeroU16};
use oer_protocols_ieee80211_contracts_port::{
    DataPort, IndeterminateCause, Protection, RetryStage, SingleResponse,
};

/// TXTIME by hand, IEEE Std 802.11-2020 15.4.3 and 17.4.3: DSSS long
/// preamble 192 us and 8 us an octet at 1 Mb/s; 6 Mb/s OFDM 20 us preamble,
/// 4 us symbols of 24 bits after 16 SERVICE and 6 tail bits, 6 us signal
/// extension at 2.4 GHz.
const ACK_AT_1M: u32 = 192 + 14 * 8;
const ACK_AT_6M: u32 = 20 + 4 * (16 + 14 * 8 + 6_u32).div_ceil(24) + 6;
const BLOCK_ACK_AT_6M: u32 = 20 + 4 * (16 + 32 * 8 + 6_u32).div_ceil(24) + 6;

/// A 30-octet MPDU and its FCS.
const PSDU: u32 = 30 + 4;

/// The PPDU of [`PSDU`] at an OFDM rate of `bits_per_symbol`.
const fn ppdu(bits_per_symbol: u32) -> u32 {
    20 + 4 * (16 + 8 * PSDU + 6).div_ceil(bits_per_symbol) + 6
}

const fn air(transmissions: u16, rate: PhyRate) -> Air {
    Air {
        transmissions: match NonZeroU16::new(transmissions) {
            Some(count) => count,
            None => panic!("a transmission"),
        },
        rate,
    }
}

const fn legacy(rate: LegacyRate) -> PhyRate {
    PhyRate::Legacy(rate)
}

#[test]
fn a_response_is_its_sifs_and_its_frame_at_the_declared_rate() {
    let dsss = legacy(LegacyRate::Cck11M(
        oer_protocols_ieee80211_wire::phy::DsssPreamble::Long,
    ));
    let ofdm = legacy(LegacyRate::Ofdm54M);
    assert_eq!(response_micros(dsss, Response::Ack), Some(10 + ACK_AT_1M));
    assert_eq!(response_micros(ofdm, Response::Ack), Some(10 + ACK_AT_6M));
    assert_eq!(
        response_micros(ofdm, Response::BlockAck),
        Some(10 + BLOCK_ACK_AT_6M)
    );
    assert_eq!(response_micros(ofdm, Response::None), Some(0));
}

#[test]
fn a_publication_costs_its_own_ppdu_and_response_only() {
    installed(|model| {
        let rate = model.port.rate(legacy(LegacyRate::Ofdm24M)).unwrap();
        let plan = RetryPlan::new(RetryStage {
            rate,
            transmissions: NonZeroU8::MIN,
            protection: Protection::None,
        });
        let outcome: SingleOutcome<(), ()> = TxOutcome::Ended(SingleEnd::Delivered {
            air: air(1, rate.get()),
            response: SingleResponse::Ack(()),
        });
        // 24 Mb/s: 96 bits a symbol; 16 + 8000 + 6 bits take 84 symbols.
        let ppdu = 20 + 4 * 84 + 6;
        assert_eq!(
            single_cost(&plan, 1_000, &outcome),
            TxEstimate::Estimated(oer_portable_time::Duration::from_micros(u64::from(
                ppdu + 10 + ACK_AT_6M
            )))
        );
    });
}

#[test]
fn a_publication_the_model_cannot_time_is_unknown() {
    installed(|model| {
        // At 1 Mb/s a PSDU of `u32::MAX` octets leaves a `u32` of
        // microseconds.
        let slow = model.port.rate(legacy(LegacyRate::Dsss1M)).unwrap();
        let plan = RetryPlan::new(RetryStage {
            rate: slow,
            transmissions: NonZeroU8::MIN,
            protection: Protection::None,
        });
        let outcome: SingleOutcome<(), ()> =
            TxOutcome::Ended(SingleEnd::AckTimeout(air(1, slow.get())));
        assert_eq!(single_cost(&plan, u32::MAX, &outcome), TxEstimate::Unknown);
        let aggregate: AggregateOutcome<(), ()> =
            TxOutcome::Ended(AggregateEnd::AckTimeout(air(1, slow.get())));
        assert_eq!(aggregate_cost(u32::MAX, &aggregate), TxEstimate::Unknown);
    });
}

/// A single MPDU's plan that falls back from 24 to 6 Mb/s: its attempt
/// costs each transmission at the rate it went at, PPDU, SIFS and the Ack
/// the model declares, summed; never the last rate times the count.
#[test]
fn a_single_arena_s_cost_sums_each_publication_at_its_own_rate() {
    installed(|model| {
        let fast = model.port.rate(legacy(LegacyRate::Ofdm24M)).unwrap();
        let slow = model.port.rate(legacy(LegacyRate::Ofdm6M)).unwrap();
        let plan = RetryPlan::new(RetryStage {
            rate: fast,
            transmissions: NonZeroU8::MIN,
            protection: Protection::None,
        })
        .then(RetryStage {
            rate: slow,
            transmissions: NonZeroU8::MIN,
            protection: Protection::None,
        })
        .unwrap_or_else(|_| panic!("two stages"));
        let outcome: SingleOutcome<(), ()> = TxOutcome::Ended(SingleEnd::Delivered {
            air: air(2, slow.get()),
            response: SingleResponse::Ack(()),
        });
        let ack = 10 + ACK_AT_6M;
        assert_eq!(
            single_cost(&plan, PSDU, &outcome),
            TxEstimate::Estimated(oer_portable_time::Duration::from_micros(u64::from(
                ppdu(96) + ack + ppdu(24) + ack
            )))
        );
    });
}

/// A transmission that never reached the air (a collision ended it before
/// its PPDU) adds nothing, and what the attempt's earlier transmission cost
/// stays: one PPDU, SIFS and Ack.
#[test]
fn a_publication_that_never_reached_the_air_adds_no_cost_and_keeps_the_earlier() {
    installed(|model| {
        let rate = model.port.rate(legacy(LegacyRate::Ofdm24M)).unwrap();
        let plan = RetryPlan::new(RetryStage {
            rate,
            transmissions: NonZeroU8::new(2).unwrap(),
            protection: Protection::None,
        });
        let outcome: SingleOutcome<(), ()> =
            TxOutcome::Ended(SingleEnd::ContentionLimit(Some(air(1, rate.get()))));
        assert_eq!(
            single_cost(&plan, PSDU, &outcome),
            TxEstimate::Estimated(oer_portable_time::Duration::from_micros(u64::from(
                ppdu(96) + 10 + ACK_AT_6M
            )))
        );
    });
}

/// An attempt whose end is not known may have reached the air: its cost is
/// unknown, never zero; one withdrawn before the air costs nothing.
#[test]
fn an_aborted_publication_makes_its_attempt_s_cost_unknown() {
    installed(|model| {
        let rate = model.port.rate(legacy(LegacyRate::Ofdm24M)).unwrap();
        let plan = RetryPlan::new(RetryStage {
            rate,
            transmissions: NonZeroU8::MIN,
            protection: Protection::None,
        });
        let aborted: SingleOutcome<(), ()> =
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout);
        assert_eq!(single_cost(&plan, PSDU, &aborted), TxEstimate::Unknown);
        let aggregate: AggregateOutcome<(), ()> =
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout);
        assert_eq!(aggregate_cost(PSDU, &aggregate), TxEstimate::Unknown);
        let withdrawn: SingleOutcome<(), ()> =
            TxOutcome::Withdrawn(oer_protocols_ieee80211_contracts_port::WithdrawCause::Disabled);
        assert_eq!(single_cost(&plan, PSDU, &withdrawn), TxEstimate::NONE);
    });
}

/// An aggregate's transmissions each cost its PPDU at the aggregate's rate,
/// a SIFS and a BlockAck.
#[test]
fn an_aggregate_costs_each_transmission_with_its_block_ack() {
    let rate = legacy(LegacyRate::Ofdm24M);
    let outcome: AggregateOutcome<(), ()> =
        TxOutcome::Ended(AggregateEnd::AckTimeout(air(2, rate)));
    assert_eq!(
        aggregate_cost(PSDU, &outcome),
        TxEstimate::Estimated(oer_portable_time::Duration::from_micros(u64::from(
            2 * (ppdu(96) + 10 + BLOCK_ACK_AT_6M)
        )))
    );
}

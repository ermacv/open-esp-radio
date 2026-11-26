//! What one attempt of the network's data costs on the air, by the declared
//! model ([`TxEstimate`]), which the fair queues charge.
//!
//! The model: each transmission an attempt made costs its own PPDU at the
//! rate it went at, one SIFS and the response it solicits at the lowest
//! mandatory rate of its modulation class. AIFS, backoff and CCA waits are
//! not in it, nor a transmission that never reached the air: it is not a
//! measured medium occupancy or a frame's service time. A single's
//! transmissions go at the rates of its plan's stages in order, as many as
//! the outcome's [`Air`] counts; an aggregate's at its rate. Where a rate
//! cannot be timed, or the outcome does not say whether the air was
//! reached, the cost is unknown, never zero.

use oer_protocols_ieee80211_contracts_port::{
    AggregateEnd, AggregateOutcome, Air, PhyRate, PortRate, RetryPlan, SingleEnd, SingleOutcome,
    TxEstimate, TxOutcome,
};
use oer_protocols_ieee80211_wire::phy::LegacyRate;

/// The response a transmission solicits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Response {
    /// None.
    None,
    /// An Ack.
    Ack,
    /// A compressed BlockAck.
    BlockAck,
}

/// SIFS at 2.4 GHz.
///
/// SOURCE: IEEE Std 802.11-2020 Table 15-5 (DSSS), Table 18-5 (ERP) and
/// Table 19-25 (HT, 2.4 GHz) give aSIFSTime 10 us; Table 27-54 keeps it for
/// HE at 2.4 GHz.
pub const SIFS_MICROS: u32 = 10;

/// An Ack, its FCS included.
///
/// SOURCE: IEEE Std 802.11-2020 9.3.1.3 (Frame Control, Duration, RA, FCS).
const ACK_OCTETS: u32 = 14;

/// A compressed BlockAck of a 64-frame window, its FCS included.
///
/// SOURCE: IEEE Std 802.11-2020 9.3.1.8.2 (Frame Control, Duration, RA, TA,
/// BA Control, Starting Sequence Control, an 8-octet bitmap, FCS).
const BLOCK_ACK_OCTETS: u32 = 32;

/// The response's SIFS and frame, at the lowest mandatory rate of the
/// eliciting PPDU's modulation class: 1 Mb/s DSSS long preamble after
/// DSSS/CCK, 6 Mb/s OFDM non-HT after OFDM, HT and HE. The access point's
/// real response rate is at least that, so the model errs long. `None`
/// for a rate the model cannot time.
///
/// SOURCE: IEEE Std 802.11-2020 10.6.6.5.2 (a control response goes at a
/// basic rate of the eliciting frame's modulation class), 15.4.3 and 17.4.3
/// (TXTIME).
pub fn response_micros(rate: PhyRate, response: Response) -> Option<u32> {
    let octets = match response {
        Response::None => return Some(0),
        Response::Ack => ACK_OCTETS,
        Response::BlockAck => BLOCK_ACK_OCTETS,
    };
    let basic = match rate {
        PhyRate::Legacy(rate) if !rate.is_ofdm() => LegacyRate::Dsss1M,
        PhyRate::Legacy(_) | PhyRate::Ht(_) | PhyRate::He(_) => LegacyRate::Ofdm6M,
    };
    PhyRate::Legacy(basic)
        .max_ppdu_duration(octets)
        .and_then(micros)?
        .checked_add(SIFS_MICROS)
}

/// One transmission of `psdu_octets` at `rate` and its response.
pub fn transmission_micros(rate: PhyRate, psdu_octets: u32, response: Response) -> Option<u32> {
    micros(rate.max_ppdu_duration(psdu_octets)?)?.checked_add(response_micros(rate, response)?)
}

/// The microseconds of `span`: the model's sums stay within a `u32`, as
/// [`PhyRate::max_ppdu_duration`] bounds its own; `None` past it.
fn micros(span: oer_portable_time::Duration) -> Option<u32> {
    u32::try_from(span.as_micros()).ok()
}

/// `micros` as an estimate: unknown when the model could not time it.
const fn estimate(micros: Option<u32>) -> TxEstimate {
    match micros {
        Some(micros) => {
            TxEstimate::Estimated(oer_portable_time::Duration::from_micros(micros as u64))
        }
        None => TxEstimate::Unknown,
    }
}

/// What a single's attempt of `psdu_octets` cost, by `plan` and its
/// outcome: its transmissions each at its stage's rate, each with its Ack.
pub fn single_cost<K, A, D>(
    plan: &RetryPlan<PortRate<'_, K>>,
    psdu_octets: u32,
    outcome: &SingleOutcome<A, D>,
) -> TxEstimate {
    let air = match outcome {
        TxOutcome::Ended(
            SingleEnd::Delivered { air, .. }
            | SingleEnd::AckTimeout(air)
            | SingleEnd::CtsTimeout(Some(air))
            | SingleEnd::ContentionLimit(Some(air))
            | SingleEnd::AirDenied { air: Some(air), .. },
        ) => *air,
        TxOutcome::Ended(
            SingleEnd::CtsTimeout(None)
            | SingleEnd::ContentionLimit(None)
            | SingleEnd::AirDenied { air: None, .. },
        )
        | TxOutcome::Expired { .. }
        | TxOutcome::Withdrawn(_) => return TxEstimate::NONE,
        TxOutcome::Indeterminate(_) => return TxEstimate::Unknown,
    };
    let rates = plan.stages().flat_map(|stage| {
        core::iter::repeat_n(stage.rate.get(), usize::from(stage.transmissions.get()))
    });
    rates
        .take(usize::from(air.transmissions.get()))
        .fold(TxEstimate::NONE, |cost, rate| {
            cost.add(estimate(transmission_micros(
                rate,
                psdu_octets,
                Response::Ack,
            )))
        })
}

/// What an aggregate's attempt of `psdu_octets` cost, by its outcome: its
/// transmissions at its rate, each with its BlockAck.
pub fn aggregate_cost<B, D>(psdu_octets: u32, outcome: &AggregateOutcome<B, D>) -> TxEstimate {
    let air: Air = match outcome {
        TxOutcome::Ended(
            AggregateEnd::Delivered { air, .. }
            | AggregateEnd::AckTimeout(air)
            | AggregateEnd::CtsTimeout(Some(air))
            | AggregateEnd::ContentionLimit(Some(air))
            | AggregateEnd::AirDenied { air: Some(air), .. },
        ) => *air,
        TxOutcome::Ended(
            AggregateEnd::CtsTimeout(None)
            | AggregateEnd::ContentionLimit(None)
            | AggregateEnd::AirDenied { air: None, .. },
        )
        | TxOutcome::Expired { .. }
        | TxOutcome::Withdrawn(_) => return TxEstimate::NONE,
        TxOutcome::Indeterminate(_) => return TxEstimate::Unknown,
    };
    let once = transmission_micros(air.rate, psdu_octets, Response::BlockAck);
    estimate(once.and_then(|micros| micros.checked_mul(u32::from(air.transmissions.get()))))
}

#[cfg(test)]
mod tests;

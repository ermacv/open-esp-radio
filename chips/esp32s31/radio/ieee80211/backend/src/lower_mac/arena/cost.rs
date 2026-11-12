//! What a publication costs by the arena's declared model.
//!
//! The model ([`TxEstimate`]): for each publication that reached the air,
//! its modelled PPDU duration ([`MacTxWork`] and its [`PpduTiming`]), one
//! SIFS and the response it solicits at the lowest mandatory rate of its
//! modulation class. AIFS, backoff and CCA waits are not in it, nor
//! publications that never reached the air: it is not a measured medium
//! occupancy or a frame's service time. That each publication is one PPDU
//! on the air, with no transmission the MAC repeats unseen inside it, is an
//! unverified assumption, which bounds how far the model follows the air.

use core::num::{NonZeroU16, NonZeroU32};

use oer_protocols_ieee80211_contracts_port::TxEstimate;

use crate::mac::tx::TxPhyRate;
use crate::mac::tx_cost::{PpduTiming, TxCost, TxProtectionTiming, TxResponse};
use crate::mac::tx_work::MacTxWork;

/// The response a publication solicits, in the cost model.
#[derive(Clone, Copy)]
pub(crate) enum Response {
    None,
    Ack,
    BlockAck,
}

/// SIFS at 2.4 GHz.
///
/// SOURCE: IEEE Std 802.11-2020 Table 15-5 (DSSS), Table 18-5 (ERP) and
/// Table 19-25 (HT, 2.4 GHz) give aSIFSTime 10 us; Table 27-54 keeps it for
/// HE at 2.4 GHz.
pub(super) const SIFS_MICROS: u16 = 10;

/// A compressed BlockAck of a 64-frame window, its FCS included; an Ack is
/// the 14 octets [`TxCost::estimate`](crate::mac::tx_cost::TxCost::estimate)
/// times.
///
/// SOURCE: IEEE Std 802.11-2020 9.3.1.3 (Ack: Frame Control, Duration, RA,
/// FCS) and 9.3.1.8.2 (Compressed BlockAck: Frame Control, Duration, RA,
/// TA, BA Control, Starting Sequence Control, an 8-octet bitmap, FCS).
const BLOCK_ACK_OCTETS: u16 = 32;

/// The declared response model: the response at the lowest mandatory rate
/// of the eliciting PPDU's modulation class, 1 Mb/s DSSS long preamble after
/// DSSS/CCK and 6 Mb/s OFDM non-HT after OFDM, HT and HE. The access
/// point's real response rate is at least that, so the model errs long.
///
/// SOURCE: IEEE Std 802.11-2020 10.6.6.5.2 (a control response goes at a
/// basic rate of the eliciting frame's modulation class), 15.4.3 and 17.4.3
/// (TXTIME), 17.3.2 Table 17-4 (6 Mb/s: 24 data bits per symbol).
fn response_micros(rate: TxPhyRate, response: Response) -> Option<u32> {
    let dsss = matches!(rate.ppdu_timing(), Some(PpduTiming::Dsss { .. }));
    let timing = if dsss {
        PpduTiming::Dsss {
            bitrate_kbps: const {
                match NonZeroU32::new(1_000) {
                    Some(kbps) => kbps,
                    None => panic!("1 Mb/s is not zero"),
                }
            },
            preamble_micros: 192,
        }
    } else {
        PpduTiming::BccOfdm {
            data_bits_per_symbol: const {
                match NonZeroU16::new(24) {
                    Some(bits) => bits,
                    None => panic!("6 Mb/s carries bits"),
                }
            },
            symbol_nanos: const {
                match NonZeroU16::new(4_000) {
                    Some(nanos) => nanos,
                    None => panic!("an OFDM symbol lasts"),
                }
            },
            preamble_micros: 20,
            tail_bits: 6,
            signal_extension_micros: 6,
        }
    };
    let response = match response {
        Response::None => TxResponse::None,
        Response::Ack => TxResponse::Ack(timing),
        Response::BlockAck => TxResponse::BlockAck {
            timing,
            psdu_bytes: const {
                match NonZeroU16::new(BLOCK_ACK_OCTETS) {
                    Some(octets) => octets,
                    None => panic!("a BlockAck has octets"),
                }
            },
        },
    };
    TxCost::estimate(
        0,
        None,
        SIFS_MICROS,
        response,
        TxProtectionTiming::None,
        None,
    )
    .response_micros
}

/// What one publication costs by the model ([`TxEstimate`]): the work its
/// submission recorded between the two snapshots, its modelled PPDU, plus
/// [`response_micros`]. A publication the model cannot time is unknown.
pub(crate) fn publication_cost(
    before: Option<MacTxWork>,
    after: Option<MacTxWork>,
    rate: TxPhyRate,
    response: Response,
) -> TxEstimate {
    let (Some(before), Some(after)) = (before, after) else {
        return TxEstimate::Unknown;
    };
    let Some(overhead) = response_micros(rate, response) else {
        return TxEstimate::Unknown;
    };
    after
        .since(before)
        .estimated_exchange_micros(overhead)
        .map_or(TxEstimate::Unknown, |micros| TxEstimate::Estimated {
            micros: micros.get(),
        })
}

#[cfg(test)]
mod tests;

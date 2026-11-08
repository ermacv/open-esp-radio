//! What a publication costs by the arena's declared model, and the most an
//! MSDU can cost by it on the path this backend executes.
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
use oer_protocols_ieee80211_wire::phy::{
    FecCoding, HeGiLtf, HeMcs, HeRate, HtMcs, HtRate, LegacyRate, PhyRate, PpduBandwidth,
    SpatialStreams,
};

use crate::mac::tx::TxPhyRate;
use crate::mac::tx_cost::{PpduTiming, TxCost, TxProtectionTiming, TxResponse};
use crate::mac::tx_work::MacTxWork;

/// The response a publication solicits, in the cost model.
#[derive(Clone, Copy)]
pub(super) enum Response {
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
pub(super) fn publication_cost(
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

/// The longest Ethernet header a network frame's length counts.
const ETHERNET_HEADER: usize = 14;

/// What a data MPDU adds to its Ethernet frame's payload on the air: the
/// QoS Data header, the CCMP header and LLC/SNAP the data plane writes
/// before the payload, an HT/HE Control field the MAC may insert, the CCMP
/// MIC and the FCS.
///
/// SOURCE: IEEE Std 802.11-2020 9.3.2.1 (QoS Data header), 9.2.4.6 (HT
/// Control, 4 octets), 12.5.3.2 (CCMP header and MIC); the data plane's
/// `MSDU_PAYLOAD_OFFSET` (`roles/sta` `data.rs`) is the same sum before the
/// payload.
const MPDU_GROWTH: usize = oer_protocols_ieee80211_wire::data::IEEE80211_QOS_DATA_HEADER_LEN
    + oer_protocols_ieee80211_wire::ccmp::CCMP_HEADER_LEN
    + oer_protocols_ieee80211_wire::data::LLC_SNAP_HEADER_LEN
    + 4
    + crate::ordinary_tx::TX_CCMP_MIC_SIZE
    + crate::ordinary_tx::TX_FCS_SIZE;

/// The slowest single MPDU the data plans publish: DSSS 1 Mb/s, long
/// preamble, the last rate every rate schedule walks down to.
///
/// SOURCE: the data plans' rate walk (`rate_selection::walk` over
/// `ordinary_retry_rate`, `oer-chips-family-radio-ieee80211-policy`), whose
/// HT, 11g and 11b records end at rate code 0x00 (`rate_control.rs`,
/// `rate_schedule.rs`; vendor `libpp[trc.o]::rcGetRate`).
const SLOWEST_SINGLE: PhyRate = PhyRate::Legacy(LegacyRate::Dsss1M);

/// The slowest HE aggregate this backend may publish: HE20 SU, one stream,
/// MCS 0 at the longest HE-LTF and guard interval, each coding, no DCM.
const fn slowest_he(fec: FecCoding) -> Option<PhyRate> {
    let Some(mcs) = HeMcs::new(0) else {
        return None;
    };
    match HeRate::new(
        mcs,
        SpatialStreams::ONE,
        PpduBandwidth::Mhz20,
        HeGiLtf::Ltf4xGi3200Ns,
        fec,
        false,
    ) {
        Some(rate) => Some(PhyRate::He(rate)),
        None => None,
    }
}

/// The longest aggregate PPDU this backend publishes, by mode, signal
/// extension included:
/// - HT: the data plane admits an aggregate only within its rate's TXTIME
///   limit ([`PhyRate::max_txtime_micros`], 5484 us from the L-SIG LENGTH,
///   `roles/sta` `data/aggregate.rs`), which leaves out the 6 us signal
///   extension;
/// - HE: no time limit is applied, so the bound is the longest PPDU of
///   [`ESP32S31_HE_AMPDU_MAX_LENGTH`](super::super::ampdu::ESP32S31_HE_AMPDU_MAX_LENGTH)
///   octets, the most an HE aggregate holds here, at the slowest HE rate
///   ([`PhyRate::max_ppdu_duration_micros`], the longest packet extension
///   included).
fn longest_aggregate_micros() -> Option<u32> {
    let ht = HtRate::new(HtMcs::new(0)?, PpduBandwidth::Mhz20, false)
        .map(PhyRate::Ht)?
        .max_txtime_micros()?
        .checked_add(6)?;
    let octets = u32::from(super::super::ampdu::ESP32S31_HE_AMPDU_MAX_LENGTH);
    let he = [FecCoding::Bcc, FecCoding::Ldpc]
        .into_iter()
        .map(|fec| slowest_he(fec)?.max_ppdu_duration_micros(octets))
        .try_fold(0_u32, |most, duration| Some(most.max(duration?)))?;
    Some(ht.max(he))
}

/// The most an MSDU whose Ethernet frame holds `frame_octets` can cost by
/// the model ([`TxEstimate`]) on the path this backend executes, for an
/// egress policy to charge where the cost is not known. It is a margin, not
/// a statement of accuracy:
/// - up to `aggregate_attempts` aggregate PPDUs on the air, each charged
///   whole ([`longest_aggregate_micros`]) with SIFS and its BlockAck, since
///   an MSDU's part of one is at most the whole;
/// - then, its aggregate gone to a single MPDU, up to `single_publications`
///   publications at the slowest single rate, each with SIFS and its Ack.
///
/// The caller names the executed bounds: the data plane's attempts of an
/// aggregate member (`AggregateRetry::attempts`), and the largest
/// publication budget of any rate record a single plan walks. `None` when
/// the bound does not fit a `u32`.
pub fn msdu_cost_ceiling_micros(
    frame_octets: usize,
    aggregate_attempts: u8,
    single_publications: u8,
) -> Option<NonZeroU32> {
    let block_ack = response_micros(
        TxPhyRate::Legacy(oer_chips_esp32s31_hal::types::MacLegacyRate::Ofdm6M),
        Response::BlockAck,
    )?;
    let aggregate = longest_aggregate_micros()?.checked_add(block_ack)?;
    let mpdu = frame_octets
        .checked_sub(ETHERNET_HEADER)?
        .checked_add(MPDU_GROWTH)?;
    let ack = response_micros(
        TxPhyRate::Legacy(oer_chips_esp32s31_hal::types::MacLegacyRate::Dsss1MLong),
        Response::Ack,
    )?;
    let single = SLOWEST_SINGLE
        .max_ppdu_duration_micros(u32::try_from(mpdu).ok()?)?
        .checked_add(ack)?;
    let total = aggregate
        .checked_mul(u32::from(aggregate_attempts))?
        .checked_add(single.checked_mul(u32::from(single_publications))?)?;
    NonZeroU32::new(total)
}

#[cfg(test)]
mod tests;

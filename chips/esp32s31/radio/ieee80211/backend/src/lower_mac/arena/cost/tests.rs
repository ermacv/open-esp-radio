use super::*;
use oer_chips_esp32s31_hal::types::MacLegacyRate;

/// TXTIME by hand, IEEE Std 802.11-2020 15.4.3 and 17.4.3: DSSS long
/// preamble 192 us and 8 us an octet at 1 Mb/s; 6 Mb/s OFDM 20 us preamble,
/// 4 us symbols of 24 bits after 16 SERVICE and 6 tail bits, 6 us signal
/// extension at 2.4 GHz.
const ACK_AT_1M: u32 = 192 + 14 * 8;
const ACK_AT_6M: u32 = 20 + 4 * (16 + 14 * 8 + 6_u32).div_ceil(24) + 6;
const BLOCK_ACK_AT_6M: u32 = 20 + 4 * (16 + 32 * 8 + 6_u32).div_ceil(24) + 6;

#[test]
fn a_response_is_its_sifs_and_its_frame_at_the_declared_rate() {
    let dsss = TxPhyRate::Legacy(MacLegacyRate::Cck11MLong);
    let ofdm = TxPhyRate::Legacy(MacLegacyRate::Ofdm54M);
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
    let rate = MacLegacyRate::Ofdm24M;
    let timing = TxPhyRate::Legacy(rate).ppdu_timing();
    let mut work = MacTxWork::new();
    // An earlier publication of the batch is not this one's.
    work.record_publication(500, 1, None, timing, None);
    let before = work;
    work.record_publication(1_000, 1, None, timing, None);
    // 24 Mb/s: 96 bits a symbol; 16 + 8000 + 6 bits take 84 symbols.
    let ppdu = 20 + 4 * 84 + 6;
    assert_eq!(
        publication_cost(
            Some(before),
            Some(work),
            TxPhyRate::Legacy(rate),
            Response::Ack
        ),
        TxEstimate::Estimated {
            micros: ppdu + 10 + ACK_AT_6M
        }
    );
}

#[test]
fn a_publication_the_model_cannot_time_is_unknown() {
    let rate = TxPhyRate::Legacy(MacLegacyRate::Ofdm24M);
    let before = MacTxWork::new();
    let mut work = before;
    work.record_publication(1_000, 1, None, None, None);
    assert_eq!(
        publication_cost(Some(before), Some(work), rate, Response::Ack),
        TxEstimate::Unknown
    );
    // No publication recorded is no estimate either.
    assert_eq!(
        publication_cost(Some(before), Some(before), rate, Response::Ack),
        TxEstimate::Unknown
    );
    assert_eq!(
        publication_cost(None, Some(work), rate, Response::Ack),
        TxEstimate::Unknown
    );
}

use super::*;
use crate::TxQueues;
use core::num::NonZeroU16;
use oer_ieee80211_softmac::{
    MacTxWork,
    tx_cost::{PpduTiming, TxCost, TxProtection, TxResponse},
};

fn data_phy(peer: u8) -> PpduTiming {
    PpduTiming::BccOfdm {
        data_bits_per_symbol: NonZeroU16::new(if peer == 1 { 26 } else { 540 }).unwrap(),
        symbol_nanos: NonZeroU16::new(if peer == 1 { 4000 } else { 3600 }).unwrap(),
        preamble_micros: 36,
        tail_bits: 6,
        signal_extension_micros: 6,
    }
}
fn reply(peer: u8) -> TxResponse {
    TxResponse::BlockAck {
        timing: PpduTiming::BccOfdm {
            data_bits_per_symbol: NonZeroU16::new(if peer == 1 { 24 } else { 96 }).unwrap(),
            symbol_nanos: NonZeroU16::new(4000).unwrap(),
            preamble_micros: 20,
            tail_bits: 6,
            signal_extension_micros: 6,
        },
        psdu_bytes: NonZeroU16::new(32).unwrap(),
    }
}

#[test]
fn queue_heads_are_read_only_and_once_per_peer_across_transport_flows() {
    let mut queue = TxQueues::<u8, u32, 8, u8>::new();
    queue.push_flow(1, 10, 100).unwrap();
    queue.push_flow(1, 11, 101).unwrap();
    queue.push_flow(2, 20, 200).unwrap();
    for _ in 0..3 {
        let mut heads = queue.heads();
        assert_eq!(heads.next(), Some((1, &100)));
        assert_eq!(heads.next(), Some((2, &200)));
        assert_eq!(heads.next(), None);
    }
    let mut storage = AirtimeStorage::<_, 2>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let grant = scheduler
        .reserve(queue.heads().map(|(key, _)| candidate(key, 100)))
        .unwrap()
        .unwrap();
    assert_eq!(queue.len(), 3);
    assert_eq!(queue.pop(grant.key()), Some(100));
    assert_eq!(queue.pop(grant.key()), Some(101));
    scheduler
        .settle(grant.published().completed(()), us(900))
        .unwrap();
    assert_eq!(queue.pop(2), Some(200));
}

#[test]
fn ht_grants_limit_bursts_and_retry_receipts_equalize_service_not_packet_counts() {
    let mut queue = TxQueues::<u8, u16, 64, u8>::new();
    for index in 0..32 {
        // One peer has eight transport flows; the other has one.
        queue.push_flow(1, index % 8, 1000).unwrap();
        queue.push_flow(2, 0, 1000).unwrap();
    }
    let mut storage = AirtimeStorage::<_, 2>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let mut charged = [0u64; 2];
    let mut packets = [0u32; 2];
    for iteration in 0..2000 {
        let candidates = queue.heads().map(|(key, bytes)| {
            let minimum = TxCost::estimate(
                2 * (bytes + 4),
                Some(data_phy(key)),
                10,
                reply(key),
                TxProtection::None,
                None,
            )
            .exchange_micros()
            .unwrap();
            candidate(key, minimum)
        });
        let grant = scheduler.reserve(candidates).unwrap().unwrap();
        let key = grant.key();
        let limit = TxCost::maximum_psdu_bytes(
            grant.budget_micros().get(),
            Some(data_phy(key)),
            10,
            reply(key),
            TxProtection::None,
        )
        .unwrap()
        .get();
        let mut bytes = 0u16;
        let mut frames = 0u8;
        while frames < 32 && u32::from(bytes) + 1004 <= u32::from(limit) {
            assert_eq!(queue.pop(key), Some(1000));
            bytes += 1004; // Four-byte delimiter; each fixture MPDU is aligned.
            frames += 1;
        }
        assert!(frames >= 2);
        let mut receipt = MacTxWork::new();
        receipt.record_publication(bytes, frames, None, Some(data_phy(key)), None);
        if iteration % 5 == 0 {
            // The same peer owns retransmission work; no new scheduler turn.
            receipt.record_publication(bytes, frames, None, Some(data_phy(key)), None);
        }
        let overhead = TxCost::estimate(0, None, 10, reply(key), TxProtection::None, None)
            .response_micros
            .unwrap();
        let charge = receipt.estimated_exchange_micros(overhead).unwrap();
        let index = usize::from(key - 1);
        charged[index] += u64::from(charge.get());
        packets[index] += u32::from(frames);
        scheduler
            .settle(grant.published().completed(receipt), charge)
            .unwrap();
        for flow in 0..frames {
            queue
                .push_flow(key, if key == 1 { flow % 8 } else { 0 }, 1000)
                .unwrap();
        }
    }
    assert!(
        charged[0].abs_diff(charged[1]) < 6000,
        "charged={charged:?}"
    );
    assert!(packets[1] > packets[0] * 5, "packets={packets:?}");
    assert_eq!(queue.len_for(1), 32);
    assert_eq!(queue.len_for(2), 32);
}

#[test]
fn failed_reservation_and_cancelled_preparation_preserve_packet_ownership() {
    use core::cell::Cell;
    #[derive(Debug)]
    struct Owner<'a>(&'a Cell<u32>);
    impl Drop for Owner<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let drops = Cell::new(0);
    let mut queue = TxQueues::<u8, _, 4>::new();
    queue.push(1, Owner(&drops)).unwrap();
    queue.push(2, Owner(&drops)).unwrap();
    let mut storage = AirtimeStorage::<_, 2, 1>::new(us(1000));
    let mut scheduler = storage.scheduler();
    let grant = scheduler
        .reserve(queue.heads().map(|(key, _)| candidate(key, 100)))
        .unwrap()
        .unwrap();
    assert_eq!(
        scheduler.reserve(queue.heads().map(|(key, _)| candidate(key, 100))),
        Err(AirtimeError::ReservationCapacity)
    );
    assert_eq!(queue.len(), 2);
    assert_eq!(drops.get(), 0);
    scheduler.cancel(grant).unwrap();
    assert_eq!(queue.len(), 2);
    assert_eq!(drops.get(), 0);
    drop(queue.pop(1).unwrap());
    assert_eq!(drops.get(), 1);
    drop(queue);
    assert_eq!(drops.get(), 2);
}

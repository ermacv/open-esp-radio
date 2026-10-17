use super::*;
use crate::tx::{HtChannelWidth, HtGuardInterval, HtMcs, LegacyRate, TxPhyRate};

#[test]
fn exchange_byte_limit_stops_existing_builder_without_consuming_the_next_owner() {
    let rate = HtRate::new(
        HtMcs::Mcs0,
        HtGuardInterval::Long800Ns,
        HtChannelWidth::Mhz20,
    );
    let reply = TxPhyRate::Legacy(LegacyRate::Ofdm24M)
        .ppdu_timing()
        .unwrap();
    let limit = rate
        .ampdu_exchange_byte_limit(250, reply, 8191)
        .unwrap()
        .get();
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<4, 0>::new());
    let pool: PinnedDmaTxPoolHandle<256, 0, 0, 4> = PinnedDmaTxPool::pin_static(
        std::boxed::Box::leak(std::boxed::Box::new(PinnedDmaTxPool::new())),
    );
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    owner.configure_max_aggregate_bytes(limit).unwrap();
    let cookie = owner.begin().unwrap();
    for index in 0..2 {
        assert!(
            owner
                .can_commit_referenced_ht_frame(cookie, 32, 8, 0, rate, 256)
                .unwrap()
        );
        let (index, ()) = pool
            .claim_network(index)
            .publish(TX_AMPDU_METADATA_SIZE + 32, |_| {});
        owner
            .commit_ht(
                cookie,
                pool.claim_radio(index),
                ht_frame_request(0, 32, 8, 0, rate),
            )
            .unwrap();
    }
    let next = pool.claim_network(2);
    let before = owner.prepared_aggregate(cookie).unwrap();
    assert!(
        !owner
            .can_commit_referenced_ht_frame(cookie, 32, 8, 0, rate, 256)
            .unwrap()
    );
    let after = owner.prepared_aggregate(cookie).unwrap();
    assert_eq!(before, after);
    assert_eq!(after.subframes, 2);
    assert_eq!(owner.held_backing_count(), 2);
    assert_eq!(next.release(), 2);
    assert!(
        TxPhyRate::Ht(rate)
            .ppdu_timing()
            .unwrap()
            .duration_micros(after.bytes)
            + 48
            <= 250
    );
    owner.cancel(cookie).unwrap();
    assert_eq!(pool.claimed_slots(), 0);
}

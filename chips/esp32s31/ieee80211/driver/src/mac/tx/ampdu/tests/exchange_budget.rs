use super::*;
use crate::mac::tx::{HtChannelWidth, HtGuardInterval, HtMcs, TxPhyRate};
use oer_chips_esp32s31_hal::types::MacLegacyRate;

#[test]
fn exchange_byte_limit_refuses_the_next_frame_and_hands_its_lease_back() {
    let rate = HtRate::new(
        HtMcs::Mcs0,
        HtGuardInterval::Long800Ns,
        HtChannelWidth::Mhz20,
    );
    let reply = TxPhyRate::Legacy(MacLegacyRate::Ofdm24M)
        .ppdu_timing()
        .unwrap();
    let limit = rate
        .ampdu_exchange_byte_limit(250, reply, 8191)
        .unwrap()
        .get();
    let mut storage = core::pin::pin!(HtAmpduTxStorage::<4>::new());
    let pool: PlacedTxPool<256, 0, 0, 4> = host_pool(std::boxed::Box::leak(std::boxed::Box::new(
        PinnedDmaTxPool::new(),
    )));
    let mut retention = RetainedAmpduDmaStorage::new();
    let mut owner = RetainedDmaAmpduTx::new_model(storage.as_mut(), &mut retention).unwrap();
    owner.configure_max_aggregate_bytes(limit).unwrap();
    let cookie = owner.begin().unwrap();
    for index in 0..2 {
        let (index, ()) = pool.claim_network(index).unwrap().publish(
            oer_portable_memory::FrameLen::new(TX_AMPDU_METADATA_SIZE + 32)
                .expect("test frame length"),
            |_| {},
        );
        commit(
            &mut owner,
            cookie,
            pool.claim_radio(index).expect("ready placed slot"),
            32,
            8,
        )
        .unwrap();
    }
    let before = prepared(&mut owner, cookie).unwrap();
    // A third MPDU passes the exchange's byte limit: intake refuses it and
    // hands its lease back, the batch unchanged.
    let (index, ()) = pool.claim_network(2).unwrap().publish(
        oer_portable_memory::FrameLen::new(TX_AMPDU_METADATA_SIZE + 32).expect("test frame length"),
        |_| {},
    );
    assert_eq!(
        commit(
            &mut owner,
            cookie,
            pool.claim_radio(index).expect("ready placed slot"),
            32,
            8
        ),
        Err(HtAmpduTxError::AggregateFull)
    );
    let after = prepared(&mut owner, cookie).unwrap();
    assert_eq!(before, after);
    assert_eq!(after.subframes, 2);
    assert_eq!(owner.held_backing_count(), 2);
    assert_eq!(pool.claimed_slots(), 2);
    assert!(
        TxPhyRate::Ht(rate)
            .ppdu_timing()
            .unwrap()
            .duration_micros(after.bytes)
            + 48
            <= 250
    );
    owner.cancel(cookie, drop).unwrap();
    assert_eq!(pool.claimed_slots(), 0);
}

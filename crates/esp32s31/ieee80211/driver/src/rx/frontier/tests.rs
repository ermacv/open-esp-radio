use crate::rx::storage::ReceiveDmaStorage;

use oer_esp32s31_hal::ieee80211::dma::{
    rx_dma::model::{RxWalkerModel, RxWalkerOperation},
    rx_storage::PlacedRxDma,
    space::HostDma,
};

use oer_esp32s31_ieee80211_mac::rx::RxRingHalted;

use super::*;

const COUNT: usize = 2;
const BUFFER_SIZE: usize = 64;
const STORAGE_SIZE: usize = 128;
/// The host arena's descriptor base: descriptors lead the arena.
const BASE: u32 = oer_esp32s31_hal::ieee80211::dma::space::HOST_ARENA_BASE;

/// A placed static host arena, as the product's is placed.
fn leaked<const N: usize, const S: usize>() -> PlacedRxDma<N, BUFFER_SIZE, S, HostDma> {
    let placed = ReceiveDmaStorage::place(oer_esp32s31_hal::dma_static::Placed::host(
        std::boxed::Box::leak(std::boxed::Box::new(ReceiveDmaStorage::new())),
    ))
    .unwrap();
    assert_eq!(placed.descriptor_base(), BASE);
    placed
}

fn halted_ring(
    hardware: &mut RxWalkerModel,
    storage: PlacedRxDma<COUNT, BUFFER_SIZE, STORAGE_SIZE, HostDma>,
) -> RxRingHalted<'static, COUNT> {
    storage.prepare_ring(hardware).unwrap().into_halted()
}

#[test]
fn exhausted_terminal_writeback_without_a_fresh_irq_retains_task_continuation() {
    let placed = leaked::<COUNT, STORAGE_SIZE>();
    let storage = placed.storage();
    let mut hardware = RxWalkerModel::default();
    let prepared = placed.prepare_ring(&mut hardware).unwrap();
    let mut rx = ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_prepared(prepared);
    block_on(rx.start_with_storage(
        &oer_base_time_virtual::SkipClock::new(),
        &mut hardware,
        storage,
    ))
    .unwrap();

    // The walker reached the accepted terminal and stopped at NEXT=0. The
    // descriptor writeback can become visible after the RX-success edge which
    // woke the task, so no second interrupt may follow. Complete vendor
    // datapathProcessRxSucDataAll remains in its LAST-refresh loop here.
    hardware.set_cursor(BASE, COUNT - 1, None);
    assert_eq!(
        rx.service_continuation(&mut hardware).unwrap(),
        RxFrontierContinuation::ProbePending
    );

    // A nonzero walker cursor removes this exhausted-terminal condition; no
    // elapsed time or repeated sample is treated as ownership evidence.
    hardware.next_descriptor_low = BASE & 0x000f_ffff;
    assert_eq!(
        rx.service_continuation(&mut hardware).unwrap(),
        RxFrontierContinuation::AwaitInterrupt
    );
}

#[test]
fn owner_round_trips_a_live_ring_between_phases() {
    let placed = leaked::<COUNT, STORAGE_SIZE>();
    let storage = placed.storage();
    let mut hardware = RxWalkerModel::default();
    let mut rx =
        ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted_ring(&mut hardware, placed));
    block_on(rx.start_with_storage(
        &oer_base_time_virtual::SkipClock::new(),
        &mut hardware,
        storage,
    ))
    .unwrap();
    assert_eq!(rx.phase(), RxFrontierPhase::Live);

    let mut moved = rx.take().unwrap();
    assert_eq!(rx.phase(), RxFrontierPhase::Vacant);
    assert_eq!(moved.phase(), RxFrontierPhase::Live);
    moved.stop(&mut hardware).unwrap();
    assert_eq!(moved.phase(), RxFrontierPhase::Halted);
    rx = moved;
    assert_eq!(rx.phase(), RxFrontierPhase::Halted);
}

#[test]
fn consuming_connected_promotion_preserves_live_frontier_or_exact_owner() {
    let placed = leaked::<COUNT, STORAGE_SIZE>();
    let storage = placed.storage();
    let mut hardware = RxWalkerModel::default();
    let rx = ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted_ring(&mut hardware, placed));
    let live = block_on(rx.try_into_live_with_storage(
        &oer_base_time_virtual::SkipClock::new(),
        &mut hardware,
        storage,
    ))
    .unwrap_or_else(|_| panic!("fresh halted owner must become live"));
    assert_eq!(live.descriptor_base(), BASE);

    let halted = match live.try_stop(&mut hardware) {
        Ok(halted) => halted,
        Err(_) => panic!("mock walker must stop before the failure case"),
    };
    let mut already_live = ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted);
    block_on(already_live.start_with_storage(
        &oer_base_time_virtual::SkipClock::new(),
        &mut hardware,
        storage,
    ))
    .expect("finite protocol phase starts the ring");
    storage.descriptors()[0].write_word0(storage.descriptors()[0].word0().with_rx_done(true));
    // The final protocol frame remains observed before promotion.
    let ring = already_live.live_mut().expect("the frontier is live");
    // The walker reports descriptor zero as LAST: only that releases it.
    hardware.set_cursor(BASE, 0, Some(1));
    let frozen = ring.freeze_cursor(&mut hardware).unwrap();
    assert!(storage.take_completed(ring, frozen, 0).unwrap().is_some());
    let enable_count = hardware.operation_count(RxWalkerOperation::Enable);
    let disable_count = hardware.operation_count(RxWalkerOperation::Stop);
    let reload_count = hardware.operation_count(RxWalkerOperation::RequestReload);
    let live = block_on(already_live.try_into_live_with_storage(
        &oer_base_time_virtual::SkipClock::new(),
        &mut hardware,
        storage,
    ))
    .unwrap_or_else(|_| panic!("an existing live frontier must transfer cleanly"));
    assert_eq!(
        hardware.operation_count(RxWalkerOperation::Enable),
        enable_count
    );
    assert_eq!(
        hardware.operation_count(RxWalkerOperation::Stop),
        disable_count
    );
    assert_eq!(
        hardware.operation_count(RxWalkerOperation::RequestReload),
        reload_count
    );
    assert!(storage.descriptors()[0].word0().rx_done());
    assert!(!live.observed_mask().is_empty());
    let halted = match live.try_stop(&mut hardware) {
        Ok(halted) => halted,
        Err(_) => panic!("mock walker must stop after the live handoff"),
    };
    let mut vacant = ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted);
    let retained = vacant.take().expect("test retains the exact halted owner");
    let failure = block_on(vacant.try_into_live_with_storage(
        &oer_base_time_virtual::SkipClock::new(),
        &mut hardware,
        storage,
    ))
    .err()
    .expect("vacant frontier must return its placeholder owner");
    assert_eq!(failure.error, RxFrontierError::OwnerUnavailable);
    assert_eq!(failure.owner.phase(), RxFrontierPhase::Vacant);
    assert_eq!(retained.phase(), RxFrontierPhase::Halted);
}

// Every delay in this suite is ReadyDelay. Poll once so an unexpected
// asynchronous ownership edge fails instead of requiring an executor.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    let mut context = core::task::Context::from_waker(core::task::Waker::noop());
    match future.as_mut().poll(&mut context) {
        core::task::Poll::Ready(output) => output,
        core::task::Poll::Pending => panic!("finite RX test unexpectedly yielded"),
    }
}

#[test]
fn the_walker_starts_only_after_its_settle_on_the_caller_timer() {
    use core::{
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use oer_base_time::{Duration, Instant};

    let placed = leaked::<COUNT, STORAGE_SIZE>();
    let storage = placed.storage();
    let mut hardware = RxWalkerModel::default();
    let mut rx =
        ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted_ring(&mut hardware, placed));
    let clock: oer_base_time_virtual::VirtualClock =
        oer_base_time_virtual::VirtualClock::starting_at(Instant::from_micros(100));
    let mut context = Context::from_waker(Waker::noop());
    {
        let mut start = pin!(rx.start_with_storage(&clock, &mut hardware, storage));
        assert!(start.as_mut().poll(&mut context).is_pending());
        assert_eq!(clock.next_deadline(), Some(Instant::from_micros(105)));
        clock.advance(Duration::from_micros(4)).unwrap();
        assert!(start.as_mut().poll(&mut context).is_pending());
        clock.advance(Duration::from_micros(1)).unwrap();
        assert_eq!(start.as_mut().poll(&mut context), Poll::Ready(Ok(())));
    }
    assert_eq!(hardware.operation_count(RxWalkerOperation::Enable), 1);
    assert_eq!(rx.phase(), RxFrontierPhase::Live);
}

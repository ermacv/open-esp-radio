use crate::rx::storage::ReceiveDmaStorage;

use oer_esp32s31_ieee80211_dma::descriptor::{BIT_30, DESCRIPTOR_BYTES};

use oer_esp32s31_ieee80211_mac::rx::{
    RxDma, RxDmaBinding, RxDmaWalkerStopped, RxRingHalted, RxRingStopped,
};

use super::*;

const COUNT: usize = 2;
const BUFFER_SIZE: usize = 64;
const STORAGE_SIZE: usize = 128;
const BASE: u32 = 0x2f00_1000;
const BUFFERS: [u32; COUNT] = [0x2f00_2000, 0x2f00_2100];

#[derive(Default)]
struct Hardware {
    walker: bool,
    descriptor_base: u32,
    next_descriptor_low: u32,
    last_descriptor_low: u32,
    enable_count: u32,
    disable_count: u32,
    reload_count: u32,
}

impl Hardware {
    fn release_through(&mut self, last_index: usize, next_index: Option<usize>) {
        self.last_descriptor_low = (BASE + last_index as u32 * DESCRIPTOR_BYTES) & 0x000f_ffff;
        self.next_descriptor_low = next_index
            .map(|index| (BASE + index as u32 * DESCRIPTOR_BYTES) & 0x000f_ffff)
            .unwrap_or(0);
    }
}

impl RxDma for Hardware {
    fn last_descriptor_low(&mut self) -> u32 {
        self.last_descriptor_low
    }
    fn next_descriptor_low(&mut self) -> u32 {
        self.next_descriptor_low
    }
    fn next_descriptor(&mut self) -> oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor {
        oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor::validation(
            self.next_descriptor_low,
            false,
        )
    }
    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaCursorObservation<'confirmation>,
        ) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        self.fence();
        let next = self.next_descriptor_low();
        self.fence();
        observed(oer_esp32s31_ieee80211_mac::rx::RxDmaCursorObservation::validation(last, next))
    }
    fn walker_enabled(&mut self) -> bool {
        self.walker
    }
    fn reload_pending(&mut self) -> bool {
        false
    }
    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaReloadSettled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        (!self.reload_pending())
            .then(|| settled(oer_esp32s31_ieee80211_mac::rx::RxDmaReloadSettled::validation()))
    }
    fn configure_descriptor_window(&mut self, _: &RxDmaBinding) {}
    fn write_descriptor_base(&mut self, _: &RxDmaBinding, address: u32) {
        self.descriptor_base = address;
    }
    fn publish_walker_enable(&mut self, _: &RxDmaBinding) {
        self.walker = true;
        if self.next_descriptor_low == 0 {
            self.next_descriptor_low = self.descriptor_base & 0x000f_ffff;
        }
    }
    fn request_reload(&mut self, _: &RxDmaBinding) {
        self.reload_count += 1;
    }
    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        if self.walker {
            None
        } else {
            self.walker = true;
            if self.next_descriptor_low == 0 {
                self.next_descriptor_low = self.descriptor_base & 0x000f_ffff;
            }
            self.enable_count += 1;
            Some(enabled(
                oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled::validation(),
            ))
        }
    }
    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        if self.walker {
            self.walker = false;
            self.disable_count += 1;
            Some(stopped(RxDmaWalkerStopped::validation()))
        } else {
            None
        }
    }
    fn fence(&mut self) {}
}

#[test]
fn completed_descriptors_are_delivered_in_ring_order_across_wrap() {
    const WRAP_COUNT: usize = 4;
    const WRAP_STORAGE_SIZE: usize = WRAP_COUNT * BUFFER_SIZE;
    let storage = ReceiveDmaStorage::<WRAP_COUNT, BUFFER_SIZE, WRAP_STORAGE_SIZE>::new();
    let addresses = [0x2f00_2000, 0x2f00_2100, 0x2f00_2200, 0x2f00_2300];
    let mut hardware = Hardware::default();
    let halted = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap()
    .into_halted();
    let mut rx = ReceiveFrontier::<WRAP_COUNT, BUFFER_SIZE>::from_halted(halted);
    block_on(rx.start_with_storage(&oer_time_virtual::SkipClock::new(), &mut hardware, &storage))
        .unwrap();
    // First reclaim 0,1 so the next logical receive frontier begins at two.
    for index in [0, 1, 2] {
        storage.descriptors()[index].write_word0(storage.descriptors()[index].word0() | BIT_30);
    }
    hardware.release_through(2, Some(3));
    rx.service_completed(&mut hardware, &storage, |_| RxFrontierDirective::Continue)
        .unwrap();

    // The subsequent finite interval 2,3,0 crosses physical zero.
    for index in [2, 3, 0] {
        storage.descriptors()[index].write_word0(storage.descriptors()[index].word0() | BIT_30);
    }
    hardware.release_through(0, Some(1));

    let mut observed = [usize::MAX; 3];
    let mut count = 0;
    let first = rx
        .service_completed(&mut hardware, &storage, |segment| {
            observed[count] =
                usize::try_from((segment.descriptor_address - BASE) / DESCRIPTOR_BYTES).unwrap();
            count += 1;
            RxFrontierDirective::Continue
        })
        .unwrap();
    let second = rx
        .service_completed(&mut hardware, &storage, |segment| {
            observed[count] =
                usize::try_from((segment.descriptor_address - BASE) / DESCRIPTOR_BYTES).unwrap();
            count += 1;
            RxFrontierDirective::Continue
        })
        .unwrap();

    assert_eq!(first.completed, 2);
    assert_eq!(second.completed, 1);
    assert_eq!(observed, [2, 3, 0]);
}

fn halted_ring<'a>(
    hardware: &mut Hardware,
    storage: &'a ReceiveDmaStorage<COUNT, BUFFER_SIZE, STORAGE_SIZE>,
) -> RxRingHalted<'a, COUNT> {
    RxRingStopped::prepare(
        hardware,
        storage.descriptors(),
        BASE,
        &BUFFERS,
        BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap()
    .into_halted()
}

#[test]
fn exhausted_terminal_writeback_without_a_fresh_irq_retains_task_continuation() {
    let storage = ReceiveDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
    let mut hardware = Hardware::default();
    let prepared = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &BUFFERS,
        BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap();
    let mut rx = ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_prepared(prepared);
    block_on(rx.start_with_storage(&oer_time_virtual::SkipClock::new(), &mut hardware, &storage))
        .unwrap();

    // The walker reached the accepted terminal and stopped at NEXT=0. The
    // descriptor writeback can become visible after the RX-success edge which
    // woke the task, so no second interrupt may follow. Complete vendor
    // datapathProcessRxSucDataAll remains in its LAST-refresh loop here.
    hardware.release_through(COUNT - 1, None);
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
fn owner_services_a_terminal_descriptor_and_round_trips_between_phases() {
    let storage = ReceiveDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
    let mut hardware = Hardware::default();
    let mut rx =
        ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted_ring(&mut hardware, &storage));
    block_on(rx.start_with_storage(&oer_time_virtual::SkipClock::new(), &mut hardware, &storage))
        .unwrap();
    assert_eq!(rx.phase(), RxFrontierPhase::Live);

    for descriptor in storage.descriptors() {
        descriptor.write_word0(descriptor.word0() | BIT_30);
    }
    hardware.release_through(1, None);
    let progress = rx
        .service_completed(&mut hardware, &storage, |segment| {
            assert_eq!(segment.descriptor_address, BASE);
            assert_eq!(segment.buffer.len(), BUFFER_SIZE);
            RxFrontierDirective::Stop
        })
        .unwrap();
    assert_eq!(
        progress,
        RxFrontierProgress {
            completed: 1,
            stopped: true,
        }
    );

    let mut moved = rx.take().unwrap();
    assert_eq!(rx.phase(), RxFrontierPhase::Vacant);
    assert_eq!(moved.phase(), RxFrontierPhase::Live);
    moved.stop(&mut hardware).unwrap();
    assert_eq!(moved.phase(), RxFrontierPhase::Halted);
    rx = moved;
    assert_eq!(rx.phase(), RxFrontierPhase::Halted);
}

#[test]
fn pause_bounds_one_service_pass_without_retaining_a_terminal_descriptor() {
    let storage = ReceiveDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
    let mut hardware = Hardware::default();
    let mut rx =
        ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted_ring(&mut hardware, &storage));
    block_on(rx.start_with_storage(&oer_time_virtual::SkipClock::new(), &mut hardware, &storage))
        .unwrap();
    for descriptor in storage.descriptors() {
        descriptor.write_word0(descriptor.word0() | BIT_30);
    }
    hardware.release_through(0, Some(1));

    let first = rx
        .service_completed(&mut hardware, &storage, |_| RxFrontierDirective::Pause)
        .unwrap();
    assert_eq!(first.completed, 1);
    assert!(!first.stopped);
    // One cursor observation only records a release candidate.  The
    // descriptor remains DMA-owned until a later service turn confirms it.
    assert_ne!(storage.descriptors()[0].word0() & BIT_30, 0);

    hardware.release_through(1, Some(0));
    let second = rx
        .service_completed(&mut hardware, &storage, |_| RxFrontierDirective::Continue)
        .unwrap();
    assert_eq!(second.completed, 1);
    assert!(!second.stopped);
    // Advancing LAST to descriptor one proves that hardware has crossed the
    // first descriptor, so that older candidate may be recycled immediately.
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
    assert_ne!(storage.descriptors()[1].word0() & BIT_30, 0);

    let retained = rx
        .service_completed(&mut hardware, &storage, |_| {
            panic!("confirmation must not manufacture another completion")
        })
        .unwrap();
    assert_eq!(retained.completed, 0);
    assert_ne!(storage.descriptors()[1].word0() & BIT_30, 0);
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
}

#[test]
fn an_append_is_published_only_after_a_later_service_confirms_link_release() {
    let storage = ReceiveDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
    let mut hardware = Hardware::default();
    let mut rx =
        ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted_ring(&mut hardware, &storage));
    block_on(rx.start_with_storage(&oer_time_virtual::SkipClock::new(), &mut hardware, &storage))
        .unwrap();
    storage.descriptors()[0].write_word0(storage.descriptors()[0].word0() | BIT_30);
    hardware.release_through(0, Some(1));

    let completed = rx
        .service_completed(&mut hardware, &storage, |_| RxFrontierDirective::Continue)
        .unwrap();
    assert_eq!(completed.completed, 1);
    assert!(!rx.reload_pending());
    assert_ne!(storage.descriptors()[0].word0() & BIT_30, 0);

    storage.descriptors()[1].write_word0(storage.descriptors()[1].word0() | BIT_30);
    hardware.release_through(1, None);
    let confirmed = rx
        .service_completed(&mut hardware, &storage, |_| RxFrontierDirective::Continue)
        .unwrap();
    assert_eq!(confirmed.completed, 1);
    assert_eq!(storage.descriptors()[0].word0() & BIT_30, 0);
    // This synchronous finite-phase service deliberately leaves the append
    // suffix explicit; the following role turn settles it before reuse.
    assert!(rx.reload_pending());
}

#[test]
fn consuming_connected_promotion_preserves_live_frontier_or_exact_owner() {
    let storage = ReceiveDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
    let mut hardware = Hardware::default();
    let rx =
        ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted_ring(&mut hardware, &storage));
    let live = block_on(rx.try_into_live_with_storage(
        &oer_time_virtual::SkipClock::new(),
        &mut hardware,
        &storage,
    ))
    .unwrap_or_else(|_| panic!("fresh halted owner must become live"));
    assert_eq!(live.descriptor_base(), BASE);

    let halted = match live.try_stop(&mut hardware) {
        Ok(halted) => halted,
        Err(_) => panic!("mock walker must stop before the failure case"),
    };
    let mut already_live = ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted);
    block_on(already_live.start_with_storage(
        &oer_time_virtual::SkipClock::new(),
        &mut hardware,
        &storage,
    ))
    .expect("finite protocol phase starts the ring");
    storage.descriptors()[0].write_word0(storage.descriptors()[0].word0() | BIT_30);
    already_live
        .service_completed(&mut hardware, &storage, |_| RxFrontierDirective::Stop)
        .expect("the final protocol frame remains observed before promotion");
    let enable_count = hardware.enable_count;
    let disable_count = hardware.disable_count;
    let reload_count = hardware.reload_count;
    let live = block_on(already_live.try_into_live_with_storage(
        &oer_time_virtual::SkipClock::new(),
        &mut hardware,
        &storage,
    ))
    .unwrap_or_else(|_| panic!("an existing live frontier must transfer cleanly"));
    assert_eq!(hardware.enable_count, enable_count);
    assert_eq!(hardware.disable_count, disable_count);
    assert_eq!(hardware.reload_count, reload_count);
    assert_ne!(storage.descriptors()[0].word0() & BIT_30, 0);
    assert!(!live.observed_mask().is_empty());
    let halted = match live.try_stop(&mut hardware) {
        Ok(halted) => halted,
        Err(_) => panic!("mock walker must stop after the live handoff"),
    };
    let mut vacant = ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted);
    let retained = vacant.take().expect("test retains the exact halted owner");
    let failure = block_on(vacant.try_into_live_with_storage(
        &oer_time_virtual::SkipClock::new(),
        &mut hardware,
        &storage,
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
    use oer_time::{Duration, Instant};

    let storage = ReceiveDmaStorage::<COUNT, BUFFER_SIZE, STORAGE_SIZE>::new();
    let mut hardware = Hardware::default();
    let mut rx =
        ReceiveFrontier::<COUNT, BUFFER_SIZE>::from_halted(halted_ring(&mut hardware, &storage));
    let clock: oer_time_virtual::VirtualClock =
        oer_time_virtual::VirtualClock::starting_at(Instant::from_micros(100));
    let mut context = Context::from_waker(Waker::noop());
    {
        let mut start = pin!(rx.start_with_storage(&clock, &mut hardware, &storage));
        assert!(start.as_mut().poll(&mut context).is_pending());
        assert_eq!(clock.next_deadline(), Some(Instant::from_micros(105)));
        clock.advance(Duration::from_micros(4)).unwrap();
        assert!(start.as_mut().poll(&mut context).is_pending());
        clock.advance(Duration::from_micros(1)).unwrap();
        assert_eq!(start.as_mut().poll(&mut context), Poll::Ready(Ok(())));
    }
    assert_eq!(hardware.enable_count, 1);
    assert_eq!(rx.phase(), RxFrontierPhase::Live);
}

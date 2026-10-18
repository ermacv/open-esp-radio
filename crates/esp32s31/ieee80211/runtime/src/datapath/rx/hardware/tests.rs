use oer_esp32s31_ieee80211_dma::descriptor::DESCRIPTOR_BYTES;
use oer_esp32s31_ieee80211_dma::{
    rx_ring::{RxDmaArenaState, RxRingError},
    rx_storage::RxDmaStorage,
};
use oer_esp32s31_ieee80211_mac::rx::{RxDma, RxDmaBinding, RxDmaWalkerStopped};

const BASE: u32 = 0x2f00_1000;

#[derive(Default)]
struct MockRxDma {
    walker_enabled: bool,
    reload_requested: bool,
    pending_samples: u32,
    reload_reads: u32,
    descriptor_base: u32,
}

impl RxDma for MockRxDma {
    fn last_descriptor_low(&mut self) -> u32 {
        if self.walker_enabled {
            (BASE + DESCRIPTOR_BYTES) & 0x000f_ffff
        } else {
            0
        }
    }

    fn next_descriptor_low(&mut self) -> u32 {
        if self.walker_enabled {
            (BASE + DESCRIPTOR_BYTES) & 0x000f_ffff
        } else {
            0
        }
    }

    fn next_descriptor(&mut self) -> oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor {
        oer_esp32s31_ieee80211_dma::rx_dma::RxDmaNextDescriptor::validation(
            self.next_descriptor_low(),
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
        self.walker_enabled
    }

    fn reload_pending(&mut self) -> bool {
        self.reload_reads = self.reload_reads.saturating_add(1);
        if self.reload_requested && self.pending_samples != 0 {
            self.pending_samples -= 1;
            true
        } else {
            false
        }
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
        self.walker_enabled = true;
    }

    fn request_reload(&mut self, _: &RxDmaBinding) {
        self.reload_requested = true;
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding,
        enabled: impl for<'confirmation> FnOnce(
            oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        if self.walker_enabled {
            None
        } else {
            self.walker_enabled = true;
            Some(enabled(
                oer_esp32s31_ieee80211_mac::rx::RxDmaWalkerEnabled::validation(),
            ))
        }
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.walker_enabled = false;
        Some(stopped(RxDmaWalkerStopped::validation()))
    }

    fn fence(&mut self) {}
}

#[test]
fn dropped_live_ring_poison_is_sticky_in_the_static_dma_arena() {
    const COUNT: usize = 2;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = RxDmaStorage::<COUNT, 256, 260>::new();
    let mut mmio = MockRxDma::default();
    let stopped = storage.prepare_ring(&mut mmio, BASE, &buffers).unwrap();
    let ring = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);

    drop(ring);

    assert!(mmio.walker_enabled);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    assert!(matches!(
        storage.prepare_ring(&mut mmio, BASE, &buffers),
        Err(RxRingError::ResetRequired)
    ));
}

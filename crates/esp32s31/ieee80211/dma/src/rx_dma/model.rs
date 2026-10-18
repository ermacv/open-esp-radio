//! Host model of the RX descriptor walker's registers (test support only).
//!
//! [`RxWalkerModel`] implements [`RxDma`] over the register images the
//! product reads and writes: `LAST`, `NEXT`, `BASE`, `WALKER_ENABLE` and the
//! `APPEND_DESCRIPTOR_RELOAD` doorbell. It follows the PAC implementation's
//! observable behaviour (an enable of a running walker is refused, a stop
//! reads back as stopped unless the test refuses it) and records every
//! operation in a fixed-capacity log, so it needs no allocator on either
//! target. Tests play the hardware's part by setting the cursor images.

use super::{
    RxDma, RxDmaBinding, RxDmaCursorObservation, RxDmaNextDescriptor, RxDmaReloadSettled,
    RxDmaWalkerEnabled, RxDmaWalkerStopped,
};
use crate::descriptor::DESCRIPTOR_BYTES;

/// The walker's cursor registers report the low 20 address bits.
const CURSOR_LOW_MASK: u32 = 0x000f_ffff;

/// Operations the log can hold before [`RxWalkerModel::overflowed`] is set.
pub const RX_WALKER_LOG_CAPACITY: usize = 512;

/// One operation the product performed on the modelled walker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxWalkerOperation {
    ObserveLast,
    ObserveNext,
    ObserveWalkerEnabled,
    ObserveReloadPending,
    ConfigureDescriptorWindow,
    WriteDescriptorBase(u32),
    PublishWalkerEnable,
    RequestReload,
    /// An enable request, accepted or refused.
    Enable,
    /// A stop request, accepted or refused.
    Stop,
    Fence,
}

/// Register-level model of the RX descriptor walker.
pub struct RxWalkerModel {
    /// `RX_LAST_DESCRIPTOR` low address bits.
    pub last_descriptor_low: u32,
    /// `RX_NEXT_DESCRIPTOR` low address bits.
    pub next_descriptor_low: u32,
    /// Whether `RX_NEXT_DESCRIPTOR` reports unassigned upper bits.
    pub next_unknown_upper: bool,
    /// `RX_DESCRIPTOR_BASE` as last written.
    pub descriptor_base: u32,
    /// `RX_CONTROL.WALKER_ENABLE`.
    pub walker_enabled: bool,
    /// Observations of the reload doorbell still to read as pending.
    pub reload_pending_observations: u32,
    /// Pending observations each [`RxDma::request_reload`] sets.
    pub reload_latency: u32,
    /// The doorbell never clears.
    pub reload_stuck: bool,
    /// A stop request leaves the walker running and reads back as enabled.
    pub refuse_stop: bool,
    /// The starvation counter, when the modelled platform has one.
    pub buffer_full_count: Option<u16>,
    log: [RxWalkerOperation; RX_WALKER_LOG_CAPACITY],
    logged: usize,
    overflowed: bool,
}

impl Default for RxWalkerModel {
    fn default() -> Self {
        Self::new()
    }
}

impl RxWalkerModel {
    /// A stopped walker with zero cursors and a settled doorbell.
    pub const fn new() -> Self {
        Self {
            last_descriptor_low: 0,
            next_descriptor_low: 0,
            next_unknown_upper: false,
            descriptor_base: 0,
            walker_enabled: false,
            reload_pending_observations: 0,
            reload_latency: 0,
            reload_stuck: false,
            refuse_stop: false,
            buffer_full_count: None,
            log: [RxWalkerOperation::Fence; RX_WALKER_LOG_CAPACITY],
            logged: 0,
            overflowed: false,
        }
    }

    /// Point `LAST` at descriptor `last` and `NEXT` at descriptor `next`
    /// (zero for `None`, the exhausted list) of the ring at `descriptor_base`.
    pub fn set_cursor(&mut self, descriptor_base: u32, last: usize, next: Option<usize>) {
        self.last_descriptor_low = descriptor_low(descriptor_base, last);
        self.next_descriptor_low = next.map_or(0, |next| descriptor_low(descriptor_base, next));
    }

    /// The logged operations, oldest first.
    pub fn operations(&self) -> &[RxWalkerOperation] {
        self.log.get(..self.logged).unwrap_or_default()
    }

    /// Whether operations were performed after the log filled.
    pub const fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// How many logged operations equal `operation`.
    ///
    /// # Panics
    ///
    /// When the log overflowed, since the count would be short; clear the
    /// log between phases of a long test.
    pub fn operation_count(&self, operation: RxWalkerOperation) -> usize {
        assert!(!self.overflowed, "the RX walker model log overflowed");
        self.operations()
            .iter()
            .filter(|logged| **logged == operation)
            .count()
    }

    /// Forget the logged operations.
    pub fn clear_operations(&mut self) {
        self.logged = 0;
        self.overflowed = false;
    }

    fn record(&mut self, operation: RxWalkerOperation) {
        match self.log.get_mut(self.logged) {
            Some(slot) => {
                *slot = operation;
                self.logged += 1;
            }
            None => self.overflowed = true,
        }
    }

    fn observe_reload_pending(&mut self) -> bool {
        self.record(RxWalkerOperation::ObserveReloadPending);
        if self.reload_stuck {
            return true;
        }
        match self.reload_pending_observations.checked_sub(1) {
            Some(remaining) => {
                self.reload_pending_observations = remaining;
                true
            }
            None => false,
        }
    }
}

fn descriptor_low(descriptor_base: u32, index: usize) -> u32 {
    // A ring holds at most 96 descriptors, so the index fits `u32`.
    let offset = (index as u32).wrapping_mul(DESCRIPTOR_BYTES);
    descriptor_base.wrapping_add(offset) & CURSOR_LOW_MASK
}

impl RxDma for RxWalkerModel {
    fn buffer_full_count(&mut self) -> Option<u16> {
        self.buffer_full_count
    }

    fn descriptor_base_register(&mut self) -> Option<u32> {
        Some(self.descriptor_base)
    }

    fn last_descriptor_low(&mut self) -> u32 {
        self.record(RxWalkerOperation::ObserveLast);
        self.last_descriptor_low
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.record(RxWalkerOperation::ObserveNext);
        self.next_descriptor_low
    }

    fn next_descriptor(&mut self) -> RxDmaNextDescriptor {
        let address_low = self.next_descriptor_low();
        RxDmaNextDescriptor::from_fields(address_low, self.next_unknown_upper)
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        self.fence();
        let next = self.next_descriptor_low();
        self.fence();
        observed(RxDmaCursorObservation::confirmed(last, next))
    }

    fn walker_enabled(&mut self) -> bool {
        self.record(RxWalkerOperation::ObserveWalkerEnabled);
        self.walker_enabled
    }

    fn reload_pending(&mut self) -> bool {
        self.observe_reload_pending()
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        (!self.observe_reload_pending()).then(|| settled(RxDmaReloadSettled::confirmed()))
    }

    fn configure_descriptor_window(&mut self, _: &RxDmaBinding<'_>) {
        self.record(RxWalkerOperation::ConfigureDescriptorWindow);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32) {
        assert!(
            binding.admits(address),
            "RX descriptor base must belong to the bound static ring"
        );
        self.record(RxWalkerOperation::WriteDescriptorBase(address));
        self.descriptor_base = address;
    }

    fn publish_walker_enable(&mut self, _: &RxDmaBinding<'_>) {
        self.record(RxWalkerOperation::PublishWalkerEnable);
        self.start_walker();
    }

    fn request_reload(&mut self, _: &RxDmaBinding<'_>) {
        self.record(RxWalkerOperation::RequestReload);
        self.reload_pending_observations = self.reload_latency;
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        self.record(RxWalkerOperation::Enable);
        if self.walker_enabled {
            return None;
        }
        self.start_walker();
        Some(enabled(RxDmaWalkerEnabled::confirmed()))
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.record(RxWalkerOperation::Stop);
        if self.refuse_stop {
            return None;
        }
        self.walker_enabled = false;
        Some(stopped(RxDmaWalkerStopped::confirmed(binding)))
    }

    fn fence(&mut self) {
        self.record(RxWalkerOperation::Fence);
    }
}

impl RxWalkerModel {
    /// An enabled walker with an exhausted cursor fetches the published head.
    fn start_walker(&mut self) {
        self.walker_enabled = true;
        if self.next_descriptor_low == 0 {
            self.next_descriptor_low = self.descriptor_base & CURSOR_LOW_MASK;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{rx_storage::RxDmaStorage, space::HostDma};

    #[test]
    fn the_model_follows_the_pac_walker_and_logs_in_order() {
        extern crate std;
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            RxDmaStorage::<2, 16, 20, HostDma>::new(),
        ));
        let mut buffers = [0; 2];
        let base = storage.dma_layout(&mut buffers).unwrap();
        let mut walker = RxWalkerModel::new();
        let live = storage
            .prepare_ring(&mut walker, base, &buffers)
            .unwrap()
            .try_start(&mut walker)
            .map_err(|(_, error)| error)
            .unwrap();
        assert!(walker.walker_enabled);
        assert_eq!(walker.descriptor_base, base);
        // The diagnostic read reports the written base and logs nothing.
        let logged = walker.operations().len();
        assert_eq!(walker.descriptor_base_register(), Some(base));
        assert_eq!(walker.operations().len(), logged);
        assert_eq!(walker.next_descriptor_low, base & CURSOR_LOW_MASK);
        assert_eq!(
            walker.operation_count(RxWalkerOperation::WriteDescriptorBase(base)),
            1
        );

        walker.clear_operations();
        walker.set_cursor(base, 1, None);
        assert_eq!(
            walker.with_ordered_cursor(|cursor| (
                cursor.last_descriptor_low(),
                cursor.next_descriptor_low()
            )),
            ((base + DESCRIPTOR_BYTES) & CURSOR_LOW_MASK, 0)
        );
        assert_eq!(
            walker.operations(),
            [
                RxWalkerOperation::ObserveLast,
                RxWalkerOperation::Fence,
                RxWalkerOperation::ObserveNext,
                RxWalkerOperation::Fence,
            ]
        );

        walker.reload_pending_observations = 2;
        assert!(walker.reload_pending());
        assert!(walker.try_with_reload_settled(|_| ()).is_none());
        assert!(walker.try_with_reload_settled(|_| ()).is_some());

        assert!(live.try_stop(&mut walker).is_ok());
        assert!(!walker.walker_enabled);
    }
}

//! Host model of the RX descriptor walker's registers (test support only).
//!
//! [`RxWalkerModel`] implements [`RxDma`] over the register images the
//! product reads and writes: `LAST`, `NEXT`, `BASE`, `WALKER_ENABLE` and the
//! `APPEND_DESCRIPTOR_RELOAD` doorbell. It follows the PAC implementation's
//! observable behaviour (an enable of a running walker is refused, a stop
//! reads back as stopped unless the test refuses it) and records every
//! operation in a fixed-capacity log, so it needs no allocator on either
//! target. Tests play the hardware's part by setting the cursor images.
//!
//! The doorbell follows the rule the W1 stand observed
//! (HIL_W1_RX_WALKER_2026_10_18, `chips/esp32s31/notes/wifi/rx-walker.md`):
//! rung on an enabled walker whose NEXT is zero, it resumes the walker at
//! `LAST.next`, read when the doorbell is rung and applied when it clears.
//! [`RxWalkerModel::lose_reloads`] makes doorbells miss that resumption.
//! A doorbell rung while NEXT is nonzero changes nothing; that case is not
//! verified on hardware.
//!
//! [`RxWalkerModel::land`] plays the DMA engine writing one frame where NEXT
//! points, and honours the owner bit as the stand observed it
//! (HIL_W1_OWNER_BIT_2026_10_18): a descriptor whose owner bit is clear
//! stops the walker there, unwritten, with NEXT naming it; once that
//! descriptor carries the owner bit again, one doorbell resumes the walker
//! in place. The doorbell that follows `LAST.next` on an exhausted walker
//! does not check LAST's owner bit (H1). The model clears a stop on the
//! enable edge, which the stand did not exercise.

use super::RxWalkerTable;
use super::{
    RxDma, RxDmaBinding, RxDmaCursorObservation, RxDmaNextDescriptor, RxDmaReloadSettled,
    RxDmaWalkerEnabled, RxDmaWalkerStopped,
};
use crate::ieee80211::dma::descriptor::{DESCRIPTOR_BYTES, rx_completed_word};

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

/// Where a model walker also reports each operation as it happens, so a
/// test orders the walker's operations among its other hardware's events.
pub trait RxWalkerJournal {
    fn record(&self, operation: RxWalkerOperation);
}

/// Where one modelled frame went ([`RxWalkerModel::land`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxWalkerLanding {
    /// Written into the descriptor at this low address.
    Landed(u32),
    /// The walker is disabled or exhausted (NEXT zero), or NEXT names no
    /// descriptor of the table: nothing was written.
    Exhausted,
    /// The walker stopped on the descriptor at this low address, NEXT, whose
    /// owner bit was clear (or which it already stopped on, awaiting its
    /// doorbell): nothing was written.
    StoppedOnOwner(u32),
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
    /// Doorbells still to lose: each leaves NEXT unchanged instead of
    /// resuming an exhausted walker. `u32::MAX` loses every doorbell.
    pub lose_reloads: u32,
    /// The NEXT a rung doorbell applies when it clears.
    reload_resumes_at: Option<u32>,
    /// The walker stopped on NEXT, a descriptor whose owner bit was clear.
    pub stopped_on_owner: bool,
    /// A rung doorbell resumes the stopped walker in place when it clears.
    reload_resumes_in_place: bool,
    /// A stop request leaves the walker running and reads back as enabled.
    pub refuse_stop: bool,
    log: [RxWalkerOperation; RX_WALKER_LOG_CAPACITY],
    logged: usize,
    overflowed: bool,
    journal: Option<&'static dyn RxWalkerJournal>,
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
            lose_reloads: 0,
            reload_resumes_at: None,
            stopped_on_owner: false,
            reload_resumes_in_place: false,
            refuse_stop: false,
            log: [RxWalkerOperation::Fence; RX_WALKER_LOG_CAPACITY],
            logged: 0,
            overflowed: false,
            journal: None,
        }
    }

    /// Point `LAST` at descriptor `last` and `NEXT` at descriptor `next`
    /// (zero for `None`, the exhausted list) of the ring at `descriptor_base`.
    pub fn set_cursor(&mut self, descriptor_base: u32, last: usize, next: Option<usize>) {
        self.last_descriptor_low = descriptor_low(descriptor_base, last);
        self.next_descriptor_low = next.map_or(0, |next| descriptor_low(descriptor_base, next));
    }

    /// Report every later operation to `journal` as well as to the log.
    pub fn set_journal(&mut self, journal: &'static dyn RxWalkerJournal) {
        self.journal = Some(journal);
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

    /// Play the DMA engine: write one frame of `length` bytes where NEXT
    /// points (`terminal`: its last segment, which sets `RX_DONE` and moves
    /// LAST there) and follow that descriptor's link.
    ///
    /// The walker checks the owner bit first: on a clear one it stops,
    /// writes nothing and leaves NEXT naming that descriptor, until the
    /// descriptor carries the owner bit again and a doorbell resumes it.
    /// The frame bytes are the caller's to write into the landed
    /// descriptor's buffer.
    pub fn land(
        &mut self,
        table: RxWalkerTable<'_>,
        length: u32,
        terminal: bool,
    ) -> RxWalkerLanding {
        let next = self.next_descriptor_low;
        if !self.walker_enabled || next == 0 {
            return RxWalkerLanding::Exhausted;
        }
        let Some(descriptor) = table.descriptor_at_low(next) else {
            return RxWalkerLanding::Exhausted;
        };
        if self.stopped_on_owner || !descriptor.word0().owner() {
            self.stopped_on_owner = true;
            return RxWalkerLanding::StoppedOnOwner(next);
        }
        descriptor.write_word0(
            rx_completed_word(descriptor.word0(), length, terminal)
                .expect("a modelled landing length fits the length field"),
        );
        if terminal {
            self.last_descriptor_low = next;
        }
        self.next_descriptor_low = descriptor.next_address() & CURSOR_LOW_MASK;
        RxWalkerLanding::Landed(next)
    }

    fn record(&mut self, operation: RxWalkerOperation) {
        if let Some(journal) = self.journal {
            journal.record(operation);
        }
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
            None => {
                // The doorbell clears: an exhausted walker resumes now, a
                // walker stopped on a rearmed descriptor resumes in place.
                if let Some(next) = self.reload_resumes_at.take() {
                    self.next_descriptor_low = next;
                }
                if core::mem::take(&mut self.reload_resumes_in_place) {
                    self.stopped_on_owner = false;
                }
                false
            }
        }
    }
}

fn descriptor_low(descriptor_base: u32, index: usize) -> u32 {
    // A ring holds at most 96 descriptors, so the index fits `u32`.
    let offset = (index as u32).wrapping_mul(DESCRIPTOR_BYTES);
    descriptor_base.wrapping_add(offset) & CURSOR_LOW_MASK
}

impl super::sealed::Sealed for RxWalkerModel {}

impl RxDma for RxWalkerModel {
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

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>) {
        let address = binding.reserved_address();
        self.record(RxWalkerOperation::WriteDescriptorBase(address));
        self.descriptor_base = address;
    }

    fn publish_walker_enable(&mut self, _: &RxDmaBinding<'_>) {
        self.record(RxWalkerOperation::PublishWalkerEnable);
        self.start_walker();
    }

    fn request_reload(&mut self, binding: &RxDmaBinding<'_>) {
        self.record(RxWalkerOperation::RequestReload);
        self.reload_pending_observations = self.reload_latency;
        if !self.walker_enabled {
            return;
        }
        if self.stopped_on_owner {
            // Resumed in place only once the descriptor it stopped on
            // carries the owner bit again.
            self.reload_resumes_in_place = binding
                .descriptor_at_low(self.next_descriptor_low)
                .is_some_and(|stopped| stopped.word0().owner());
            return;
        }
        if self.next_descriptor_low != 0 {
            return;
        }
        if self.lose_reloads != 0 {
            if self.lose_reloads != u32::MAX {
                self.lose_reloads -= 1;
            }
            return;
        }
        self.reload_resumes_at = binding
            .descriptor_at_low(self.last_descriptor_low)
            .map(|last| last.next_address() & CURSOR_LOW_MASK)
            .filter(|&next| next != 0);
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

/// A walker model shared between the code under test and the test that
/// plays the hardware: each operation borrows the model for its own
/// duration only, so the test can move the cursors between two of them.
#[derive(Clone, Copy)]
pub struct SharedRxWalker<'a>(pub &'a core::cell::RefCell<RxWalkerModel>);

impl super::sealed::Sealed for SharedRxWalker<'_> {}

impl RxDma for SharedRxWalker<'_> {
    fn descriptor_base_register(&mut self) -> Option<u32> {
        self.0.borrow_mut().descriptor_base_register()
    }

    fn last_descriptor_low(&mut self) -> u32 {
        self.0.borrow_mut().last_descriptor_low()
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.0.borrow_mut().next_descriptor_low()
    }

    fn next_descriptor(&mut self) -> RxDmaNextDescriptor {
        self.0.borrow_mut().next_descriptor()
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        self.0.borrow_mut().with_ordered_cursor(observed)
    }

    fn walker_enabled(&mut self) -> bool {
        self.0.borrow_mut().walker_enabled()
    }

    fn reload_pending(&mut self) -> bool {
        self.0.borrow_mut().reload_pending()
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        self.0.borrow_mut().try_with_reload_settled(settled)
    }

    fn configure_descriptor_window(&mut self, binding: &RxDmaBinding<'_>) {
        self.0.borrow_mut().configure_descriptor_window(binding);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>) {
        self.0.borrow_mut().write_descriptor_base(binding);
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>) {
        self.0.borrow_mut().publish_walker_enable(binding);
    }

    fn request_reload(&mut self, binding: &RxDmaBinding<'_>) {
        self.0.borrow_mut().request_reload(binding);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        self.0
            .borrow_mut()
            .try_with_walker_enabled(binding, enabled)
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.0
            .borrow_mut()
            .try_with_walker_stopped(binding, stopped)
    }

    fn fence(&mut self) {
        self.0.borrow_mut().fence();
    }
}

impl RxWalkerModel {
    /// An enabled walker with an exhausted cursor fetches the published head.
    fn start_walker(&mut self) {
        self.walker_enabled = true;
        self.stopped_on_owner = false;
        self.reload_resumes_in_place = false;
        if self.next_descriptor_low == 0 {
            self.next_descriptor_low = self.descriptor_base & CURSOR_LOW_MASK;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ieee80211::dma::{
        rx_ring::{RxArenaWords, RxDescriptorTable},
        rx_storage::RxDmaStorage,
        space::{HOST_ARENA_BASE, HostDma},
    };
    use crate::layouts::WifiRxDmaDescriptorControl;

    extern crate std;

    #[test]
    fn the_model_follows_the_pac_walker_and_logs_in_order() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(
            RxDmaStorage::<2, 16, 20, HostDma>::new(),
        ));
        let placed = RxDmaStorage::place(crate::dma_static::Placed::host(storage)).unwrap();
        let storage = placed.storage();
        let base = placed.descriptor_base();
        let reserved = base + 2 * DESCRIPTOR_BYTES;
        let mut walker = RxWalkerModel::new();
        let live = placed
            .prepare_ring(&mut walker)
            .unwrap()
            .try_start(&mut walker)
            .map_err(|(_, error)| error)
            .unwrap();
        assert!(walker.walker_enabled);
        // BASE names the reserved descriptor, and the started walker fetches it.
        assert_eq!(walker.descriptor_base, reserved);
        // The diagnostic read reports the written base and logs nothing.
        let logged = walker.operations().len();
        assert_eq!(walker.descriptor_base_register(), Some(reserved));
        assert_eq!(walker.operations().len(), logged);
        assert_eq!(walker.next_descriptor_low, reserved & CURSOR_LOW_MASK);
        assert_eq!(
            walker.operation_count(RxWalkerOperation::WriteDescriptorBase(reserved)),
            1
        );
        assert_eq!(storage.reserved_descriptor().next_address(), base);

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

    const BASE: u32 = HOST_ARENA_BASE;

    /// A 16-byte descriptor the walker completed with `length` bytes as a
    /// unit's terminal.
    fn completed(length: u32) -> WifiRxDmaDescriptorControl {
        let armed = crate::ieee80211::dma::descriptor::rx_armed_word(16).unwrap();
        rx_completed_word(armed, length, true).unwrap()
    }

    /// A bound four-descriptor table at [`BASE`] whose descriptor 3 links to
    /// descriptor 1 and whose reserved descriptor links to descriptor 2.
    fn linked_binding() -> RxDmaBinding<'static> {
        let table = std::boxed::Box::leak(std::boxed::Box::new(RxDescriptorTable::<4>::new()));
        let arena = std::boxed::Box::leak(std::boxed::Box::new(RxArenaWords::new()));
        table.ring()[3].inject_corrupt_words(
            WifiRxDmaDescriptorControl::ZERO,
            0,
            BASE + DESCRIPTOR_BYTES,
        );
        table.reserved().inject_corrupt_words(
            WifiRxDmaDescriptorControl::ZERO,
            0,
            BASE + 2 * DESCRIPTOR_BYTES,
        );
        RxDmaBinding::new(table, BASE, arena).unwrap()
    }

    fn exhausted_at(last: usize) -> RxWalkerModel {
        let mut walker = RxWalkerModel::new();
        walker.walker_enabled = true;
        walker.set_cursor(BASE, last, None);
        walker
    }

    #[test]
    fn a_doorbell_on_an_exhausted_walker_resumes_it_at_last_next_when_it_clears() {
        let binding = linked_binding();
        let mut walker = exhausted_at(3);
        walker.reload_latency = 2;
        walker.request_reload(&binding);
        assert!(walker.reload_pending());
        assert_eq!(
            walker.next_descriptor_low, 0,
            "not before the doorbell clears"
        );
        assert!(walker.reload_pending());
        assert!(!walker.reload_pending());
        assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 1));

        // LAST naming the reserved descriptor resumes at its link.
        let mut walker = exhausted_at(4);
        walker.request_reload(&binding);
        assert!(walker.try_with_reload_settled(|_| ()).is_some());
        assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 2));
    }

    #[test]
    fn a_doorbell_resumes_nothing_with_next_nonzero_a_stopped_walker_or_a_zero_link() {
        let binding = linked_binding();
        let mut walker = exhausted_at(3);
        walker.set_cursor(BASE, 3, Some(0));
        walker.request_reload(&binding);
        assert!(!walker.reload_pending());
        assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 0));

        let mut walker = exhausted_at(3);
        walker.walker_enabled = false;
        walker.request_reload(&binding);
        assert!(!walker.reload_pending());
        assert_eq!(walker.next_descriptor_low, 0);

        // Descriptor 0's link is zero: there is nothing to resume at.
        let mut walker = exhausted_at(0);
        walker.request_reload(&binding);
        assert!(!walker.reload_pending());
        assert_eq!(walker.next_descriptor_low, 0);
    }

    #[test]
    fn lost_doorbells_leave_next_zero() {
        let binding = linked_binding();
        let mut walker = exhausted_at(3);
        walker.lose_reloads = 1;
        walker.request_reload(&binding);
        assert!(!walker.reload_pending());
        assert_eq!(walker.next_descriptor_low, 0, "the first doorbell is lost");
        walker.request_reload(&binding);
        assert!(!walker.reload_pending());
        assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 1));

        let mut walker = exhausted_at(3);
        walker.lose_reloads = u32::MAX;
        for _ in 0..3 {
            walker.request_reload(&binding);
            assert!(!walker.reload_pending());
        }
        assert_eq!(walker.next_descriptor_low, 0, "every doorbell is lost");
        assert_eq!(walker.lose_reloads, u32::MAX);
    }

    /// A bound four-descriptor table at [`BASE`] linked 0 → 1 → 2 → 3 → 0,
    /// every descriptor armed with a 16-byte buffer, and its binding.
    fn armed_table() -> (RxWalkerTable<'static>, RxDmaBinding<'static>) {
        let table = std::boxed::Box::leak(std::boxed::Box::new(RxDescriptorTable::<4>::new()));
        let arena = std::boxed::Box::leak(std::boxed::Box::new(RxArenaWords::new()));
        let armed = crate::ieee80211::dma::descriptor::rx_armed_word(16).unwrap();
        for (index, descriptor) in table.ring().iter().enumerate() {
            let next = BASE + ((index as u32 + 1) % 4) * DESCRIPTOR_BYTES;
            descriptor.inject_corrupt_words(armed, 0x2f01_0000, next);
        }
        let walker_table = RxWalkerTable::new(BASE, table.ring(), table.reserved());
        (walker_table, RxDmaBinding::new(table, BASE, arena).unwrap())
    }

    fn walking_at(next: usize) -> RxWalkerModel {
        let mut walker = RxWalkerModel::new();
        walker.walker_enabled = true;
        walker.set_cursor(BASE, 3, Some(next));
        walker
    }

    #[test]
    fn a_frame_lands_where_next_points_and_the_walker_follows_the_link() {
        let (table, _) = armed_table();
        let mut walker = walking_at(1);
        assert_eq!(
            walker.land(table, 5, false),
            RxWalkerLanding::Landed(descriptor_low(BASE, 1))
        );
        let first = table.ring[1].word0();
        assert_eq!((first.owner(), first.rx_done()), (true, false));
        assert_eq!(walker.last_descriptor_low, descriptor_low(BASE, 3));
        assert_eq!(
            walker.land(table, 7, true),
            RxWalkerLanding::Landed(descriptor_low(BASE, 2))
        );
        let terminal = table.ring[2].word0();
        assert_eq!(terminal, completed(7));
        assert_eq!(walker.last_descriptor_low, descriptor_low(BASE, 2));
        assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 3));

        let mut walker = walking_at(1);
        walker.next_descriptor_low = 0;
        assert_eq!(walker.land(table, 5, true), RxWalkerLanding::Exhausted);
    }

    #[test]
    fn the_walker_stops_on_a_clear_owner_bit_without_writing_it() {
        let (table, _) = armed_table();
        let detached = crate::ieee80211::dma::descriptor::rx_detached_word(completed(9));
        table.ring[1].write_word0(detached);
        let mut walker = walking_at(1);
        for _ in 0..3 {
            assert_eq!(
                walker.land(table, 5, true),
                RxWalkerLanding::StoppedOnOwner(descriptor_low(BASE, 1))
            );
        }
        assert!(walker.stopped_on_owner);
        assert_eq!(table.ring[1].word0(), detached, "never written");
        assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 1));
        assert_eq!(walker.last_descriptor_low, descriptor_low(BASE, 3));
    }

    #[test]
    fn a_rearmed_descriptor_and_one_doorbell_resume_the_stopped_walker_in_place() {
        let (table, binding) = armed_table();
        let completed = completed(9);
        table.ring[1].write_word0(crate::ieee80211::dma::descriptor::rx_detached_word(
            completed,
        ));
        let mut walker = walking_at(1);
        let stopped = RxWalkerLanding::StoppedOnOwner(descriptor_low(BASE, 1));
        assert_eq!(walker.land(table, 5, true), stopped);

        // A doorbell while the descriptor is still detached resumes nothing.
        walker.request_reload(&binding);
        assert!(!walker.reload_pending());
        assert_eq!(walker.land(table, 5, true), stopped);

        // Rearmed but no doorbell: still stopped.
        table.ring[1]
            .write_word0(crate::ieee80211::dma::descriptor::rx_rearm_word(completed).unwrap());
        assert_eq!(walker.land(table, 5, true), stopped);

        // One doorbell resumes it in place, once it clears.
        walker.reload_latency = 1;
        walker.request_reload(&binding);
        assert!(walker.reload_pending());
        assert_eq!(walker.land(table, 5, true), stopped);
        assert!(!walker.reload_pending());
        assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 1));
        assert_eq!(
            walker.land(table, 5, true),
            RxWalkerLanding::Landed(descriptor_low(BASE, 1))
        );
        assert!(!walker.stopped_on_owner);
    }

    #[test]
    fn a_doorbell_follows_last_next_whatever_last_s_owner_bit() {
        let (table, binding) = armed_table();
        // LAST, descriptor 3, is detached: its owner bit is clear.
        table.ring[3].write_word0(crate::ieee80211::dma::descriptor::rx_detached_word(
            completed(9),
        ));
        let mut walker = exhausted_at(3);
        walker.request_reload(&binding);
        assert!(!walker.reload_pending());
        assert_eq!(walker.next_descriptor_low, descriptor_low(BASE, 0));
        assert_eq!(
            walker.land(table, 5, true),
            RxWalkerLanding::Landed(descriptor_low(BASE, 0))
        );
    }

    /// A test's event log the walker journals into, beside its own events.
    struct Events(core::cell::RefCell<std::vec::Vec<&'static str>>);

    impl RxWalkerJournal for Events {
        fn record(&self, operation: RxWalkerOperation) {
            self.0.borrow_mut().push(match operation {
                RxWalkerOperation::Stop => "walker stop",
                RxWalkerOperation::RequestReload => "doorbell",
                _ => "walker",
            });
        }
    }

    #[test]
    fn a_shared_walker_journals_its_operations_among_the_test_s_events() {
        let binding = linked_binding();
        let events: &'static Events = std::boxed::Box::leak(std::boxed::Box::new(Events(
            core::cell::RefCell::new(std::vec::Vec::new()),
        )));
        let model = core::cell::RefCell::new(exhausted_at(3));
        model.borrow_mut().set_journal(events);
        let mut walker = SharedRxWalker(&model);

        events.0.borrow_mut().push("mac stop");
        walker.request_reload(&binding);
        // The test plays the hardware between two operations.
        model.borrow_mut().set_cursor(BASE, 3, Some(0));
        assert_eq!(walker.next_descriptor_low(), descriptor_low(BASE, 0));
        assert!(walker.try_with_walker_stopped(&binding, |_| ()).is_some());
        events.0.borrow_mut().push("mac resume");

        assert_eq!(
            *events.0.borrow(),
            [
                "mac stop",
                "doorbell",
                "walker",
                "walker stop",
                "mac resume"
            ]
        );
        // The model's own log holds the same operations.
        assert_eq!(
            model.borrow().operations(),
            [
                RxWalkerOperation::RequestReload,
                RxWalkerOperation::ObserveNext,
                RxWalkerOperation::Stop,
            ]
        );
    }
}

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
//! SOURCE: HIL_W1_RX_WALKER_2026_10_18 in registers/evidence/hil-open.toml:
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
use crate::radio::ieee80211::dma::descriptor::{DESCRIPTOR_BYTES, rx_completed_word};

/// The walker's cursor registers report the low 20 address bits.
const CURSOR_LOW_MASK: u32 = crate::radio::ieee80211::registers::WifiDescriptorAddressLow::MAX;

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

/// Whether the walker stopped on NEXT, a descriptor whose owner bit was
/// clear, and whether a rung doorbell resumes it there.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerStop {
    /// Not stopped.
    Running,
    /// Stopped on NEXT.
    Stopped,
    /// Stopped on NEXT, rearmed, with a doorbell rung: it resumes in place
    /// when the doorbell clears.
    Rearmed,
}

/// `RX_CONTROL.WALKER_ENABLE` of the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WalkerEnable {
    Disabled,
    Enabled,
}

/// Whether `RX_NEXT_DESCRIPTOR` reports unassigned upper bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NextUpperBits {
    Assigned,
    Unknown,
}

/// Whether the reload doorbell clears.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReloadDoorbell {
    Clears,
    /// The doorbell never clears.
    Stuck,
}

/// What a stop request does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopRequest {
    Obeyed,
    /// The walker keeps running and reads back as enabled.
    Refused,
}

/// Register-level model of the RX descriptor walker.
pub struct RxWalkerModel {
    /// `RX_LAST_DESCRIPTOR` low address bits.
    pub last_descriptor_low: u32,
    /// `RX_NEXT_DESCRIPTOR` low address bits.
    pub next_descriptor_low: u32,
    /// Whether `RX_NEXT_DESCRIPTOR` reports unassigned upper bits.
    pub next_upper_bits: NextUpperBits,
    /// `RX_DESCRIPTOR_BASE` as last written.
    pub descriptor_base: u32,
    /// `RX_CONTROL.WALKER_ENABLE`.
    pub walker_enabled: WalkerEnable,
    /// Observations of the reload doorbell still to read as pending.
    pub reload_pending_observations: u32,
    /// Pending observations each [`RxDma::request_reload`] sets.
    pub reload_latency: u32,
    /// Whether the doorbell clears.
    pub reload_doorbell: ReloadDoorbell,
    /// Doorbells still to lose: each leaves NEXT unchanged instead of
    /// resuming an exhausted walker. `u32::MAX` loses every doorbell.
    pub lose_reloads: u32,
    /// The NEXT a rung doorbell applies when it clears.
    reload_resumes_at: Option<u32>,
    /// Whether the walker stopped on NEXT, and resumes there.
    pub owner_stop: OwnerStop,
    /// What a stop request does.
    pub stop_request: StopRequest,
    /// What an enable of a stopped walker does.
    pub start: RxWalkerStart,
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
            next_upper_bits: NextUpperBits::Assigned,
            descriptor_base: 0,
            walker_enabled: WalkerEnable::Disabled,
            reload_pending_observations: 0,
            reload_latency: 0,
            reload_doorbell: ReloadDoorbell::Clears,
            lose_reloads: 0,
            reload_resumes_at: None,
            owner_stop: OwnerStop::Running,
            stop_request: StopRequest::Obeyed,
            start: RxWalkerStart::Confirmed,
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

    /// The descriptor [`Self::land`] writes next: enabled, on NEXT, its
    /// owner bit set. `None` when a landing writes nothing.
    pub(crate) fn armed_next<'storage>(
        &self,
        table: RxWalkerTable<'storage>,
    ) -> Option<&'storage crate::radio::ieee80211::dma::descriptor::RxDescriptor> {
        if self.walker_enabled == WalkerEnable::Disabled
            || self.next_descriptor_low == 0
            || self.owner_stop != OwnerStop::Running
        {
            return None;
        }
        table
            .descriptor_at_low(self.next_descriptor_low)
            .filter(|descriptor| descriptor.word0().owner())
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
        if self.walker_enabled == WalkerEnable::Disabled || next == 0 {
            return RxWalkerLanding::Exhausted;
        }
        let Some(descriptor) = table.descriptor_at_low(next) else {
            return RxWalkerLanding::Exhausted;
        };
        if self.owner_stop != OwnerStop::Running || !descriptor.word0().owner() {
            if self.owner_stop == OwnerStop::Running {
                self.owner_stop = OwnerStop::Stopped;
            }
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
        if self.reload_doorbell == ReloadDoorbell::Stuck {
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
                if self.owner_stop == OwnerStop::Rearmed {
                    self.owner_stop = OwnerStop::Running;
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
        RxDmaNextDescriptor::from_fields(
            address_low,
            self.next_upper_bits == NextUpperBits::Unknown,
        )
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
        self.walker_enabled == WalkerEnable::Enabled
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
        if self.walker_enabled == WalkerEnable::Disabled {
            return;
        }
        if self.owner_stop != OwnerStop::Running {
            // Resumed in place only once the descriptor it stopped on
            // carries the owner bit again.
            let rearmed = binding
                .descriptor_at_low(self.next_descriptor_low)
                .is_some_and(|stopped| stopped.word0().owner());
            self.owner_stop = if rearmed {
                OwnerStop::Rearmed
            } else {
                OwnerStop::Stopped
            };
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
        if self.walker_enabled == WalkerEnable::Enabled {
            return None;
        }
        match self.start {
            RxWalkerStart::Confirmed => {
                self.start_walker();
                Some(enabled(RxDmaWalkerEnabled::confirmed()))
            }
            RxWalkerStart::Refused => None,
            RxWalkerStart::Unconfirmed => {
                self.start_walker();
                None
            }
        }
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.record(RxWalkerOperation::Stop);
        if self.stop_request == StopRequest::Refused {
            return None;
        }
        self.walker_enabled = WalkerEnable::Disabled;
        Some(stopped(RxDmaWalkerStopped::confirmed(binding)))
    }

    fn fence(&mut self) {
        self.record(RxWalkerOperation::Fence);
    }
}

/// What [`RxWalkerModel`]'s enable of a stopped walker does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxWalkerStart {
    /// The walker starts and the enable reads back confirmed.
    Confirmed,
    /// The walker stays stopped and reads so; no proof comes back.
    Refused,
    /// The walker starts, yet no proof comes back.
    Unconfirmed,
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
        self.walker_enabled = WalkerEnable::Enabled;
        self.owner_stop = OwnerStop::Running;
        if self.next_descriptor_low == 0 {
            self.next_descriptor_low = self.descriptor_base & CURSOR_LOW_MASK;
        }
    }
}

#[cfg(test)]
mod tests;

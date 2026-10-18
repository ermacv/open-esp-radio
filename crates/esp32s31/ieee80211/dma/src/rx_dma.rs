//! Semantic MMIO contract for the ESP32-S31 RX descriptor walker.
//!
//! The contract exposes finite RX-DMA operations instead of register
//! identities. Descriptor-ring ownership is modeled separately, so production
//! and host models share the same state machine without exposing the generated
//! PAC above the register leaf.

use core::marker::PhantomData;

use crate::{
    descriptor::DESCRIPTOR_BYTES,
    rx_ring::{RxArenaWords, RxDescriptorTable},
};

use oer_memory::StableDmaRange;

use oer_esp32s31_hal::{ieee80211::mac::WifiMacHal, owner::RadioRuntimeOwner};

/// Unforgeable authority for mutating one validated RX descriptor walker.
///
/// Public trait methods accept this type so owning a runtime radio capability
/// alone is insufficient to publish an arbitrary DMA address. Constructors remain
/// inside the chip DMA leaf and ring typestates keep the value private.
///
/// The binding names one descriptor table: the ring and the reserved
/// descriptor R after it. [`RxDma::write_descriptor_base`] publishes R and
/// nothing else, so a ring descriptor can never be written to
/// `RX_DESCRIPTOR_BASE`: every live publication goes through the reload
/// doorbell.
pub struct RxDmaBinding<'storage> {
    reserved_address: u32,
    /// The table, for a model walker that follows its links.
    #[cfg(any(test, feature = "test-support"))]
    table: RxWalkerTable<'storage>,
    range: StableDmaRange<'storage>,
    arena: &'storage RxArenaWords,
}

/// Opaque proof that the RX walker disable request completed and read back
/// as stopped, naming the arena whose binding the stop was requested for.
///
/// Production code can obtain this value only from the PAC-backed
/// [`RxDma`] implementation, which builds it from the binding it was handed.
/// A ring accepts the proof only when it names the ring's own arena, so a
/// forwarding layer which stops the walker for another binding cannot
/// authorize a descriptor rebuild. Native models construct a model proof
/// for deterministic state-machine tests.
///
/// The proof stays on the core that observed the walker: it is not `Send`.
/// ```compile_fail,E0277
/// fn send<T: Send>() {}
/// send::<oer_esp32s31_ieee80211_dma::rx_dma::RxDmaWalkerStopped<'static>>();
/// ```
pub struct RxDmaWalkerStopped<'confirmation> {
    arena: &'confirmation RxArenaWords,
    _not_send: PhantomData<*mut ()>,
}

/// Opaque current proof that the RX walker enable edge was accepted.
pub struct RxDmaWalkerEnabled<'confirmation> {
    _confirmation: PhantomData<&'confirmation mut ()>,
}

impl RxDmaWalkerEnabled<'_> {
    const fn confirmed() -> Self {
        Self {
            _confirmation: PhantomData,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn for_host_test() -> Self {
        Self::confirmed()
    }
}

/// Opaque current proof that the append/reload doorbell has self-cleared.
pub struct RxDmaReloadSettled<'confirmation> {
    _confirmation: PhantomData<&'confirmation mut ()>,
}

impl RxDmaReloadSettled<'_> {
    const fn confirmed() -> Self {
        Self {
            _confirmation: PhantomData,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn for_host_test() -> Self {
        Self::confirmed()
    }
}

impl<'confirmation> RxDmaWalkerStopped<'confirmation> {
    pub(crate) const fn confirmed(binding: &'confirmation RxDmaBinding<'_>) -> Self {
        Self {
            arena: binding.arena,
            _not_send: PhantomData,
        }
    }

    /// A model walker's stop proof for the binding it was handed.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn for_host_test(binding: &'confirmation RxDmaBinding<'_>) -> Self {
        Self::confirmed(binding)
    }

    /// Whether this proof was issued for `arena`'s binding.
    pub(crate) fn names(&self, arena: &RxArenaWords) -> bool {
        core::ptr::eq(self.arena, arena)
    }
}

/// One ordered RX walker cursor observation.
///
/// The PAC implementation samples `LAST`, executes a device fence, samples
/// `NEXT`, then executes the second fence before invoking the callback. The
/// confirmation lifetime prevents safe forwarding layers from retaining and
/// replaying a stale observation as current ownership evidence.
pub struct RxDmaCursorObservation<'confirmation> {
    last_descriptor_low: u32,
    next_descriptor_low: u32,
    _confirmation: PhantomData<&'confirmation mut ()>,
}

/// Semantic field observation of the hardware next-descriptor state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxDmaNextDescriptor {
    address_low: u32,
    unknown_upper_nonzero: bool,
}

impl RxDmaNextDescriptor {
    const fn from_fields(address_low: u32, unknown_upper_nonzero: bool) -> Self {
        Self {
            address_low,
            unknown_upper_nonzero,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn for_host_test(address_low: u32, unknown_upper_nonzero: bool) -> Self {
        Self::from_fields(address_low, unknown_upper_nonzero)
    }

    pub const fn address_low(self) -> u32 {
        self.address_low
    }

    pub const fn is_zero(self) -> bool {
        self.address_low == 0 && !self.unknown_upper_nonzero
    }

    pub const fn has_unknown_upper_state(self) -> bool {
        self.unknown_upper_nonzero
    }
}

/// Ordered observation used only by the reload settle.
///
/// `wDev_AppendRxBlocks` samples `NEXT` first and reads `LAST` only when the
/// walker reports a zero successor; the ring keeps that order to decide
/// whether a settled doorbell needs one re-ring. This is intentionally a
/// different contract from [`RxDmaCursorObservation`], whose `LAST -> NEXT`
/// order proves release of a completed descriptor link. Combining the two
/// observations can pair values from different hardware epochs.
pub struct RxDmaReloadObservation<'confirmation> {
    next_descriptor: RxDmaNextDescriptor,
    last_descriptor_low: Option<u32>,
    _confirmation: PhantomData<&'confirmation mut ()>,
}

impl RxDmaReloadObservation<'_> {
    const fn confirmed(
        next_descriptor: RxDmaNextDescriptor,
        last_descriptor_low: Option<u32>,
    ) -> Self {
        Self {
            next_descriptor,
            last_descriptor_low,
            _confirmation: PhantomData,
        }
    }

    pub const fn next_descriptor(&self) -> RxDmaNextDescriptor {
        self.next_descriptor
    }

    pub const fn next_descriptor_low(&self) -> u32 {
        self.next_descriptor.address_low()
    }

    /// Return `LAST` only for the vendor branch where the preceding `NEXT`
    /// observation was zero.
    pub const fn exhausted_last_descriptor_low(&self) -> Option<u32> {
        self.last_descriptor_low
    }
}

impl RxDmaCursorObservation<'_> {
    const fn confirmed(last_descriptor_low: u32, next_descriptor_low: u32) -> Self {
        Self {
            last_descriptor_low,
            next_descriptor_low,
            _confirmation: PhantomData,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub const fn for_host_test(last_descriptor_low: u32, next_descriptor_low: u32) -> Self {
        Self::confirmed(last_descriptor_low, next_descriptor_low)
    }

    pub const fn last_descriptor_low(&self) -> u32 {
        self.last_descriptor_low
    }

    pub const fn next_descriptor_low(&self) -> u32 {
        self.next_descriptor_low
    }
}

/// The descriptors a model walker resolves its cursors against: the ring
/// at `descriptor_base` and the reserved descriptor right after it (test
/// support only).
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy)]
pub struct RxWalkerTable<'storage> {
    descriptor_base: u32,
    ring: &'storage [crate::descriptor::RxDescriptor],
    reserved: &'storage crate::descriptor::RxDescriptor,
}

#[cfg(any(test, feature = "test-support"))]
impl<'storage> RxWalkerTable<'storage> {
    pub(crate) const fn new(
        descriptor_base: u32,
        ring: &'storage [crate::descriptor::RxDescriptor],
        reserved: &'storage crate::descriptor::RxDescriptor,
    ) -> Self {
        Self {
            descriptor_base,
            ring,
            reserved,
        }
    }

    /// The descriptor at the low address bits `low`, which the walker's
    /// LAST/NEXT registers report, when it belongs to this table.
    pub fn descriptor_at_low(&self, low: u32) -> Option<&'storage crate::descriptor::RxDescriptor> {
        const LOW_MASK: u32 = 0x000f_ffff;
        let reserved_offset = u32::try_from(self.ring.len())
            .ok()?
            .checked_mul(DESCRIPTOR_BYTES)?;
        if low == self.descriptor_base.wrapping_add(reserved_offset) & LOW_MASK {
            return Some(self.reserved);
        }
        let offset = low.checked_sub(self.descriptor_base & LOW_MASK)?;
        if offset % DESCRIPTOR_BYTES != 0 {
            return None;
        }
        self.ring
            .get(usize::try_from(offset / DESCRIPTOR_BYTES).ok()?)
    }
}

impl<'storage> RxDmaBinding<'storage> {
    #[allow(
        unsafe_code,
        reason = "ring construction retains the descriptor owner for the DMA epoch"
    )]
    pub(crate) fn new<const COUNT: usize>(
        table: &'storage RxDescriptorTable<COUNT>,
        descriptor_base: u32,
        arena: &'storage RxArenaWords,
    ) -> Option<Self> {
        if COUNT == 0 || u8::try_from(COUNT).is_err() {
            return None;
        }
        let range_len = u32::try_from(core::mem::size_of_val(table)).ok()?;
        // `RxDescriptorTable` is `repr(C)`: COUNT 12-byte, 4-aligned ring
        // descriptors, then R, so R lies exactly one descriptor after the
        // ring and the table has no padding.
        let reserved_offset = u32::try_from(COUNT).ok()?.checked_mul(DESCRIPTOR_BYTES)?;
        if range_len != reserved_offset.checked_add(DESCRIPTOR_BYTES)? {
            return None;
        }
        let reserved_address = descriptor_base.checked_add(reserved_offset)?;
        // SAFETY: the binding retains the descriptor-table borrow. Only
        // `RxDmaStorage` lends it, and its first ring requires static
        // storage (`prepare_ring`). Native models have no asynchronous DMA
        // actor.
        let range = unsafe { StableDmaRange::from_owner(table, descriptor_base, range_len) }?;
        Some(Self {
            reserved_address,
            #[cfg(any(test, feature = "test-support"))]
            table: RxWalkerTable::new(descriptor_base, table.ring(), table.reserved()),
            range,
            arena,
        })
    }

    /// The DMA address of the reserved descriptor R, the only value
    /// `RX_DESCRIPTOR_BASE` is ever given.
    pub const fn reserved_address(&self) -> u32 {
        self.reserved_address
    }

    /// The descriptor at the low address bits `low`, which the walker's
    /// LAST/NEXT registers report, when it belongs to this table.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn descriptor_at_low(
        &self,
        low: u32,
    ) -> Option<&'storage crate::descriptor::RxDescriptor> {
        self.table.descriptor_at_low(low)
    }

    fn range(&self) -> &StableDmaRange<'storage> {
        &self.range
    }
}

/// Semantic ownership boundary for the S31 RX descriptor walker.
///
/// Production uses the generated PAC implementation below. Host tests model
/// these finite operations without receiving arbitrary register identities.
pub trait RxDma {
    /// Optional monotonic hardware starvation counter for boundary telemetry.
    ///
    /// This observation never participates in descriptor ownership. Host
    /// models and platforms without such a counter keep the default `None`.
    fn buffer_full_count(&mut self) -> Option<u16> {
        None
    }

    /// Optional read of `RX_DESCRIPTOR_BASE` for diagnostics.
    ///
    /// Like [`Self::buffer_full_count`] it never participates in descriptor
    /// ownership: a reading says what the register holds, not which list
    /// the walker follows. Models without such a reading keep `None`.
    fn descriptor_base_register(&mut self) -> Option<u32> {
        None
    }

    fn last_descriptor_low(&mut self) -> u32;
    fn next_descriptor_low(&mut self) -> u32;

    /// Read the address and unassigned upper fields in one hardware sample.
    fn next_descriptor(&mut self) -> RxDmaNextDescriptor;
    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R;

    /// Preserve the exact reload observation order recovered from
    /// `wDev_AppendRxBlocks`: `NEXT`, a device fence, and conditional `LAST`.
    fn with_reload_observation<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaReloadObservation<'confirmation>) -> R,
    ) -> R {
        let next_descriptor = self.next_descriptor();
        self.fence();
        let last_descriptor_low = if next_descriptor.is_zero() {
            let last_descriptor_low = self.last_descriptor_low();
            self.fence();
            Some(last_descriptor_low)
        } else {
            None
        };
        observed(RxDmaReloadObservation::confirmed(
            next_descriptor,
            last_descriptor_low,
        ))
    }
    fn walker_enabled(&mut self) -> bool;
    fn reload_pending(&mut self) -> bool;
    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R>;
    fn configure_descriptor_window(&mut self, binding: &RxDmaBinding<'_>);
    /// Write the binding's reserved descriptor R to `RX_DESCRIPTOR_BASE`.
    ///
    /// The cold publication of an epoch is the only caller: one BASE write
    /// per epoch, always R.
    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>);
    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>);
    fn request_reload(&mut self, binding: &RxDmaBinding<'_>);
    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R>;
    /// Stop the walker for `binding` and pass a proof naming its arena.
    ///
    /// `None` means the walker did not read back as stopped.
    fn try_with_walker_stopped<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R>;
    fn fence(&mut self);
}

impl RxDma for WifiMacHal<'_> {
    fn buffer_full_count(&mut self) -> Option<u16> {
        Some(self.rx_buffer_full_count())
    }

    fn descriptor_base_register(&mut self) -> Option<u32> {
        Some(self.receive_dma_snapshot().descriptor_base)
    }

    fn last_descriptor_low(&mut self) -> u32 {
        self.rx_last_descriptor_low()
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.rx_next_descriptor_low()
    }

    fn next_descriptor(&mut self) -> RxDmaNextDescriptor {
        let observation = self.rx_next_descriptor();
        RxDmaNextDescriptor::from_fields(
            observation.address_low(),
            observation.has_unknown_upper_state(),
        )
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        let last_descriptor_low = self.rx_last_descriptor_low();
        self.order_device_accesses();
        let next_descriptor_low = self.rx_next_descriptor_low();
        self.order_device_accesses();
        observed(RxDmaCursorObservation::confirmed(
            last_descriptor_low,
            next_descriptor_low,
        ))
    }

    fn walker_enabled(&mut self) -> bool {
        self.rx_walker_enabled()
    }

    fn reload_pending(&mut self) -> bool {
        self.rx_reload_pending()
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        (!self.rx_reload_pending()).then(|| settled(RxDmaReloadSettled::confirmed()))
    }

    fn configure_descriptor_window(&mut self, binding: &RxDmaBinding<'_>) {
        self.configure_rx_descriptor_window(binding.range());
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>) {
        self.write_rx_descriptor_base(binding.range(), binding.reserved_address());
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>) {
        self.publish_rx_walker_enable(binding.range());
    }

    fn request_reload(&mut self, binding: &RxDmaBinding<'_>) {
        self.request_rx_descriptor_reload(binding.range());
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        self.try_enable_rx_walker(binding.range())
            .then(|| enabled(RxDmaWalkerEnabled::confirmed()))
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.try_disable_rx_walker()
            .then(|| stopped(RxDmaWalkerStopped::confirmed(binding)))
    }

    fn fence(&mut self) {
        self.order_device_accesses();
    }
}

impl RxDma for RadioRuntimeOwner {
    fn buffer_full_count(&mut self) -> Option<u16> {
        RxDma::buffer_full_count(&mut self.wifi_mac_hal())
    }

    fn descriptor_base_register(&mut self) -> Option<u32> {
        RxDma::descriptor_base_register(&mut self.wifi_mac_hal())
    }

    fn last_descriptor_low(&mut self) -> u32 {
        RxDma::last_descriptor_low(&mut self.wifi_mac_hal())
    }

    fn next_descriptor_low(&mut self) -> u32 {
        RxDma::next_descriptor_low(&mut self.wifi_mac_hal())
    }

    fn next_descriptor(&mut self) -> RxDmaNextDescriptor {
        RxDma::next_descriptor(&mut self.wifi_mac_hal())
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R {
        RxDma::with_ordered_cursor(&mut self.wifi_mac_hal(), observed)
    }

    fn walker_enabled(&mut self) -> bool {
        RxDma::walker_enabled(&mut self.wifi_mac_hal())
    }

    fn reload_pending(&mut self) -> bool {
        RxDma::reload_pending(&mut self.wifi_mac_hal())
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_reload_settled(&mut self.wifi_mac_hal(), settled)
    }

    fn configure_descriptor_window(&mut self, binding: &RxDmaBinding<'_>) {
        RxDma::configure_descriptor_window(&mut self.wifi_mac_hal(), binding);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>) {
        RxDma::write_descriptor_base(&mut self.wifi_mac_hal(), binding);
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>) {
        RxDma::publish_walker_enable(&mut self.wifi_mac_hal(), binding);
    }

    fn request_reload(&mut self, binding: &RxDmaBinding<'_>) {
        RxDma::request_reload(&mut self.wifi_mac_hal(), binding);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_walker_enabled(&mut self.wifi_mac_hal(), binding, enabled)
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_walker_stopped(&mut self.wifi_mac_hal(), binding, stopped)
    }

    fn fence(&mut self) {
        RxDma::fence(&mut self.wifi_mac_hal());
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod model;

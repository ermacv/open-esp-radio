//! Semantic MMIO contract for the ESP32-S31 RX descriptor walker.
//!
//! The contract exposes finite RX-DMA operations instead of register
//! identities. Descriptor-ring ownership is modeled separately, so production
//! and host models share the same state machine without exposing the generated
//! PAC above the register leaf.

use core::marker::PhantomData;

use crate::descriptor::{DESCRIPTOR_BYTES, Descriptor};

use oer_memory::StableDmaRange;

use oer_esp32s31_hal::{ieee80211::mac::WifiMacHal, owner::RadioRuntimeOwner};

/// Unforgeable authority for mutating one validated RX descriptor walker.
///
/// Public trait methods accept this type so owning a runtime radio capability
/// alone is insufficient to publish an arbitrary DMA address. Constructors remain
/// inside the chip DMA leaf and ring typestates keep the value private.
pub struct RxDmaBinding<'storage> {
    descriptor_base: u32,
    descriptor_count: u8,
    range: StableDmaRange<'storage>,
}

/// Opaque proof that the RX walker disable request completed and read back
/// as stopped.
///
/// Production code can obtain this value only from the PAC-backed
/// [`RxDma`] implementation. Native models and explicit raw-DMA validation
/// may construct a model proof for deterministic state-machine tests.
pub struct RxDmaWalkerStopped<'confirmation> {
    _confirmation: PhantomData<&'confirmation mut ()>,
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

    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub const fn validation() -> Self {
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

    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub const fn validation() -> Self {
        Self::confirmed()
    }
}

impl RxDmaWalkerStopped<'_> {
    const fn confirmed() -> Self {
        Self {
            _confirmation: PhantomData,
        }
    }

    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub const fn validation() -> Self {
        Self::confirmed()
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

    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub const fn validation(address_low: u32, unknown_upper_nonzero: bool) -> Self {
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

/// Ordered observation used only by the vendor reload-repair suffix.
///
/// `wDev_AppendRxBlocks` samples `NEXT` first and reads `LAST` only when the
/// walker reports a zero successor. This is intentionally a different
/// contract from [`RxDmaCursorObservation`], whose `LAST -> NEXT` order proves
/// release of a completed descriptor link. Combining the two observations
/// can pair values from different hardware epochs and authorize a stale BASE
/// repair.
pub struct RxDmaReloadRepairObservation<'confirmation> {
    next_descriptor: RxDmaNextDescriptor,
    last_descriptor_low: Option<u32>,
    _confirmation: PhantomData<&'confirmation mut ()>,
}

impl RxDmaReloadRepairObservation<'_> {
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

    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub const fn validation(last_descriptor_low: u32, next_descriptor_low: u32) -> Self {
        Self::confirmed(last_descriptor_low, next_descriptor_low)
    }

    pub const fn last_descriptor_low(&self) -> u32 {
        self.last_descriptor_low
    }

    pub const fn next_descriptor_low(&self) -> u32 {
        self.next_descriptor_low
    }
}

impl<'storage> RxDmaBinding<'storage> {
    #[allow(
        unsafe_code,
        reason = "ring construction retains the descriptor owner for the DMA epoch"
    )]
    pub(crate) fn new<const COUNT: usize>(
        descriptors: &'storage [Descriptor; COUNT],
        descriptor_base: u32,
    ) -> Option<Self> {
        let descriptor_count = u8::try_from(COUNT).ok()?;
        let range_len = u32::try_from(core::mem::size_of_val(descriptors)).ok()?;
        // SAFETY: the binding retains the descriptor borrow. The safe target
        // entry requires static `RxDmaStorage`; the raw target entry states
        // the equivalent allocation/lifetime proof in its unsafe contract.
        // Native entry points have no asynchronous DMA actor.
        let range = unsafe { StableDmaRange::from_owner(descriptors, descriptor_base, range_len) }?;
        (descriptor_count != 0).then_some(Self {
            descriptor_base,
            descriptor_count,
            range,
        })
    }

    #[allow(
        unsafe_code,
        reason = "raw target publication is already an unsafe validation boundary"
    )]
    #[cfg(any(test, feature = "test-support", feature = "validation-raw-dma"))]
    pub(crate) fn raw_validation(descriptor_base: u32) -> RxDmaBinding<'static> {
        // SAFETY: target callers cross an unsafe public API and retain the
        // addressed ring; native callers use a model with no DMA actor.
        let range = unsafe {
            StableDmaRange::from_raw_parts(descriptor_base, DESCRIPTOR_BYTES)
                .expect("one descriptor always forms a non-empty DMA range")
        };
        RxDmaBinding {
            descriptor_base,
            descriptor_count: 1,
            range,
        }
    }

    fn admits(&self, address: u32) -> bool {
        let Some(offset) = address.checked_sub(self.descriptor_base) else {
            return false;
        };
        offset % DESCRIPTOR_BYTES == 0
            && offset / DESCRIPTOR_BYTES < u32::from(self.descriptor_count)
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

    fn last_descriptor_low(&mut self) -> u32;
    fn next_descriptor_low(&mut self) -> u32;

    /// Read the address and unassigned upper fields in one hardware sample.
    fn next_descriptor(&mut self) -> RxDmaNextDescriptor;
    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaCursorObservation<'confirmation>) -> R,
    ) -> R;

    /// Preserve the exact reload-repair observation order recovered from
    /// `wDev_AppendRxBlocks`: `NEXT`, a device fence, and conditional `LAST`.
    fn with_reload_repair_observation<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(RxDmaReloadRepairObservation<'confirmation>) -> R,
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
        observed(RxDmaReloadRepairObservation::confirmed(
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
    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32);
    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>);
    fn request_reload(&mut self, binding: &RxDmaBinding<'_>);
    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R>;
    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R>;
    fn fence(&mut self);
}

impl RxDma for WifiMacHal<'_> {
    fn buffer_full_count(&mut self) -> Option<u16> {
        Some(self.rx_buffer_full_count())
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

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32) {
        assert!(
            binding.admits(address),
            "RX descriptor base must belong to the bound static ring"
        );
        self.write_rx_descriptor_base(binding.range(), address);
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
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.try_disable_rx_walker()
            .then(|| stopped(RxDmaWalkerStopped::confirmed()))
    }

    fn fence(&mut self) {
        self.order_device_accesses();
    }
}

impl RxDma for RadioRuntimeOwner {
    fn buffer_full_count(&mut self) -> Option<u16> {
        RxDma::buffer_full_count(&mut self.wifi_mac_hal())
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

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32) {
        RxDma::write_descriptor_base(&mut self.wifi_mac_hal(), binding, address);
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
        stopped: impl for<'confirmation> FnOnce(RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        RxDma::try_with_walker_stopped(&mut self.wifi_mac_hal(), stopped)
    }

    fn fence(&mut self) {
        RxDma::fence(&mut self.wifi_mac_hal());
    }
}

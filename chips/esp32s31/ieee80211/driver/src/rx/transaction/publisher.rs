//! Affine publication boundary; endpoint state remains with the adapter.

use super::{CompletedUnit, Preview};
use crate::mac::rx::pool::NetworkRxFrame;

/// A single ordered publisher borrowing the original staged DMA allocation.
///
/// Success transfers the frame to the endpoint. Rejection returns that same
/// frame; the physical transaction retains responsibility for its release.
/// This contract supplies no descriptor or hardware mutation authority.
pub trait Publisher<'pool, const CAPACITY: usize, const SLOTS: usize> {
    const DEPTH: usize;
    fn free_capacity(&self) -> usize;
    fn preview(&self, unit: CompletedUnit, bytes: [u8; 24]) -> Preview;
    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview;
    /// Transfer the affine frame, returning it intact if publication fails.
    ///
    /// The caller can forward the result while preserving rejection ownership:
    ///
    /// ```no_run
    /// # #![cfg_attr(target_arch = "riscv32", no_std)]
    /// # #![cfg_attr(target_arch = "riscv32", no_main)]
    /// # #[cfg(target_arch = "riscv32")]
    /// # #[panic_handler]
    /// # fn panic(_: &core::panic::PanicInfo) -> ! {
    /// #     loop {}
    /// # }
    /// use oer_esp32s31_ieee80211_driver::rx::transaction::Publisher;
    /// use oer_esp32s31_ieee80211_driver::mac::rx::pool::NetworkRxFrame;
    ///
    /// fn publish<'pool, P: Publisher<'pool, 64, 1>>(
    ///     publisher: &P,
    ///     frame: NetworkRxFrame<'pool, 1, 64>,
    /// ) -> Result<(), NetworkRxFrame<'pool, 1, 64>> {
    ///     publisher.try_send(frame)
    /// }
    /// ```
    ///
    /// After transfer, only an `Err` result can restore the caller's frame:
    ///
    /// ```compile_fail,E0382
    /// # #![cfg_attr(target_arch = "riscv32", no_std)]
    /// # #![cfg_attr(target_arch = "riscv32", no_main)]
    /// # #[cfg(target_arch = "riscv32")]
    /// # #[panic_handler]
    /// # fn panic(_: &core::panic::PanicInfo) -> ! {
    /// #     loop {}
    /// # }
    /// use oer_esp32s31_ieee80211_driver::rx::transaction::Publisher;
    /// use oer_esp32s31_ieee80211_driver::mac::rx::pool::NetworkRxFrame;
    ///
    /// fn publish<'pool, P: Publisher<'pool, 64, 1>>(
    ///     publisher: &P,
    ///     frame: NetworkRxFrame<'pool, 1, 64>,
    /// ) -> Result<(), NetworkRxFrame<'pool, 1, 64>> {
    ///     let result = publisher.try_send(frame);
    ///     let _ = frame.segment(); // The endpoint or result now owns this lease.
    ///     result
    /// }
    /// ```
    fn try_send(
        &self,
        frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    ) -> Result<(), NetworkRxFrame<'pool, SLOTS, CAPACITY>>;
}

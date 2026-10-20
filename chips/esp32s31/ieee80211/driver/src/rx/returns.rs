//! The return of a staged RX lease wakes the receive runner.
//!
//! A frame handed above the radio executor keeps its original DMA buffer.
//! While the upper layer holds leases, the ring cannot rearm their
//! descriptors, nor any descriptor after them: the walker can run out, and
//! an exhausted walker raises no further RX interrupt. So the return itself
//! must wake the runner. Dropping a lease marks its buffer released (the
//! stage pool's own release edge); [`RxReturnNotifier::notify`] then posts
//! the RX-capacity fact, which starts one drain on P2: the runner rearms the
//! released prefix, stages what waited, and unmasks RX through the doorbell.
//!
//! The notifier touches no ring state and no register: it only publishes a
//! fact and wakes the runner. A notice while RX is live, or after the epoch
//! parked, starts at most one harmless pass.

use core::ops::{Deref, DerefMut};
use oer_interrupt_table::signal::SignalExecutor;

use oer_esp32s31_ieee80211_mac::{irq::consumer::IrqPublisher, rx::pool::NetworkRxFrame};

/// The upper layer's handle: tell the runner a lease came back.
pub struct RxReturnNotifier<'a, X: SignalExecutor> {
    publisher: &'a IrqPublisher<'a, X>,
}

impl<'a, X: SignalExecutor> RxReturnNotifier<'a, X> {
    pub(crate) const fn new(publisher: &'a IrqPublisher<'a, X>) -> Self {
        Self { publisher }
    }

    /// Post the RX-capacity fact and wake the runner. Call it after the
    /// lease was dropped; repeated notices before the runner runs coalesce.
    pub fn notify(&self) {
        self.publisher.notify_rx_capacity();
    }

    /// Hold `frame` above the radio executor; dropping the holder returns
    /// the lease, then notifies.
    pub fn hold<'pool, const SLOTS: usize, const CAPACITY: usize>(
        self,
        frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    ) -> HeldRxFrame<'pool, 'a, SLOTS, CAPACITY, X> {
        HeldRxFrame {
            frame,
            _notice: ReturnNotice(self),
        }
    }
}

/// A staged frame held above the radio executor. Dropping it releases the
/// lease first (fields drop in order) and then wakes the runner.
pub struct HeldRxFrame<'pool, 'a, const SLOTS: usize, const CAPACITY: usize, X: SignalExecutor> {
    frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    _notice: ReturnNotice<'a, X>,
}

/// Notifies when dropped, after the frame declared before it.
struct ReturnNotice<'a, X: SignalExecutor>(RxReturnNotifier<'a, X>);

impl<X: SignalExecutor> Drop for ReturnNotice<'_, X> {
    fn drop(&mut self) {
        self.0.notify();
    }
}

impl<'pool, const SLOTS: usize, const CAPACITY: usize, X: SignalExecutor> Deref
    for HeldRxFrame<'pool, '_, SLOTS, CAPACITY, X>
{
    type Target = NetworkRxFrame<'pool, SLOTS, CAPACITY>;

    fn deref(&self) -> &Self::Target {
        &self.frame
    }
}

impl<const SLOTS: usize, const CAPACITY: usize, X: SignalExecutor> DerefMut
    for HeldRxFrame<'_, '_, SLOTS, CAPACITY, X>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.frame
    }
}

impl<'a, X: SignalExecutor> Copy for RxReturnNotifier<'a, X> {}
impl<'a, X: SignalExecutor> Clone for RxReturnNotifier<'a, X> {
    fn clone(&self) -> Self {
        *self
    }
}

//! Executor-independent publication and exclusive, finite IRQ consumption.
//!
//! Splitting storage lends one affine consumer. Every wait borrows that
//! consumer exclusively until completion or cancellation. There are no lane
//! waits competing with a batch for the same facts or waker. The ISR owns only
//! a publisher; neither polling nor cancelling a wait touches registers.

use atomic_waker::AtomicWaker;
use core::{
    future::poll_fn,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll, Waker},
};
use oer_esp32s31_hal::types::MacPowerInterruptObservation;

use super::{EVENT_KNOWN_MASK, EVENT_RX_SUCCESS, IrqSnapshot, IrqWork, next_irq_work};

const RX_CAPACITY: u32 = 1 << 4;
/// Both interrupt banks of the epoch were masked. Not a hardware event: the
/// owner of the interrupt line posts it once, after applying the mask.
const MASKED: u32 = 1 << 5;

/// One finite snapshot in vendor order: RX, TX completion, timeout, collision.
/// Multiplicity stays in the descriptor/completion rings. Facts published
/// after this snapshot remain pending for the following driver step.
#[must_use = "process or transfer every fact before discarding the batch"]
#[derive(Debug, Default, Eq, PartialEq)]
pub struct IrqBatch {
    pending: u32,
}

impl IrqBatch {
    pub const fn rx_capacity_returned(&self) -> bool {
        self.pending & RX_CAPACITY != 0
    }

    /// Whether the interrupt line masked both banks since the last batch.
    pub const fn masked(&self) -> bool {
        self.pending & MASKED != 0
    }

    pub const fn is_empty(&self) -> bool {
        self.pending == 0
    }
}

impl Iterator for IrqBatch {
    type Item = IrqWork;

    fn next(&mut self) -> Option<Self::Item> {
        let work = next_irq_work(self.pending)?;
        self.pending &= !work.event_bit();
        Some(work)
    }
}

/// Storage owned by an installation, independent of its executor.
pub struct IrqStorage {
    pending: AtomicU32,
    waker: AtomicWaker,
    #[cfg(test)]
    rx_posts: AtomicU32,
    #[cfg(test)]
    entries: AtomicU32,
    #[cfg(test)]
    auxiliary: AtomicU32,
    #[cfg(test)]
    unhandled: AtomicU32,
    #[cfg(test)]
    empty: AtomicU32,
    #[cfg(test)]
    rx_epochs: AtomicU32,
    #[cfg(test)]
    tx_epochs: AtomicU32,
    #[cfg(test)]
    mac_irq_cycles: AtomicU32,
}

impl IrqStorage {
    pub const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
            waker: AtomicWaker::new(),
            #[cfg(test)]
            rx_posts: AtomicU32::new(0),
            #[cfg(test)]
            entries: AtomicU32::new(0),
            #[cfg(test)]
            auxiliary: AtomicU32::new(0),
            #[cfg(test)]
            unhandled: AtomicU32::new(0),
            #[cfg(test)]
            empty: AtomicU32::new(0),
            #[cfg(test)]
            rx_epochs: AtomicU32::new(0),
            #[cfg(test)]
            tx_epochs: AtomicU32::new(0),
            #[cfg(test)]
            mac_irq_cycles: AtomicU32::new(0),
        }
    }

    /// A second split cannot borrow the storage while either handle is live.
    ///
    /// ```no_run
    /// # #![cfg_attr(target_arch = "riscv32", no_std)]
    /// # #![cfg_attr(target_arch = "riscv32", no_main)]
    /// # #[cfg(target_arch = "riscv32")]
    /// # #[panic_handler]
    /// # fn panic(_: &core::panic::PanicInfo) -> ! {
    /// #     loop {}
    /// # }
    /// use oer_esp32s31_ieee80211_mac::irq::consumer::IrqStorage;
    /// async fn sequential(storage: &mut IrqStorage) {
    ///     let (_publisher, mut consumer) = storage.split();
    ///     let first = consumer.wait();
    ///     drop(first);
    ///     let _next = consumer.wait();
    /// }
    /// ```
    ///
    /// ```compile_fail,E0499
    /// # #![cfg_attr(target_arch = "riscv32", no_std)]
    /// # #![cfg_attr(target_arch = "riscv32", no_main)]
    /// # #[cfg(target_arch = "riscv32")]
    /// # #[panic_handler]
    /// # fn panic(_: &core::panic::PanicInfo) -> ! {
    /// #     loop {}
    /// # }
    /// use oer_esp32s31_ieee80211_mac::irq::consumer::IrqStorage;
    /// async fn overlapping(storage: &mut IrqStorage) {
    ///     let (_publisher, mut consumer) = storage.split();
    ///     let first = consumer.wait();
    ///     let second = consumer.wait();
    ///     drop((first, second));
    /// }
    /// ```
    pub fn split(&mut self) -> (IrqPublisher<'_>, IrqConsumer<'_>) {
        (
            IrqPublisher { storage: self },
            IrqConsumer { storage: self },
        )
    }

    #[inline(always)]
    fn post(&self, events: u32) -> IrqPublication {
        let previous = self.pending.fetch_or(events, Ordering::Release);
        let added = events & !previous;
        IrqPublication {
            #[cfg(test)]
            added,
            #[cfg(test)]
            previous,
            wake: if added != 0 { self.waker.take() } else { None },
        }
    }
}

impl Default for IrqStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared publication authority, with no ability to consume or wait.
pub struct IrqPublisher<'irq> {
    storage: &'irq IrqStorage,
}

impl IrqPublisher<'_> {
    oer_esp32s31_hal::place_irq_path! {
    /// Publish one acknowledged hardware snapshot with one Release operation.
    #[inline(never)]
    pub fn publish(&self, events: u32) {
        self.publish_deferred(events).wake();
    }
    }

    /// Release the MMIO/register borrow before invoking an arbitrary waker.
    #[inline(always)]
    pub fn publish_deferred(&self, events: u32) -> IrqPublication {
        let events = events & EVENT_KNOWN_MASK;
        #[cfg(test)]
        if events & EVENT_RX_SUCCESS != 0 {
            self.storage.rx_posts.fetch_add(1, Ordering::Relaxed);
        }
        self.storage.post(events)
    }

    /// Count the complete bounded entry without a global observer callback.
    /// The semantic facts still travel through the one consumer's batch.
    #[inline(always)]
    pub fn publish_snapshot_deferred(&self, snapshot: IrqSnapshot) -> IrqPublication {
        #[cfg(test)]
        {
            self.storage.entries.fetch_add(1, Ordering::Relaxed);
            self.storage
                .auxiliary
                .fetch_add(u32::from(snapshot.had_auxiliary_event), Ordering::Relaxed);
            self.storage
                .unhandled
                .fetch_add(u32::from(snapshot.had_unhandled_event), Ordering::Relaxed);
        }
        let publication = self.publish_deferred(snapshot.posted_events);
        #[cfg(test)]
        {
            self.storage
                .empty
                .fetch_add(u32::from(!snapshot.had_status), Ordering::Relaxed);
            self.storage.rx_epochs.fetch_add(
                u32::from(publication.added & EVENT_RX_SUCCESS != 0),
                Ordering::Relaxed,
            );
            const TX: u32 =
                super::EVENT_TX_COMPLETE | super::EVENT_TX_TIMEOUT | super::EVENT_COLLISION;
            self.storage.tx_epochs.fetch_add(
                u32::from(publication.added & TX != 0 && publication.previous & TX == 0),
                Ordering::Relaxed,
            );
        }
        publication
    }

    /// Probe the durable RX frontier after a handoff; not hardware evidence.
    pub fn notify_rx_handoff(&self) {
        self.storage.post(EVENT_RX_SUCCESS).wake();
    }

    /// A returned staging credit wakes the same finite driver step.
    pub fn notify_rx_capacity(&self) {
        self.storage.post(RX_CAPACITY).wake();
    }

    /// Report that both interrupt banks are masked. A repeated report before
    /// the consumer takes its batch coalesces and wakes no one again.
    pub fn notify_masked(&self) {
        self.storage.post(MASKED).wake();
    }

    /// Begin the diagnostic interval before reading the MAC status. Finishing
    /// it returns an owned publication with no borrow of this installation.
    #[inline(always)]
    pub fn begin_mac_entry(&self) -> MacIrqEntry<'_> {
        MacIrqEntry {
            _borrow: core::marker::PhantomData,
            #[cfg(test)]
            storage: self.storage,
            #[cfg(test)]
            started: irq_cycles(),
        }
    }

    /// Borrow the wrapping counters of this installation.
    #[cfg(test)]
    pub fn diagnostics(&self) -> IrqDiagnostics<'_> {
        IrqDiagnostics {
            storage: self.storage,
        }
    }
}

/// Read-only diagnostics borrowed from one installation's IRQ owner.
/// The type, field and accessor do not exist when counting is disabled.
#[cfg(test)]
pub struct IrqDiagnostics<'irq> {
    storage: &'irq IrqStorage,
}

#[cfg(test)]
impl IrqDiagnostics<'_> {
    pub fn rx_post_count(&self) -> u32 {
        self.storage.rx_posts.load(Ordering::Relaxed)
    }
    pub fn snapshot(&self) -> IrqDiagnosticSnapshot {
        IrqDiagnosticSnapshot {
            rx_posts: self.rx_post_count(),
            mac_irq_entries: self.storage.entries.load(Ordering::Relaxed),
            auxiliary_entries: self.storage.auxiliary.load(Ordering::Relaxed),
            unhandled_entries: self.storage.unhandled.load(Ordering::Relaxed),
            empty_entries: self.storage.empty.load(Ordering::Relaxed),
            rx_epochs: self.storage.rx_epochs.load(Ordering::Relaxed),
            tx_epochs: self.storage.tx_epochs.load(Ordering::Relaxed),
            #[cfg(test)]
            mac_irq_cycles: self.storage.mac_irq_cycles.load(Ordering::Relaxed),
        }
    }
}

/// Wrapping counts of this installation, never fabricated when disabled.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IrqDiagnosticSnapshot {
    pub rx_posts: u32,
    pub mac_irq_entries: u32,
    pub auxiliary_entries: u32,
    pub unhandled_entries: u32,
    pub empty_entries: u32,
    /// Hardware publications which begin a previously empty RX batch frontier.
    pub rx_epochs: u32,
    /// Hardware publications which begin a previously empty TX batch frontier.
    /// Adding a different TX fact to an already pending batch is the same epoch.
    pub tx_epochs: u32,
    /// MAC ISR cycles through status, facts and counters, before waking.
    /// This explicitly excludes the cost of invoking the external waker.
    /// Absent when the target has no enabled cycle counter.
    #[cfg(test)]
    pub mac_irq_cycles: u32,
}

/// The borrowed part of an IRQ diagnostic interval. Consume it before waking;
/// the returned publication owns only its waker, never installation memory.
#[must_use = "finish the measurement before invoking the returned waker"]
pub struct MacIrqEntry<'irq> {
    _borrow: core::marker::PhantomData<&'irq IrqStorage>,
    #[cfg(test)]
    storage: &'irq IrqStorage,
    #[cfg(test)]
    started: u32,
}
impl MacIrqEntry<'_> {
    #[inline(always)]
    pub fn finish(self, publication: IrqPublication) -> IrqPublication {
        #[cfg(test)]
        self.storage
            .mac_irq_cycles
            .fetch_add(irq_cycles().wrapping_sub(self.started), Ordering::Relaxed);
        publication
    }
}

#[cfg(test)]
fn irq_cycles() -> u32 {
    tests::cycles()
}

/// The sole MAC waiter and consumer of an installation's coalesced facts.
#[must_use = "the driver must retain its exclusive IRQ consumer"]
pub struct IrqConsumer<'irq> {
    storage: &'irq IrqStorage,
}

impl IrqConsumer<'_> {
    /// Observe remaining work without removing any fact from its next batch.
    /// An executor uses this Acquire check after arming its event sources.
    pub fn has_pending(&self) -> bool {
        self.storage.pending.load(Ordering::Acquire) != 0
    }

    pub fn take_batch(&mut self) -> IrqBatch {
        IrqBatch {
            pending: self.storage.pending.swap(0, Ordering::AcqRel),
        }
    }

    /// Registration precedes the Acquire snapshot, including after cancellation.
    pub fn poll_batch(&mut self, cx: &mut Context<'_>) -> Poll<IrqBatch> {
        self.storage.waker.register(cx.waker());
        let batch = self.take_batch();
        if batch.is_empty() {
            Poll::Pending
        } else {
            Poll::Ready(batch)
        }
    }

    pub async fn wait(&mut self) -> IrqBatch {
        poll_fn(|cx| self.poll_batch(cx)).await
    }
}

/// Coalesced power facts. Bits are local semantic facts, never register images.
pub struct PowerIrqStorage {
    pending: AtomicU32,
    waker: AtomicWaker,
}

impl PowerIrqStorage {
    const TSF0: u32 = 1;
    const TSF1: u32 = 2;
    const TSF2: u32 = 4;
    const TSF3: u32 = 8;
    const TBTT: u32 = 16;
    const UNHANDLED: u32 = 32;

    pub const fn new() -> Self {
        Self {
            pending: AtomicU32::new(0),
            waker: AtomicWaker::new(),
        }
    }

    pub fn split(&mut self) -> (PowerIrqPublisher<'_>, PowerIrqConsumer<'_>) {
        (
            PowerIrqPublisher { storage: self },
            PowerIrqConsumer { storage: self },
        )
    }

    fn observation(events: u32) -> MacPowerInterruptObservation {
        MacPowerInterruptObservation::from_semantic_events(
            events & Self::TSF0 != 0,
            events & Self::TSF1 != 0,
            events & Self::TSF2 != 0,
            events & Self::TSF3 != 0,
            events & Self::TBTT != 0,
            events & Self::UNHANDLED != 0,
        )
    }
}

impl Default for PowerIrqStorage {
    fn default() -> Self {
        Self::new()
    }
}

pub struct PowerIrqPublisher<'irq> {
    storage: &'irq PowerIrqStorage,
}

impl PowerIrqPublisher<'_> {
    oer_esp32s31_hal::place_irq_path! {
    #[inline(never)]
    pub fn publish(&self, observation: MacPowerInterruptObservation) {
        self.publish_deferred(observation).wake();
    }
    }

    #[inline(always)]
    pub fn publish_deferred(&self, observation: MacPowerInterruptObservation) -> IrqPublication {
        let events = (u32::from(observation.tsf_timer_0()) * PowerIrqStorage::TSF0)
            | (u32::from(observation.tsf_timer_1()) * PowerIrqStorage::TSF1)
            | (u32::from(observation.tsf_timer_2()) * PowerIrqStorage::TSF2)
            | (u32::from(observation.tsf_timer_3()) * PowerIrqStorage::TSF3)
            | (u32::from(observation.sta_tbtt()) * PowerIrqStorage::TBTT)
            | (u32::from(observation.has_unhandled_event()) * PowerIrqStorage::UNHANDLED);
        let previous = self.storage.pending.fetch_or(events, Ordering::Release);
        let added = events & !previous;
        IrqPublication {
            #[cfg(test)]
            added,
            #[cfg(test)]
            previous,
            wake: if added != 0 {
                self.storage.waker.take()
            } else {
                None
            },
        }
    }
}

/// Acknowledged publication and its detached executor wake.
#[must_use = "deliver the wake after ending the register transaction"]
pub struct IrqPublication {
    #[cfg(test)]
    added: u32,
    #[cfg(test)]
    previous: u32,
    wake: Option<Waker>,
}

impl IrqPublication {
    pub fn wake(self) {
        if let Some(waker) = self.wake {
            waker.wake();
        }
    }
}

#[must_use = "the driver must retain its exclusive power IRQ consumer"]
pub struct PowerIrqConsumer<'irq> {
    storage: &'irq PowerIrqStorage,
}

impl PowerIrqConsumer<'_> {
    /// Observe remaining work without consuming its next power observation.
    pub fn has_pending(&self) -> bool {
        self.storage.pending.load(Ordering::Acquire) != 0
    }

    pub fn take(&mut self) -> MacPowerInterruptObservation {
        PowerIrqStorage::observation(self.storage.pending.swap(0, Ordering::AcqRel))
    }

    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<MacPowerInterruptObservation> {
        self.storage.waker.register(cx.waker());
        let observation = self.take();
        if observation.is_empty() {
            Poll::Pending
        } else {
            Poll::Ready(observation)
        }
    }

    pub async fn wait(&mut self) -> MacPowerInterruptObservation {
        poll_fn(|cx| self.poll(cx)).await
    }
}

#[cfg(test)]
mod tests;

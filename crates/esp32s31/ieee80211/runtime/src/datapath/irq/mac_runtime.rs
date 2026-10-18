use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};
use oer_esp32s31_ieee80211_mac::irq::{IrqSink, IrqState, IrqWork, next_irq_work};

/// Driver-owned S31 MAC interrupt handoff for one Embassy radio task.
///
/// `publish` drains one hardware snapshot through
/// [`IrqState::try_take_next`], which orders RX success before TX completion,
/// timeout and collision. Consumers that wait for RX and TX concurrently must
/// poll [`Self::wait_rx`] first; this is the same ordering contract as the
/// vendor FIQ and is intentionally visible at the adapter boundary.
///
/// Duplicate work of one kind coalesces in `Signal`, matching the vendor
/// worker latch. Descriptor and completion rings remain the durable source of
/// multiplicity.
///
/// SOURCE: complete `libpp[wdev.o]::wDev_ProcessFiq` services
/// `RX_SUCCESS` before `TX_COMPLETE`, `TX_TIMEOUT` and `COLLISION`. Complete
/// `libpp[pp.o]::pp_post,libpp[pp.o]::ppTask` coalesces the corresponding
/// worker wake while the descriptor/completion state remains hardware-owned.
// CAPABILITY: wifi-interrupt-epoch-owners
pub struct EmbassyMacIrqRuntime<M: RawMutex> {
    state: IrqState,
    rx: Signal<M, ()>,
    rx_capacity: Signal<M, ()>,
    tx: Signal<M, ()>,
    tx_pending: AtomicU32,
    #[cfg(any(feature = "diagnostics", test))]
    rx_post_count: AtomicU32,
    rx_moderation_active: AtomicBool,
    unmask_rx_delivery: Option<fn()>,
}

/// Coalesced executor work discarded after an interrupt epoch is quiesced.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EmbassyMacIrqDrain {
    pub rx: bool,
    pub rx_capacity: bool,
    pub tx_events: u32,
}

impl<M: RawMutex> EmbassyMacIrqRuntime<M> {
    pub const fn new() -> Self {
        Self {
            state: IrqState::new(),
            rx: Signal::new(),
            rx_capacity: Signal::new(),
            tx: Signal::new(),
            tx_pending: AtomicU32::new(0),
            #[cfg(any(feature = "diagnostics", test))]
            rx_post_count: AtomicU32::new(0),
            rx_moderation_active: AtomicBool::new(false),
            unmask_rx_delivery: None,
        }
    }

    /// Construct a runtime with a narrow platform callback for restoring the
    /// RX delivery interrupt group after a bottom-half drain.
    pub const fn new_with_rx_moderation(unmask_rx_delivery: fn()) -> Self {
        Self {
            state: IrqState::new(),
            rx: Signal::new(),
            rx_capacity: Signal::new(),
            tx: Signal::new(),
            tx_pending: AtomicU32::new(0),
            #[cfg(any(feature = "diagnostics", test))]
            rx_post_count: AtomicU32::new(0),
            rx_moderation_active: AtomicBool::new(false),
            unmask_rx_delivery: Some(unmask_rx_delivery),
        }
    }

    /// Enable source moderation for one explicitly selected role epoch.
    pub fn begin_rx_moderation(&self) {
        assert!(
            self.unmask_rx_delivery.is_some(),
            "RX moderation requires a platform unmask capability"
        );
        self.rx_moderation_active.store(true, Ordering::Release);
    }

    /// Stop source moderation after the hardware interrupt route is closed.
    pub fn end_rx_moderation(&self) {
        self.rx_moderation_active.store(false, Ordering::Release);
    }

    /// Whether one active interrupt epoch currently owns RX source moderation.
    pub fn is_rx_moderation_active(&self) -> bool {
        self.rx_moderation_active.load(Ordering::Acquire)
    }

    /// Restore RX delivery after the bottom half proved the durable frontier
    /// drained. Returns whether moderation was active for this epoch.
    pub fn unmask_rx_after_drain(&self) -> bool {
        if !self.rx_moderation_active.load(Ordering::Acquire) {
            return false;
        }
        (self
            .unmask_rx_delivery
            .expect("active RX moderation retains its platform callback"))();
        true
    }

    /// Publish one acknowledged MAC interrupt snapshot.
    ///
    /// The executor signals are emitted only after the executor-neutral state
    /// has selected each work item in recovered vendor priority order.
    #[inline]
    pub fn publish(&self, events: u32) {
        // `publish` runs synchronously inside the sole MAC ISR. Do not move the
        // already-local task event set through IrqState's atomic producer and
        // immediately consume it again in the same call. The Embassy signals
        // and TX event set are the durable cross-context handoff.
        let mut pending = events;
        while let Some(work) = next_irq_work(pending) {
            pending &= !work.event_bit();
            match work {
                IrqWork::RxSuccess => {
                    crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                        .record_rx_interrupt_post();
                    #[cfg(any(feature = "diagnostics", test))]
                    self.rx_post_count.fetch_add(1, Ordering::Relaxed);
                    if !self.rx.signaled() {
                        self.rx.signal(());
                    }
                }
                IrqWork::TxComplete | IrqWork::TxTimeout | IrqWork::Collision => {
                    self.tx_pending
                        .fetch_or(work.event_bit(), Ordering::Release);
                    if !self.tx.signaled() {
                        self.tx.signal(());
                    }
                }
            }
        }
    }

    /// Wait for a coalesced RX-success bottom-half edge.
    pub async fn wait_rx(&self) {
        self.rx.wait().await;
    }

    /// Schedule one RX bottom-half probe when an already-live DMA ring moves
    /// from polling ownership to the interrupt-driven DATAPATH runner.
    ///
    /// A completion may become durable before the CPU route is unmasked. Some
    /// interrupt controllers do not replay that old edge, so waiting only for
    /// a future interrupt can strand the completed frontier indefinitely.
    /// This wake does not increment diagnostic `rx_post_count`: it is a handoff
    /// probe, not fabricated interrupt evidence. The bounded RX service still
    /// decides from descriptor ownership whether any work exists.
    #[inline]
    pub fn notify_rx_handoff(&self) {
        self.rx.signal(());
    }

    /// Wake a radio actor stopped by staging ownership backpressure.
    ///
    /// This is distinct from a hardware RX edge: while backpressured, new RX
    /// interrupts must not repeatedly win ordered arbitration over a pending TX
    /// completion. The protocol consumer emits this only after it has actually
    /// returned one staging credit.
    #[inline]
    pub fn notify_rx_capacity(&self) {
        self.rx_capacity.signal(());
    }

    /// Wait until protocol processing returns at least one staging credit.
    pub async fn wait_rx_capacity(&self) {
        self.rx_capacity.wait().await;
    }

    /// Wait for and consume coalesced TX completion, timeout or collision bits.
    pub async fn wait_tx(&self) -> u32 {
        self.tx.wait().await;
        self.tx_pending.swap(0, Ordering::Acquire)
    }

    /// Whether the RX bottom half has durable pending work.
    #[inline]
    pub fn rx_signaled(&self) -> bool {
        self.rx.signaled()
    }

    /// Whether a staging-capacity wake is pending.
    #[cfg(feature = "diagnostics")]
    #[inline]
    pub fn rx_capacity_signaled(&self) -> bool {
        self.rx_capacity.signaled()
    }

    /// Whether the TX bottom half has durable pending work.
    #[inline]
    pub fn tx_signaled(&self) -> bool {
        self.tx.signaled()
    }

    /// Consume a stale TX wake before publishing a new transaction.
    #[inline]
    pub fn try_take_tx(&self) -> Option<u32> {
        self.tx.try_take()?;
        Some(self.tx_pending.swap(0, Ordering::Acquire))
    }

    /// Remove every coalesced executor wake after hardware publication stops.
    ///
    /// Descriptor and transaction owners remain the durable source of truth;
    /// this only prevents one epoch's already-acknowledged wake from being
    /// interpreted as work in a later connected epoch. During same-epoch
    /// suspension the caller detaches CPU routes and retains this result for
    /// replay; peripheral masks and latched hardware causes remain intact.
    pub fn drain_pending(&self) -> EmbassyMacIrqDrain {
        let rx = self.rx.try_take().is_some();
        let rx_capacity = self.rx_capacity.try_take().is_some();
        let _tx_wake = self.tx.try_take();
        let tx_events = self.tx_pending.swap(0, Ordering::Acquire);
        EmbassyMacIrqDrain {
            rx,
            rx_capacity,
            tx_events,
        }
    }

    /// Number of RX-success work publications, with wrapping semantics.
    #[cfg(any(feature = "diagnostics", test))]
    #[inline]
    pub fn rx_post_count(&self) -> u32 {
        self.rx_post_count.load(Ordering::Relaxed)
    }

    /// Whether the shared hard ISR observed an unhandled MAC cause.
    #[inline]
    pub fn observed_unhandled(&self) -> bool {
        self.state.observed_unhandled()
    }
}

impl<M: RawMutex> IrqSink for EmbassyMacIrqRuntime<M> {
    #[inline]
    fn post(&self, events: u32) {
        self.publish(events);
    }

    #[inline]
    fn record_unhandled_event(&self) {
        self.state.record_unhandled_event();
    }

    #[inline]
    fn moderate_rx_success(&self) -> bool {
        self.rx_moderation_active.load(Ordering::Acquire)
    }
}

impl<M: RawMutex> Default for EmbassyMacIrqRuntime<M> {
    fn default() -> Self {
        Self::new()
    }
}

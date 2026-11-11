//! The one static arena of a Wi-Fi receive epoch.
//!
//! Every value the radio executor, the interrupt line and the application
//! share for one epoch lives in one zeroed static ([`WifiRxStatic`]): the
//! arena (the interrupt fact storages, the request word, the counters, the
//! stop handle's words, the slot of the interrupt line, the staged-frame
//! queue and the staging pool) beside the HAL's RX DMA storage. Its zero
//! bytes are reusable; [`bind`] places the DMA storage, writes the few
//! values whose zero bytes are not valid and lends every part once.
//!
//! [`bind`]: WifiRxArena::bind

use core::mem::MaybeUninit;
use oer_chips_family_platform_interrupt_table::signal::SignalExecutor;

use crate::mac::{
    irq::consumer::{IrqConsumer, IrqPublisher, IrqStorage, PowerIrqConsumer, PowerIrqStorage},
    rx::pool::RxStagePool,
};
use oer_chips_esp32s31_hal::{
    radio::ieee80211::dma::{
        rx_storage::{PlacedRxDma, RxDmaStorage, RxDmaStorageError},
        space::{ChipDma, DmaSpace},
    },
    system::memory::dma_static::Placed,
};

use super::{
    control::{ReceiveControl, ReceiveRunnerControl, ReceiveStopHandle},
    counters::WifiRxCounters,
    returns::RxReturnNotifier,
    upper::{RxFrameEnds, RxFrameQueue},
};
use crate::irq::{WifiIrqLine, WifiIrqLineParts, WifiIrqRequests};

/// The image's three static signals of the radio executor that carry the
/// line's facts: the MAC facts but the receive ones, the receive facts and
/// the power facts.
pub struct WifiIrqSignals<X: SignalExecutor> {
    pub mac: &'static oer_chips_family_platform_interrupt_table::signal::IrqSignal<X>,
    pub receive: &'static oer_chips_family_platform_interrupt_table::signal::IrqSignal<X>,
    pub power: &'static oer_chips_family_platform_interrupt_table::signal::IrqSignal<X>,
}

/// The static arena of one receive epoch.
///
/// `COUNT` descriptors of `BUFFER` bytes (including the leading recycle
/// guard), `STORAGE` backing bytes per buffer, and `STAGE_SLOTS` staging leases
/// of up to `BUFFER` bytes.
#[repr(C)]
#[derive(bytemuck::Zeroable)]
#[zeroable(bound = "")]
pub struct WifiRxArena<
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    X: SignalExecutor,
    S: DmaSpace = ChipDma,
> {
    irq: MaybeUninit<IrqStorage<X>>,
    /// The one MAC fact publisher: the line posts hardware facts through it,
    /// the upper layer the return of a staged lease.
    irq_publisher: MaybeUninit<IrqPublisher<'static, X>>,
    power: MaybeUninit<PowerIrqStorage<X>>,
    requests: WifiIrqRequests,
    counters: WifiRxCounters,
    control: MaybeUninit<ReceiveControl>,
    line: MaybeUninit<WifiIrqLine<X>>,
    /// The staged-frame queue from the runner to the application.
    frames: MaybeUninit<RxFrameQueue<'static, STAGE_SLOTS, BUFFER>>,
    stage: RxStagePool<STAGE_SLOTS, BUFFER>,
    /// The DMA space of the storage placed beside it.
    space: core::marker::PhantomData<S>,
}

/// The zeroed static of one receive epoch: the arena and the RX DMA
/// storage, placed together ([`oer_chips_esp32s31_hal::dma_static!`]).
pub type WifiRxStatic<
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    X,
    S = ChipDma,
> = (
    WifiRxArena<COUNT, BUFFER, STORAGE, STAGE_SLOTS, X, S>,
    RxDmaStorage<COUNT, BUFFER, STORAGE, S>,
);

/// A refused bind ([`WifiRxArena::bind`]): its cause and every input, the
/// static's two placed halves as the bind split them, untouched but for the
/// storage's recorded buffer addresses, and the line's signals.
pub struct WifiRxBindRefused<
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    X: SignalExecutor,
    S: DmaSpace = ChipDma,
> {
    /// Why the storage did not place.
    pub error: RxDmaStorageError,
    /// The arena's half of the static.
    pub arena: Placed<WifiRxArena<COUNT, BUFFER, STORAGE, STAGE_SLOTS, X, S>>,
    /// The RX DMA storage's half of the static.
    pub rx: Placed<RxDmaStorage<COUNT, BUFFER, STORAGE, S>>,
    /// The line's signals.
    pub signals: WifiIrqSignals<X>,
}

impl<
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    X: SignalExecutor,
    S: DmaSpace,
> core::fmt::Debug for WifiRxBindRefused<COUNT, BUFFER, STORAGE, STAGE_SLOTS, X, S>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WifiRxBindRefused")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Every part of a bound arena, lent once.
pub struct WifiRxParts<
    'a,
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    S: DmaSpace,
    X: SignalExecutor,
> {
    pub(crate) rx: PlacedRxDma<COUNT, BUFFER, STORAGE, S>,
    pub(crate) stage: &'a RxStagePool<STAGE_SLOTS, BUFFER>,
    pub(crate) irq: IrqConsumer<'a, X>,
    pub(crate) power: PowerIrqConsumer<'a, X>,
    pub(crate) requests: &'a WifiIrqRequests,
    pub(crate) counters: &'a WifiRxCounters,
    pub(crate) control: ReceiveRunnerControl<'a>,
    pub(crate) line: WifiIrqLineParts<'a, X>,
    pub(crate) returns: RxReturnNotifier<'a, X>,
}

impl<
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    S: DmaSpace,
    X: SignalExecutor,
> WifiRxArena<COUNT, BUFFER, STORAGE, STAGE_SLOTS, X, S>
{
    /// Place the DMA storage, write the values whose zero bytes are not
    /// valid, and lend every part: the runner's parts, the application's
    /// stop handle and the two ends of the staged-frame queue (the runner's
    /// publisher end and the application's receiver). The line's facts
    /// travel in `signals`, whose task is the runner on their executor.
    ///
    /// The static is `placed` in the DMA window
    /// ([`oer_chips_esp32s31_hal::dma_static!`]): the storage's descriptors and
    /// buffers are published to the MAC by their own addresses.
    ///
    /// # Errors
    ///
    /// The DMA storage has no valid layout in its address space; nothing
    /// is lent, and every input comes back ([`WifiRxBindRefused`]).
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "the parts, the stop handle and the queue ends are the owners of one bind; a refusal returns every input"
    )]
    pub fn bind(
        placed: Placed<WifiRxStatic<COUNT, BUFFER, STORAGE, STAGE_SLOTS, X, S>>,
        signals: WifiIrqSignals<X>,
    ) -> Result<
        (
            WifiRxParts<'static, COUNT, BUFFER, STORAGE, STAGE_SLOTS, S, X>,
            ReceiveStopHandle<'static>,
            RxFrameEnds<'static, STAGE_SLOTS, BUFFER, X>,
        ),
        WifiRxBindRefused<COUNT, BUFFER, STORAGE, STAGE_SLOTS, X, S>,
    > {
        // A unit spanning the whole ring cannot be a frame, so the ring may
        // reject one instead of republishing the whole ring
        // (`rx_ring_outspans_any_unit`).
        const {
            assert!(
                oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_ring::rx_ring_outspans_any_unit(
                    COUNT, BUFFER
                ),
                "one receive unit could span the whole ring"
            );
        }
        let (arena, rx) = placed.split();
        let rx = match RxDmaStorage::place(rx) {
            Ok(rx) => rx,
            Err((error, rx)) => {
                return Err(WifiRxBindRefused {
                    error,
                    arena,
                    rx,
                    signals,
                });
            }
        };
        let Self {
            irq,
            irq_publisher: publisher_slot,
            power,
            requests,
            counters,
            control,
            line,
            frames,
            stage,
            space: _,
        } = arena.into_inner();
        let (requests, counters): (&'static WifiIrqRequests, &'static WifiRxCounters) =
            (requests, counters);
        let (irq_publisher, irq) = irq
            .write(IrqStorage::new(signals.mac, signals.receive))
            .split();
        let irq_publisher: &'static IrqPublisher<'static, X> = publisher_slot.write(irq_publisher);
        let (power_publisher, power) = power.write(PowerIrqStorage::new(signals.power)).split();
        let (control, stop) = ReceiveControl::init_in_place(control).split();
        let returns = RxReturnNotifier::new(irq_publisher);
        let frames = RxFrameQueue::init_in_place(frames).split(counters, returns);
        Ok((
            WifiRxParts {
                rx,
                stage,
                irq,
                power,
                requests,
                counters,
                control,
                returns,
                line: WifiIrqLineParts {
                    mac: irq_publisher,
                    power: power_publisher,
                    requests,
                    counters,
                    slot: line,
                },
            },
            stop,
            frames,
        ))
    }
}

impl<
    'a,
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    S: DmaSpace,
    X: SignalExecutor,
> WifiRxParts<'a, COUNT, BUFFER, STORAGE, STAGE_SLOTS, S, X>
{
    pub const fn rx(&self) -> &'static RxDmaStorage<COUNT, BUFFER, STORAGE, S> {
        self.rx.storage()
    }

    /// The DMA address of the first descriptor.
    pub const fn descriptor_base(&self) -> u32 {
        self.rx.descriptor_base()
    }

    pub const fn stage(&self) -> &'a RxStagePool<STAGE_SLOTS, BUFFER> {
        self.stage
    }

    pub const fn requests(&self) -> &'a WifiIrqRequests {
        self.requests
    }

    pub const fn counters(&self) -> &'a WifiRxCounters {
        self.counters
    }

    /// The handle the upper layer returns staged leases through.
    pub const fn returns(&self) -> RxReturnNotifier<'a, X> {
        self.returns
    }
}

#[cfg(test)]
mod tests;

impl<X: SignalExecutor> Copy for WifiIrqSignals<X> {}
impl<X: SignalExecutor> Clone for WifiIrqSignals<X> {
    fn clone(&self) -> Self {
        *self
    }
}

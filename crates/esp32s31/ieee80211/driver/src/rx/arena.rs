//! The one static arena of a Wi-Fi receive epoch.
//!
//! Every value the radio executor, the interrupt line and the application
//! share for one epoch lives here, in one zeroed static: the interrupt fact
//! storages, the request word, the counters, the stop handle's words, the
//! slot of the interrupt line, the staged-frame queue, the buffer address
//! table, the staging pool and the RX DMA storage. Its zero bytes are a
//! reusable arena; [`bind`] writes the few values whose zero bytes are not
//! valid and lends every part once.
//!
//! The non-descriptor fields come first, so the descriptors never start the
//! arena (or the section the image places it in).
//!
//! [`bind`]: WifiRxArena::bind

use core::mem::MaybeUninit;

use oer_esp32s31_hal::ieee80211::dma::{
    rx_storage::{RxDmaStorage, RxDmaStorageError},
    space::{ChipDma, DmaSpace},
};
use oer_esp32s31_ieee80211_mac::{
    irq::{
        consumer::{IrqConsumer, IrqPublisher, IrqStorage, PowerIrqConsumer, PowerIrqStorage},
        epoch::{ActiveEpoch, Moderated},
    },
    rx::{RxDmaBufferAddresses, pool::RxStagePool},
};

use super::{
    control::{ReceiveControl, ReceiveRunnerControl, ReceiveStopHandle},
    counters::WifiRxCounters,
    returns::RxReturnNotifier,
    upper::{RxFrameEnds, RxFrameQueue},
};
use crate::irq::{WifiIrqLine, WifiIrqLineParts, WifiIrqRequests};

/// The static arena of one receive epoch.
///
/// `COUNT` descriptors of `BUFFER` bytes (`STORAGE` with the recycle
/// guard), `STAGE_SLOTS` staging leases of up to `BUFFER` bytes. `E` is the
/// active interrupt epoch the line will own.
#[repr(C)]
#[derive(bytemuck::Zeroable)]
#[zeroable(bound = "")]
pub struct WifiRxArena<
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    S: DmaSpace = ChipDma,
    E = ActiveEpoch<Moderated>,
> {
    irq: MaybeUninit<IrqStorage>,
    /// The one MAC fact publisher: the line posts hardware facts through it,
    /// the upper layer the return of a staged lease.
    irq_publisher: MaybeUninit<IrqPublisher<'static>>,
    power: MaybeUninit<PowerIrqStorage>,
    requests: WifiIrqRequests,
    counters: WifiRxCounters,
    control: MaybeUninit<ReceiveControl>,
    line: MaybeUninit<WifiIrqLine<E>>,
    /// The staged-frame queue from the runner to the application.
    frames: MaybeUninit<RxFrameQueue<'static, STAGE_SLOTS, BUFFER>>,
    buffer_addresses: RxDmaBufferAddresses<COUNT>,
    stage: RxStagePool<STAGE_SLOTS, BUFFER>,
    rx: RxDmaStorage<COUNT, BUFFER, STORAGE, S>,
}

/// Every part of a bound arena, lent once.
pub struct WifiRxParts<
    'a,
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    S: DmaSpace,
    E,
> {
    pub(crate) rx: &'a RxDmaStorage<COUNT, BUFFER, STORAGE, S>,
    pub(crate) descriptor_base: u32,
    pub(crate) buffer_addresses: &'a RxDmaBufferAddresses<COUNT>,
    pub(crate) stage: &'a RxStagePool<STAGE_SLOTS, BUFFER>,
    pub(crate) irq: IrqConsumer<'a>,
    pub(crate) power: PowerIrqConsumer<'a>,
    pub(crate) requests: &'a WifiIrqRequests,
    pub(crate) counters: &'a WifiRxCounters,
    pub(crate) control: ReceiveRunnerControl<'a>,
    pub(crate) line: WifiIrqLineParts<'a, E>,
    pub(crate) returns: RxReturnNotifier<'a>,
}

impl<
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    S: DmaSpace,
    E,
> WifiRxArena<COUNT, BUFFER, STORAGE, STAGE_SLOTS, S, E>
{
    /// Lay the DMA storage out, write the values whose zero bytes are not
    /// valid, and lend every part: the runner's parts, the application's
    /// stop handle and the two ends of the staged-frame queue (the runner's
    /// publisher end and the application's receiver).
    ///
    /// The arena is `placed` in the DMA window
    /// ([`oer_esp32s31_hal::dma_static!`]): its descriptors and buffers are
    /// published to the MAC by their own addresses.
    ///
    /// # Errors
    ///
    /// The DMA storage has no valid layout in its address space; nothing
    /// is lent.
    #[allow(
        clippy::type_complexity,
        reason = "the parts, the stop handle and the queue ends are the owners of one bind"
    )]
    pub fn bind(
        placed: oer_esp32s31_hal::dma_static::Placed<Self>,
    ) -> Result<
        (
            WifiRxParts<'static, COUNT, BUFFER, STORAGE, STAGE_SLOTS, S, E>,
            ReceiveStopHandle<'static>,
            RxFrameEnds<'static, STAGE_SLOTS, BUFFER>,
        ),
        RxDmaStorageError,
    > {
        // A unit spanning the whole ring cannot be a frame, so the ring may
        // reject one instead of republishing the whole ring
        // (`rx_ring_outspans_any_unit`).
        const {
            assert!(
                oer_esp32s31_hal::ieee80211::dma::rx_ring::rx_ring_outspans_any_unit(COUNT, BUFFER),
                "one receive unit could span the whole ring"
            );
        }
        let Self {
            irq,
            irq_publisher: publisher_slot,
            power,
            requests,
            counters,
            control,
            line,
            frames,
            buffer_addresses,
            stage,
            rx,
        } = placed.into_inner();
        let descriptor_base = rx.dma_layout(buffer_addresses)?;
        let (requests, counters): (&'static WifiIrqRequests, &'static WifiRxCounters) =
            (requests, counters);
        let (irq_publisher, irq) = irq.write(IrqStorage::new()).split();
        let irq_publisher: &'static IrqPublisher<'static> = publisher_slot.write(irq_publisher);
        let (power_publisher, power) = power.write(PowerIrqStorage::new()).split();
        let (control, stop) = control.write(ReceiveControl::new()).split();
        let returns = RxReturnNotifier::new(irq_publisher);
        let frames = frames.write(RxFrameQueue::new()).split(counters, returns);
        Ok((
            WifiRxParts {
                rx,
                descriptor_base,
                buffer_addresses,
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
    E,
> WifiRxParts<'a, COUNT, BUFFER, STORAGE, STAGE_SLOTS, S, E>
{
    pub const fn rx(&self) -> &'a RxDmaStorage<COUNT, BUFFER, STORAGE, S> {
        self.rx
    }

    /// The DMA address of the first descriptor.
    pub const fn descriptor_base(&self) -> u32 {
        self.descriptor_base
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
    pub const fn returns(&self) -> RxReturnNotifier<'a> {
        self.returns
    }
}

#[cfg(test)]
mod tests;

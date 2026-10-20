//! Reusable receive ownership across stopped channel transactions. The line
//! stays in its handler cell; its interrupt epoch leaves only after the normal
//! mask/drain/walker stop, with CPU routes silent, and returns before restart.

use super::{
    arena::WifiRxParts,
    control::{ReceiveRunnerControl, WifiRxCause, WifiRxEnd, WifiRxExit},
    counters::WifiRxCounters,
    frontier::ReceiveFrontier,
    receive::Receiver,
    sink::CountingAdmission,
    transaction::Publisher,
};
use crate::irq::{RadioDoorbell, WifiIrqLine, WifiIrqRequests, WifiIrqRoutes};
use core::task::{Context, Poll};
use oer_esp32s31_hal::{
    ieee80211::dma::{rx_storage::RxDmaStorage, space::ChipDma},
    owner::{MacInterruptSetup, RadioAccess},
};
use oer_memory::HandlerCell;
use oer_time::{Instant, Timer};
use {
    oer_esp32s31_hal::ieee80211::init::MAC_STATION_INTERRUPT_MASK,
    oer_esp32s31_ieee80211_mac::{
        init::{MacRuntimeStopHardware, activate_promiscuous_receive},
        irq::{
            consumer::{IrqBatch, IrqConsumer, PowerIrqConsumer},
            epoch::{ActiveEpoch, InactiveEpoch, Moderated},
        },
        rx::pool::RxStagePool,
    },
};

type Epoch = ActiveEpoch<Moderated>;

pub struct ReceiveSession<const N: usize, const B: usize, const S: usize, const Q: usize> {
    rx: &'static RxDmaStorage<N, B, S>,
    stage: &'static RxStagePool<Q, B>,
    irq: IrqConsumer<'static>,
    power: PowerIrqConsumer<'static>,
    requests: &'static WifiIrqRequests,
    counters: &'static WifiRxCounters,
    control: ReceiveRunnerControl<'static>,
    frontier: ReceiveFrontier<'static, N, B>,
    cell: &'static HandlerCell<WifiIrqLine<Epoch>>,
    totals: [u64; 3],
    full_since: Option<(Instant, u32)>,
}

impl<const N: usize, const B: usize, const S: usize, const Q: usize> ReceiveSession<N, B, S, Q> {
    /// Bind once while the role is stopped. Failure is terminal for this arena;
    /// the caller retains its role/register owners for quarantine or reset.
    pub fn new(
        parts: WifiRxParts<'static, N, B, S, Q, ChipDma, Epoch>,
        setup: MacInterruptSetup,
        registers: &mut RadioAccess,
        cell: &'static HandlerCell<WifiIrqLine<Epoch>>,
    ) -> Result<Self, WifiRxCause> {
        let WifiRxParts {
            rx,
            stage,
            irq,
            power,
            requests,
            counters,
            control,
            line,
            returns: _,
        } = parts;
        let storage = rx.storage();
        let (_, dma) = registers.split_rx_dma();
        let frontier = ReceiveFrontier::prepare_initial(dma, rx).map_err(WifiRxCause::Prepare)?;
        if let Err(refused) = line.install(
            InactiveEpoch::new(setup).activate_rx_moderated(MAC_STATION_INTERRUPT_MASK),
            cell,
        ) {
            refused.epoch_mut().mask_all();
            return Err(WifiRxCause::LineRefused);
        }
        Ok(Self {
            rx: storage,
            stage,
            irq,
            power,
            requests,
            counters,
            control,
            frontier,
            cell,
            totals: [0; 3],
            full_since: None,
        })
    }

    pub const fn counters(&self) -> &'static WifiRxCounters {
        self.counters
    }

    /// Start the initial prepared ring, or the same ring after a retune.
    pub async fn start(
        &mut self,
        registers: &mut RadioAccess,
        timer: &impl Timer,
        routes: &mut impl WifiIrqRoutes,
    ) -> Result<(), WifiRxCause> {
        let (mut mac, dma) = registers.split_rx_dma();
        activate_promiscuous_receive(&mut mac);
        routes.enable();
        self.frontier
            .start_with_storage(timer, dma, self.rx)
            .await
            .map_err(WifiRxCause::Start)?;
        mac.resume_mac_runtime();
        Ok(())
    }

    /// Every MAC fact is returned, including TX completion/timeout/color facts.
    /// The caller services TX in the same task before discarding the batch.
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Result<IrqBatch, WifiRxCause>> {
        while let Poll::Ready(Ok(_)) = self.power.poll(cx) {}
        self.irq
            .poll_batch(cx)
            .map(|r| r.map_err(|_| WifiRxCause::ForeignExecutor))
    }

    /// Drain through the same bounded service/reload path as W1. Frames remain
    /// affine leases in the supplied publisher until its receiver drops them.
    pub async fn drain<P: Publisher<'static, B, Q>>(
        &mut self,
        registers: &mut RadioAccess,
        routes: &mut impl WifiIrqRoutes,
        doorbell: &impl RadioDoorbell,
        timer: &impl Timer,
        publisher: &P,
    ) -> Result<(), WifiRxCause> {
        let (mut mac, dma) = registers.split_rx_dma();
        let admission = CountingAdmission::new(self.counters);
        let mut receiver = Receiver {
            hardware: &mut mac,
            rx_dma: dma,
            rx: self.rx,
            stage: self.stage,
            irq: &mut self.irq,
            power: &mut self.power,
            requests: self.requests,
            counters: self.counters,
            control: &mut self.control,
            routes,
            doorbell,
            timer,
            publisher,
            admission: &admission,
            totals: &mut self.totals,
            full_since: &mut self.full_since,
        };
        receiver
            .drain(self.frontier.live_mut().map_err(WifiRxCause::Start)?, None)
            .await
    }

    /// Stop once and recover IRQ setup only after the normal mask, route
    /// silence, lease drain and walker stop. The caller must drain its frame
    /// receiver before awaiting this boundary; no held frame is discarded.
    pub async fn pause<P: Publisher<'static, B, Q>>(
        &mut self,
        registers: &mut RadioAccess,
        routes: &mut impl WifiIrqRoutes,
        doorbell: &impl RadioDoorbell,
        timer: &impl Timer,
        publisher: &P,
    ) -> Result<MacInterruptSetup, WifiRxExit> {
        let (mut mac, dma) = registers.split_rx_dma();
        let admission = CountingAdmission::new(self.counters);
        let mut receiver = Receiver {
            hardware: &mut mac,
            rx_dma: dma,
            rx: self.rx,
            stage: self.stage,
            irq: &mut self.irq,
            power: &mut self.power,
            requests: self.requests,
            counters: self.counters,
            control: &mut self.control,
            routes,
            doorbell,
            timer,
            publisher,
            admission: &admission,
            totals: &mut self.totals,
            full_since: &mut self.full_since,
        };
        let exit = receiver
            .teardown(&mut self.frontier, WifiRxCause::StopRequested)
            .await;
        if exit.end != WifiRxEnd::Halted {
            return Err(exit);
        }
        let epoch = self
            .cell
            .with(|line| line.take_epoch())
            .flatten()
            .ok_or(WifiRxExit {
                cause: WifiRxCause::LineRefused,
                end: WifiRxEnd::Halted,
            })?;
        Ok(epoch.quiesce().into_setup())
    }

    /// Reattach the exact returned interrupt ownership while routes are silent.
    pub fn reattach(&mut self, setup: MacInterruptSetup) -> Result<(), WifiRxCause> {
        let epoch = InactiveEpoch::new(setup).activate_rx_moderated(MAC_STATION_INTERRUPT_MASK);
        self.cell
            .with(|line| line.put_epoch(epoch))
            .ok_or(WifiRxCause::LineRefused)?
            .map_err(|_| WifiRxCause::LineRefused)
    }
}

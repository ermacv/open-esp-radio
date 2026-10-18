#![expect(
    clippy::type_complexity,
    reason = "epoch conversion exposes the exact disconnected and reconnected owner graphs"
)]

//! Persistent ESP32-S31 station resources between connected epochs.
//!
//! Connected shutdown returns a stopped RX service plus the peer-independent
//! network, hardware, aggregate-TX and control owners. A running scan may
//! temporarily split out only hardware and RX. Preparing the next join then
//! consumes the stopped RX service exactly once and separates its halted ring
//! from the staging resources needed by the next connected service.

use core::marker::PhantomData;

use crate::datapath::rx::{
    dma::{RxEpochResources, StagedRxProducer, StoppedReceive},
    frontier::ReceiveFrontier,
};

use embassy_sync::blocking_mutex::raw::RawMutex;

/// RX conversion required at the disconnected-to-reconnected boundary.
///
/// This interface exists so the station epoch does not expose every RX
/// storage lifetime and capacity as an argument of its own. The production
/// implementation below is the exact stopped ESP32-S31 RX owner.
pub trait StoppedStaRx {
    type Preconnected;
    type Persistent;

    fn split_for_reconnect(self) -> (Self::Preconnected, Self::Persistent);
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> StoppedStaRx
    for StoppedReceive<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    type Preconnected = ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>;
    type Persistent = RxEpochResources<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >;

    fn split_for_reconnect(self) -> (Self::Preconnected, Self::Persistent) {
        let (ring, resources) = self.into_epoch_parts();
        (ReceiveFrontier::from_halted(ring), resources)
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
> StoppedStaRx
    for StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
    >
{
    type Preconnected = ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>;
    type Persistent = RxEpochResources<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >;

    fn split_for_reconnect(self) -> (Self::Preconnected, Self::Persistent) {
        let (ring, resources) = self
            .try_into_live_epoch_parts()
            .unwrap_or_else(|_| panic!("parked station RX retained a staging lease"));
        (ReceiveFrontier::from_live(ring), resources)
    }
}

/// Complete peer-independent owner after a connected STA epoch has stopped.
pub struct DisconnectedStaEpoch<N, H, R, A, C> {
    network: N,
    hardware: H,
    rx: R,
    aggregate_tx: A,
    control: C,
}

impl<N, H, R, A, C> DisconnectedStaEpoch<N, H, R, A, C> {
    pub const fn new(network: N, hardware: H, rx: R, aggregate_tx: A, control: C) -> Self {
        Self {
            network,
            hardware,
            rx,
            aggregate_tx,
            control,
        }
    }

    pub const fn hardware(&self) -> &H {
        &self.hardware
    }

    pub const fn rx(&self) -> &R {
        &self.rx
    }

    /// Consume the peer-independent stopped frontier for role
    /// dematerialization or board-level resource regrouping.
    pub fn into_parts(self) -> DisconnectedStaEpochParts<N, H, R, A, C> {
        DisconnectedStaEpochParts {
            network: self.network,
            hardware: self.hardware,
            rx: self.rx,
            aggregate_tx: self.aggregate_tx,
            control: self.control,
        }
    }

    /// Split out only the capabilities used by a finite running scan.
    pub fn into_running_scan_parts(self) -> RunningScanEpochParts<N, H, R, A, C> {
        RunningScanEpochParts {
            retained: RunningScanRetained {
                network: self.network,
                aggregate_tx: self.aggregate_tx,
                control: self.control,
                _split: PhantomData,
            },
            hardware: self.hardware,
            rx: self.rx,
        }
    }
}

/// Complete value returned when the disconnected station epoch is
/// dematerialized. No field is optional because connected teardown has
/// already returned every child owner.
pub struct DisconnectedStaEpochParts<N, H, R, A, C> {
    pub network: N,
    pub hardware: H,
    pub rx: R,
    pub aggregate_tx: A,
    pub control: C,
}

impl<N, H, R, A, C> DisconnectedStaEpoch<N, H, R, A, C>
where
    R: StoppedStaRx,
{
    /// Consume the stopped RX service and form the next finite join epoch.
    pub fn prepare_reconnect(
        self,
    ) -> (
        N,
        ReconnectedStaEpoch<H, R::Preconnected, R::Persistent, A, C>,
    ) {
        let (rx, rx_resources) = self.rx.split_for_reconnect();
        (
            self.network,
            ReconnectedStaEpoch::new(
                self.hardware,
                rx,
                rx_resources,
                self.aggregate_tx,
                self.control,
            ),
        )
    }
}

/// Named split used while running scan temporarily owns hardware and RX.
pub struct RunningScanEpochParts<N, H, R, A, C> {
    pub retained: RunningScanRetained<N, H, R, A, C>,
    pub hardware: H,
    pub rx: R,
}

/// Owners which running scan cannot observe or replace.
pub struct RunningScanRetained<N, H, R, A, C> {
    network: N,
    aggregate_tx: A,
    control: C,
    _split: PhantomData<(H, R)>,
}

impl<N, H, R, A, C> RunningScanRetained<N, H, R, A, C> {
    /// Reunite the exact hardware and stopped RX returned by running scan.
    pub fn restore(self, hardware: H, rx: R) -> DisconnectedStaEpoch<N, H, R, A, C> {
        DisconnectedStaEpoch::new(self.network, hardware, rx, self.aggregate_tx, self.control)
    }
}

/// Radio resources for Authentication, Association and WPA2 after reconnect.
pub struct ReconnectedStaEpoch<H, R, E, A, C> {
    hardware: H,
    rx: R,
    rx_resources: E,
    aggregate_tx: A,
    control: C,
}

impl<H, R, E, A, C> ReconnectedStaEpoch<H, R, E, A, C> {
    /// Reassemble the exact stopped reconnect frontier after a role-neutral
    /// Wi-Fi transition returned and republished its hardware owner.
    pub const fn new(hardware: H, rx: R, rx_resources: E, aggregate_tx: A, control: C) -> Self {
        Self {
            hardware,
            rx,
            rx_resources,
            aggregate_tx,
            control,
        }
    }

    /// Borrow the only resources finite join phases are allowed to mutate.
    pub fn hardware_and_rx_mut(&mut self) -> (&mut H, &mut R) {
        (&mut self.hardware, &mut self.rx)
    }

    /// Borrow the sole connected hardware owner before the epoch is started.
    pub fn hardware_mut(&mut self) -> &mut H {
        &mut self.hardware
    }

    /// Consume the successful join frontier for connected-service assembly.
    pub fn into_parts(self) -> ReconnectedStaEpochParts<H, R, E, A, C> {
        ReconnectedStaEpochParts {
            hardware: self.hardware,
            rx: self.rx,
            rx_resources: self.rx_resources,
            aggregate_tx: self.aggregate_tx,
            control: self.control,
        }
    }
}

/// Named connected-assembly frontier returned after a successful finite join.
pub struct ReconnectedStaEpochParts<H, R, E, A, C> {
    pub hardware: H,
    pub rx: R,
    pub rx_resources: E,
    pub aggregate_tx: A,
    pub control: C,
}

#[cfg(test)]
mod tests;

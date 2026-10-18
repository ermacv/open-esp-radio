//! Exact owner transition from a successful join into connected RX.
//!
//! The first association owns raw PAC registers and one-time static resources.
//! A later association owns the exact cooperative hardware and persistent RX,
//! aggregate-TX and control resources returned by the preceding teardown.  This
//! module is the only place which may turn either frontier into the uniform
//! connected owner set.

use core::future::Future;

use crate::{
    datapath::rx::{
        dma::{RxEpochResources, StagedRxProducer},
        frontier::{ReceiveFrontier, RxFrontierError},
    },
    roles::station::epoch::{ReconnectedStaEpoch, ReconnectedStaEpochParts},
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_hal::{
    ieee80211::arena::{RadioOwnerArena, RadioOwnerArenaError},
    owner::RadioRuntimeOwner,
};

use oer_esp32s31_ieee80211::cooperative_hardware::CooperativeRadioHardware;

/// Whether RX promotion was attempted for the first or a later association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedEpochStartPhase {
    Initial,
    Reconnected,
}

/// One-time resources consumed only by the first connected association.
///
/// Supplying these only to [`start_esp32s31_initial_connected_epoch`]
/// prevents reconnect from initializing the same static cells again.
pub struct InitialConnectedEpochResources<'arena, X, A, C> {
    registers: &'arena RadioOwnerArena,
    rx: X,
    aggregate_tx: A,
    control: C,
}

impl<'arena, X, A, C> InitialConnectedEpochResources<'arena, X, A, C> {
    pub const fn new(
        registers: &'arena RadioOwnerArena,
        rx: X,
        aggregate_tx: A,
        control: C,
    ) -> Self {
        Self {
            registers,
            rx,
            aggregate_tx,
            control,
        }
    }
}

/// Uniform connected owner set produced for both initial and reconnected
/// associations.
pub struct ConnectedEpochStarted<H, X, A, C> {
    pub hardware: H,
    pub rx: X,
    pub aggregate_tx: A,
    pub control: C,
}

/// Complete owner return when connected materialization cannot finish.
///
/// Register publication failure retains raw PAC ownership. RX promotion
/// failure retains the published cooperative owner together with the exact
/// pre-connected and persistent resources. Neither case can be mistaken for a
/// successfully running connected epoch.
#[allow(clippy::large_enum_variant)]
pub enum ConnectedEpochStartFailure<I, H, P, X, A, C, E> {
    RegisterPublication {
        error: RadioOwnerArenaError,
        hardware: RadioRuntimeOwner,
        receive: P,
        initial: I,
    },
    Receive {
        phase: ConnectedEpochStartPhase,
        error: E,
        hardware: H,
        receive: P,
        rx_resources: X,
        aggregate_tx: A,
        control: C,
    },
}

/// Internal capability for joining a pre-connected RX frontier to its
/// persistent connected resources.
///
/// The trait exists to keep every storage lifetime and capacity out of the
/// public connected-epoch transition. Its production implementation below is
/// still concrete and returns both owners on failure.
#[doc(hidden)]
pub trait ConnectedRxMaterializer<H, P>: Sized {
    type Connected;
    type Error;

    fn materialize(
        self,
        receive: P,
        hardware: &mut H,
    ) -> impl Future<Output = Result<Self::Connected, (P, Self, Self::Error)>>;
}

impl<
    'arena,
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
>
    ConnectedRxMaterializer<
        CooperativeRadioHardware<'arena>,
        ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
    >
    for RxEpochResources<
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
where
    D: oer_time::Timer,
{
    type Connected = StagedRxProducer<
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
    type Error = RxFrontierError;

    async fn materialize(
        self,
        receive: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
        hardware: &mut CooperativeRadioHardware<'arena>,
    ) -> Result<
        Self::Connected,
        (
            ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
            Self,
            Self::Error,
        ),
    > {
        let storage = self.storage();
        match receive
            .try_into_live_with_storage(self.delay(), hardware, storage)
            .await
        {
            Ok(ring) => Ok(self.with_live_ring(ring)),
            Err(failure) => Err((failure.owner, self, failure.error)),
        }
    }
}

/// Materialize the first connected owner from raw PAC and the complete
/// initial-only resource graph.
///
/// Reconnect cannot call this function, so absence or repeated construction
/// of initial static resources is not representable at this boundary.
#[allow(clippy::type_complexity)]
pub async fn start_esp32s31_initial_connected_epoch<'arena, P, X, A, C>(
    hardware: RadioRuntimeOwner,
    receive: P,
    initial: InitialConnectedEpochResources<'arena, X, A, C>,
) -> Result<
    ConnectedEpochStarted<CooperativeRadioHardware<'arena>, X::Connected, A, C>,
    ConnectedEpochStartFailure<
        InitialConnectedEpochResources<'arena, X, A, C>,
        CooperativeRadioHardware<'arena>,
        P,
        X,
        A,
        C,
        X::Error,
    >,
>
where
    X: ConnectedRxMaterializer<CooperativeRadioHardware<'arena>, P>,
{
    let published = match initial.registers.publish(hardware) {
        Ok(published) => published,
        Err(failure) => {
            return Err(ConnectedEpochStartFailure::RegisterPublication {
                error: failure.error,
                hardware: failure.owner,
                receive,
                initial,
            });
        }
    };
    materialize_esp32s31_connected_rx(
        ConnectedEpochStartPhase::Initial,
        CooperativeRadioHardware::new(published),
        receive,
        initial.rx,
        initial.aggregate_tx,
        initial.control,
    )
    .await
}

/// Restore a later connected owner exclusively from a completed disconnected
/// epoch. No initial resource factory exists on this path.
#[allow(clippy::type_complexity)]
pub async fn start_esp32s31_reconnected_connected_epoch<'arena, P, X, A, C>(
    epoch: ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, P, X, A, C>,
) -> Result<
    ConnectedEpochStarted<CooperativeRadioHardware<'arena>, X::Connected, A, C>,
    ConnectedEpochStartFailure<
        InitialConnectedEpochResources<'arena, X, A, C>,
        CooperativeRadioHardware<'arena>,
        P,
        X,
        A,
        C,
        X::Error,
    >,
>
where
    X: ConnectedRxMaterializer<CooperativeRadioHardware<'arena>, P>,
{
    let ReconnectedStaEpochParts {
        hardware,
        rx,
        rx_resources,
        aggregate_tx,
        control,
    } = epoch.into_parts();
    materialize_esp32s31_connected_rx(
        ConnectedEpochStartPhase::Reconnected,
        hardware,
        rx,
        rx_resources,
        aggregate_tx,
        control,
    )
    .await
}

async fn materialize_esp32s31_connected_rx<'arena, P, X, A, C>(
    phase: ConnectedEpochStartPhase,
    mut hardware: CooperativeRadioHardware<'arena>,
    receive: P,
    rx_resources: X,
    aggregate_tx: A,
    control: C,
) -> Result<
    ConnectedEpochStarted<CooperativeRadioHardware<'arena>, X::Connected, A, C>,
    ConnectedEpochStartFailure<
        InitialConnectedEpochResources<'arena, X, A, C>,
        CooperativeRadioHardware<'arena>,
        P,
        X,
        A,
        C,
        X::Error,
    >,
>
where
    X: ConnectedRxMaterializer<CooperativeRadioHardware<'arena>, P>,
{
    match rx_resources.materialize(receive, &mut hardware).await {
        Ok(rx) => Ok(ConnectedEpochStarted {
            hardware,
            rx,
            aggregate_tx,
            control,
        }),
        Err((receive, rx_resources, error)) => Err(ConnectedEpochStartFailure::Receive {
            phase,
            error,
            hardware,
            receive,
            rx_resources,
            aggregate_tx,
            control,
        }),
    }
}

#[cfg(test)]
mod tests;

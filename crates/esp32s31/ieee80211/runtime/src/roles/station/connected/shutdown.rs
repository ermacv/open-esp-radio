//! Ordered executor shutdown frontier for one connected station epoch.
//!
//! A returned radio runner proves only that it no longer starts new hardware
//! work. The interrupt route may still publish wakes. Protocol ownership is
//! part of the runner's RX service, so closing IRQ publication is the sole
//! asynchronous frontier before driver teardown.

use crate::{
    datapath::{
        DatapathRunner, DatapathServices,
        irq::{InterruptEpoch, MacInterruptEpochDrain, MacInterruptEpochQuiesceError},
        services::SingleRoleServices,
    },
    roles::station::teardown::{
        ConnectedStaControlTeardown, ConnectedStaGroupSecurity, ConnectedStaRxPark,
        ConnectedStaTeardownFailure, ConnectedStaTeardownPort, ConnectedStaTeardownSuccess,
        ConnectedStaTxTeardown,
    },
};

use embassy_sync::blocking_mutex::raw::{RawMutex, RawMutex as NetworkRawMutex};

use oer_esp32s31_ieee80211_mac::{crypto::CcmpKeyHardware, irq::MacInterruptRoute};

/// Consuming owner interface needed by the common shutdown transaction.
///
/// The trait deliberately exposes no service methods. A caller can only
/// recover the network and driver owners after IRQ publication and every
/// attached task have been proved quiescent.
pub trait ConnectedEpochRunnerOwner: Sized {
    type Network;
    type Services;

    fn into_connected_epoch_parts(self) -> (Self::Network, Self::Services);
}

impl<'irq, M, N, B, RX, C> ConnectedEpochRunnerOwner for DatapathRunner<'irq, M, N, B, RX, C>
where
    M: NetworkRawMutex,
    N: crate::datapath::network::DatapathNetwork,
    B: DatapathServices<N::TxFrame, N::PhysicalTxFrame>,
    RX: crate::datapath::network::DatapathNetworkRxSet,
{
    type Network = N;
    type Services = B;

    fn into_connected_epoch_parts(self) -> (Self::Network, Self::Services) {
        self.into_parts()
    }
}

/// Complete reusable frontier after connected executor activity has stopped.
pub struct ConnectedEpochQuiesced<I, N, S> {
    pub interrupt: I,
    pub interrupt_drain: MacInterruptEpochDrain,
    pub network: N,
    pub services: S,
}

impl<I, N, S> ConnectedEpochQuiesced<I, N, S> {
    /// Replace an observation/fault decorator without exposing any other
    /// returned owner. HIL uses this to remove its services wrapper before the
    /// same production teardown transaction as ordinary firmware.
    pub fn map_services<U>(self, map: impl FnOnce(S) -> U) -> ConnectedEpochQuiesced<I, N, U> {
        ConnectedEpochQuiesced {
            interrupt: self.interrupt,
            interrupt_drain: self.interrupt_drain,
            network: self.network,
            services: map(self.services),
        }
    }
}

/// Complete reusable connected frontier after driver teardown succeeds.
pub struct ConnectedEpochTeardown<I, N, D> {
    pub interrupt: I,
    pub interrupt_drain: MacInterruptEpochDrain,
    pub network: N,
    pub driver: D,
}

/// Owner-preserving quarantined frontier after IRQ stopped but driver
/// teardown could not complete.
pub struct ConnectedEpochTeardownFailure<I, N, E> {
    pub interrupt: I,
    pub interrupt_drain: MacInterruptEpochDrain,
    pub network: N,
    pub error: E,
}

impl<I, N, H, R, X, C> ConnectedEpochQuiesced<I, N, SingleRoleServices<H, R, X, C>>
where
    H: CcmpKeyHardware,
    C: ConnectedStaControlTeardown<H, X>,
    R: ConnectedStaRxPark<H>,
    X: ConnectedStaTxTeardown,
{
    /// Stop control, RX DMA and TX, then clear both association keys while
    /// retaining network and task owners on every failure.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_teardown(
        self,
        group_security: ConnectedStaGroupSecurity,
    ) -> Result<
        ConnectedEpochTeardown<
            I,
            N,
            ConnectedStaTeardownSuccess<
                H,
                R::Parked,
                X::Resources,
                X::Aggregate,
                C::Report,
                C::Released,
            >,
        >,
        ConnectedEpochTeardownFailure<
            I,
            N,
            ConnectedStaTeardownFailure<H, R, R::Parked, X, C, C::Error, R::Error>,
        >,
    > {
        let Self {
            interrupt,
            interrupt_drain,
            network,
            services,
        } = self;
        match ConnectedStaTeardownPort::try_teardown(services, group_security) {
            Ok(driver) => Ok(ConnectedEpochTeardown {
                interrupt,
                interrupt_drain,
                network,
                driver,
            }),
            Err(error) => Err(ConnectedEpochTeardownFailure {
                interrupt,
                interrupt_drain,
                network,
                error,
            }),
        }
    }
}

/// Hardware-quiescence failure retaining the exact radio runner.
///
/// The only fallible edge here is disabling and draining the hardware
/// interrupt route.
pub enum ConnectedEpochQuiesceFailure<I, C, E> {
    Interrupt {
        error: MacInterruptEpochQuiesceError<E>,
        interrupt: I,
        runner: C,
    },
}

/// Park the logical IRQ consumer, then reveal the radio runner's network and
/// driver owners.
///
/// The runner has already reached its finite connected exit before this call.
/// The physical MAC route remains installed across role cutovers. Keeping the
/// runner opaque until its coalesced publications are drained prevents the
/// next consumer from interpreting stale work while preserving service for a
/// still-powered MAC.
#[allow(
    clippy::type_complexity,
    reason = "the public result retains the exact IRQ, network, services, and retry owners"
)]
pub fn quiesce_esp32s31_connected_epoch<'runtime, R, M, C>(
    mut interrupt: InterruptEpoch<'runtime, R, M>,
    platform: &R::Platform,
    runner: C,
) -> Result<
    ConnectedEpochQuiesced<InterruptEpoch<'runtime, R, M>, C::Network, C::Services>,
    ConnectedEpochQuiesceFailure<InterruptEpoch<'runtime, R, M>, C, R::Error>,
>
where
    R: MacInterruptRoute,
    M: RawMutex,
    C: ConnectedEpochRunnerOwner,
{
    let _ = platform;
    let interrupt_drain = match interrupt.park() {
        Ok(drain) => drain,
        Err(error) => {
            return Err(ConnectedEpochQuiesceFailure::Interrupt {
                error,
                interrupt,
                runner,
            });
        }
    };
    let (network, services) = runner.into_connected_epoch_parts();
    Ok(ConnectedEpochQuiesced {
        interrupt,
        interrupt_drain,
        network,
        services,
    })
}

#[cfg(test)]
mod tests;

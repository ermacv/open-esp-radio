//! Ordered connected-to-disconnected ESP32-S31 station transition.
//!
//! Interrupt publication and the staged protocol consumer must already be
//! quiesced by the platform executor. This transaction then owns the reusable
//! driver order: revoke association control state, stop RX DMA, prove TX idle,
//! recover descriptor/sequence resources and clear both association keys.

use crate::{
    datapath::{
        PinnedTxFrame, rx::dma::StagedRxProducer, services::SingleRoleServices,
        tx::resources::AggregateTxResources,
    },
    roles::station::{
        control::{
            ConnectedControl, ConnectedControlError, ConnectedControlHardware,
            ConnectedControlShutdown, ConnectedControlTx,
        },
        tx::{ConnectedTx, ConnectedTxTeardownParts},
    },
};
use oer_esp32s31_ieee80211_sta::connected::management_protection::StationManagementProtection;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile};

use oer_esp32s31_ieee80211_mac::{
    crypto::{
        CcmpKeyHardware, StaCcmpClearReport, StaGroupCcmpKeyMaterial, StaGroupCcmpSlot,
        clear_sta_ccmp_slots,
    },
    rx::{RxDma, RxRingError},
};

use oer_esp32s31_ieee80211_sta::{
    connected_rx::StaCcmpRxReplayControlEndpoint,
    single_mpdu_tx::{ConnectedTxSecurity, WifiTxResources},
};

use oer_ieee80211_mac::station::StaTxSequenceCounters;

/// RX owner already parked by a wider physical composition.
///
/// STA+AP has one common DMA producer, so the paired boundary must stop it
/// before either logical role is dismantled. Wrapping that exact stopped
/// owner lets the ordinary STA teardown retain its control/TX/key ordering
/// without attempting to stop the hardware a second time.
pub struct AlreadyParkedRx<R>(R);

impl<R> AlreadyParkedRx<R> {
    pub const fn new(parked: R) -> Self {
        Self(parked)
    }

    pub fn into_inner(self) -> R {
        self.0
    }
}

impl<H, R> ConnectedStaRxPark<H> for AlreadyParkedRx<R> {
    type Parked = R;
    type Error = core::convert::Infallible;

    fn try_park(self, _hardware: &mut H) -> Result<Self::Parked, (Self, Self::Error)> {
        Ok(self.0)
    }
}

/// Control-plane shutdown capability used by the connected teardown port.
pub trait ConnectedStaControlTeardown<H, X> {
    type Report;
    type Error;
    /// What the control's owner gets back once teardown completed: nothing
    /// for a control held by value, the empty slot for a placed one.
    type Released;

    fn shutdown(&mut self, hardware: &mut H, tx: &mut X) -> Result<Self::Report, Self::Error>;

    /// End the control after a complete teardown.
    fn release(self) -> Self::Released;
}

impl<'resources, M, H, X, const CAPACITY: usize> ConnectedStaControlTeardown<H, X>
    for ConnectedControl<'resources, M, CAPACITY>
where
    M: RawMutex,
    H: ConnectedControlHardware,
    X: ConnectedControlTx,
{
    type Report = ConnectedControlShutdown;
    type Error = ConnectedControlError;
    type Released = ();

    fn shutdown(&mut self, hardware: &mut H, tx: &mut X) -> Result<Self::Report, Self::Error> {
        ConnectedControl::shutdown(self, hardware, tx)
    }

    fn release(self) -> Self::Released {}
}

impl<
    'resources,
    M,
    H,
    X,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
> ConnectedStaControlTeardown<H, X>
    for crate::roles::station::esp_now_tx::EspNowConnectedControl<
        'resources,
        M,
        CONTROL_CAPACITY,
        TX_CAPACITY,
        PEERS,
    >
where
    M: RawMutex,
    H: ConnectedControlHardware,
    X: ConnectedControlTx,
{
    type Report = crate::roles::station::esp_now_tx::EspNowConnectedControlShutdown<PEERS>;
    type Error = crate::roles::station::esp_now_tx::EspNowConnectedControlError;
    type Released = crate::roles::station::control_slot::ReleasedConnectedControl<
        'resources,
        M,
        CONTROL_CAPACITY,
    >;

    fn shutdown(&mut self, hardware: &mut H, tx: &mut X) -> Result<Self::Report, Self::Error> {
        crate::roles::station::esp_now_tx::EspNowConnectedControl::shutdown(self, hardware, tx)
    }

    fn release(self) -> Self::Released {
        crate::roles::station::esp_now_tx::EspNowConnectedControl::release(self)
    }
}

/// Logical RX parking capability used by the connected teardown port.
///
/// Parking ends peer/protocol ownership but deliberately leaves the physical
/// DMA walker live for the next Wi-Fi role.
pub trait ConnectedStaRxPark<H>: Sized {
    type Parked;
    type Error;

    fn try_park(self, hardware: &mut H) -> Result<Self::Parked, (Self, Self::Error)>;
}

impl<
    'storage,
    'pool,
    'queue,
    H,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
> ConnectedStaRxPark<H>
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
where
    H: RxDma,
{
    type Parked = Self;
    type Error = RxRingError;

    fn try_park(self, _hardware: &mut H) -> Result<Self::Parked, (Self, Self::Error)> {
        if self.can_park_for_role_handoff() {
            Ok(self)
        } else {
            Err((self, RxRingError::Busy))
        }
    }
}

/// Idle connected-TX decomposition used by the connected teardown port.
pub trait ConnectedStaTxTeardown: Sized {
    type Resources;
    type Aggregate;

    fn try_return(self)
    -> Result<ConnectedTxTeardownParts<Self::Resources, Self::Aggregate>, Self>;
}

impl<
    'slot,
    'ampdu,
    'resources,
    M,
    P,
    E,
    T,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
    const ORDINARY_BUFFER_SIZE: usize,
> ConnectedStaTxTeardown
    for ConnectedTx<
        'slot,
        'ampdu,
        crate::datapath::PinnedTxFrame<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            QUEUE_DEPTH,
        >,
        P,
        E,
        T,
        SLOTS,
        AMPDU_BUFFER_SIZE,
        ORDINARY_BUFFER_SIZE,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    type Resources = WifiTxResources<'slot, P, E, T, ORDINARY_BUFFER_SIZE>;
    type Aggregate = AggregateTxResources<
        'ampdu,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        SLOTS,
        AMPDU_BUFFER_SIZE,
    >;

    fn try_return(
        self,
    ) -> Result<ConnectedTxTeardownParts<Self::Resources, Self::Aggregate>, Self> {
        self.try_into_station_parts()
    }
}

/// Complete successful driver frontier after one connected epoch.
pub struct ConnectedStaTeardownSuccess<H, R, T, A, C, L> {
    pub hardware: H,
    pub parked_rx: R,
    pub tx_resources: T,
    pub sequences: StaTxSequenceCounters,
    pub aggregate: A,
    pub control: C,
    /// What the control's owner gets back ([`ConnectedStaControlTeardown::release`]).
    pub released_control: L,
    pub security: ConnectedStaSecurityStopReport,
}

/// Group-key ownership retained outside the connected ordinary TX owner.
pub enum ConnectedStaGroupSecurity {
    Open,
    Wpa2Personal(StaGroupCcmpSlot),
    /// Pre-control owner retaining the secret rollback key and replay-control
    /// endpoint beside the hardware slot. Once installed into connected
    /// control, shutdown returns the ordinary slot-only variant above.
    Wpa2PersonalRekey {
        group: StaGroupCcmpSlot,
        material: StaGroupCcmpKeyMaterial,
        replay: StaCcmpRxReplayControlEndpoint,
        /// Receive protection of robust management frames, when negotiated.
        management: Option<StationManagementProtection>,
    },
}

/// Observable result of the security teardown edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedStaSecurityStopReport {
    OpenNoKeys,
    Wpa2Personal(StaCcmpClearReport),
    /// A composition bug supplied unlike pairwise/group modes. Every token
    /// that did exist was still cleared before this report was returned.
    ModeMismatchCleared {
        pairwise_hardware_index: Option<u8>,
        group_hardware_index: Option<u8>,
    },
}

/// Owner-preserving failure at the exact teardown stage that could not
/// complete. A board-level fault policy can retain these values without
/// guessing which hardware frontier remains live.
// CAPABILITY: timeout-error-recovery
pub enum ConnectedStaTeardownFailure<H, R, S, X, C, CE, RE> {
    Control {
        error: CE,
        services: SingleRoleServices<H, R, X, C>,
        group_security: ConnectedStaGroupSecurity,
    },
    Rx {
        error: RE,
        hardware: H,
        rx: R,
        tx: X,
        control: C,
        group_security: ConnectedStaGroupSecurity,
    },
    TxActive {
        hardware: H,
        parked_rx: S,
        tx: X,
        control: C,
        group_security: ConnectedStaGroupSecurity,
    },
}

// CAPABILITY: timeout-error-recovery
pub struct ConnectedStaTeardownPort;

impl ConnectedStaTeardownPort {
    /// Consume stopped connected services and return the peer-independent
    /// station owners in the only hardware-safe order.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn try_teardown<H, R, X, C>(
        services: SingleRoleServices<H, R, X, C>,
        group_security: ConnectedStaGroupSecurity,
    ) -> Result<
        ConnectedStaTeardownSuccess<
            H,
            R::Parked,
            X::Resources,
            X::Aggregate,
            C::Report,
            C::Released,
        >,
        ConnectedStaTeardownFailure<H, R, R::Parked, X, C, C::Error, R::Error>,
    >
    where
        H: CcmpKeyHardware,
        C: ConnectedStaControlTeardown<H, X>,
        R: ConnectedStaRxPark<H>,
        X: ConnectedStaTxTeardown,
    {
        let (mut hardware, rx, mut tx, mut control) = services.into_parts();
        let control_observation = match control.shutdown(&mut hardware, &mut tx) {
            Ok(report) => report,
            Err(error) => {
                return Err(ConnectedStaTeardownFailure::Control {
                    error,
                    services: SingleRoleServices::with_control(hardware, rx, tx, control),
                    group_security,
                });
            }
        };
        let parked_rx = match rx.try_park(&mut hardware) {
            Ok(parked) => parked,
            Err((rx, error)) => {
                return Err(ConnectedStaTeardownFailure::Rx {
                    error,
                    hardware,
                    rx,
                    tx,
                    control,
                    group_security,
                });
            }
        };
        let returned_tx = match tx.try_return() {
            Ok(returned) => returned,
            Err(tx) => {
                return Err(ConnectedStaTeardownFailure::TxActive {
                    hardware,
                    parked_rx,
                    tx,
                    control,
                    group_security,
                });
            }
        };
        let security = match (returned_tx.security, group_security) {
            (ConnectedTxSecurity::Open, ConnectedStaGroupSecurity::Open) => {
                ConnectedStaSecurityStopReport::OpenNoKeys
            }
            (
                ConnectedTxSecurity::Wpa2Personal(pairwise),
                ConnectedStaGroupSecurity::Wpa2Personal(group),
            ) => ConnectedStaSecurityStopReport::Wpa2Personal(clear_sta_ccmp_slots(
                &mut hardware,
                pairwise,
                group,
            )),
            (
                ConnectedTxSecurity::Wpa2Personal(pairwise),
                ConnectedStaGroupSecurity::Wpa2PersonalRekey { group, .. },
            ) => ConnectedStaSecurityStopReport::Wpa2Personal(clear_sta_ccmp_slots(
                &mut hardware,
                pairwise,
                group,
            )),
            (ConnectedTxSecurity::Wpa2Personal(pairwise), ConnectedStaGroupSecurity::Open) => {
                let pairwise_hardware_index = pairwise.hardware_index();
                pairwise.clear(&mut hardware);
                ConnectedStaSecurityStopReport::ModeMismatchCleared {
                    pairwise_hardware_index: Some(pairwise_hardware_index),
                    group_hardware_index: None,
                }
            }
            (ConnectedTxSecurity::Open, ConnectedStaGroupSecurity::Wpa2Personal(group)) => {
                let group_hardware_index = group.hardware_index();
                group.clear(&mut hardware);
                ConnectedStaSecurityStopReport::ModeMismatchCleared {
                    pairwise_hardware_index: None,
                    group_hardware_index: Some(group_hardware_index),
                }
            }
            (
                ConnectedTxSecurity::Open,
                ConnectedStaGroupSecurity::Wpa2PersonalRekey { group, .. },
            ) => {
                let group_hardware_index = group.hardware_index();
                group.clear(&mut hardware);
                ConnectedStaSecurityStopReport::ModeMismatchCleared {
                    pairwise_hardware_index: None,
                    group_hardware_index: Some(group_hardware_index),
                }
            }
        };
        Ok(ConnectedStaTeardownSuccess {
            hardware,
            parked_rx,
            tx_resources: returned_tx.resources,
            sequences: returned_tx.sequences,
            aggregate: returned_tx.aggregate,
            control: control_observation,
            released_control: control.release(),
            security,
        })
    }
}

#[cfg(test)]
mod tests;

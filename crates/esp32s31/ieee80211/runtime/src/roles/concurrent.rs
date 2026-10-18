//! Shared physical owners for one same-channel STA plus SoftAP composition.
//!
//! These types describe resources that exist once in hardware even though two
//! logical interfaces consume them. Role-local protocol state must not create
//! another physical owner.

use core::{cell::RefCell, future::Future};

use crate::datapath::{
    DatapathInterfaceScope, DatapathRunner, DatapathServices,
    irq::EmbassyMacIrqRuntime,
    network::{
        AP_NETWORK_INTERFACE_ID, DatapathNetwork, DatapathNetworkRxEndpoints,
        STA_NETWORK_INTERFACE_ID,
    },
    rx::{
        routed::{StaApStagedRxFrame, StaApStagedRxReceiver},
        staging::StagedRxFrame,
    },
};

use embassy_sync::blocking_mutex::{
    Mutex,
    raw::{CriticalSectionRawMutex, RawMutex},
};

use oer_memory::TaggedStableDmaBacking;

use oer_esp32s31_ieee80211_mac::{
    MacInterface,
    rx::{
        RxError,
        ampdu::{
            RX_BLOCK_ACK_BANK_COUNT, RxBlockAckActivation, RxBlockAckRequest, RxBlockAckSessions,
            RxBlockAckSessionsError, RxBlockAckSnapshot,
        },
        pool::{VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT},
    },
};

use oer_ieee80211_mac::vif::{StaApRxRoute, StaApVif};

use oer_network_interface::NetworkInterfaceId;

mod control;
mod rx_service;
mod station_rx_role;

pub use control::{
    StaApAccessPointControlProgress, StaApAccessPointControlRole, StaApControlArbiter,
    StaApControlError, StaApControlExit, StaApStationControlRole,
};

pub use rx_service::{
    StaApAccessPointRxRole, StaApRxService, StaApRxServiceError, StaApStationRxRole,
};

pub use station_rx_role::{StaApStationRxError, StaApStationRxSink};

/// The one ordinary/A-MPDU hardware owner shared by both logical roles.
/// Role-local state may hold these resources only while this owner records
/// the matching lent role.
pub type StaApPhysicalTx<Ordinary, Aggregate> =
    crate::datapath::paired::DatapathPairedPhysicalTx<Ordinary, Aggregate>;

/// Complete paired DATAPATH services graph before it is joined to the permanent
/// dual-interface network runner.
pub type StaApDatapathServices<H, PhysicalTx, Rx, Station, AccessPoint, T> =
    crate::datapath::paired::ConcurrentRoleServices<
        H,
        PhysicalTx,
        Rx,
        Station,
        AccessPoint,
        StaApControlArbiter<T>,
    >;

/// Compose the only supported station-plus-SoftAP role ordering.
///
/// Callers supply unique owners but cannot swap endpoint identities or attach
/// an unrelated control scheduler. Construction does not start either role;
/// the production supervisor validates hardware and enters the paired
/// lifecycle with this owner graph.
pub fn compose_sta_ap_datapath_services<H, PhysicalTx, Rx, Station, AccessPoint, T>(
    hardware: H,
    physical_tx: PhysicalTx,
    rx: Rx,
    station: Station,
    access_point: AccessPoint,
    timer: T,
) -> StaApDatapathServices<H, PhysicalTx, Rx, Station, AccessPoint, T> {
    crate::datapath::paired::ConcurrentRoleServices::new(
        STA_NETWORK_INTERFACE_ID,
        AP_NETWORK_INTERFACE_ID,
        hardware,
        physical_tx,
        rx,
        station,
        access_point,
        StaApControlArbiter::new(timer),
    )
}

/// Complete paired production DATAPATH owner.
///
/// The two network publishers are derived here from the permanent network
/// owner. Callers cannot attach an AP protocol role to the STA endpoint (or
/// vice versa), and there is no single-interface fallback in this graph.
pub type StaApDatapathRunner<'irq, M, N, Services, C> = DatapathRunner<
    'irq,
    M,
    N,
    Services,
    DatapathNetworkRxEndpoints<
        <N as DatapathNetwork>::RxPublisher,
        <N as DatapathNetwork>::RxPublisher,
    >,
    C,
>;

/// Bind a complete paired services graph to the one permanent tagged network
/// owner and the one MAC interrupt runtime.
///
/// This is the production composition boundary. Building a service set alone
/// does not start RX/TX or expose network endpoints; those become live only
/// while the production supervisor runs the returned paired owner.
pub fn compose_sta_ap_datapath_runner<'irq, M, N, Services, C>(
    irq: &'irq EmbassyMacIrqRuntime<M>,
    network: N,
    services: Services,
    clock: C,
) -> StaApDatapathRunner<'irq, M, N, Services, C>
where
    M: RawMutex,
    N: DatapathNetwork,
    N::RxPublisher: crate::datapath::network::DatapathNetworkRx,
    Services: DatapathServices<N::TxFrame, N::PhysicalTxFrame>,
{
    let endpoints = DatapathNetworkRxEndpoints::new(
        STA_NETWORK_INTERFACE_ID,
        network.rx_publisher(STA_NETWORK_INTERFACE_ID),
        AP_NETWORK_INTERFACE_ID,
        network.rx_publisher(AP_NETWORK_INTERFACE_ID),
    );
    DatapathRunner::new_with_scope(
        irq,
        network,
        DatapathInterfaceScope::pair(STA_NETWORK_INTERFACE_ID, AP_NETWORK_INTERFACE_ID),
        endpoints,
        services,
        clock,
    )
}

/// One upstream station peer plus every peer admitted by the SoftAP policy.
pub const STA_AP_RX_BLOCK_ACK_PEER_CAPACITY: usize =
    oer_esp32s31_ieee80211_ap::protocol::AP_MAX_CLIENTS + 1;

/// Serialized software owner of the eight physical ordinary RX BlockAck banks.
///
/// The ordinary station and primary SoftAP hardware paths share the same
/// direct banks. Both roles may therefore retain this shared reference, but
/// every mutation is serialized here and the session allocator exists only
/// once. The larger peer table tracks logical identities; it does not
/// manufacture more hardware banks.
pub struct StaApRxBlockAck {
    sessions: Mutex<
        CriticalSectionRawMutex,
        RefCell<RxBlockAckSessions<STA_AP_RX_BLOCK_ACK_PEER_CAPACITY>>,
    >,
}

impl StaApRxBlockAck {
    pub const fn new() -> Self {
        Self {
            sessions: Mutex::new(RefCell::new(RxBlockAckSessions::new())),
        }
    }

    pub const fn with_maximum_window(maximum_window: u16) -> Result<Self, RxBlockAckSessionsError> {
        match RxBlockAckSessions::with_maximum_window(maximum_window) {
            Ok(sessions) => Ok(Self {
                sessions: Mutex::new(RefCell::new(sessions)),
            }),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn with_sessions<R>(
        &self,
        operation: impl FnOnce(&mut RxBlockAckSessions<STA_AP_RX_BLOCK_ACK_PEER_CAPACITY>) -> R,
    ) -> R {
        self.sessions
            .lock(|sessions| operation(&mut sessions.borrow_mut()))
    }

    pub fn maximum_window(&self) -> u16 {
        self.sessions
            .lock(|sessions| sessions.borrow().maximum_window())
    }

    pub fn reset_after_hardware_reset(&self) {
        self.with_sessions(RxBlockAckSessions::reset_after_hardware_reset);
    }

    pub fn prepare_interface(
        &self,
        interface: MacInterface,
    ) -> Result<(), RxBlockAckSessionsError> {
        self.with_sessions(|sessions| sessions.prepare_interface(interface))
    }

    pub fn offer(&self, request: RxBlockAckRequest) -> Result<(), RxBlockAckSessionsError> {
        self.with_sessions(|sessions| sessions.offer(request))
    }

    pub fn begin_pending(&self) -> Result<Option<RxBlockAckActivation>, RxBlockAckSessionsError> {
        self.with_sessions(RxBlockAckSessions::begin_pending)
    }

    pub fn commit(
        &self,
        activation: RxBlockAckActivation,
    ) -> Result<RxBlockAckSnapshot, RxBlockAckSessionsError> {
        self.with_sessions(|sessions| sessions.commit(activation))
    }

    pub fn cancel(&self, activation: RxBlockAckActivation) -> Result<(), RxBlockAckSessionsError> {
        self.with_sessions(|sessions| sessions.cancel(activation))
    }

    pub fn stop(
        &self,
        interface: MacInterface,
        peer: [u8; 6],
        tid: u8,
    ) -> Option<RxBlockAckSnapshot> {
        self.with_sessions(|sessions| sessions.stop(interface, peer, tid))
    }

    pub fn discard_pending(&self, interface: MacInterface, peer: [u8; 6], tid: u8) -> bool {
        self.with_sessions(|sessions| sessions.discard_pending(interface, peer, tid))
    }

    pub fn stop_peer(
        &self,
        interface: MacInterface,
        peer: [u8; 6],
    ) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        self.with_sessions(|sessions| sessions.stop_peer(interface, peer))
    }

    pub fn snapshots(&self) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        self.sessions.lock(|sessions| sessions.borrow().snapshots())
    }

    pub fn snapshots_for(
        &self,
        interface: MacInterface,
    ) -> [Option<RxBlockAckSnapshot>; RX_BLOCK_ACK_BANK_COUNT] {
        self.sessions
            .lock(|sessions| sessions.borrow().snapshots_for(interface))
    }
}

impl Default for StaApRxBlockAck {
    fn default() -> Self {
        Self::new()
    }
}

/// Lower the role-neutral logical-interface identity only at the chip edge.
pub const fn lower_sta_ap_vif(interface: StaApVif) -> MacInterface {
    match interface {
        StaApVif::Station => MacInterface::Station,
        StaApVif::AccessPoint => MacInterface::AccessPoint,
    }
}

pub const fn network_interface_id(interface: StaApVif) -> NetworkInterfaceId {
    match interface {
        StaApVif::Station => STA_NETWORK_INTERFACE_ID,
        StaApVif::AccessPoint => AP_NETWORK_INTERFACE_ID,
    }
}

/// Decode only the two endpoint identities owned by this composition. An
/// unknown tag is never guessed from Ethernet contents or the active role.
pub const fn sta_ap_vif(interface: NetworkInterfaceId) -> Option<StaApVif> {
    if interface.value() == STA_NETWORK_INTERFACE_ID.value() {
        Some(StaApVif::Station)
    } else if interface.value() == AP_NETWORK_INTERFACE_ID.value() {
        Some(StaApVif::AccessPoint)
    } else {
        None
    }
}

/// Ownership-preserving TX dispatch result for the shared physical queue.
/// Unknown tags retain their exact lease and must be rejected explicitly by
/// the composition root; no role encoder may guess from Ethernet contents.
pub enum StaApTxDispatch<B> {
    Station(TaggedStableDmaBacking<NetworkInterfaceId, B>),
    AccessPoint(TaggedStableDmaBacking<NetworkInterfaceId, B>),
    Unknown(TaggedStableDmaBacking<NetworkInterfaceId, B>),
}

pub fn dispatch_sta_ap_tx<B>(
    frame: TaggedStableDmaBacking<NetworkInterfaceId, B>,
) -> StaApTxDispatch<B> {
    match sta_ap_vif(*frame.tag()) {
        Some(StaApVif::Station) => StaApTxDispatch::Station(frame),
        Some(StaApVif::AccessPoint) => StaApTxDispatch::AccessPoint(frame),
        None => StaApTxDispatch::Unknown(frame),
    }
}

/// Protocol-side ownership result from the common ordered RX stream.
pub enum StaApRxDispatch<
    'pool,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    Station(StagedRxFrame<'pool, CAPACITY, SLOTS>),
    AccessPoint(StagedRxFrame<'pool, CAPACITY, SLOTS>),
    Rejected {
        classification: Result<StaApRxRoute, RxError>,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
    },
}

/// Ownership disposition returned by one role-specific processor.
///
/// `Deferred` retains the exact staging lease. It is not a request to copy or
/// reconstruct the frame and can therefore be restored to the ordered common
/// frontier without changing DMA ownership identity.
pub enum RoutedRxDisposition<F> {
    Processed,
    Deferred(F),
}

/// Result of one finite common protocol-dispatch turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApRxTurn<D> {
    Idle,
    Station(D),
    AccessPoint,
    DeferredAccessPoint,
    Rejected(Result<StaApRxRoute, RxError>),
}

impl<'pool, const CAPACITY: usize, const SLOTS: usize> StaApRxDispatch<'pool, CAPACITY, SLOTS> {
    fn from_staged(frame: StaApStagedRxFrame<'pool, CAPACITY, SLOTS>) -> Self {
        let (classification, frame) = frame.into_parts();
        match classification {
            Ok(StaApRxRoute::Interface(StaApVif::Station)) => Self::Station(frame),
            Ok(StaApRxRoute::Interface(StaApVif::AccessPoint)) => Self::AccessPoint(frame),
            classification => Self::Rejected {
                classification,
                frame,
            },
        }
    }

    pub fn into_frame(self) -> StagedRxFrame<'pool, CAPACITY, SLOTS> {
        match self {
            Self::Station(frame) | Self::AccessPoint(frame) | Self::Rejected { frame, .. } => frame,
        }
    }
}

/// Sole protocol-side consumer of the ordered STA+AP stream.
///
/// The consumer chooses a role before handing ownership onward. Role-specific
/// tasks therefore cannot race, scan past one another or reorder leases in
/// separate queues.
pub struct StaApRxConsumer<
    'pool,
    'queue,
    M: RawMutex,
    const DEPTH: usize,
    const CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
> {
    frames: StaApStagedRxReceiver<'pool, 'queue, M, DEPTH, CAPACITY, SLOTS>,
    deferred: Option<StaApRxDispatch<'pool, CAPACITY, SLOTS>>,
}

impl<'pool, 'queue, M: RawMutex, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    StaApRxConsumer<'pool, 'queue, M, DEPTH, CAPACITY, SLOTS>
{
    /// Take sole protocol ownership of the receiving end of one paired epoch.
    pub const fn new(
        frames: StaApStagedRxReceiver<'pool, 'queue, M, DEPTH, CAPACITY, SLOTS>,
    ) -> Self {
        Self {
            frames,
            deferred: None,
        }
    }

    pub fn try_receive(&mut self) -> Option<StaApRxDispatch<'pool, CAPACITY, SLOTS>> {
        if let Some(frame) = self.deferred.take() {
            return Some(frame);
        }
        self.frames.try_receive().map(StaApRxDispatch::from_staged)
    }

    /// Restore a frame that the selected role could not process without
    /// violating its current TX/publication ordering edge.
    ///
    /// Only one ordered head may be deferred. Returning the exact input on a
    /// second attempt makes accidental overwrite impossible.
    pub fn defer(
        &mut self,
        frame: StaApRxDispatch<'pool, CAPACITY, SLOTS>,
    ) -> Result<(), StaApRxDispatch<'pool, CAPACITY, SLOTS>> {
        if self.deferred.is_some() {
            return Err(frame);
        }
        self.deferred = Some(frame);
        Ok(())
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len() + usize::from(self.deferred.is_some())
    }

    pub fn discard_queued(&mut self) -> usize {
        let mut discarded = usize::from(self.deferred.take().is_some());
        while let Some(frame) = self.frames.try_receive() {
            drop(frame);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }

    /// Consume at most one ordered staged owner and delegate it to the
    /// already-selected role processor.
    ///
    /// Station processing may await final network capacity. AP processing is
    /// deliberately finite: hardware/control actions remain in the common
    /// DATAPATH transaction domain, and an unsafe ordering edge returns the exact
    /// owner through [`RoutedRxDisposition::Deferred`]. No closure can
    /// observe a frame classified for the other role.
    pub async fn service_next<Station, StationFuture, StationDispatch, AccessPoint, Error>(
        &mut self,
        station: Station,
        access_point: AccessPoint,
    ) -> Result<StaApRxTurn<StationDispatch>, Error>
    where
        Station: FnOnce(StagedRxFrame<'pool, CAPACITY, SLOTS>) -> StationFuture,
        StationFuture: Future<Output = StationDispatch>,
        AccessPoint:
            FnOnce(
                StagedRxFrame<'pool, CAPACITY, SLOTS>,
            )
                -> Result<RoutedRxDisposition<StagedRxFrame<'pool, CAPACITY, SLOTS>>, Error>,
    {
        let Some(dispatch) = self.try_receive() else {
            return Ok(StaApRxTurn::Idle);
        };
        match dispatch {
            StaApRxDispatch::Station(frame) => Ok(StaApRxTurn::Station(station(frame).await)),
            StaApRxDispatch::AccessPoint(frame) => match access_point(frame)? {
                RoutedRxDisposition::Processed => Ok(StaApRxTurn::AccessPoint),
                RoutedRxDisposition::Deferred(frame) => {
                    // `try_receive` removed the only possible deferred head,
                    // and neither closure can borrow this consumer. Restoring
                    // the exact owner therefore cannot overwrite another one.
                    debug_assert!(self.deferred.is_none());
                    self.deferred = Some(StaApRxDispatch::AccessPoint(frame));
                    Ok(StaApRxTurn::DeferredAccessPoint)
                }
            },
            StaApRxDispatch::Rejected {
                classification,
                frame,
            } => {
                drop(frame);
                Ok(StaApRxTurn::Rejected(classification))
            }
        }
    }
}

#[cfg(test)]
mod tests;

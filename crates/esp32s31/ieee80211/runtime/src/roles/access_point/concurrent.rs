#![expect(
    clippy::manual_async_fn,
    reason = "role implementations retain explicit borrowed Future contracts"
)]
#![expect(
    clippy::result_large_err,
    reason = "no-alloc AP park and activation failures return the exact role owners"
)]

//! SoftAP protocol role borrowed by the common same-channel DATAPATH owner.

use crate::datapath::{
    TxBatchDemand,
    paired::{
        DatapathPairRole, DatapathPairedNetworkTxService, DatapathPairedPhysicalTx,
        DatapathPairedPhysicalTxError, DatapathPairedRoleOwner, DatapathPairedRoleTransitionError,
        DatapathPairedStopProgress,
    },
    rx::staging::StagedRxFrame,
};

use super::*;

type StaApNetworkTxBacking<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

/// AP RX failure preserving protocol versus final network publication origin.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApAccessPointRxError {
    Control(AccessPointControlError),
    Network(FrameLengthError),
}

/// Queue-independent AP protocol state and its role-local environment.
///
/// Physical RX/DMA, interrupt and final network endpoints remain outside this
/// value. TX/control state will be added to this same role owner rather than
/// manufacturing a second AP protocol object for the paired runtime.
pub struct AccessPointRoleRuntime<Processor, NetworkTx, Security, StatusObserver = ()> {
    protocol: Processor,
    network_tx: NetworkTx,
    security_material: Security,
    status_observer: StatusObserver,
    last_status_revision: u32,
    #[cfg(feature = "diagnostics")]
    network_backpressure_since_micros: Option<u64>,
    #[cfg(feature = "diagnostics")]
    delivery_observer: Option<&'static dyn RxNetworkDeliveryObserver>,
}

impl<Processor> AccessPointRoleRuntime<Processor, (), ()> {
    /// Compose the same AP protocol owner used by concurrent mode for a
    /// standalone epoch. Physical RX, TX scheduling and network publication
    /// remain in [`AccessPointControl`], outside the role-local state.
    pub const fn standalone(protocol: Processor) -> Self {
        Self {
            protocol,
            network_tx: (),
            security_material: (),
            status_observer: (),
            last_status_revision: 0,
            #[cfg(feature = "diagnostics")]
            network_backpressure_since_micros: None,
            #[cfg(feature = "diagnostics")]
            delivery_observer: None,
        }
    }
}

pub struct StaApAccessPointTxActive<Processor, Aggregate> {
    processor: Processor,
    aggregate: Aggregate,
}

pub struct StaApAccessPointTxParked<Processor, Aggregate> {
    processor: Processor,
    aggregate: Aggregate,
}

impl<Processor, Aggregate> StaApAccessPointTxActive<Processor, Aggregate> {
    pub const fn processor(&self) -> &Processor {
        &self.processor
    }

    pub fn processor_mut(&mut self) -> &mut Processor {
        &mut self.processor
    }

    pub const fn aggregate(&self) -> &Aggregate {
        &self.aggregate
    }

    pub fn aggregate_mut(&mut self) -> &mut Aggregate {
        &mut self.aggregate
    }

    pub fn into_parts(self) -> (Processor, Aggregate) {
        (self.processor, self.aggregate)
    }
}

impl<Processor, Aggregate> StaApAccessPointTxParked<Processor, Aggregate> {
    pub const fn processor(&self) -> &Processor {
        &self.processor
    }

    pub fn processor_mut(&mut self) -> &mut Processor {
        &mut self.processor
    }

    pub fn into_parts(self) -> (Processor, Aggregate) {
        (self.processor, self.aggregate)
    }
}

/// Failure to establish the paired AP ownership boundary. Both original
/// owners are returned; the caller never receives a half-parked role.
pub struct StaApAccessPointParkError<Role, Aggregate> {
    pub reason: StaApAccessPointParkFailure,
    pub role: Role,
    pub aggregate: Aggregate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApAccessPointParkFailure {
    Busy,
    Physical(DatapathPairedPhysicalTxError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApAccessPointTxOwnershipError {
    AlreadyActive,
    AlreadyParked,
    Busy,
    Physical(DatapathPairedPhysicalTxError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApAccessPointTxError {
    Operation(AccessPointDatapathError),
    Ownership(StaApAccessPointTxOwnershipError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApAccessPointPairedRxError {
    Role(StaApAccessPointRxError),
    PowerSave(AccessPointDatapathError),
    Ownership(StaApAccessPointTxOwnershipError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApAccessPointPairedControlError {
    Role(AccessPointControlError),
    PowerSave(AccessPointDatapathError),
    Ownership(StaApAccessPointTxOwnershipError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaApAccessPointFinishReason {
    Activation(StaApAccessPointTxOwnershipError),
    AggregateBusy,
    ProtocolBusy,
}

/// Complete AP teardown result before the station reclaims physical TX.
pub struct StaApAccessPointFinished<Stopped, NetworkTx, Security, PhysicalTx> {
    pub stopped: Stopped,
    pub network_tx: NetworkTx,
    pub security_material: Security,
    pub physical_tx: PhysicalTx,
}

/// Exact paired AP frontier retained when shutdown cannot cross an idle edge.
pub struct StaApAccessPointFinishFailure<Role, PhysicalTx> {
    pub reason: StaApAccessPointFinishReason,
    pub role: Role,
    pub physical_tx: PhysicalTx,
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
/// Return an initialized AP role to the already-existing paired physical
/// owner. `physical` must record a prior `Second` lend used to construct
/// `role` and `aggregate`; this function cannot manufacture another owner.
pub fn park_sta_ap_access_point_role<
    'storage,
    'beacon,
    'slot,
    'ampdu,
    P,
    E,
    T,
    NetworkTx,
    Security,
    StatusObserver,
    B,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>(
    role: AccessPointRoleRuntime<
        AccessPointProtocolProcessor<
            'storage,
            'beacon,
            'slot,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        NetworkTx,
        Security,
        StatusObserver,
    >,
    aggregate: AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
    physical: &mut DatapathPairedPhysicalTx<
        WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
        crate::datapath::tx::resources::AggregateTxResources<
            'ampdu,
            B,
            AMPDU_SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
    >,
) -> Result<
    AccessPointRoleRuntime<
        DatapathPairedRoleOwner<
            StaApAccessPointTxActive<
                AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
            >,
            StaApAccessPointTxParked<
                AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::AccessPointAmpduParked,
            >,
        >,
        NetworkTx,
        Security,
        StatusObserver,
    >,
    StaApAccessPointParkError<
        AccessPointRoleRuntime<
            AccessPointProtocolProcessor<
                'storage,
                'beacon,
                'slot,
                P,
                E,
                T,
                DMA_BUFFER_SIZE,
                TX_BUFFER_SIZE,
            >,
            NetworkTx,
            Security,
            StatusObserver,
        >,
        AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
    >,
>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    B: StableDmaBacking + 'ampdu,
{
    let AccessPointRoleRuntime {
        protocol,
        network_tx,
        security_material,
        status_observer,
        last_status_revision,
        #[cfg(feature = "diagnostics")]
        network_backpressure_since_micros,
        #[cfg(feature = "diagnostics")]
        delivery_observer,
    } = role;
    let (ordinary, protocol) = match protocol.try_park() {
        Ok(parts) => parts,
        Err(protocol) => {
            return Err(StaApAccessPointParkError {
                reason: StaApAccessPointParkFailure::Busy,
                role: AccessPointRoleRuntime {
                    protocol,
                    network_tx,
                    security_material,
                    status_observer,
                    last_status_revision,
                    #[cfg(feature = "diagnostics")]
                    network_backpressure_since_micros,
                    #[cfg(feature = "diagnostics")]
                    delivery_observer,
                },
                aggregate,
            });
        }
    };
    let (aggregate_resources, aggregate) = match aggregate.try_park() {
        Ok(parts) => parts,
        Err(aggregate) => {
            return Err(StaApAccessPointParkError {
                reason: StaApAccessPointParkFailure::Busy,
                role: AccessPointRoleRuntime {
                    protocol: AccessPointProtocolProcessor::resume(ordinary, protocol),
                    network_tx,
                    security_material,
                    status_observer,
                    last_status_revision,
                    #[cfg(feature = "diagnostics")]
                    network_backpressure_since_micros,
                    #[cfg(feature = "diagnostics")]
                    delivery_observer,
                },
                aggregate,
            });
        }
    };
    if let Err((error, ordinary, aggregate_resources)) =
        physical.restore(DatapathPairRole::Second, ordinary, aggregate_resources)
    {
        return Err(StaApAccessPointParkError {
            reason: StaApAccessPointParkFailure::Physical(error),
            role: AccessPointRoleRuntime {
                protocol: AccessPointProtocolProcessor::resume(ordinary, protocol),
                network_tx,
                security_material,
                status_observer,
                last_status_revision,
                #[cfg(feature = "diagnostics")]
                network_backpressure_since_micros,
                #[cfg(feature = "diagnostics")]
                delivery_observer,
            },
            aggregate: AccessPointAmpdu::resume(aggregate_resources, aggregate),
        });
    }
    Ok(AccessPointRoleRuntime {
        protocol: DatapathPairedRoleOwner::parked(StaApAccessPointTxParked {
            processor: protocol,
            aggregate,
        }),
        network_tx,
        security_material,
        status_observer,
        last_status_revision,
        #[cfg(feature = "diagnostics")]
        network_backpressure_since_micros,
        #[cfg(feature = "diagnostics")]
        delivery_observer,
    })
}

/// Stop and detach a quiescent paired AP role, returning the exact physical
/// TX pair to the shared owner.
///
/// The paired DATAPATH must first drive `service_stop` to `Stopped`.  This
/// transaction then verifies that both protocol and aggregate owners are
/// idle, stops the AP engine, and restores ordinary/A-MPDU resources without
/// constructing replacements.
#[allow(clippy::result_large_err, clippy::type_complexity)]
pub fn finish_sta_ap_access_point_role<
    'storage,
    'beacon,
    'slot,
    'ampdu,
    P,
    E,
    T,
    NetworkTx,
    Security,
    StatusObserver,
    B,
    H,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>(
    mut role: AccessPointRoleRuntime<
        DatapathPairedRoleOwner<
            StaApAccessPointTxActive<
                AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
            >,
            StaApAccessPointTxParked<
                AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::AccessPointAmpduParked,
            >,
        >,
        NetworkTx,
        Security,
        StatusObserver,
    >,
    mut physical_tx: DatapathPairedPhysicalTx<
        WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
        crate::datapath::tx::resources::AggregateTxResources<
            'ampdu,
            B,
            AMPDU_SLOTS,
            AMPDU_BUFFER_SIZE,
        >,
    >,
    hardware: &mut H,
) -> Result<
    StaApAccessPointFinished<
        AccessPointProtocolFinished<'storage, 'beacon, DMA_BUFFER_SIZE>,
        NetworkTx,
        Security,
        DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                B,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    >,
    StaApAccessPointFinishFailure<
        AccessPointRoleRuntime<
            DatapathPairedRoleOwner<
                StaApAccessPointTxActive<
                    AccessPointProtocolProcessor<
                        'storage,
                        'beacon,
                        'slot,
                        P,
                        E,
                        T,
                        DMA_BUFFER_SIZE,
                        TX_BUFFER_SIZE,
                    >,
                    AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
                >,
                StaApAccessPointTxParked<
                    AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                    super::ampdu::AccessPointAmpduParked,
                >,
            >,
            NetworkTx,
            Security,
            StatusObserver,
        >,
        DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                B,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    >,
>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    B: StableDmaBacking + 'ampdu,
    H: ApRuntimeHardware,
{
    if let Err(reason) = role.activate_tx(&mut physical_tx) {
        return Err(StaApAccessPointFinishFailure {
            reason: StaApAccessPointFinishReason::Activation(reason),
            role,
            physical_tx,
        });
    }
    let AccessPointRoleRuntime {
        protocol,
        network_tx,
        security_material,
        status_observer,
        last_status_revision,
        #[cfg(feature = "diagnostics")]
        network_backpressure_since_micros,
        #[cfg(feature = "diagnostics")]
        delivery_observer,
    } = role;
    let active = match protocol.try_into_active() {
        Ok(active) => active,
        Err(protocol) => {
            return Err(StaApAccessPointFinishFailure {
                reason: StaApAccessPointFinishReason::Activation(
                    StaApAccessPointTxOwnershipError::AlreadyParked,
                ),
                role: AccessPointRoleRuntime {
                    protocol,
                    network_tx,
                    security_material,
                    status_observer,
                    last_status_revision,
                    #[cfg(feature = "diagnostics")]
                    network_backpressure_since_micros,
                    #[cfg(feature = "diagnostics")]
                    delivery_observer,
                },
                physical_tx,
            });
        }
    };
    let (processor, aggregate) = active.into_parts();
    let (aggregate_resources, aggregate_state) = match aggregate.try_park() {
        Ok(parts) => parts,
        Err(aggregate) => {
            return Err(StaApAccessPointFinishFailure {
                reason: StaApAccessPointFinishReason::AggregateBusy,
                role: AccessPointRoleRuntime {
                    protocol: DatapathPairedRoleOwner::from_active(StaApAccessPointTxActive {
                        processor,
                        aggregate,
                    }),
                    network_tx,
                    security_material,
                    status_observer,
                    last_status_revision,
                    #[cfg(feature = "diagnostics")]
                    network_backpressure_since_micros,
                    #[cfg(feature = "diagnostics")]
                    delivery_observer,
                },
                physical_tx,
            });
        }
    };
    let stopped = match processor.try_finish_paired(hardware) {
        Ok(stopped) => stopped,
        Err(processor) => {
            return Err(StaApAccessPointFinishFailure {
                reason: StaApAccessPointFinishReason::ProtocolBusy,
                role: AccessPointRoleRuntime {
                    protocol: DatapathPairedRoleOwner::from_active(StaApAccessPointTxActive {
                        processor,
                        aggregate: AccessPointAmpdu::resume(aggregate_resources, aggregate_state),
                    }),
                    network_tx,
                    security_material,
                    status_observer,
                    last_status_revision,
                    #[cfg(feature = "diagnostics")]
                    network_backpressure_since_micros,
                    #[cfg(feature = "diagnostics")]
                    delivery_observer,
                },
                physical_tx,
            });
        }
    };
    let (ordinary, stopped) = stopped.into_parts();
    physical_tx
        .restore(DatapathPairRole::Second, ordinary, aggregate_resources)
        .unwrap_or_else(|_| unreachable!("paired AP activation records the second role"));
    drop(status_observer);
    let _ = last_status_revision;

    Ok(StaApAccessPointFinished {
        stopped,
        network_tx,
        security_material,
        physical_tx,
    })
}

impl<
    'storage,
    'beacon,
    'slot,
    'ampdu,
    P,
    E,
    T,
    NetworkTx,
    Security,
    StatusObserver,
    B,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>
    AccessPointRoleRuntime<
        DatapathPairedRoleOwner<
            StaApAccessPointTxActive<
                AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
            >,
            StaApAccessPointTxParked<
                AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::AccessPointAmpduParked,
            >,
        >,
        NetworkTx,
        Security,
        StatusObserver,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    B: StableDmaBacking + 'ampdu,
{
    fn observe_role_state(&mut self)
    where
        StatusObserver: FnMut(AccessPointServiceStatus),
    {
        let (revision, status) = self.protocol.active().map_or_else(
            || {
                let processor = &self
                    .protocol
                    .parked_state()
                    .expect("paired AP role is active or parked")
                    .processor;
                (processor.role_status_revision(), processor.role_status())
            },
            |active| {
                (
                    active.processor.role_status_revision(),
                    active.processor.role_status(),
                )
            },
        );
        if revision != self.last_status_revision {
            (self.status_observer)(status);
            self.last_status_revision = revision;
        }
    }

    pub fn activate_tx(
        &mut self,
        physical: &mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                B,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    ) -> Result<(), StaApAccessPointTxOwnershipError> {
        if !self.protocol.is_parked() {
            return Err(StaApAccessPointTxOwnershipError::AlreadyActive);
        }
        let (ordinary, aggregate) = physical
            .try_lend(DatapathPairRole::Second)
            .map_err(StaApAccessPointTxOwnershipError::Physical)?;
        self.protocol
            .try_activate(|parked| {
                let (processor, aggregate_state) = parked.into_parts();
                Ok::<_, (core::convert::Infallible, _)>(StaApAccessPointTxActive {
                    processor: AccessPointProtocolProcessor::resume(ordinary, processor),
                    aggregate: AccessPointAmpdu::resume(aggregate, aggregate_state),
                })
            })
            .map_err(|error| match error {
                DatapathPairedRoleTransitionError::AlreadyActive => {
                    StaApAccessPointTxOwnershipError::AlreadyActive
                }
                DatapathPairedRoleTransitionError::AlreadyParked => {
                    StaApAccessPointTxOwnershipError::AlreadyParked
                }
                DatapathPairedRoleTransitionError::Conversion(never) => match never {},
            })
    }

    pub fn park_tx(
        &mut self,
        physical: &mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                B,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    ) -> Result<(), StaApAccessPointTxOwnershipError> {
        if self.protocol.is_parked() {
            return Err(StaApAccessPointTxOwnershipError::AlreadyParked);
        }
        self.protocol
            .try_park(|active| {
                let (processor, aggregate) = active.into_parts();
                let (ordinary, processor_state) = match processor.try_park() {
                    Ok(parts) => parts,
                    Err(processor) => {
                        return Err((
                            StaApAccessPointTxOwnershipError::Busy,
                            StaApAccessPointTxActive {
                                processor,
                                aggregate,
                            },
                        ));
                    }
                };
                let (aggregate_resources, aggregate_state) = match aggregate.try_park() {
                    Ok(parts) => parts,
                    Err(aggregate) => {
                        return Err((
                            StaApAccessPointTxOwnershipError::Busy,
                            StaApAccessPointTxActive {
                                processor: AccessPointProtocolProcessor::resume(
                                    ordinary,
                                    processor_state,
                                ),
                                aggregate,
                            },
                        ));
                    }
                };
                match physical.restore(DatapathPairRole::Second, ordinary, aggregate_resources) {
                    Ok(()) => Ok(StaApAccessPointTxParked {
                        processor: processor_state,
                        aggregate: aggregate_state,
                    }),
                    Err((error, ordinary, aggregate_resources)) => Err((
                        StaApAccessPointTxOwnershipError::Physical(error),
                        StaApAccessPointTxActive {
                            processor: AccessPointProtocolProcessor::resume(
                                ordinary,
                                processor_state,
                            ),
                            aggregate: AccessPointAmpdu::resume(
                                aggregate_resources,
                                aggregate_state,
                            ),
                        },
                    )),
                }
            })
            .map_err(|error| match error {
                DatapathPairedRoleTransitionError::AlreadyActive => {
                    StaApAccessPointTxOwnershipError::AlreadyActive
                }
                DatapathPairedRoleTransitionError::AlreadyParked => {
                    StaApAccessPointTxOwnershipError::AlreadyParked
                }
                DatapathPairedRoleTransitionError::Conversion(error) => error,
            })
    }
}

impl<
    'resources,
    'storage,
    'beacon,
    'slot,
    'ampdu,
    M,
    H,
    P,
    E,
    T,
    Security,
    StatusObserver,
    SoftwareFrame,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>
    DatapathPairedNetworkTxService<
        H,
        DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        SoftwareFrame,
        PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    >
    for AccessPointRoleRuntime<
        DatapathPairedRoleOwner<
            StaApAccessPointTxActive<
                AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                AccessPointAmpdu<
                    'ampdu,
                    PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
                    AMPDU_SLOTS,
                    AMPDU_BUFFER_SIZE,
                >,
            >,
            StaApAccessPointTxParked<
                AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::AccessPointAmpduParked,
            >,
        >,
        network_tx::AccessPointNetworkTx<
            'ampdu,
            PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
            SoftwareFrame,
        >,
        Security,
        StatusObserver,
    >
where
    M: RawMutex,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer + crate::mac_clock::MacClockReader,
    H: TxHardware
        + ApRuntimeHardware
        + RxBlockAckHardware
        + oer_esp32s31_ieee80211_mac::tx::ampdu::HtAmpduHardware,
    'resources: 'ampdu,
    SoftwareFrame: SoftwareTxFrame,
{
    type Error = StaApAccessPointTxError;

    fn last_started_frame_count(&self) -> usize {
        self.network_tx.last_started_frame_count()
    }

    fn start<'a, I>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a
    where
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            > + 'a,
    {
        async move {
            self.activate_tx(physical)
                .map_err(StaApAccessPointTxError::Ownership)?;
            let active = self.protocol.active_mut().expect("activated above");
            let progress = self
                .network_tx
                .start(
                    &mut active.aggregate,
                    &mut active.processor,
                    hardware,
                    frame,
                    network,
                )
                .await
                .map_err(StaApAccessPointTxError::Operation)?;
            if progress == WifiTxProgress::Complete && !self.network_tx.has_prepared() {
                self.park_tx(physical)
                    .map_err(StaApAccessPointTxError::Ownership)?;
            }
            #[cfg(any(feature = "diagnostics", test))]
            self.network_tx.observe_service_boundary();
            Ok(progress)
        }
    }

    fn wait_deadline<'a>(
        &'a mut self,
        _physical: &'a mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    ) -> impl Future<Output = ()> + 'a {
        async move {
            let active = self
                .protocol
                .active_mut()
                .expect("paired scheduler retains the active AP role until TX terminal");
            self.network_tx.wait_deadline(&mut active.processor).await;
        }
    }

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        physical: &'a mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        wake: WifiTxWake,
    ) -> impl Future<Output = Result<WifiTxProgress, Self::Error>> + 'a {
        async move {
            let active = self
                .protocol
                .active_mut()
                .ok_or(StaApAccessPointTxError::Ownership(
                    StaApAccessPointTxOwnershipError::AlreadyParked,
                ))?;
            let progress = self
                .network_tx
                .service(&mut active.aggregate, &mut active.processor, hardware, wake)
                .map_err(StaApAccessPointTxError::Operation)?;
            if progress == WifiTxProgress::Complete && !self.network_tx.has_prepared() {
                self.park_tx(physical)
                    .map_err(StaApAccessPointTxError::Ownership)?;
            }
            Ok(progress)
        }
    }

    fn has_prepared(&self) -> bool {
        self.network_tx.has_prepared()
    }

    fn batch_demand<I>(&self, network: &I) -> TxBatchDemand
    where
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            >,
    {
        let Some(active) = self.protocol.active() else {
            return TxBatchDemand::single(network.queue_len());
        };
        let engine = active.processor.mac.engine();
        self.network_tx.batch_demand(
            |destination| {
                access_point_tx_batch_target(
                    engine.operational_tx_block_ack_window(destination),
                    AMPDU_SLOTS,
                )
            },
            network,
        )
    }

    fn prepared_frame_count(&self) -> usize {
        self.network_tx.prepared_frame_count()
    }

    fn prepared_start_ready(&self) -> bool {
        self.network_tx.prepared_start_ready()
    }

    fn advance_prepared<I>(
        &mut self,
        _hardware: &mut H,
        _physical: &mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        network: &I,
    ) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            >,
    {
        let active = self
            .protocol
            .active_mut()
            .ok_or(StaApAccessPointTxError::Ownership(
                StaApAccessPointTxOwnershipError::AlreadyParked,
            ))?;
        self.network_tx
            .advance_prepared(&mut active.aggregate, &mut active.processor, network)
            .map_err(StaApAccessPointTxError::Operation)
    }

    #[cfg(any(feature = "diagnostics", test))]
    fn mark_prepared_scheduler_phase(&mut self, phase: PreparedTxSchedulerPhase, at_micros: u64) {
        self.network_tx
            .mark_prepared_scheduler_phase(phase, at_micros);
    }

    fn start_prepared<I>(
        &mut self,
        hardware: &mut H,
        physical: &mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        network: &I,
    ) -> Result<WifiTxProgress, Self::Error>
    where
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            >,
    {
        let active = self
            .protocol
            .active_mut()
            .ok_or(StaApAccessPointTxError::Ownership(
                StaApAccessPointTxOwnershipError::AlreadyParked,
            ))?;
        let progress = self
            .network_tx
            .start_prepared(
                &mut active.aggregate,
                &mut active.processor,
                hardware,
                network,
            )
            .map_err(StaApAccessPointTxError::Operation)?;
        if progress == WifiTxProgress::Complete && !self.network_tx.has_prepared() {
            self.park_tx(physical)
                .map_err(StaApAccessPointTxError::Ownership)?;
        }
        Ok(progress)
    }

    fn cancel_prepared<I>(
        &mut self,
        physical: &mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        network: &I,
    ) -> Result<(), Self::Error>
    where
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            >,
    {
        let active = self
            .protocol
            .active_mut()
            .ok_or(StaApAccessPointTxError::Ownership(
                StaApAccessPointTxOwnershipError::AlreadyParked,
            ))?;
        self.network_tx
            .cancel_prepared(&mut active.aggregate, &mut active.processor, network)
            .map_err(StaApAccessPointTxError::Operation)?;
        self.park_tx(physical)
            .map_err(StaApAccessPointTxError::Ownership)
    }

    fn can_prepare(
        &self,
        _physical: &DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    ) -> bool {
        self.protocol.active().is_some_and(|active| {
            self.network_tx
                .can_prepare(&active.aggregate, active.processor.tx_pending())
        })
    }

    fn prepare<'a, I>(
        &'a mut self,
        _physical: &'a mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                StaApNetworkTxBacking<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        frame: SoftwareFrame,
        network: &'a I,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        H: 'a,
        I: SelectedBurstMaterializer<
                SoftwareFrame = SoftwareFrame,
                PhysicalFrame = PinnedTxFrame<
                    'resources,
                    M,
                    FRAME_CAPACITY,
                    HEADROOM,
                    TRAILER,
                    QUEUE_DEPTH,
                >,
            > + 'a,
    {
        async move {
            let active = self
                .protocol
                .active_mut()
                .ok_or(StaApAccessPointTxError::Ownership(
                    StaApAccessPointTxOwnershipError::AlreadyParked,
                ))?;
            self.network_tx
                .prepare(&mut active.aggregate, &mut active.processor, frame, network)
                .map_err(StaApAccessPointTxError::Operation)
        }
    }
}

impl<
    'pool,
    'storage,
    'beacon,
    'slot,
    'ampdu,
    H,
    P,
    E,
    T,
    NetworkTx,
    Security,
    StatusObserver,
    B,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>
    crate::roles::concurrent::StaApAccessPointRxRole<
        'pool,
        H,
        DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                B,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        STAGE_CAPACITY,
        STAGE_SLOTS,
    >
    for AccessPointRoleRuntime<
        DatapathPairedRoleOwner<
            StaApAccessPointTxActive<
                AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
            >,
            StaApAccessPointTxParked<
                AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::AccessPointAmpduParked,
            >,
        >,
        NetworkTx,
        Security,
        StatusObserver,
    >
where
    H: TxHardware + ApRuntimeHardware + RxBlockAckHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    B: StableDmaBacking + 'ampdu,
    NetworkTx: network_tx::AccessPointPowerSaveNetworkTx<P, E, T, DMA_BUFFER_SIZE, TX_BUFFER_SIZE>,
    Security: FnMut() -> ([u8; 32], u64),
    StatusObserver: FnMut(AccessPointServiceStatus),
{
    type Error = StaApAccessPointPairedRxError;

    fn publish_pending_rx(
        &mut self,
        physical_tx: &mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                B,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        network: &mut dyn DatapathNetworkRx,
        now: Instant,
    ) -> Result<DatapathRxProgress, Self::Error> {
        loop {
            let record = if let Some(active) = self.protocol.active() {
                active.processor.rx_batch_record()
            } else {
                self.protocol
                    .parked_state()
                    .expect("paired AP role is active or parked")
                    .processor
                    .rx_batch_record()
            }
            .map_err(|error| {
                StaApAccessPointPairedRxError::Role(StaApAccessPointRxError::Control(error))
            })?;
            let Some(record) = record else {
                let reorder_work_due = self.protocol.active().map_or_else(
                    || {
                        self.protocol
                            .parked_state()
                            .expect("paired AP role is active or parked")
                            .processor
                            .rx_reorder_work_due(now)
                    },
                    |active| active.processor.rx_reorder_work_due(now),
                );
                if !reorder_work_due {
                    break;
                }

                // A retained release or an expired gap has no new MAC IRQ
                // edge. Reacquire the AP protocol owner only at an available
                // physical boundary, service one ordered release, then return
                // the ordinary-TX resources before publishing its network
                // batch. If station TX currently owns the hardware, the exact
                // software work remains pending for its terminal edge.
                if physical_tx.lent_to().is_some() {
                    return Ok(DatapathRxProgress::ProtocolBlockedByTx);
                }
                let activated_for_reorder = self.protocol.is_parked();
                if activated_for_reorder {
                    self.activate_tx(physical_tx)
                        .map_err(StaApAccessPointPairedRxError::Ownership)?;
                }
                let serviced = self
                    .protocol
                    .active_mut()
                    .expect("AP reorder maintenance owns the physical TX boundary")
                    .processor
                    .service_rx_reorder_expiry(now)
                    .map_err(|error| {
                        StaApAccessPointPairedRxError::Role(StaApAccessPointRxError::Control(error))
                    })?;
                if activated_for_reorder {
                    self.park_tx(physical_tx)
                        .map_err(StaApAccessPointPairedRxError::Ownership)?;
                }
                if serviced {
                    continue;
                }
                break;
            };
            let frame = record.frame;
            let next_offset = record.next_offset;
            #[cfg(not(feature = "diagnostics"))]
            let result = network.try_send_parts(frame);
            #[cfg(feature = "diagnostics")]
            let result = {
                let delivery = RxNetworkDeliveryEvent::decoded(frame, None);
                let observer = self.delivery_observer;
                let mut before_publish = || {
                    if let Some(observer) = observer {
                        observer.admitted(delivery);
                    }
                };
                network.try_send_parts_observed(frame, &mut before_publish)
            };

            match result {
                Ok(()) => {
                    #[cfg(feature = "diagnostics")]
                    let protocol = ethernet_parts_protocol(frame);
                    if let Some(active) = self.protocol.active_mut() {
                        active.processor.commit_rx_batch_record(next_offset);
                    } else {
                        let parked = self
                            .protocol
                            .parked_state_mut()
                            .expect("paired AP role is active or parked");
                        parked.processor.commit_rx_batch_record(next_offset);
                    }
                    #[cfg(feature = "diagnostics")]
                    {
                        let observation = if let Some(active) = self.protocol.active_mut() {
                            &mut active.processor.observer.observation
                        } else {
                            &mut self
                                .protocol
                                .parked_state_mut()
                                .expect("paired AP role is active or parked")
                                .processor
                                .observer
                                .observation
                        };
                        observation.ethernet_frames_staged =
                            observation.ethernet_frames_staged.saturating_add(1);
                        match protocol {
                            Some(EthernetProtocol::ArpRequest) => {
                                observation.ethernet_arp_requests_staged =
                                    observation.ethernet_arp_requests_staged.saturating_add(1);
                            }
                            Some(EthernetProtocol::Ipv4Tcp) => {
                                observation.ethernet_tcp_frames_staged =
                                    observation.ethernet_tcp_frames_staged.saturating_add(1);
                            }
                            _ => {}
                        }
                    }
                }
                Err(RxEnqueueError::PoolExhausted)
                    if network.pool_exhaustion()
                        == crate::datapath::network::RxPoolExhaustion::DropFrame =>
                {
                    #[cfg(feature = "diagnostics")]
                    if let Some(observer) = self.delivery_observer {
                        observer.dropped(
                            RxNetworkDeliveryEvent::decoded(frame, None),
                            RxEnqueueError::PoolExhausted,
                        );
                    }
                    if let Some(active) = self.protocol.active_mut() {
                        active.processor.commit_rx_batch_record(next_offset);
                    } else {
                        self.protocol
                            .parked_state_mut()
                            .expect("paired AP role is active or parked")
                            .processor
                            .commit_rx_batch_record(next_offset);
                    }
                }
                Err(
                    RxEnqueueError::QueueFull
                    | RxEnqueueError::PoolExhausted
                    | RxEnqueueError::LinkDown,
                ) => {
                    #[cfg(feature = "diagnostics")]
                    self.network_backpressure_since_micros
                        .get_or_insert(now.as_micros());
                    return Ok(DatapathRxProgress::NetworkBackpressured);
                }
                Err(RxEnqueueError::InvalidLength(error)) => {
                    #[cfg(feature = "diagnostics")]
                    if let Some(observer) = self.delivery_observer {
                        observer.dropped(
                            RxNetworkDeliveryEvent::decoded(frame, None),
                            RxEnqueueError::InvalidLength(error),
                        );
                    }
                    return Err(StaApAccessPointPairedRxError::Role(
                        StaApAccessPointRxError::Network(error),
                    ));
                }
            }
        }

        #[cfg(feature = "diagnostics")]
        if let Some(started) = self.network_backpressure_since_micros.take() {
            let elapsed = now.as_micros().saturating_sub(started);
            let report = if let Some(active) = self.protocol.active_mut() {
                &mut active.processor.observer.observation
            } else {
                &mut self
                    .protocol
                    .parked_state_mut()
                    .expect("paired AP role is active or parked")
                    .processor
                    .observer
                    .observation
            };
            report.maximum_network_backpressure_micros = report
                .maximum_network_backpressure_micros
                .max(u32::try_from(elapsed).unwrap_or(u32::MAX));
        }
        Ok(DatapathRxProgress::Drained)
    }

    fn service_access_point_rx(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                B,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        frame: StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
        now: Instant,
    ) -> Result<
        crate::roles::concurrent::RoutedRxDisposition<
            StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
        >,
        Self::Error,
    > {
        let frame = if self.protocol.is_parked() {
            let parked = self
                .protocol
                .parked_state_mut()
                .expect("parked AP RX retains its role-local protocol owner");
            match parked
                .processor
                .service_routed_rx_while_parked(
                    frame,
                    now,
                    #[cfg(feature = "diagnostics")]
                    self.delivery_observer,
                )
                .map_err(|error| {
                    StaApAccessPointPairedRxError::Role(StaApAccessPointRxError::Control(error))
                })? {
                crate::roles::concurrent::RoutedRxDisposition::Processed => {
                    self.network_tx
                        .refresh_awake_demand(parked.processor.mac.engine());
                    if self.network_tx.has_power_save_release() {
                        self.activate_tx(physical_tx)
                            .map_err(StaApAccessPointPairedRxError::Ownership)?;
                    }
                    self.observe_role_state();
                    return Ok(crate::roles::concurrent::RoutedRxDisposition::Processed);
                }
                crate::roles::concurrent::RoutedRxDisposition::Deferred(frame) => frame,
            }
        } else {
            frame
        };
        if self.protocol.is_parked() {
            self.activate_tx(physical_tx)
                .map_err(StaApAccessPointPairedRxError::Ownership)?;
        }
        let result = self
            .protocol
            .active_mut()
            .expect("AP RX activated the physical TX owner")
            .processor
            .service_routed_rx(
                hardware,
                frame,
                &mut self.security_material,
                now,
                #[cfg(feature = "diagnostics")]
                self.delivery_observer,
            )
            .map_err(|error| {
                StaApAccessPointPairedRxError::Role(StaApAccessPointRxError::Control(error))
            });
        if result.is_ok() {
            let active = self
                .protocol
                .active_mut()
                .expect("AP RX retains the activated protocol owner");
            self.network_tx
                .refresh_awake_demand(active.processor.mac.engine());
            if !active.processor.tx_pending() {
                self.network_tx
                    .refresh_power_save_demand(&mut active.processor)
                    .map_err(StaApAccessPointPairedRxError::PowerSave)?;
            }
        }
        let may_park = self
            .protocol
            .active()
            .is_some_and(|active| !active.processor.tx_pending())
            && !self.network_tx.has_power_save_release();
        if may_park
            && let Err(error) = self.park_tx(physical_tx)
            && result.is_ok()
        {
            return Err(StaApAccessPointPairedRxError::Ownership(error));
        }
        self.observe_role_state();
        result
    }

    fn service_access_point_rx_during_tx(
        &mut self,
        frame: StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
        now: Instant,
    ) -> Result<
        crate::roles::concurrent::RoutedRxDisposition<
            StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
        >,
        Self::Error,
    > {
        let Some(active) = self.protocol.active_mut() else {
            // The physical ordinary-TX owner belongs to the station role.
            // Keep the exact ordered AP head until that transaction ends;
            // recovering AP TX resources here would violate affine ownership.
            return Ok(crate::roles::concurrent::RoutedRxDisposition::Deferred(
                frame,
            ));
        };
        let result = active
            .processor
            .service_routed_rx_during_tx::<H, _, _>(
                frame,
                &mut self.security_material,
                now,
                #[cfg(feature = "diagnostics")]
                self.delivery_observer,
            )
            .map_err(|error| {
                StaApAccessPointPairedRxError::Role(StaApAccessPointRxError::Control(error))
            });
        if result.is_ok() {
            self.network_tx
                .refresh_awake_demand(active.processor.mac.engine());
        }
        self.observe_role_state();
        result
    }

    fn has_pending_rx(&self, now: Instant) -> bool {
        self.protocol.active().map_or_else(
            || {
                let processor = &self
                    .protocol
                    .parked_state()
                    .expect("paired AP role is active or parked")
                    .processor;
                processor.rx_batch_pending() || processor.rx_reorder_work_due(now)
            },
            |active| {
                active.processor.rx_batch_pending() || active.processor.rx_reorder_work_due(now)
            },
        )
    }

    fn tx_pending(&self) -> bool {
        self.protocol
            .active()
            .is_some_and(|active| active.processor.tx_pending())
    }
}

impl<
    'storage,
    'beacon,
    'slot,
    'ampdu,
    H,
    P,
    E,
    T,
    NetworkTx,
    Security,
    StatusObserver,
    B,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
    const AMPDU_SLOTS: usize,
    const AMPDU_BUFFER_SIZE: usize,
>
    crate::roles::concurrent::StaApAccessPointControlRole<
        H,
        DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                B,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    >
    for AccessPointRoleRuntime<
        DatapathPairedRoleOwner<
            StaApAccessPointTxActive<
                AccessPointProtocolProcessor<
                    'storage,
                    'beacon,
                    'slot,
                    P,
                    E,
                    T,
                    DMA_BUFFER_SIZE,
                    TX_BUFFER_SIZE,
                >,
                AccessPointAmpdu<'ampdu, B, AMPDU_SLOTS, AMPDU_BUFFER_SIZE>,
            >,
            StaApAccessPointTxParked<
                AccessPointProtocolProcessorParked<'storage, 'beacon, DMA_BUFFER_SIZE>,
                super::ampdu::AccessPointAmpduParked,
            >,
        >,
        NetworkTx,
        Security,
        StatusObserver,
    >
where
    H: TxHardware + ApRuntimeHardware + RxBlockAckHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    B: StableDmaBacking + 'ampdu,
    NetworkTx: network_tx::AccessPointPowerSaveNetworkTx<P, E, T, DMA_BUFFER_SIZE, TX_BUFFER_SIZE>,
    StatusObserver: FnMut(AccessPointServiceStatus),
{
    type Error = StaApAccessPointPairedControlError;

    fn beacon_publication_due(&self, now: oer_time::Instant) -> bool {
        self.protocol.active().map_or_else(
            || {
                self.protocol
                    .parked_state()
                    .expect("paired AP role is active or parked")
                    .processor
                    .beacon_publication_due(now)
            },
            |active| active.processor.beacon_publication_due(now),
        )
    }

    fn service_access_point_control(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                B,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
        now: oer_time::Instant,
        retain_physical_tx: bool,
    ) -> Result<crate::roles::concurrent::StaApAccessPointControlProgress, Self::Error> {
        if self.protocol.is_parked() {
            self.activate_tx(physical_tx)
                .map_err(StaApAccessPointPairedControlError::Ownership)?;
        }
        let processor = &mut self
            .protocol
            .active_mut()
            .expect("AP control activated the physical TX owner")
            .processor;
        processor
            .apply_pending_protocol_actions(hardware)
            .map_err(StaApAccessPointPairedControlError::Role)?;
        let progress = processor
            .service_control(hardware, now)
            .map_err(StaApAccessPointPairedControlError::Role)?;
        self.observe_role_state();
        match progress {
            DatapathControlProgress::Idle => {
                if !retain_physical_tx {
                    self.park_tx(physical_tx)
                        .map_err(StaApAccessPointPairedControlError::Ownership)?;
                }
                Ok(crate::roles::concurrent::StaApAccessPointControlProgress::Idle)
            }
            DatapathControlProgress::More => {
                if !retain_physical_tx {
                    self.park_tx(physical_tx)
                        .map_err(StaApAccessPointPairedControlError::Ownership)?;
                }
                Ok(crate::roles::concurrent::StaApAccessPointControlProgress::More)
            }
            DatapathControlProgress::TxPending => {
                Ok(crate::roles::concurrent::StaApAccessPointControlProgress::TxPending)
            }
            DatapathControlProgress::Exit(never) => match never {},
        }
    }

    fn service_access_point_stop(
        &mut self,
        hardware: &mut H,
        physical_tx: &mut DatapathPairedPhysicalTx<
            WifiTxResources<'slot, P, E, T, TX_BUFFER_SIZE>,
            crate::datapath::tx::resources::AggregateTxResources<
                'ampdu,
                B,
                AMPDU_SLOTS,
                AMPDU_BUFFER_SIZE,
            >,
        >,
    ) -> Result<DatapathPairedStopProgress, Self::Error> {
        if self.protocol.is_parked() {
            self.activate_tx(physical_tx)
                .map_err(StaApAccessPointPairedControlError::Ownership)?;
        }
        {
            let processor = &mut self
                .protocol
                .active_mut()
                .expect("AP stop activated the physical TX owner")
                .processor;
            self.network_tx
                .discard_group_power_save(processor)
                .map_err(StaApAccessPointPairedControlError::PowerSave)?;
        }
        match self
            .protocol
            .active_mut()
            .expect("AP stop activated the physical TX owner")
            .processor
            .service_stop(hardware)
            .map_err(StaApAccessPointPairedControlError::Role)?
        {
            DatapathStopProgress::More => {
                self.park_tx(physical_tx)
                    .map_err(StaApAccessPointPairedControlError::Ownership)?;
                Ok(DatapathPairedStopProgress::More)
            }
            DatapathStopProgress::TxPending => Ok(DatapathPairedStopProgress::TxPending(
                DatapathPairRole::Second,
            )),
            DatapathStopProgress::Stopped => {
                self.park_tx(physical_tx)
                    .map_err(StaApAccessPointPairedControlError::Ownership)?;
                Ok(DatapathPairedStopProgress::Stopped)
            }
        }
    }

    fn next_access_point_control_deadline(&self) -> Result<oer_time::Instant, Self::Error> {
        if let Some(active) = self.protocol.active() {
            active.processor.next_control_deadline()
        } else {
            self.protocol
                .parked_state()
                .expect("paired AP role is active or parked")
                .processor
                .next_control_deadline()
        }
        .map_err(StaApAccessPointPairedControlError::Role)
    }
}

impl<Processor, NetworkTx, Security, StatusObserver>
    AccessPointRoleRuntime<Processor, NetworkTx, Security, StatusObserver>
{
    pub const fn new(
        protocol: Processor,
        network_tx: NetworkTx,
        security_material: Security,
        status_observer: StatusObserver,
    ) -> Self {
        Self {
            protocol,
            network_tx,
            security_material,
            status_observer,
            last_status_revision: 0,
            #[cfg(feature = "diagnostics")]
            network_backpressure_since_micros: None,
            #[cfg(feature = "diagnostics")]
            delivery_observer: None,
        }
    }

    pub const fn protocol(&self) -> &Processor {
        &self.protocol
    }

    pub fn protocol_mut(&mut self) -> &mut Processor {
        &mut self.protocol
    }

    pub fn into_parts(self) -> (Processor, NetworkTx, Security, StatusObserver) {
        (
            self.protocol,
            self.network_tx,
            self.security_material,
            self.status_observer,
        )
    }
}

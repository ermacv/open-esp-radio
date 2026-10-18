//! Role-neutral PAC reclaim from a finite station lifecycle phase.
//!
//! Scan and initial-join phases own `RadioRuntimeOwner` directly. Connected
//! phases own the same PAC through one cooperative register arena. This module
//! normalizes those four clean frontiers without knowing HIL, board, network
//! or executor policy. A failed arena reclaim returns the exact original
//! phase; callers may retain it for board-level fault policy and cannot
//! mistake it for stopped Wi-Fi.

use crate::{
    datapath::{
        irq::InterruptEpoch,
        rx::{
            dma::{ReceiveDmaStorage, RxEpochResources, StagedRxProducer},
            frontier::ReceiveFrontier,
        },
    },
    roles::{
        scan::rx::ScanRx,
        station::epoch::{DisconnectedStaEpoch, ReconnectedStaEpoch},
    },
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_hal::{
    ieee80211::arena::{RadioOwnerArenaError, RadioOwnerRepublish},
    owner::RadioRuntimeOwner,
};

use oer_esp32s31_ieee80211::cooperative_hardware::CooperativeRadioHardware;

use oer_esp32s31_ieee80211_mac::irq::MacInterruptRoute;

use oer_esp32s31_ieee80211_sta::attempt::{StaAttemptSecurity, StaAttemptStation, StaIdentity};

use oer_ieee80211_mac::channel::{WifiChannel, WifiChannelError};

use oer_ieee80211_runtime::station_network::{RunningStationNetwork, StationNetworkResources};

use super::{
    StationRuntimeResources, StationServiceOwner, StationServicePhase, StationStorageResources,
    WifiRoleOwner,
};

/// Exact non-PAC resources retained at the phase where station stop landed.
///
/// Connected variants carry the arena republish capability beside all
/// protocol owners. Restarting STA can republish the returned PAC into that
/// exact arena; another role may instead retain these resources while it owns
/// the role-neutral Wi-Fi frontier.
pub enum StationStoppedPhaseResources<'arena, S, J, N, DN, DR, A, C, E> {
    InitialScan {
        receive: S,
        network: N,
        identity: StaIdentity,
    },
    InitialJoin {
        receive: J,
        network: N,
        station: StaAttemptStation,
    },
    Disconnected {
        network: DN,
        receive: DR,
        aggregate_tx: A,
        control: C,
        station: StaAttemptStation,
        registers: RadioOwnerRepublish<'arena>,
    },
    Reconnected {
        network: N,
        receive: J,
        rx: E,
        aggregate_tx: A,
        control: C,
        station: StaAttemptStation,
        registers: RadioOwnerRepublish<'arena>,
    },
}

/// PAC and role-local resources returned by one successful phase reclaim.
pub struct StationPhaseReclaimed<R> {
    registers: RadioRuntimeOwner,
    resources: R,
    primary_channel: Option<WifiChannel>,
}

impl<R> StationPhaseReclaimed<R> {
    pub fn into_parts(self) -> (RadioRuntimeOwner, R, Option<WifiChannel>) {
        (self.registers, self.resources, self.primary_channel)
    }
}

/// Why a finite station phase could not be normalized into a reusable PAC.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationPhaseReclaimError {
    Registers(RadioOwnerArenaError),
    InvalidChannel(WifiChannelError),
    /// The phase still owns an active connected data-plane transaction and
    /// therefore has not reached a reclaimable stop frontier.
    ConnectedActive,
}

/// Failed phase reclaim with the exact original phase still retained.
pub struct StationPhaseReclaimFailure<P> {
    pub error: StationPhaseReclaimError,
    pub phase: P,
}

/// Failed republish while rebuilding a previously stopped station phase.
///
/// Both the role-neutral PAC and every role-local resource are returned. A
/// caller may retain this quarantined frontier but cannot lose the arena token
/// or retry with a newly fabricated register owner.
pub struct StationPhaseRestoreFailure<R> {
    pub error: RadioOwnerArenaError,
    pub registers: RadioRuntimeOwner,
    pub resources: R,
}

/// A supposedly stopped phase could not be normalized for a new station
/// request. The complete original resource graph is retained.
pub struct StationPhaseRebindFailure<R> {
    pub resources: R,
}

type RebindableStationPhase<
    'arena,
    'storage,
    'pool,
    'queue,
    RD,
    M,
    ND,
    NR,
    NS,
    A,
    C,
    P,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> = StationStoppedPhaseResources<
    'arena,
    ScanRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
    StationNetworkResources<ND, NR, NS>,
    RunningStationNetwork<NS, NR>,
    P,
    A,
    C,
    RxEpochResources<
        'storage,
        'pool,
        'queue,
        RD,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
>;

/// Minimum interrupt-epoch evidence needed before PAC reclaim may begin.
///
/// This is deliberately an observation, not an API for disabling an IRQ.
/// The active role must quiesce its route before returning its lifecycle
/// owner; reclaim refuses to repair or weaken that contract afterward.
pub trait StationInterruptEpochState {
    fn is_active(&self) -> bool;
}

impl<R, M> StationInterruptEpochState for InterruptEpoch<'_, R, M>
where
    R: MacInterruptRoute,
    M: RawMutex,
{
    fn is_active(&self) -> bool {
        InterruptEpoch::is_active(self)
    }
}

/// Complete role and reusable station graph after phase and PAC reclaim.
///
/// The concrete composition still owns the final interrupt-route extraction:
/// this value merely proves the epoch reported inactive before it moved. It
/// retains every exact resource required either to reassemble `WifiStopped`
/// or to restart the returned station phase later.
pub struct StationRuntimeReclaimed<
    'storage,
    'security,
    P,
    Membership,
    I,
    D,
    T,
    B,
    S,
    const RECORDS: usize,
> {
    registers: RadioRuntimeOwner,
    role: WifiRoleOwner<P, Membership>,
    interrupt: I,
    storage: StationStorageResources<'storage, D, T, RECORDS>,
    board: B,
    phase: S,
    security: StaAttemptSecurity<'security>,
    primary_channel: Option<WifiChannel>,
}

impl<'storage, 'security, P, Membership, I, D, T, B, S, const RECORDS: usize>
    StationRuntimeReclaimed<'storage, 'security, P, Membership, I, D, T, B, S, RECORDS>
{
    #[allow(clippy::type_complexity)]
    pub fn into_parts(
        self,
    ) -> (
        RadioRuntimeOwner,
        WifiRoleOwner<P, Membership>,
        I,
        StationStorageResources<'storage, D, T, RECORDS>,
        B,
        S,
        StaAttemptSecurity<'security>,
        Option<WifiChannel>,
    ) {
        (
            self.registers,
            self.role,
            self.interrupt,
            self.storage,
            self.board,
            self.phase,
            self.security,
            self.primary_channel,
        )
    }
}

/// Exact lifecycle owner retained when runtime reclaim cannot prove safety.
pub enum StationRuntimeReclaimFailure<O> {
    Phase {
        error: StationPhaseReclaimError,
        owner: O,
    },
}

/// Consume any clean finite station phase and recover its sole PAC owner.
///
/// The returned channel is absent only when stop occurred during initial scan,
/// before a peer had been selected. The caller updates the role-neutral radio
/// context after the logical role returns its phase owners. The physical IRQ
/// epoch may remain installed and moves unchanged into the next role.
#[allow(clippy::type_complexity)]
#[expect(
    clippy::result_large_err,
    reason = "recovery returns the unchanged affine phase and its hardware owners without allocation"
)]
pub fn try_reclaim_esp32s31_station_phase<'arena, S, J, N, DN, DR, A, C, E, K>(
    phase: StationServicePhase<
        RadioRuntimeOwner,
        S,
        J,
        N,
        DisconnectedStaEpoch<DN, CooperativeRadioHardware<'arena>, DR, A, C>,
        ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, J, E, A, C>,
        K,
    >,
) -> Result<
    StationPhaseReclaimed<StationStoppedPhaseResources<'arena, S, J, N, DN, DR, A, C, E>>,
    StationPhaseReclaimFailure<
        StationServicePhase<
            RadioRuntimeOwner,
            S,
            J,
            N,
            DisconnectedStaEpoch<DN, CooperativeRadioHardware<'arena>, DR, A, C>,
            ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, J, E, A, C>,
            K,
        >,
    >,
> {
    let primary_channel = match &phase {
        StationServicePhase::InitialScan { .. } => None,
        StationServicePhase::InitialJoin { station, .. }
        | StationServicePhase::RunningScan { station, .. }
        | StationServicePhase::Reconnected { station, .. } => match station.selected_channel() {
            Ok(channel) => Some(channel),
            Err(error) => {
                return Err(StationPhaseReclaimFailure {
                    error: StationPhaseReclaimError::InvalidChannel(error),
                    phase,
                });
            }
        },
        StationServicePhase::Connected { .. } => {
            return Err(StationPhaseReclaimFailure {
                error: StationPhaseReclaimError::ConnectedActive,
                phase,
            });
        }
    };
    let (registers, resources, primary_channel) = match phase {
        StationServicePhase::InitialScan {
            hardware,
            receive,
            network,
            identity,
        } => (
            hardware,
            StationStoppedPhaseResources::InitialScan {
                receive,
                network,
                identity,
            },
            primary_channel,
        ),
        StationServicePhase::InitialJoin {
            hardware,
            receive,
            network,
            station,
        } => (
            hardware,
            StationStoppedPhaseResources::InitialJoin {
                receive,
                network,
                station,
            },
            primary_channel,
        ),
        StationServicePhase::RunningScan {
            disconnected,
            station,
        } => {
            let parts = disconnected.into_parts();
            let reclaimed = match parts.hardware.try_into_reclaimed_registers() {
                Ok(reclaimed) => reclaimed,
                Err((hardware, error)) => {
                    return Err(StationPhaseReclaimFailure {
                        error: StationPhaseReclaimError::Registers(error),
                        phase: StationServicePhase::RunningScan {
                            disconnected: DisconnectedStaEpoch::new(
                                parts.network,
                                hardware,
                                parts.rx,
                                parts.aggregate_tx,
                                parts.control,
                            ),
                            station,
                        },
                    });
                }
            };
            let (registers, republish) = reclaimed.into_parts();
            (
                registers,
                StationStoppedPhaseResources::Disconnected {
                    network: parts.network,
                    receive: parts.rx,
                    aggregate_tx: parts.aggregate_tx,
                    control: parts.control,
                    station,
                    registers: republish,
                },
                primary_channel,
            )
        }
        StationServicePhase::Reconnected {
            epoch,
            network,
            station,
        } => {
            let parts = epoch.into_parts();
            let reclaimed = match parts.hardware.try_into_reclaimed_registers() {
                Ok(reclaimed) => reclaimed,
                Err((hardware, error)) => {
                    return Err(StationPhaseReclaimFailure {
                        error: StationPhaseReclaimError::Registers(error),
                        phase: StationServicePhase::Reconnected {
                            epoch: ReconnectedStaEpoch::new(
                                hardware,
                                parts.rx,
                                parts.rx_resources,
                                parts.aggregate_tx,
                                parts.control,
                            ),
                            network,
                            station,
                        },
                    });
                }
            };
            let (registers, republish) = reclaimed.into_parts();
            (
                registers,
                StationStoppedPhaseResources::Reconnected {
                    network,
                    receive: parts.rx,
                    rx: parts.rx_resources,
                    aggregate_tx: parts.aggregate_tx,
                    control: parts.control,
                    station,
                    registers: republish,
                },
                primary_channel,
            )
        }
        StationServicePhase::Connected { connected } => {
            return Err(StationPhaseReclaimFailure {
                error: StationPhaseReclaimError::ConnectedActive,
                phase: StationServicePhase::Connected { connected },
            });
        }
    };
    Ok(StationPhaseReclaimed {
        registers,
        resources,
        primary_channel,
    })
}

/// Republish a role-neutral PAC into the exact arena retained by a stopped
/// connected phase, or directly restore a scan/join phase.
#[allow(clippy::type_complexity)]
#[expect(
    clippy::result_large_err,
    reason = "recovery returns the unchanged affine phase and its hardware owners without allocation"
)]
pub fn try_restore_esp32s31_station_phase<'arena, S, J, N, DN, DR, A, C, E, K>(
    registers: RadioRuntimeOwner,
    resources: StationStoppedPhaseResources<'arena, S, J, N, DN, DR, A, C, E>,
) -> Result<
    StationServicePhase<
        RadioRuntimeOwner,
        S,
        J,
        N,
        DisconnectedStaEpoch<DN, CooperativeRadioHardware<'arena>, DR, A, C>,
        ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, J, E, A, C>,
        K,
    >,
    StationPhaseRestoreFailure<StationStoppedPhaseResources<'arena, S, J, N, DN, DR, A, C, E>>,
> {
    match resources {
        StationStoppedPhaseResources::InitialScan {
            receive,
            network,
            identity,
        } => Ok(StationServicePhase::InitialScan {
            hardware: registers,
            receive,
            network,
            identity,
        }),
        StationStoppedPhaseResources::InitialJoin {
            receive,
            network,
            station,
        } => Ok(StationServicePhase::InitialJoin {
            hardware: registers,
            receive,
            network,
            station,
        }),
        StationStoppedPhaseResources::Disconnected {
            network,
            receive,
            aggregate_tx,
            control,
            station,
            registers: republish,
        } => match republish.try_publish(registers) {
            Ok(published) => Ok(StationServicePhase::RunningScan {
                disconnected: DisconnectedStaEpoch::new(
                    network,
                    CooperativeRadioHardware::new(published),
                    receive,
                    aggregate_tx,
                    control,
                ),
                station,
            }),
            Err(failure) => Err(StationPhaseRestoreFailure {
                error: failure.error,
                registers: failure.owner,
                resources: StationStoppedPhaseResources::Disconnected {
                    network,
                    receive,
                    aggregate_tx,
                    control,
                    station,
                    registers: failure.republish,
                },
            }),
        },
        StationStoppedPhaseResources::Reconnected {
            network,
            receive,
            rx,
            aggregate_tx,
            control,
            station,
            registers: republish,
        } => match republish.try_publish(registers) {
            Ok(published) => Ok(StationServicePhase::Reconnected {
                epoch: ReconnectedStaEpoch::new(
                    CooperativeRadioHardware::new(published),
                    receive,
                    rx,
                    aggregate_tx,
                    control,
                ),
                network,
                station,
            }),
            Err(failure) => Err(StationPhaseRestoreFailure {
                error: failure.error,
                registers: failure.owner,
                resources: StationStoppedPhaseResources::Reconnected {
                    network,
                    receive,
                    rx,
                    aggregate_tx,
                    control,
                    station,
                    registers: failure.republish,
                },
            }),
        },
    }
}

/// Normalize a stopped station phase for a fresh SSID/security request.
///
/// Initial join discards its old candidate and returns to initial scan.
/// Disconnected and reconnected phases retain their already-created network,
/// aggregate and RX infrastructure but enter running scan, where the next
/// candidate replaces the old peer before join. No DMA operation is started
/// or stopped here.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
#[expect(
    clippy::result_large_err,
    reason = "recovery returns the unchanged affine phase and its hardware owners without allocation"
)]
pub fn try_rebind_esp32s31_station_phase<
    'arena,
    'storage,
    'pool,
    'queue,
    RD,
    M,
    ND,
    NR,
    NS,
    A,
    C,
    P,
    F,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    resources: RebindableStationPhase<
        'arena,
        'storage,
        'pool,
        'queue,
        RD,
        M,
        ND,
        NR,
        NS,
        A,
        C,
        P,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
    storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    identity: StaIdentity,
    from_live: F,
) -> Result<
    RebindableStationPhase<
        'arena,
        'storage,
        'pool,
        'queue,
        RD,
        M,
        ND,
        NR,
        NS,
        A,
        C,
        P,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
    StationPhaseRebindFailure<
        RebindableStationPhase<
            'arena,
            'storage,
            'pool,
            'queue,
            RD,
            M,
            ND,
            NR,
            NS,
            A,
            C,
            P,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
    >,
>
where
    M: RawMutex,
    F: FnOnce(
        StagedRxProducer<
            'storage,
            'pool,
            'queue,
            RD,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
    ) -> P,
{
    match resources {
        StationStoppedPhaseResources::InitialScan {
            receive, network, ..
        } => Ok(StationStoppedPhaseResources::InitialScan {
            receive,
            network,
            identity,
        }),
        StationStoppedPhaseResources::InitialJoin {
            receive,
            network,
            station: _,
        } => match receive.phase() {
            crate::datapath::rx::frontier::RxFrontierPhase::Live => {
                let ring = receive.try_into_live().unwrap_or_else(|_| {
                    unreachable!("live initial-join phase owns a live RX ring")
                });
                Ok(StationStoppedPhaseResources::InitialScan {
                    receive: ScanRx::from_live(ring, storage),
                    network,
                    identity,
                })
            }
            _ => {
                let ring = receive.try_into_halted().unwrap_or_else(|_| {
                    unreachable!("quiescent initial-join phase owns a halted RX ring")
                });
                Ok(StationStoppedPhaseResources::InitialScan {
                    receive: ScanRx::from_halted(ring, storage),
                    network,
                    identity,
                })
            }
        },
        StationStoppedPhaseResources::Disconnected {
            network,
            receive,
            aggregate_tx,
            control,
            mut station,
            registers,
        } => {
            station.station_address = identity.station_address;
            station.association_preference = identity.association_preference;
            Ok(StationStoppedPhaseResources::Disconnected {
                network,
                receive,
                aggregate_tx,
                control,
                station,
                registers,
            })
        }
        StationStoppedPhaseResources::Reconnected {
            network,
            receive,
            rx,
            aggregate_tx,
            control,
            mut station,
            registers,
        } => {
            let network = match network {
                StationNetworkResources::Running(network) => network,
                network @ StationNetworkResources::Unstarted { .. } => {
                    return Err(StationPhaseRebindFailure {
                        resources: StationStoppedPhaseResources::Reconnected {
                            network,
                            receive,
                            rx,
                            aggregate_tx,
                            control,
                            station,
                            registers,
                        },
                    });
                }
            };
            let live = match receive.phase() {
                crate::datapath::rx::frontier::RxFrontierPhase::Live => receive
                    .try_into_live()
                    .unwrap_or_else(|_| unreachable!("live reconnect phase owns a live RX ring")),
                _ => {
                    return Err(StationPhaseRebindFailure {
                        resources: StationStoppedPhaseResources::Reconnected {
                            network: StationNetworkResources::Running(network),
                            receive,
                            rx,
                            aggregate_tx,
                            control,
                            station,
                            registers,
                        },
                    });
                }
            };
            station.station_address = identity.station_address;
            station.association_preference = identity.association_preference;
            Ok(StationStoppedPhaseResources::Disconnected {
                network,
                receive: from_live(rx.with_live_ring(live)),
                aggregate_tx,
                control,
                station,
                registers,
            })
        }
    }
}

/// Reclaim one complete station lifecycle owner without losing a faulted
/// frontier.
///
/// This is the reusable boundary required by both production firmware and
/// HIL. It first verifies the returned IRQ epoch, then normalizes the phase's
/// direct or arena-backed PAC owner, and only afterward decomposes persistent
/// runtime resources. Both failure variants retain the original owner.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
#[expect(
    clippy::result_large_err,
    reason = "recovery returns the unchanged affine phase and its hardware owners without allocation"
)]
pub fn try_reclaim_esp32s31_station_runtime<
    'role,
    'storage,
    'security,
    'arena,
    P,
    Membership,
    I,
    D,
    T,
    B,
    S,
    J,
    N,
    DN,
    DR,
    A,
    C,
    E,
    K,
    const RECORDS: usize,
>(
    owner: StationServiceOwner<
        'security,
        StationRuntimeResources<'role, 'storage, WifiRoleOwner<P, Membership>, I, D, T, B, RECORDS>,
        StationServicePhase<
            RadioRuntimeOwner,
            S,
            J,
            N,
            DisconnectedStaEpoch<DN, CooperativeRadioHardware<'arena>, DR, A, C>,
            ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, J, E, A, C>,
            K,
        >,
    >,
) -> Result<
    StationRuntimeReclaimed<
        'storage,
        'security,
        P,
        Membership,
        I,
        D,
        T,
        B,
        StationStoppedPhaseResources<'arena, S, J, N, DN, DR, A, C, E>,
        RECORDS,
    >,
    StationRuntimeReclaimFailure<
        StationServiceOwner<
            'security,
            StationRuntimeResources<
                'role,
                'storage,
                WifiRoleOwner<P, Membership>,
                I,
                D,
                T,
                B,
                RECORDS,
            >,
            StationServicePhase<
                RadioRuntimeOwner,
                S,
                J,
                N,
                DisconnectedStaEpoch<DN, CooperativeRadioHardware<'arena>, DR, A, C>,
                ReconnectedStaEpoch<CooperativeRadioHardware<'arena>, J, E, A, C>,
                K,
            >,
        >,
    >,
>
where
    I: StationInterruptEpochState,
{
    let (runtime, phase, security) = owner.into_parts();
    let reclaimed = match try_reclaim_esp32s31_station_phase(phase) {
        Ok(reclaimed) => reclaimed,
        Err(failure) => {
            return Err(StationRuntimeReclaimFailure::Phase {
                error: failure.error,
                owner: StationServiceOwner::new(runtime, failure.phase, security),
            });
        }
    };
    let (registers, phase, primary_channel) = reclaimed.into_parts();
    let runtime = runtime.into_parts();
    let (role, interrupt) = runtime.radio.into_parts();
    Ok(StationRuntimeReclaimed {
        registers,
        role,
        interrupt,
        storage: runtime.storage,
        board: runtime.board,
        phase,
        security,
        primary_channel,
    })
}

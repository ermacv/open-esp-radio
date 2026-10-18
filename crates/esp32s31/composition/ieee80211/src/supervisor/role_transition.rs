#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc role-resume state retains the concrete returned station owner"
)]

//! Exact station frontiers parked and restored across physical role transitions.

use super::*;

pub(super) struct ProductionStationRoleResources {
    pub(super) scan_table: &'static mut ScanTable,
    pub(super) scan_frame: &'static mut [u8],
    pub(super) ethernet: &'static mut [u8],
    pub(super) resume: ProductionStationRoleResume,
    pub(super) board: ProductionStationBoardResources,
    pub(super) station_address: [u8; 6],
}

impl ProductionStationRoleResources {
    pub(super) const fn station_address(&self) -> [u8; 6] {
        self.station_address
    }

    pub(super) fn scan_storage(&mut self) -> (&mut ScanTable, &mut [u8]) {
        (&mut *self.scan_table, &mut *self.scan_frame)
    }

    pub(super) fn scan_table(&self) -> &ScanTable {
        self.scan_table
    }
}

/// Exact station lifecycle state retained while AP temporarily owns Wi-Fi.
///
/// Only the halted RX ring and the shared TX/network endpoints leave this
/// frontier. The next STA epoch therefore resumes the same allocation and
/// register-arena capabilities; AP switching is not a hidden reset.
pub(super) enum ProductionStationRoleResume {
    Fresh {
        network: WifiNetworkResources,
    },
    Returned {
        phase: ProductionStationReturnedPhase,
        security: StaAttemptSecurity<'static>,
    },
}

pub(super) enum ProductionStationReturnedPhase {
    InitialScan {
        network: WifiNetworkResources,
        identity: StaIdentity,
    },
    InitialJoin {
        network: WifiNetworkResources,
        station: StaAttemptStation,
    },
    Disconnected {
        network: RunningWifiNetwork,
        rx: Option<ConnectedRxEpochResources>,
        control: ConnectedControlStatics,
        station: StaAttemptStation,
        registers: RadioOwnerRepublish<'static>,
    },
    Reconnected {
        network: WifiNetworkResources,
        rx: Option<ConnectedRxEpochResources>,
        control: ConnectedControlStatics,
        station: StaAttemptStation,
        registers: RadioOwnerRepublish<'static>,
    },
}

impl ProductionStationRoleResume {
    pub(super) fn radio_runner_mut(&mut self) -> &mut NetworkRunner {
        match self {
            Self::Fresh { network }
            | Self::Returned {
                phase:
                    ProductionStationReturnedPhase::InitialScan { network, .. }
                    | ProductionStationReturnedPhase::InitialJoin { network, .. }
                    | ProductionStationReturnedPhase::Reconnected { network, .. },
                ..
            } => network.radio_runner_mut(),
            Self::Returned {
                phase: ProductionStationReturnedPhase::Disconnected { network, .. },
                ..
            } => network.radio_runner_mut(),
        }
    }

    /// Move the persistent standalone RX publisher into a role which stages
    /// through the same queue. Initial scan/join phases have not created that
    /// publisher yet.
    pub(super) fn take_retained_rx(&mut self) -> Option<ConnectedRxEpochResources> {
        match self {
            Self::Returned {
                phase:
                    ProductionStationReturnedPhase::Disconnected { rx, .. }
                    | ProductionStationReturnedPhase::Reconnected { rx, .. },
                ..
            } => rx.take(),
            Self::Fresh { .. }
            | Self::Returned {
                phase:
                    ProductionStationReturnedPhase::InitialScan { .. }
                    | ProductionStationReturnedPhase::InitialJoin { .. },
                ..
            } => None,
        }
    }

    /// Restore the persistent producer after a temporary role. A producer
    /// first created by AP before the station's initial connection is dropped
    /// here so the later connected epoch may perform the sole initial split.
    pub(super) fn restore_retained_rx(&mut self, rx: ConnectedRxEpochResources) {
        match self {
            Self::Returned {
                phase:
                    ProductionStationReturnedPhase::Disconnected { rx: slot, .. }
                    | ProductionStationReturnedPhase::Reconnected { rx: slot, .. },
                ..
            } => {
                assert!(
                    slot.replace(rx).is_none(),
                    "station RX publisher is already restored"
                );
            }
            Self::Fresh { .. }
            | Self::Returned {
                phase:
                    ProductionStationReturnedPhase::InitialScan { .. }
                    | ProductionStationReturnedPhase::InitialJoin { .. },
                ..
            } => drop(rx),
        }
    }
}

#[allow(clippy::result_large_err)]
pub(super) fn try_split_wifi_stopped_resources(
    resources: ProductionWifiStoppedResources,
) -> Result<
    (
        ProductionWifiPhysicalResources,
        ProductionStationRoleResources,
    ),
    ProductionWifiStoppedResources,
> {
    let returned = match resources {
        ProductionWifiStoppedResources::Fresh(fresh) => {
            let ProductionWifiFreshResources {
                dma,
                rx_ring,
                tx,
                scan_table,
                scan_frame,
                ethernet,
                network,
                mut board,
                station_address,
            } = fresh;
            let aggregate_tx = board
                .initial_connected
                .as_mut()
                .expect("fresh station owns initial connected resources")
                .take_aggregate();
            return Ok((
                ProductionWifiPhysicalResources {
                    dma,
                    rx_ring,
                    tx,
                    aggregate_tx,
                },
                ProductionStationRoleResources {
                    scan_table,
                    scan_frame,
                    ethernet,
                    resume: ProductionStationRoleResume::Fresh { network },
                    board,
                    station_address,
                },
            ));
        }
        ProductionWifiStoppedResources::Returned(returned) => returned,
    };
    let ProductionWifiReusableResources {
        storage,
        mut board,
        phase,
        security,
    } = returned;
    let (dma, tx_epoch, scan_table, scan_frame, ethernet) = storage.into_parts();
    let station_address = board.interface.interface.address;
    let (ring, phase, aggregate_tx) = match phase {
        StationStoppedPhaseResources::InitialScan {
            receive,
            network,
            identity,
        } => {
            let ring = match receive.phase() {
                oer_esp32s31_ieee80211_runtime::datapath::rx::frontier::RxFrontierPhase::Live => {
                    ProductionRxRing::Live(
                        receive
                            .into_live()
                            .unwrap_or_else(|_| unreachable!("live scan phase owns a live ring")),
                    )
                }
                _ => {
                    ProductionRxRing::Halted(receive.into_halted().unwrap_or_else(|_| {
                        unreachable!("quiescent scan phase owns a halted ring")
                    }))
                }
            };
            let aggregate_tx = board
                .initial_connected
                .as_mut()
                .expect("initial scan retains initial connected resources")
                .take_aggregate();
            (
                ring,
                ProductionStationReturnedPhase::InitialScan { network, identity },
                aggregate_tx,
            )
        }
        StationStoppedPhaseResources::InitialJoin {
            receive,
            network,
            station,
        } => {
            let ring = match receive.phase() {
                oer_esp32s31_ieee80211_runtime::datapath::rx::frontier::RxFrontierPhase::Live => {
                    ProductionRxRing::Live(
                        receive
                            .try_into_live()
                            .unwrap_or_else(|_| unreachable!("live join phase owns a live ring")),
                    )
                }
                _ => {
                    ProductionRxRing::Halted(receive.try_into_halted().unwrap_or_else(|_| {
                        unreachable!("quiescent join phase owns a halted ring")
                    }))
                }
            };
            let aggregate_tx = board
                .initial_connected
                .as_mut()
                .expect("initial join retains initial connected resources")
                .take_aggregate();
            (
                ring,
                ProductionStationReturnedPhase::InitialJoin { network, station },
                aggregate_tx,
            )
        }
        StationStoppedPhaseResources::Disconnected {
            network,
            receive,
            aggregate_tx,
            control,
            station,
            registers,
        } => {
            let (ring, rx) = receive.into_physical_parts();
            (
                ring,
                ProductionStationReturnedPhase::Disconnected {
                    network,
                    rx: Some(rx),
                    control,
                    station,
                    registers,
                },
                aggregate_tx,
            )
        }
        StationStoppedPhaseResources::Reconnected {
            network,
            receive,
            rx,
            aggregate_tx,
            control,
            station,
            registers,
        } => {
            let ring = match receive.phase() {
                oer_esp32s31_ieee80211_runtime::datapath::rx::frontier::RxFrontierPhase::Live => {
                    ProductionRxRing::Live(
                        receive.try_into_live().unwrap_or_else(|_| {
                            unreachable!("live reconnect phase owns a live ring")
                        }),
                    )
                }
                _ => ProductionRxRing::Halted(receive.try_into_halted().unwrap_or_else(|_| {
                    unreachable!("quiescent reconnect phase owns a halted ring")
                })),
            };
            (
                ring,
                ProductionStationReturnedPhase::Reconnected {
                    network,
                    rx: Some(rx),
                    control,
                    station,
                    registers,
                },
                aggregate_tx,
            )
        }
    };
    Ok((
        ProductionWifiPhysicalResources {
            dma,
            rx_ring: Some(ring),
            tx: ProductionOrdinaryTxResources::Epoch(tx_epoch),
            aggregate_tx,
        },
        ProductionStationRoleResources {
            scan_table,
            scan_frame,
            ethernet,
            resume: ProductionStationRoleResume::Returned { phase, security },
            board,
            station_address,
        },
    ))
}

pub(super) fn join_station_activation_resources(
    physical: ProductionWifiPhysicalResources,
    station: ProductionStationRoleResources,
) -> ProductionWifiStoppedResources {
    let ProductionWifiPhysicalResources {
        dma,
        rx_ring,
        tx,
        aggregate_tx,
    } = physical;
    let ProductionStationRoleResources {
        scan_table,
        scan_frame,
        ethernet,
        resume,
        mut board,
        station_address,
    } = station;
    match resume {
        ProductionStationRoleResume::Fresh { network } => {
            board
                .initial_connected
                .as_mut()
                .expect("fresh station retains connected resources")
                .restore_aggregate(aggregate_tx);
            ProductionWifiStoppedResources::Fresh(ProductionWifiFreshResources {
                dma,
                rx_ring,
                tx,
                scan_table,
                scan_frame,
                ethernet,
                network,
                board,
                station_address,
            })
        }
        ProductionStationRoleResume::Returned { phase, security } => {
            let ring = rx_ring.expect("returned physical Wi-Fi resources own an RX ring");
            let tx_epoch = match tx {
                ProductionOrdinaryTxResources::Epoch(tx_epoch) => tx_epoch,
                ProductionOrdinaryTxResources::Uninitialized(_) => {
                    unreachable!("a returned Wi-Fi epoch owns initialized TX storage")
                }
            };
            let mut aggregate_tx = Some(aggregate_tx);
            if matches!(
                &phase,
                ProductionStationReturnedPhase::InitialScan { .. }
                    | ProductionStationReturnedPhase::InitialJoin { .. }
            ) {
                board
                    .initial_connected
                    .as_mut()
                    .expect("initial station phase retains connected resources")
                    .restore_aggregate(
                        aggregate_tx
                            .take()
                            .expect("the physical frontier owns aggregate storage"),
                    );
            }
            let phase = match phase {
                ProductionStationReturnedPhase::InitialScan { network, identity } => {
                    StationStoppedPhaseResources::InitialScan {
                        receive: ring.into_scan(dma.storage()),
                        network,
                        identity,
                    }
                }
                ProductionStationReturnedPhase::InitialJoin { network, station } => {
                    StationStoppedPhaseResources::InitialJoin {
                        receive: match ring {
                            ProductionRxRing::Halted(ring) => ReceiveFrontier::from_halted(ring),
                            ProductionRxRing::Live(ring) => ReceiveFrontier::from_live(ring),
                        },
                        network,
                        station,
                    }
                }
                ProductionStationReturnedPhase::Disconnected {
                    network,
                    rx,
                    control,
                    station,
                    registers,
                } => StationStoppedPhaseResources::Disconnected {
                    network,
                    receive: match ring {
                        ProductionRxRing::Halted(ring) => ConnectedParkedRx::from_halted(
                            rx.expect("disconnected station must reclaim its RX publisher")
                                .with_halted_ring(ring),
                        ),
                        ProductionRxRing::Live(ring) => ConnectedParkedRx::from_live(
                            rx.expect("disconnected station must reclaim its RX publisher")
                                .with_live_ring(ring),
                        ),
                    },
                    aggregate_tx: aggregate_tx
                        .take()
                        .expect("disconnected phase reclaims AP aggregate owner"),
                    control,
                    station,
                    registers,
                },
                ProductionStationReturnedPhase::Reconnected {
                    network,
                    rx,
                    control,
                    station,
                    registers,
                } => StationStoppedPhaseResources::Reconnected {
                    network,
                    receive: match ring {
                        ProductionRxRing::Halted(ring) => ReceiveFrontier::from_halted(ring),
                        ProductionRxRing::Live(ring) => ReceiveFrontier::from_live(ring),
                    },
                    rx: rx.expect("reconnected station must reclaim its RX publisher"),
                    aggregate_tx: aggregate_tx
                        .take()
                        .expect("reconnected phase reclaims AP aggregate owner"),
                    control,
                    station,
                    registers,
                },
            };
            ProductionWifiStoppedResources::Returned(ProductionWifiReusableResources {
                storage: ProductionStationStorage::new(
                    dma, tx_epoch, scan_table, scan_frame, ethernet,
                ),
                board,
                phase,
                security,
            })
        }
    }
}

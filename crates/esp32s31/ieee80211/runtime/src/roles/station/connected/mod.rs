//! Sole integration SPI for one connected station epoch.
//!
//! The submodules are implementation owners, not alternate public facades.
//! Integration code composes the connected role through the types and finite
//! transitions re-exported here.

mod assembly;
mod epoch;
pub(crate) mod port;
mod preparation;
mod rx_service;
mod shutdown;
mod start;
mod transaction;

#[cfg(test)]
pub(in crate::roles::station) use epoch::{
    coalesce_disconnected_station_command, complete_connected_station_command,
};

pub use assembly::{
    ConnectedDriverAssembly, ConnectedDriverAssemblyFailure, ConnectedDriverAssemblyResources,
    assemble_esp32s31_connected_driver,
};

pub use epoch::{
    ConnectedEpochResources, ConnectedServiceParts, ConnectedServiceResources,
    ConnectedStationExit, StationReconnectSource, activate_esp32s31_connected_epoch,
    complete_esp32s31_connected_datapath_exit, run_esp32s31_connected_station_epoch,
};

pub use port::{
    ConnectedStaBlockAckPolicy, ConnectedStaCcmpReplayError, ConnectedStaCcmpReplayFailure,
    ConnectedStaCompositionFailure, ConnectedStaConfig, ConnectedStaConfigError,
    ConnectedStaControlResources, ConnectedStaDriverParts, ConnectedStaDrivers,
    ConnectedStaEspNowRxError, ConnectedStaNetworkTxDomain, ConnectedStaPlan, ConnectedStaPort,
    ConnectedStaPrepareFailure, ConnectedStaRateConfig, ConnectedStaReport, ConnectedStaRxPolicy,
    ConnectedStaRxProcessorResources, ConnectedStaRxProtocolResources,
    ConnectedStaTxHandoffFailure, ConnectedStaTxPolicy, ConnectedStaTxResources,
};

pub use preparation::{
    ConnectedNetworkStartedParts, ConnectedServicePrepareFailure, PreparedConnectedService,
    PreparedConnectedServiceParts, StartedNetworkConnection, prepare_esp32s31_connected_service,
};

pub use rx_service::{ConnectedStaRxParked, ConnectedStaRxService};

pub use shutdown::{
    ConnectedEpochQuiesceFailure, ConnectedEpochQuiesced, ConnectedEpochRunnerOwner,
    ConnectedEpochTeardown, ConnectedEpochTeardownFailure, quiesce_esp32s31_connected_epoch,
};

pub use start::{
    ConnectedEpochStartFailure, ConnectedEpochStartPhase, ConnectedEpochStarted,
    ConnectedRxMaterializer, InitialConnectedEpochResources,
    start_esp32s31_initial_connected_epoch, start_esp32s31_reconnected_connected_epoch,
};

pub use super::{
    command::{StationCommand, StationCommandReceiver},
    control::{ConnectedControlShutdown, ConnectedWpa2Security},
    control_mailbox::{ConnectedControlPublisher, ConnectedControlResources},
    epoch::{DisconnectedStaEpoch, ReconnectedStaEpoch},
    esp_now_mailbox::EspNowMailboxConnectedRxSink,
    esp_now_tx::{
        EspNowConnectedControl, EspNowConnectedControlConfigError, EspNowConnectedControlError,
        EspNowConnectedControlShutdown, EspNowConnectedControlStartFailure, EspNowConnectedTx,
        EspNowTxBinding, attach_esp_now_tx,
    },
    network::EmbassyNetConnectedRxSink,
    rx_protocol::{
        ConnectedProtocolStopped, ConnectedReceiveProtocol, ConnectedReceiveStorage,
        ConnectedRxProtocolSink,
    },
    teardown::{
        AlreadyParkedRx, ConnectedStaGroupSecurity, ConnectedStaSecurityStopReport,
        ConnectedStaTeardownFailure, ConnectedStaTeardownPort,
    },
    tx::ConnectedTx,
    tx_epoch::StaTxEpochExt,
};

pub use transaction::{
    ConnectedEpochCompleted, ConnectedEpochStopped, ConnectedRunObserver,
    ConnectedRunQuiesceFailure, ConnectedServiceTeardownFailure, ConnectedStationRunner,
    NoopConnectedRunObserver, quiesce_completed_esp32s31_connected_epoch,
    run_and_quiesce_esp32s31_connected_epoch,
};

/// Exact connected-driver teardown failure while retaining the complete
/// concrete no-allocation owner frontier.
pub type ConnectionTeardownFailure<'resources, M, H, R, S, X, const CONTROL_CAPACITY: usize, RE> =
    super::teardown::ConnectedStaTeardownFailure<
        H,
        R,
        S,
        X,
        super::control_slot::PlacedConnectedControl<'resources, M, CONTROL_CAPACITY>,
        super::control::ConnectedControlError,
        RE,
    >;

/// Concrete DATAPATH service graph used by production connected composition.
pub type ConnectionServices<'resources, M, H, R, P, X, const CONTROL_CAPACITY: usize> =
    crate::datapath::services::SingleRoleServices<
        H,
        ConnectedStaRxService<R, P>,
        X,
        super::control_slot::PlacedConnectedControl<'resources, M, CONTROL_CAPACITY>,
    >;

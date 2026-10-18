#![expect(
    clippy::too_many_arguments,
    reason = "connected-epoch assembly makes each independently borrowed hardware and service owner explicit"
)]

use crate::{
    datapath::{
        DatapathRunner, DatapathRunnerExit, DatapathServices,
        irq::{InterruptEpoch, MacInterruptEpochActivateError},
    },
    roles::station::connected::port::ConnectedStaConfig,
};

use embassy_sync::blocking_mutex::raw::{RawMutex, RawMutex as NetworkRawMutex};

use oer_esp32s31_hal::owner::{ConnectedStaInterruptPrepared, MacInterruptSetup};

use oer_esp32s31_ieee80211_mac::{init::MAC_COLD_RX_INTERRUPT_MASK, irq::MacInterruptRoute};

use oer_esp32s31_ieee80211_sta::{
    attempt::{StaAttemptSecurity, StaInstalledSecurity},
    connected_control::ConnectedDisconnectReason,
    peer::ConnectedStaPeer,
};

use oer_ieee80211_softmac::interface::BoundVirtualInterface;

use super::{StationCommand, StationCommandReceiver};

/// How a reconnect command reached the DATAPATH runner's safe stop edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationReconnectSource {
    /// The command future won while the runner was still connected.
    Controller,
    /// Peer loss won the same scheduling edge; the pending reconnect command
    /// was consumed after the runner had already returned link-down owners.
    CoalescedDisconnect,
}

/// Production result of one finite connected station epoch.
///
/// This keeps application control semantics out of HIL and prevents a queued
/// terminal command from leaking into a replacement epoch when peer loss wins
/// the same scheduler turn.
pub enum ConnectedStationExit<E> {
    Disconnected(ConnectedDisconnectReason),
    ReconnectRequested { source: StationReconnectSource },
    StationStopped(StationCommand),
    HardwareFailure(E),
}

/// Hardware/RX frontier accepted by one connected station epoch.
///
/// The initial variant consumes the runtime register owner and the halted
/// pre-connected ring. A later epoch can only be constructed from the exact
/// disconnected owners returned by the preceding teardown.
pub enum ConnectedEpochResources<H, R, E> {
    Initial { hardware: H, receive: R },
    Reconnected(E),
}

impl<H, R, E> ConnectedEpochResources<H, R, E> {
    pub const fn is_reconnected(&self) -> bool {
        matches!(self, Self::Reconnected(_))
    }
}

/// Complete owner handoff from a successful join into one connected service.
///
/// `R` is the role-wide runtime resource graph, `E` is the initial or
/// reconnected hardware frontier, and `N` is the one-time/running network
/// owner. Security is moved as one value so PMK, nonce and sequence spaces
/// cannot be split across competing composition roots.
pub struct ConnectedServiceResources<'security, R, E, N> {
    runtime: R,
    epoch: E,
    network: N,
    interface: BoundVirtualInterface,
    config: ConnectedStaConfig,
    peer: ConnectedStaPeer,
    installed_security: StaInstalledSecurity,
    security: StaAttemptSecurity<'security>,
}

impl<'security, R, E, N> ConnectedServiceResources<'security, R, E, N> {
    pub const fn new(
        runtime: R,
        epoch: E,
        network: N,
        interface: BoundVirtualInterface,
        config: ConnectedStaConfig,
        peer: ConnectedStaPeer,
        installed_security: StaInstalledSecurity,
        security: StaAttemptSecurity<'security>,
    ) -> Self {
        Self {
            runtime,
            epoch,
            network,
            interface,
            config,
            peer,
            installed_security,
            security,
        }
    }

    pub fn into_parts(self) -> ConnectedServiceParts<'security, R, E, N> {
        ConnectedServiceParts {
            runtime: self.runtime,
            epoch: self.epoch,
            network: self.network,
            interface: self.interface,
            config: self.config,
            peer: self.peer,
            installed_security: self.installed_security,
            security: self.security,
        }
    }
}

/// Named decomposition visible only after the connected service consumes the
/// complete join handoff.
pub struct ConnectedServiceParts<'security, R, E, N> {
    pub runtime: R,
    pub epoch: E,
    pub network: N,
    pub interface: BoundVirtualInterface,
    pub config: ConnectedStaConfig,
    pub peer: ConnectedStaPeer,
    pub installed_security: StaInstalledSecurity,
    pub security: StaAttemptSecurity<'security>,
}

/// Open the production connected interrupt route and publish one durable RX
/// handoff probe.
///
/// Scan and join intentionally run with the route masked. A descriptor may
/// complete between their last polling observation and route activation; the
/// coalesced probe guarantees the DATAPATH runner checks that frontier even
/// when hardware produces no later edge.
pub fn activate_esp32s31_connected_epoch<'runtime, R, M>(
    interrupt: &mut InterruptEpoch<'runtime, R, M>,
    platform: &R::Platform,
    _prepared: ConnectedStaInterruptPrepared,
) -> Result<(), MacInterruptEpochActivateError<R::Error>>
where
    R: MacInterruptRoute<Setup = MacInterruptSetup>,
    M: RawMutex,
{
    interrupt.activate_or_resume_rx_moderated(platform, MAC_COLD_RX_INTERRUPT_MASK)?;
    interrupt.mac_runtime().notify_rx_handoff();
    Ok(())
}

pub(in crate::roles::station) fn coalesce_disconnected_station_command<E, M: RawMutex>(
    control: &mut StationCommandReceiver<'_, M>,
    reason: ConnectedDisconnectReason,
) -> ConnectedStationExit<E> {
    match control.try_take() {
        Some(StationCommand::Reconnect) => ConnectedStationExit::ReconnectRequested {
            source: StationReconnectSource::CoalescedDisconnect,
        },
        Some(command @ (StationCommand::Disconnect | StationCommand::Stop)) => {
            control.record_terminal(command);
            ConnectedStationExit::StationStopped(command)
        }
        None => ConnectedStationExit::Disconnected(reason),
    }
}

pub(in crate::roles::station) fn complete_connected_station_command<E, M: RawMutex>(
    command: StationCommand,
    control: &mut StationCommandReceiver<'_, M>,
) -> ConnectedStationExit<E> {
    match command {
        StationCommand::Reconnect => ConnectedStationExit::ReconnectRequested {
            source: StationReconnectSource::Controller,
        },
        StationCommand::Disconnect | StationCommand::Stop => {
            control.record_terminal(command);
            ConnectedStationExit::StationStopped(command)
        }
    }
}

/// Classify a finite DATAPATH result after an outer execution boundary has
/// independently consumed an optional station command.
///
/// This is the common semantic join used by both the inline transaction and
/// an executor-affine active-DATAPATH actor. In particular, peer loss and a
/// command observed on the same scheduling edge cannot leak that command into
/// the next connected epoch.
pub fn complete_esp32s31_connected_datapath_exit<E, M: RawMutex>(
    control: &mut StationCommandReceiver<'_, M>,
    result: Result<DatapathRunnerExit<ConnectedDisconnectReason>, E>,
    requested_command: Option<StationCommand>,
) -> ConnectedStationExit<E> {
    match (result, requested_command) {
        (Ok(DatapathRunnerExit::Role(reason)), None) => {
            coalesce_disconnected_station_command(control, reason)
        }
        (Ok(DatapathRunnerExit::Stopped), Some(command)) => {
            complete_connected_station_command(command, control)
        }
        (Ok(DatapathRunnerExit::Role(_)), Some(command)) => match command {
            StationCommand::Reconnect => ConnectedStationExit::ReconnectRequested {
                source: StationReconnectSource::CoalescedDisconnect,
            },
            command @ (StationCommand::Disconnect | StationCommand::Stop) => {
                control.record_terminal(command);
                ConnectedStationExit::StationStopped(command)
            }
        },
        (Ok(DatapathRunnerExit::Stopped), None) => {
            unreachable!("a stopped station runner consumed one controller command")
        }
        (Err(error), _) => ConnectedStationExit::HardwareFailure(error),
    }
}

/// Run one connected hardware owner until peer loss or a station command.
///
/// `DatapathRunner` observes the stop future only at a transaction-safe boundary.
/// A simultaneous peer disconnect is then coalesced with any still-pending
/// application command before ownership is handed back to the outer lifecycle.
pub async fn run_esp32s31_connected_station_epoch<'irq, RM, CM, N, B, RX, C>(
    runner: &mut DatapathRunner<'irq, RM, N, B, RX, C>,
    control: &mut StationCommandReceiver<'_, CM>,
) -> ConnectedStationExit<B::Error>
where
    RM: NetworkRawMutex,
    CM: RawMutex,
    N: crate::datapath::network::DatapathNetwork,
    RX: crate::datapath::network::DatapathNetworkRxSet,
    B: DatapathServices<N::TxFrame, N::PhysicalTxFrame, Exit = ConnectedDisconnectReason>,
    C: oer_time::Timer,
{
    let requested_command = core::cell::Cell::new(None);
    let station_stop = async {
        requested_command.set(Some(control.wait().await));
    };
    let result = runner.run_until(station_stop).await;
    complete_esp32s31_connected_datapath_exit(control, result, requested_command.get())
}

//! One-slot application/owner transport and cancellation reconciliation.

use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TrySendError},
};

use oer_radio::wifi::{
    AccessPointRequest, MonitorRequest, RadioController, StationAccessPointRequest, StationRequest,
    WifiIdle, WifiRadioRestartReport, WifiScanFailure, WifiScanReport, WifiScanRequest,
    WifiStartFailure, WifiStartResult, WifiStopReport, WifiSupervisorPort,
};

use super::message::{WifiSupervisorCommand, WifiSupervisorError, WifiSupervisorResponse};

/// One outstanding controller command and one bounded completion.
const MAILBOX_CAPACITY: usize = 1;

/// Static storage for one application endpoint and one supervisor endpoint.
///
/// It contains only owned requests, value reports and wake state. PAC, DMA,
/// IRQ and protocol owners never enter this object.
pub struct WifiSupervisorControlResources<M: RawMutex, E> {
    split: AtomicBool,
    supervisor_alive: AtomicBool,
    commands: Channel<M, WifiSupervisorCommand, MAILBOX_CAPACITY>,
    responses: Channel<M, WifiSupervisorResponse<E>, MAILBOX_CAPACITY>,
}

pub type WifiSupervisorEndpoints<'resources, M, E> = (
    RadioController<WifiSupervisorMailbox<'resources, M, E>>,
    WifiSupervisorEndpoint<'resources, M, E>,
);

impl<M: RawMutex, E> WifiSupervisorControlResources<M, E> {
    pub const fn new() -> Self {
        Self {
            split: AtomicBool::new(false),
            supervisor_alive: AtomicBool::new(false),
            commands: Channel::new(),
            responses: Channel::new(),
        }
    }

    /// Permanently split this mailbox for one firmware supervisor task.
    ///
    /// Recreating endpoints after either side disappears could consume stale
    /// requests or completions, so a second split is rejected until physical
    /// radio reset also reconstructs this storage.
    pub fn split(&self) -> Result<WifiSupervisorEndpoints<'_, M, E>, WifiSupervisorControlError> {
        if self
            .split
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(WifiSupervisorControlError::InUse);
        }
        self.supervisor_alive.store(true, Ordering::Release);
        let port = WifiSupervisorMailbox {
            commands: self.commands.sender(),
            responses: self.responses.receiver(),
            supervisor_alive: &self.supervisor_alive,
            completion_pending: false,
        };
        let endpoint = WifiSupervisorEndpoint {
            commands: self.commands.receiver(),
            responses: self.responses.sender(),
            supervisor_alive: &self.supervisor_alive,
        };
        Ok((RadioController::new(WifiIdle::new(port)), endpoint))
    }

    #[cfg(test)]
    pub(super) fn no_pending_command_for_test(&self) -> bool {
        self.commands.try_receive().is_err()
    }
}

impl<M: RawMutex, E> Default for WifiSupervisorControlResources<M, E> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiSupervisorControlError {
    InUse,
}

/// Application-side mailbox transport. It owns no hardware capability.
pub struct WifiSupervisorMailbox<'resources, M: RawMutex, E> {
    commands: Sender<'resources, M, WifiSupervisorCommand, MAILBOX_CAPACITY>,
    responses: Receiver<'resources, M, WifiSupervisorResponse<E>, MAILBOX_CAPACITY>,
    supervisor_alive: &'resources AtomicBool,
    completion_pending: bool,
}

impl<M: RawMutex, E> WifiSupervisorMailbox<'_, M, E> {
    async fn response(&mut self) -> WifiSupervisorResponse<E> {
        self.responses.receive().await
    }

    fn supervisor_available(&self) -> bool {
        self.supervisor_alive.load(Ordering::Acquire)
    }

    /// Consume the completion of a command whose caller future was dropped.
    ///
    /// The public role API consumes its capability, so this path principally
    /// protects direct internal users of the transport. The mailbox remains a
    /// strict one-command transaction: a later request is never paired with a
    /// stale completion.
    async fn reconcile_cancelled_command(&mut self) {
        if self.completion_pending {
            let _ = self.response().await;
            self.completion_pending = false;
        }
    }

    async fn publish(
        &mut self,
        command: WifiSupervisorCommand,
    ) -> Result<(), WifiSupervisorCommand> {
        self.reconcile_cancelled_command().await;
        // Reconciliation may have waited while the sole endpoint disappeared.
        // In that case its terminal response belongs to the cancelled command,
        // not to this still-owned, unpublished request.
        if !self.supervisor_available() {
            return Err(command);
        }
        self.commands.send(command).await;
        self.completion_pending = true;
        Ok(())
    }

    async fn completion(&mut self) -> WifiSupervisorResponse<E> {
        let response = self.response().await;
        self.completion_pending = false;
        response
    }
}

impl<M: RawMutex, E> WifiSupervisorPort for WifiSupervisorMailbox<'_, M, E> {
    type Error = WifiSupervisorError<E>;

    async fn scan(
        &mut self,
        request: WifiScanRequest,
    ) -> Result<WifiScanReport, WifiScanFailure<WifiScanRequest, Self::Error>> {
        if !self.supervisor_available() {
            return Err(WifiScanFailure::Rejected {
                request,
                error: WifiSupervisorError::SupervisorUnavailable,
            });
        }
        if let Err(WifiSupervisorCommand::Scan(request)) =
            self.publish(WifiSupervisorCommand::Scan(request)).await
        {
            return Err(WifiScanFailure::Rejected {
                request,
                error: WifiSupervisorError::SupervisorUnavailable,
            });
        }
        match self.completion().await {
            WifiSupervisorResponse::Scan(result) => result.map_err(map_scan_failure),
            WifiSupervisorResponse::SupervisorUnavailable => Err(WifiScanFailure::Faulted {
                error: WifiSupervisorError::SupervisorUnavailable,
            }),
            _ => Err(WifiScanFailure::Faulted {
                error: WifiSupervisorError::ResponseMismatch,
            }),
        }
    }

    async fn start_station(
        &mut self,
        request: StationRequest,
    ) -> WifiStartResult<StationRequest, Self::Error> {
        if !self.supervisor_available() {
            return Err(WifiStartFailure::rejected(
                request,
                WifiSupervisorError::SupervisorUnavailable,
            ));
        }
        if let Err(WifiSupervisorCommand::StartStation(request)) = self
            .publish(WifiSupervisorCommand::StartStation(request))
            .await
        {
            return Err(WifiStartFailure::rejected(
                request,
                WifiSupervisorError::SupervisorUnavailable,
            ));
        }
        match self.completion().await {
            WifiSupervisorResponse::Station(result) => result.map_err(map_start_failure),
            WifiSupervisorResponse::SupervisorUnavailable => Err(WifiStartFailure::faulted(
                WifiSupervisorError::SupervisorUnavailable,
            )),
            _ => Err(WifiStartFailure::faulted(
                WifiSupervisorError::ResponseMismatch,
            )),
        }
    }

    async fn start_access_point(
        &mut self,
        request: AccessPointRequest,
    ) -> WifiStartResult<AccessPointRequest, Self::Error> {
        if !self.supervisor_available() {
            return Err(WifiStartFailure::rejected(
                request,
                WifiSupervisorError::SupervisorUnavailable,
            ));
        }
        if let Err(WifiSupervisorCommand::StartAccessPoint(request)) = self
            .publish(WifiSupervisorCommand::StartAccessPoint(request))
            .await
        {
            return Err(WifiStartFailure::rejected(
                request,
                WifiSupervisorError::SupervisorUnavailable,
            ));
        }
        match self.completion().await {
            WifiSupervisorResponse::AccessPoint(result) => result.map_err(map_start_failure),
            WifiSupervisorResponse::SupervisorUnavailable => Err(WifiStartFailure::faulted(
                WifiSupervisorError::SupervisorUnavailable,
            )),
            _ => Err(WifiStartFailure::faulted(
                WifiSupervisorError::ResponseMismatch,
            )),
        }
    }

    async fn start_station_access_point(
        &mut self,
        request: StationAccessPointRequest,
    ) -> WifiStartResult<StationAccessPointRequest, Self::Error> {
        if !self.supervisor_available() {
            return Err(WifiStartFailure::rejected(
                request,
                WifiSupervisorError::SupervisorUnavailable,
            ));
        }
        if let Err(WifiSupervisorCommand::StartStationAccessPoint(request)) = self
            .publish(WifiSupervisorCommand::StartStationAccessPoint(request))
            .await
        {
            return Err(WifiStartFailure::rejected(
                request,
                WifiSupervisorError::SupervisorUnavailable,
            ));
        }
        match self.completion().await {
            WifiSupervisorResponse::StationAccessPoint(result) => result.map_err(map_start_failure),
            WifiSupervisorResponse::SupervisorUnavailable => Err(WifiStartFailure::faulted(
                WifiSupervisorError::SupervisorUnavailable,
            )),
            _ => Err(WifiStartFailure::faulted(
                WifiSupervisorError::ResponseMismatch,
            )),
        }
    }

    async fn start_monitor(
        &mut self,
        request: MonitorRequest,
    ) -> WifiStartResult<MonitorRequest, Self::Error> {
        if !self.supervisor_available() {
            return Err(WifiStartFailure::rejected(
                request,
                WifiSupervisorError::SupervisorUnavailable,
            ));
        }
        if let Err(WifiSupervisorCommand::StartMonitor(request)) = self
            .publish(WifiSupervisorCommand::StartMonitor(request))
            .await
        {
            return Err(WifiStartFailure::rejected(
                request,
                WifiSupervisorError::SupervisorUnavailable,
            ));
        }
        match self.completion().await {
            WifiSupervisorResponse::Monitor(result) => result.map_err(map_start_failure),
            WifiSupervisorResponse::SupervisorUnavailable => Err(WifiStartFailure::faulted(
                WifiSupervisorError::SupervisorUnavailable,
            )),
            _ => Err(WifiStartFailure::faulted(
                WifiSupervisorError::ResponseMismatch,
            )),
        }
    }

    async fn stop(&mut self) -> Result<WifiStopReport, Self::Error> {
        if !self.supervisor_available() {
            return Err(WifiSupervisorError::SupervisorUnavailable);
        }
        if self.publish(WifiSupervisorCommand::Stop).await.is_err() {
            return Err(WifiSupervisorError::SupervisorUnavailable);
        }
        match self.completion().await {
            WifiSupervisorResponse::Stop(result) => result.map_err(WifiSupervisorError::Service),
            WifiSupervisorResponse::SupervisorUnavailable => {
                Err(WifiSupervisorError::SupervisorUnavailable)
            }
            _ => Err(WifiSupervisorError::ResponseMismatch),
        }
    }

    async fn restart_radio(&mut self) -> Result<WifiRadioRestartReport, Self::Error> {
        if !self.supervisor_available() {
            return Err(WifiSupervisorError::SupervisorUnavailable);
        }
        if self
            .publish(WifiSupervisorCommand::RestartRadio)
            .await
            .is_err()
        {
            return Err(WifiSupervisorError::SupervisorUnavailable);
        }
        match self.completion().await {
            WifiSupervisorResponse::RestartRadio(result) => {
                result.map_err(WifiSupervisorError::Service)
            }
            WifiSupervisorResponse::SupervisorUnavailable => {
                Err(WifiSupervisorError::SupervisorUnavailable)
            }
            _ => Err(WifiSupervisorError::ResponseMismatch),
        }
    }
}

fn map_start_failure<R, E>(
    failure: WifiStartFailure<R, E>,
) -> WifiStartFailure<R, WifiSupervisorError<E>> {
    match failure {
        WifiStartFailure::Rejected { request, error } => {
            WifiStartFailure::rejected(request, WifiSupervisorError::Service(error))
        }
        WifiStartFailure::Faulted { error } => {
            WifiStartFailure::faulted(WifiSupervisorError::Service(error))
        }
    }
}

fn map_scan_failure<R, E>(
    failure: WifiScanFailure<R, E>,
) -> WifiScanFailure<R, WifiSupervisorError<E>> {
    match failure {
        WifiScanFailure::Rejected { request, error } => WifiScanFailure::Rejected {
            request,
            error: WifiSupervisorError::Service(error),
        },
        WifiScanFailure::Returned { request, error } => WifiScanFailure::Returned {
            request,
            error: WifiSupervisorError::Service(error),
        },
        WifiScanFailure::Faulted { error } => WifiScanFailure::Faulted {
            error: WifiSupervisorError::Service(error),
        },
    }
}

/// Sole command endpoint held by the task which owns the radio state machine.
pub struct WifiSupervisorEndpoint<'resources, M: RawMutex, E> {
    commands: Receiver<'resources, M, WifiSupervisorCommand, MAILBOX_CAPACITY>,
    responses: Sender<'resources, M, WifiSupervisorResponse<E>, MAILBOX_CAPACITY>,
    supervisor_alive: &'resources AtomicBool,
}

impl<M: RawMutex, E> WifiSupervisorEndpoint<'_, M, E> {
    pub async fn receive(&mut self) -> WifiSupervisorCommand {
        self.commands.receive().await
    }

    pub async fn respond(&mut self, response: WifiSupervisorResponse<E>) {
        self.responses.send(response).await;
    }
}

impl<M: RawMutex, E> Drop for WifiSupervisorEndpoint<'_, M, E> {
    fn drop(&mut self) {
        self.supervisor_alive.store(false, Ordering::Release);
        if let Err(TrySendError::Full(_)) = self
            .responses
            .try_send(WifiSupervisorResponse::SupervisorUnavailable)
        {
            // One prior completion already wakes the only controller waiter.
            // Its next operation observes `supervisor_alive == false` before
            // publishing another command.
        }
    }
}

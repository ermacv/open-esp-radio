//! Connected-station scheduling for plaintext ESP-NOW v1/v2 transmit.
//!
//! The shared application mailbox lives in `roles::esp_now::mailbox`. This
//! scheduler owns peer resolution and the sole ordinary connected TX
//! transaction. It never borrows the WPA2 key slots and never manufactures a
//! PHY rate: the complete typed request is passed to the chip ESP-NOW backend.

use crate::roles::station::control_slot::PlacedConnectedControl;
use core::future::Future;

use crate::{
    datapath::{
        DatapathControlContext, DatapathControlProgress, WifiTxProgress,
        services::{DatapathControlService, SingleRoleServices},
    },
    roles::{
        esp_now::mailbox::tx::{
            EspNowOwnedV1Tx, EspNowQueuedRequest, EspNowQueuedTx, EspNowTxCancelReason,
            EspNowTxMailboxInvariantError, EspNowTxMailboxOwner, EspNowTxMailboxShutdown,
            EspNowTxRuntimeFailure, EspNowTxTerminal, EspNowV2TxRequest,
        },
        station::control::{
            ConnectedControl, ConnectedControlError, ConnectedControlHardware,
            ConnectedControlShutdown, ConnectedControlTx,
        },
    },
};

use embassy_futures::select::{Either, select};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211::esp_now::EspNowTxConfig;

use oer_esp32s31_ieee80211_mac::tx::TxHardware;

use oer_esp32s31_ieee80211_sta::{
    connected_control::ConnectedDisconnectReason, single_mpdu_tx::SingleMpduEspNowTxError,
};

use oer_ieee80211_mac::channel::WifiChannel;

use oer_ieee80211_softmac::{EspNowProtocol, interface::BoundVirtualInterface};

/// Narrow extension implemented only by the connected TX owner which already
/// arbitrates ordinary network, management and EAPOL transactions.
pub trait EspNowConnectedTx: ConnectedControlTx {
    fn start_esp_now_v1_plaintext<H: TxHardware, const PEERS: usize>(
        &mut self,
        hardware: &mut H,
        protocol: &EspNowProtocol<PEERS>,
        request: &EspNowOwnedV1Tx,
        active_channel: WifiChannel,
        active_station: BoundVirtualInterface,
        config: EspNowTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduEspNowTxError>;

    fn start_esp_now_v2_plaintext<H: TxHardware, const PEERS: usize>(
        &mut self,
        hardware: &mut H,
        protocol: &EspNowProtocol<PEERS>,
        request: EspNowV2TxRequest<'_>,
        active_channel: WifiChannel,
        active_station: BoundVirtualInterface,
        config: EspNowTxConfig,
    ) -> Result<WifiTxProgress, SingleMpduEspNowTxError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowConnectedControlConfigError {
    StationBinding {
        configured: BoundVirtualInterface,
        active: BoundVirtualInterface,
    },
    ChannelBinding {
        configured: WifiChannel,
        active: WifiChannel,
    },
}

/// Validated station/channel binding captured before an infallible services
/// decoration closure moves the protocol and mailbox owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowTxBinding {
    active_station: BoundVirtualInterface,
    active_channel: WifiChannel,
    config: EspNowTxConfig,
}

impl EspNowTxBinding {
    pub fn new<const PEERS: usize>(
        protocol: &EspNowProtocol<PEERS>,
        active_station: BoundVirtualInterface,
        active_channel: WifiChannel,
        config: EspNowTxConfig,
    ) -> Result<Self, EspNowConnectedControlConfigError> {
        let configured = protocol.config();
        if configured.station() != active_station {
            return Err(EspNowConnectedControlConfigError::StationBinding {
                configured: configured.station(),
                active: active_station,
            });
        }
        if configured.home_channel() != active_channel {
            return Err(EspNowConnectedControlConfigError::ChannelBinding {
                configured: configured.home_channel(),
                active: active_channel,
            });
        }
        Ok(Self {
            active_station,
            active_channel,
            config,
        })
    }
}

/// Failed opt-in retaining every owner supplied by the application.
pub struct EspNowConnectedControlStartFailure<
    'resources,
    M: RawMutex,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
> {
    pub error: EspNowConnectedControlConfigError,
    pub control: PlacedConnectedControl<'resources, M, CONTROL_CAPACITY>,
    pub mailbox: EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
    pub protocol: EspNowProtocol<PEERS>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowConnectedControlError {
    Connected(ConnectedControlError),
    Mailbox(EspNowTxMailboxInvariantError),
    MissingOrdinaryTxOutcome,
    AlreadyShutdown,
}

/// Explicit opt-in decorator for the stock connected control scheduler.
///
/// Existing security, BlockAck, beacon-loss and power transitions retain
/// priority. A queued ESP-NOW request is admitted only when ordinary network
/// TX is not already pending; once admitted, it uses the same descriptor and
/// IRQ/deadline completion loop as every other ordinary station MPDU.
pub struct EspNowConnectedControl<
    'resources,
    M: RawMutex,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
> {
    inner: PlacedConnectedControl<'resources, M, CONTROL_CAPACITY>,
    mailbox: Option<EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>>,
    protocol: Option<EspNowProtocol<PEERS>>,
    active_station: BoundVirtualInterface,
    active_channel: WifiChannel,
    config: EspNowTxConfig,
    active: Option<EspNowQueuedTx>,
    pending_inner_terminal: Option<Result<ConnectedDisconnectReason, ConnectedControlError>>,
}

impl<
    'resources,
    M: RawMutex,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
> EspNowConnectedControl<'resources, M, CONTROL_CAPACITY, TX_CAPACITY, PEERS>
{
    pub fn new(
        control: PlacedConnectedControl<'resources, M, CONTROL_CAPACITY>,
        mailbox: EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
        protocol: EspNowProtocol<PEERS>,
        active_station: BoundVirtualInterface,
        active_channel: WifiChannel,
        config: EspNowTxConfig,
    ) -> Result<
        Self,
        EspNowConnectedControlStartFailure<'resources, M, CONTROL_CAPACITY, TX_CAPACITY, PEERS>,
    > {
        let binding = match EspNowTxBinding::new(&protocol, active_station, active_channel, config)
        {
            Ok(binding) => binding,
            Err(error) => {
                return Err(EspNowConnectedControlStartFailure {
                    error,
                    control,
                    mailbox,
                    protocol,
                });
            }
        };
        Ok(Self::from_binding(control, mailbox, protocol, binding))
    }

    /// Infallible half of the opt-in, suitable for the connected assembly's
    /// `map_services` closure after [`EspNowTxBinding::new`] succeeds.
    pub fn from_binding(
        control: PlacedConnectedControl<'resources, M, CONTROL_CAPACITY>,
        mailbox: EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
        protocol: EspNowProtocol<PEERS>,
        binding: EspNowTxBinding,
    ) -> Self {
        debug_assert_eq!(protocol.config().station(), binding.active_station);
        debug_assert_eq!(protocol.config().home_channel(), binding.active_channel);
        Self {
            inner: control,
            mailbox: Some(mailbox),
            protocol: Some(protocol),
            active_station: binding.active_station,
            active_channel: binding.active_channel,
            config: binding.config,
            active: None,
            pending_inner_terminal: None,
        }
    }

    pub fn inner(&self) -> &ConnectedControl<'resources, M, CONTROL_CAPACITY> {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut ConnectedControl<'resources, M, CONTROL_CAPACITY> {
        &mut self.inner
    }

    /// End the decorator after a complete shutdown: the placed control
    /// empties its slot and hands it back.
    pub fn release(
        self,
    ) -> crate::roles::station::control_slot::ReleasedConnectedControl<
        'resources,
        M,
        CONTROL_CAPACITY,
    > {
        self.inner.release()
    }

    pub const fn tx_epoch(&self) -> Option<u32> {
        match &self.mailbox {
            Some(mailbox) => Some(mailbox.epoch()),
            None => None,
        }
    }

    fn mailbox(
        &self,
    ) -> Result<&EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>, EspNowConnectedControlError>
    {
        self.mailbox
            .as_ref()
            .ok_or(EspNowConnectedControlError::AlreadyShutdown)
    }

    fn mailbox_mut(
        &mut self,
    ) -> Result<&mut EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>, EspNowConnectedControlError>
    {
        self.mailbox
            .as_mut()
            .ok_or(EspNowConnectedControlError::AlreadyShutdown)
    }

    fn finish_active<X: ConnectedControlTx>(
        &mut self,
        tx: &mut X,
    ) -> Result<bool, EspNowConnectedControlError> {
        let Some(queued) = self.active.take() else {
            return Ok(false);
        };
        let Some(outcome) = tx.take_last_outcome() else {
            self.mailbox()?
                .publish(
                    queued,
                    EspNowTxTerminal::RuntimeFailure(
                        EspNowTxRuntimeFailure::MissingOrdinaryTxOutcome,
                    ),
                )
                .map_err(EspNowConnectedControlError::Mailbox)?;
            return Err(EspNowConnectedControlError::MissingOrdinaryTxOutcome);
        };
        self.mailbox()?
            .publish(queued, EspNowTxTerminal::Completed(outcome))
            .map_err(EspNowConnectedControlError::Mailbox)?;
        Ok(true)
    }

    fn close_and_cancel(
        &mut self,
        reason: EspNowTxCancelReason,
    ) -> Result<u32, EspNowConnectedControlError> {
        let mailbox = self.mailbox_mut()?;
        mailbox.close();
        mailbox
            .cancel_pending(reason)
            .map_err(EspNowConnectedControlError::Mailbox)
    }

    fn defer_inner_terminal(
        &mut self,
        terminal: Result<ConnectedDisconnectReason, ConnectedControlError>,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, EspNowConnectedControlError>
    {
        self.close_and_cancel(EspNowTxCancelReason::ConnectionEnded)?;
        self.pending_inner_terminal = Some(terminal);
        Ok(DatapathControlProgress::More)
    }

    pub async fn service_with_context<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        context: DatapathControlContext,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, EspNowConnectedControlError>
    where
        H: ConnectedControlHardware + TxHardware,
        X: ConnectedControlTx + oer_time::Timer + EspNowConnectedTx,
    {
        if self.finish_active(tx)? {
            if context.stop_pending {
                self.close_and_cancel(EspNowTxCancelReason::StationStopped)?;
            }
            return Ok(DatapathControlProgress::More);
        }

        if self.pending_inner_terminal.is_some() {
            self.close_and_cancel(EspNowTxCancelReason::ConnectionEnded)?;
            if self.mailbox()?.publishers_in_flight() != 0 {
                return Ok(DatapathControlProgress::More);
            }
            // Closing prevents a new publication lease. Once the earlier
            // leases reach zero, this second drain is a terminal fence: no
            // admitted request can appear behind it.
            self.close_and_cancel(EspNowTxCancelReason::ConnectionEnded)?;
            return match self
                .pending_inner_terminal
                .take()
                .expect("checked deferred connected-control terminal")
            {
                Ok(exit) => Ok(DatapathControlProgress::Exit(exit)),
                Err(error) => Err(EspNowConnectedControlError::Connected(error)),
            };
        }

        if context.stop_pending {
            let was_open = self.mailbox()?.is_open();
            let cancelled = self.close_and_cancel(EspNowTxCancelReason::StationStopped)?;
            if <
                ConnectedControl<'resources, M, CONTROL_CAPACITY> as DatapathControlService<
                    H,
                    X,
                >
            >::required_before_stop(&self.inner)
            {
                return match self.inner.service_with_context(hardware, tx, context).await {
                    Ok(DatapathControlProgress::Exit(exit)) => self.defer_inner_terminal(Ok(exit)),
                    Ok(progress) => Ok(progress),
                    Err(error) => self.defer_inner_terminal(Err(error)),
                };
            }
            if self.mailbox()?.publishers_in_flight() != 0 {
                return Ok(DatapathControlProgress::More);
            }
            self.close_and_cancel(EspNowTxCancelReason::StationStopped)?;
            return Ok(if was_open || cancelled != 0 {
                DatapathControlProgress::More
            } else {
                DatapathControlProgress::Idle
            });
        }

        let esp_now_pending = self.mailbox()?.has_pending();
        let inner_ready = self.inner.has_immediate_work()
            || self
                .inner
                .next_alarm_deadline()
                .is_some_and(|deadline| deadline <= tx.now());
        let inner_requires_awake =
            <ConnectedControl<'resources, M, CONTROL_CAPACITY> as DatapathControlService<
                H,
                X,
            >>::required_before_network_tx(&self.inner);
        if inner_ready || (esp_now_pending && inner_requires_awake) {
            let inner_context = DatapathControlContext {
                network_tx_pending: context.network_tx_pending || esp_now_pending,
                stop_pending: false,
            };
            return match self
                .inner
                .service_with_context(hardware, tx, inner_context)
                .await
            {
                Ok(DatapathControlProgress::Exit(exit)) => self.defer_inner_terminal(Ok(exit)),
                Ok(progress) => Ok(progress),
                Err(error) => self.defer_inner_terminal(Err(error)),
            };
        }

        // Ordinary network TX owns the next arbitration turn. Returning Idle
        // lets DATAPATH claim it in this same scheduler iteration even though
        // the bounded ESP-NOW mailbox remains ready.
        if context.network_tx_pending || !esp_now_pending || !self.inner.admits_frames() {
            return Ok(DatapathControlProgress::Idle);
        }

        let Some(queued) = self.mailbox()?.try_take() else {
            return Ok(DatapathControlProgress::Idle);
        };
        if queued.ticket.epoch() != self.mailbox()?.epoch() {
            self.mailbox()?
                .publish(
                    queued,
                    EspNowTxTerminal::Cancelled(EspNowTxCancelReason::StaleEpoch),
                )
                .map_err(EspNowConnectedControlError::Mailbox)?;
            return Ok(DatapathControlProgress::More);
        }
        let protocol = self
            .protocol
            .as_ref()
            .ok_or(EspNowConnectedControlError::AlreadyShutdown)?;
        let result = match queued.request {
            EspNowQueuedRequest::V1(request) => tx.start_esp_now_v1_plaintext(
                hardware,
                protocol,
                &request,
                self.active_channel,
                self.active_station,
                self.config,
            ),
            EspNowQueuedRequest::V2(_) => {
                let request = self.mailbox()?.with_v2_request(&queued, |request| {
                    tx.start_esp_now_v2_plaintext(
                        hardware,
                        protocol,
                        request,
                        self.active_channel,
                        self.active_station,
                        self.config,
                    )
                });
                match request {
                    Ok(result) => result,
                    Err(error) => {
                        self.mailbox()?
                            .publish(
                                queued,
                                EspNowTxTerminal::RuntimeFailure(
                                    EspNowTxRuntimeFailure::MissingV2PayloadSlot,
                                ),
                            )
                            .map_err(EspNowConnectedControlError::Mailbox)?;
                        return Err(EspNowConnectedControlError::Mailbox(error));
                    }
                }
            }
        };
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active = Some(queued);
                Ok(DatapathControlProgress::TxPending)
            }
            Ok(WifiTxProgress::Complete) => {
                self.active = Some(queued);
                self.finish_active(tx)?;
                Ok(DatapathControlProgress::More)
            }
            Err(error) => {
                self.mailbox()?
                    .publish(queued, EspNowTxTerminal::Rejected(error))
                    .map_err(EspNowConnectedControlError::Mailbox)?;
                Ok(DatapathControlProgress::More)
            }
        }
    }

    pub fn shutdown<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<EspNowConnectedControlShutdown<PEERS>, EspNowConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        self.finish_active(tx)?;
        self.close_and_cancel(EspNowTxCancelReason::OwnerShutdown)?;
        if self.mailbox()?.publishers_in_flight() != 0 {
            return Err(EspNowConnectedControlError::Mailbox(
                EspNowTxMailboxInvariantError::PublisherInFlight,
            ));
        }
        self.close_and_cancel(EspNowTxCancelReason::OwnerShutdown)?;
        let connected = self
            .inner
            .shutdown(hardware, tx)
            .map_err(EspNowConnectedControlError::Connected)?;
        let mailbox = self
            .mailbox
            .take()
            .ok_or(EspNowConnectedControlError::AlreadyShutdown)?
            .shutdown(EspNowTxCancelReason::OwnerShutdown)
            .map_err(EspNowConnectedControlError::Mailbox)?;
        let protocol = self
            .protocol
            .take()
            .ok_or(EspNowConnectedControlError::AlreadyShutdown)?;
        Ok(EspNowConnectedControlShutdown {
            connected,
            mailbox,
            protocol,
        })
    }
}

/// Replace the stock connected control member with the ESP-NOW scheduler
/// decorator while preserving the exact hardware, RX and ordinary/A-MPDU TX
/// owners. This is the compile-checked production composition frontier used
/// by custom integration roots and `assemble_esp32s31_connected_driver`'s
/// `map_services` closure.
pub fn attach_esp_now_tx<
    'resources,
    M: RawMutex,
    H,
    R,
    X,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
>(
    services: SingleRoleServices<H, R, X, PlacedConnectedControl<'resources, M, CONTROL_CAPACITY>>,
    mailbox: EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
    protocol: EspNowProtocol<PEERS>,
    binding: EspNowTxBinding,
) -> SingleRoleServices<
    H,
    R,
    X,
    EspNowConnectedControl<'resources, M, CONTROL_CAPACITY, TX_CAPACITY, PEERS>,
> {
    let (hardware, rx, tx, control) = services.into_parts();
    SingleRoleServices::with_control(
        hardware,
        rx,
        tx,
        EspNowConnectedControl::from_binding(control, mailbox, protocol, binding),
    )
}

pub struct EspNowConnectedControlShutdown<const PEERS: usize> {
    pub connected: ConnectedControlShutdown,
    pub mailbox: EspNowTxMailboxShutdown,
    pub protocol: EspNowProtocol<PEERS>,
}

impl<
    'resources,
    M,
    H,
    X,
    const CONTROL_CAPACITY: usize,
    const TX_CAPACITY: usize,
    const PEERS: usize,
> DatapathControlService<H, X>
    for EspNowConnectedControl<'resources, M, CONTROL_CAPACITY, TX_CAPACITY, PEERS>
where
    M: RawMutex,
    H: ConnectedControlHardware + TxHardware,
    X: ConnectedControlTx + oer_time::Timer + EspNowConnectedTx,
{
    type Error = EspNowConnectedControlError;
    type Exit = ConnectedDisconnectReason;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        EspNowConnectedControl::service_with_context(self, hardware, tx, context)
    }

    fn ready(&self, tx: &X, now: oer_time::Instant) -> bool {
        self.active.is_some()
            || self.pending_inner_terminal.is_some()
            || (self.inner.admits_frames()
                && self.mailbox.as_ref().is_some_and(EspNowTxMailboxOwner::has_pending))
            || <ConnectedControl<'resources, M, CONTROL_CAPACITY> as DatapathControlService<
                H,
                X,
            >>::ready(&self.inner, tx, now)
    }

    fn required_before_network_tx(&self) -> bool {
        <ConnectedControl<'resources, M, CONTROL_CAPACITY> as DatapathControlService<
            H,
            X,
        >>::required_before_network_tx(&self.inner)
    }

    fn admits_network_tx(&self) -> bool {
        self.inner.admits_frames()
    }

    fn required_before_stop(&self) -> bool {
        self.active.is_some()
            || self.pending_inner_terminal.is_some()
            || self.mailbox.as_ref().is_some_and(|mailbox| {
                mailbox.is_open()
                    || mailbox.has_pending()
                    || mailbox.publishers_in_flight() != 0
            })
            || <ConnectedControl<'resources, M, CONTROL_CAPACITY> as DatapathControlService<
                H,
                X,
            >>::required_before_stop(&self.inner)
    }

    #[allow(clippy::manual_async_fn)]
    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a {
        async move {
            let admits = self.inner.admits_frames();
            if self.active.is_some()
                || self.pending_inner_terminal.is_some()
                || (admits
                    && self
                        .mailbox
                        .as_ref()
                        .is_some_and(EspNowTxMailboxOwner::has_pending))
            {
                return;
            }
            // While power management holds frames, a queued ESP-NOW frame
            // waits for the power input that releases them.
            let Some(mailbox) = self.mailbox.as_ref().filter(|_| admits) else {
                ConnectedControl::wait_ready(&mut self.inner, tx).await;
                return;
            };
            match select(
                ConnectedControl::wait_ready(&mut self.inner, tx),
                mailbox.ready(),
            )
            .await
            {
                Either::First(()) | Either::Second(()) => {}
            }
        }
    }
}

#![expect(
    clippy::type_complexity,
    reason = "the stopped value returns every affine IRQ/RX/TX/protocol owner"
)]

//! Bounded standalone ESP-NOW runtime and stop/restart owner transition.

use core::{
    future::{Future, ready},
    pin::{Pin, pin},
};

use crate::{
    datapath::{
        irq::{
            InterruptEpoch, MacInterruptEpochActivateError, MacInterruptEpochDrain,
            MacInterruptEpochQuiesceError,
        },
        rx::frontier::RxFrontierPhase,
    },
    roles::{
        esp_now::mailbox::tx::{
            EspNowOffChannelFailureStage, EspNowQueuedRequest, EspNowQueuedTx,
            EspNowTxCancelReason, EspNowTxMailboxInvariantError, EspNowTxMailboxOwner,
            EspNowTxMailboxShutdown, EspNowTxRuntimeFailure, EspNowTxTerminal,
        },
        esp_now::{
            channel::StandaloneEspNowChannelControl,
            rx::{StandaloneEspNowReceive, StandaloneEspNowRxProgress},
        },
    },
};

use embassy_futures::{
    select::{Either, Either3, Either4, select, select3, select4},
    yield_now,
};

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211::{
    esp_now::{EspNowTxConfig, EspNowTxError},
    tx::{WifiTxProgress, WifiTxWake},
};

use oer_esp32s31_ieee80211_mac::{
    init::{
        MAC_COLD_RX_INTERRUPT_MASK, StaEspNowRxPolicyHardware,
        configure_standalone_esp_now_receive_policy,
    },
    irq::MacInterruptRoute,
    tx::TxHardware,
};

use oer_esp32s31_ieee80211_sta::{
    control_tx::ControlTransmitter,
    single_mpdu_tx::{
        SingleMpduEspNowTxError, SingleMpduTxError, WifiTxEntropy, WifiTxPowerProfile,
    },
};

use oer_ieee80211_mac::{channel::WifiChannel, station::StaSequenceCounter};

use oer_ieee80211_softmac::{
    EspNowPeerChannelPolicy, EspNowPhyMode, EspNowProtocol, WifiStandaloneEspNowPlan,
    interface::BoundVirtualInterface,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandaloneEspNowBindingError {
    StationBinding {
        topology: BoundVirtualInterface,
        protocol: BoundVirtualInterface,
    },
    ChannelBinding {
        protocol: WifiChannel,
        active: WifiChannel,
    },
}

/// Fully checked identity retained while the integration excludes retuning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StandaloneEspNowBinding {
    station: BoundVirtualInterface,
    channel: WifiChannel,
    tx: EspNowTxConfig,
}

impl StandaloneEspNowBinding {
    pub fn new<const PEERS: usize>(
        plan: WifiStandaloneEspNowPlan,
        protocol: &EspNowProtocol<PEERS>,
        active_channel: WifiChannel,
        tx: EspNowTxConfig,
    ) -> Result<Self, StandaloneEspNowBindingError> {
        let station = plan.station();
        let configured = protocol.config();
        if configured.station() != station {
            return Err(StandaloneEspNowBindingError::StationBinding {
                topology: station,
                protocol: configured.station(),
            });
        }
        if configured.home_channel() != active_channel {
            return Err(StandaloneEspNowBindingError::ChannelBinding {
                protocol: configured.home_channel(),
                active: active_channel,
            });
        }
        Ok(Self {
            station,
            channel: active_channel,
            tx,
        })
    }

    pub const fn station(self) -> BoundVirtualInterface {
        self.station
    }

    pub const fn channel(self) -> WifiChannel {
        self.channel
    }

    pub const fn tx_config(self) -> EspNowTxConfig {
        self.tx
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StandaloneEspNowRunReport {
    pub receive_wakes: u32,
    pub receive: StandaloneEspNowRxProgress,
    pub tx_started: u32,
    pub tx_completed: u32,
    pub tx_rejected: u32,
    pub off_channel_started: u32,
    pub home_channel_restored: u32,
    pub quarantined: bool,
    pub duplicate_history_cleared: usize,
    pub mailbox: Option<EspNowTxMailboxShutdown>,
    pub interrupt_drain: MacInterruptEpochDrain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandaloneEspNowStopError<InterruptError, ReceiveError> {
    NotRunning,
    Quarantined,
    Tx(SingleMpduTxError),
    MissingOrdinaryTxOutcome,
    Mailbox(EspNowTxMailboxInvariantError),
    Interrupt(MacInterruptEpochQuiesceError<InterruptError>),
    Receive(ReceiveError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandaloneEspNowRunError<InterruptError, ReceiveError> {
    AlreadyRunning,
    AlreadyStopped,
    Faulted,
    ReceiveStationBinding {
        expected: BoundVirtualInterface,
        actual: BoundVirtualInterface,
    },
    ReceiveChannelBinding {
        expected: WifiChannel,
        actual: WifiChannel,
    },
    ReceivePeerSnapshotMismatch,
    ReceiveNotPrepared(RxFrontierPhase),
    TransmitNotIdle,
    ReceivePrepare(ReceiveError),
    ReceiveStart(ReceiveError),
    Activate(MacInterruptEpochActivateError<InterruptError>),
    ActivateReceiveStop {
        activation: MacInterruptEpochActivateError<InterruptError>,
        receive: ReceiveError,
    },
    ReceiveService(ReceiveError),
    Tx(SingleMpduTxError),
    MissingOrdinaryTxOutcome,
    Mailbox(EspNowTxMailboxInvariantError),
    Stop(StandaloneEspNowStopError<InterruptError, ReceiveError>),
}

pub struct StandaloneEspNowRunFailure<InterruptError, ReceiveError> {
    pub error: StandaloneEspNowRunError<InterruptError, ReceiveError>,
    pub report: StandaloneEspNowRunReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandaloneEspNowOffChannelRunError<InterruptError, ReceiveError, ChannelError> {
    Runtime(StandaloneEspNowRunError<InterruptError, ReceiveError>),
    ControllerChannelMismatch {
        expected: WifiChannel,
        actual: WifiChannel,
    },
    ChannelSwitch {
        stage: EspNowOffChannelFailureStage,
        from: WifiChannel,
        target: WifiChannel,
        error: ChannelError,
    },
    Quarantined,
}

pub struct StandaloneEspNowOffChannelRunFailure<InterruptError, ReceiveError, ChannelError> {
    pub error: StandaloneEspNowOffChannelRunError<InterruptError, ReceiveError, ChannelError>,
    pub report: StandaloneEspNowRunReport,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServicePhase {
    Prepared,
    Running,
    Stopped,
    Faulted,
    Quarantined,
}

enum IdleWake {
    Stop,
    Receive,
    Transmit,
}

enum OffChannelRequestOutcome {
    Continue,
    Stop,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TxCompletionPublication {
    Immediate,
    AfterHomeRestore,
}

/// Complete standalone radio owner.
///
/// `RX` is a normal station RX frontier. The sole application TX endpoint is
/// the same bounded mailbox used by connected ESP-NOW; the sole ordinary TX
/// descriptor is the pre-connected owner and therefore carries no WPA2 key.
pub struct StandaloneEspNowService<
    'slot,
    'resources,
    'runtime,
    M: RawMutex,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const TX_CAPACITY: usize,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
> where
    R: MacInterruptRoute,
    R::Platform: Sized,
{
    hardware: Option<H>,
    receive: Option<RX>,
    sink: Option<S>,
    interrupts: Option<InterruptEpoch<'runtime, R, M>>,
    platform: Option<R::Platform>,
    tx: Option<ControlTransmitter<'slot, P, E, T, TX_BUFFER_SIZE>>,
    mailbox: Option<EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>>,
    protocol: Option<EspNowProtocol<PEERS>>,
    sequence: StaSequenceCounter,
    binding: StandaloneEspNowBinding,
    active: Option<EspNowQueuedTx>,
    prefer_receive: bool,
    phase: ServicePhase,
    report: StandaloneEspNowRunReport,
}

impl<
    'slot,
    'resources,
    'runtime,
    M: RawMutex,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const TX_CAPACITY: usize,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
>
    StandaloneEspNowService<
        'slot,
        'resources,
        'runtime,
        M,
        H,
        R,
        RX,
        S,
        P,
        E,
        T,
        TX_CAPACITY,
        PEERS,
        TX_BUFFER_SIZE,
    >
where
    R: MacInterruptRoute,
    R::Platform: Sized,
{
    #[allow(clippy::too_many_arguments)]
    pub fn from_binding(
        hardware: H,
        receive: RX,
        sink: S,
        interrupts: InterruptEpoch<'runtime, R, M>,
        platform: R::Platform,
        tx: ControlTransmitter<'slot, P, E, T, TX_BUFFER_SIZE>,
        mailbox: EspNowTxMailboxOwner<'resources, M, TX_CAPACITY>,
        protocol: EspNowProtocol<PEERS>,
        sequence: StaSequenceCounter,
        binding: StandaloneEspNowBinding,
    ) -> Self {
        debug_assert_eq!(protocol.config().station(), binding.station);
        debug_assert_eq!(protocol.config().home_channel(), binding.channel);
        Self {
            hardware: Some(hardware),
            receive: Some(receive),
            sink: Some(sink),
            interrupts: Some(interrupts),
            platform: Some(platform),
            tx: Some(tx),
            mailbox: Some(mailbox),
            protocol: Some(protocol),
            sequence,
            binding,
            active: None,
            prefer_receive: true,
            phase: ServicePhase::Prepared,
            report: StandaloneEspNowRunReport::default(),
        }
    }

    pub const fn binding(&self) -> StandaloneEspNowBinding {
        self.binding
    }

    pub const fn tx_epoch(&self) -> Option<u32> {
        match &self.mailbox {
            Some(mailbox) => Some(mailbox.epoch()),
            None => None,
        }
    }

    pub const fn report(&self) -> StandaloneEspNowRunReport {
        self.report
    }

    pub const fn is_quarantined(&self) -> bool {
        matches!(self.phase, ServicePhase::Quarantined)
    }
}

impl<
    'slot,
    'resources,
    'runtime,
    M,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const TX_CAPACITY: usize,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
>
    StandaloneEspNowService<
        'slot,
        'resources,
        'runtime,
        M,
        H,
        R,
        RX,
        S,
        P,
        E,
        T,
        TX_CAPACITY,
        PEERS,
        TX_BUFFER_SIZE,
    >
where
    M: RawMutex,
    H: StaEspNowRxPolicyHardware + TxHardware,
    R: MacInterruptRoute,
    R::Platform: Sized,
    RX: StandaloneEspNowReceive<H, S, PEERS>,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    /// Run until `stop` resolves, then close all application admission and
    /// return with IRQ, RX DMA and ordinary TX quiescent.
    pub async fn run_until_stopped<F>(
        &mut self,
        stop: F,
    ) -> Result<StandaloneEspNowRunReport, StandaloneEspNowRunFailure<R::Error, RX::Error>>
    where
        F: Future<Output = ()>,
    {
        if let Err(error) = self.begin_running().await {
            return Err(self.failure(error));
        }

        let mut stop = pin!(stop);
        loop {
            if self.tx().active() {
                let mac = self.interrupts().mac_runtime();
                let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
                match select4(
                    stop.as_mut(),
                    mac.wait_tx(),
                    tx.wait_deadline(),
                    mac.wait_rx(),
                )
                .await
                {
                    Either4::First(()) => break,
                    Either4::Second(events) => {
                        match self
                            .service_transmit(WifiTxWake::Interrupt { events })
                            .await
                        {
                            Ok(_) => {}
                            Err(StandaloneEspNowRunError::Tx(error)) => {
                                return self.fail_transmit(error);
                            }
                            Err(error) => return self.fail_and_stop(error).await,
                        }
                    }
                    Either4::Third(()) => match self.service_transmit(WifiTxWake::Deadline).await {
                        Ok(_) => {}
                        Err(StandaloneEspNowRunError::Tx(error)) => {
                            return self.fail_transmit(error);
                        }
                        Err(error) => return self.fail_and_stop(error).await,
                    },
                    Either4::Fourth(()) => {
                        if let Err(error) = self.service_receive().await {
                            return self
                                .fail_and_stop(StandaloneEspNowRunError::ReceiveService(error))
                                .await;
                        }
                    }
                }
            } else {
                let mac = self.interrupts().mac_runtime();
                let mailbox = self.mailbox();
                let wake = if self.prefer_receive {
                    match select3(stop.as_mut(), mac.wait_rx(), mailbox.ready()).await {
                        Either3::First(()) => IdleWake::Stop,
                        Either3::Second(()) => IdleWake::Receive,
                        Either3::Third(()) => IdleWake::Transmit,
                    }
                } else {
                    match select3(stop.as_mut(), mailbox.ready(), mac.wait_rx()).await {
                        Either3::First(()) => IdleWake::Stop,
                        Either3::Second(()) => IdleWake::Transmit,
                        Either3::Third(()) => IdleWake::Receive,
                    }
                };
                match wake {
                    IdleWake::Stop => break,
                    IdleWake::Receive => {
                        self.prefer_receive = false;
                        if let Err(error) = self.service_receive().await {
                            return self
                                .fail_and_stop(StandaloneEspNowRunError::ReceiveService(error))
                                .await;
                        }
                    }
                    IdleWake::Transmit => {
                        self.prefer_receive = true;
                        if let Err(error) = self.start_next_transmit() {
                            return self.fail_and_stop(error).await;
                        }
                    }
                }
            }
        }

        match self
            .stop_running(EspNowTxCancelReason::StationStopped)
            .await
        {
            Ok(()) => Ok(self.report),
            Err(error) => Err(self.failure(StandaloneEspNowRunError::Stop(error))),
        }
    }

    /// Run the standalone role with bounded, fixed-channel excursions for
    /// peers configured with [`EspNowPeerChannelPolicy::StandaloneFixed`].
    ///
    /// The channel owner is an explicit opt-in and is never available to the
    /// connected composition. Every excursion stops RX and IRQ first, runs at
    /// most one already-bounded ordinary TX transaction, and restores the home
    /// channel plus a fresh RX/IRQ epoch before another request is admitted.
    /// A PHY or ownership failure makes quarantine sticky; no stopped owner can
    /// be extracted from that state.
    pub async fn run_until_stopped_off_channel<F, C>(
        &mut self,
        stop: F,
        channel: &mut C,
    ) -> Result<
        StandaloneEspNowRunReport,
        StandaloneEspNowOffChannelRunFailure<R::Error, RX::Error, C::Error>,
    >
    where
        F: Future<Output = ()>,
        C: StandaloneEspNowChannelControl<H>,
    {
        let home = self.binding.channel;
        let actual = channel.current_channel();
        if actual != home {
            return Err(self.off_channel_failure(
                StandaloneEspNowOffChannelRunError::ControllerChannelMismatch {
                    expected: home,
                    actual,
                },
            ));
        }
        if let Err(error) = self.begin_running().await {
            return Err(
                self.off_channel_failure(StandaloneEspNowOffChannelRunError::Runtime(error))
            );
        }

        let mut stop = pin!(stop);
        loop {
            if self.tx().active() {
                let mac = self.interrupts().mac_runtime();
                let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
                match select4(
                    stop.as_mut(),
                    mac.wait_tx(),
                    tx.wait_deadline(),
                    mac.wait_rx(),
                )
                .await
                {
                    Either4::First(()) => break,
                    Either4::Second(events) => {
                        match self
                            .service_transmit(WifiTxWake::Interrupt { events })
                            .await
                        {
                            Ok(_) => {}
                            Err(StandaloneEspNowRunError::Tx(error)) => {
                                return match self.fail_transmit(error) {
                                    Ok(report) => Ok(report),
                                    Err(failure) => Err(map_off_channel_runtime_failure(failure)),
                                };
                            }
                            Err(error) => {
                                return match self.fail_and_stop(error).await {
                                    Ok(report) => Ok(report),
                                    Err(failure) => Err(map_off_channel_runtime_failure(failure)),
                                };
                            }
                        }
                    }
                    Either4::Third(()) => match self.service_transmit(WifiTxWake::Deadline).await {
                        Ok(_) => {}
                        Err(StandaloneEspNowRunError::Tx(error)) => {
                            return match self.fail_transmit(error) {
                                Ok(report) => Ok(report),
                                Err(failure) => Err(map_off_channel_runtime_failure(failure)),
                            };
                        }
                        Err(error) => {
                            return match self.fail_and_stop(error).await {
                                Ok(report) => Ok(report),
                                Err(failure) => Err(map_off_channel_runtime_failure(failure)),
                            };
                        }
                    },
                    Either4::Fourth(()) => {
                        if let Err(error) = self.service_receive().await {
                            return match self
                                .fail_and_stop(StandaloneEspNowRunError::ReceiveService(error))
                                .await
                            {
                                Ok(report) => Ok(report),
                                Err(failure) => Err(map_off_channel_runtime_failure(failure)),
                            };
                        }
                    }
                }
                continue;
            }

            let mac = self.interrupts().mac_runtime();
            let mailbox = self.mailbox();
            let wake = if self.prefer_receive {
                match select3(stop.as_mut(), mac.wait_rx(), mailbox.ready()).await {
                    Either3::First(()) => IdleWake::Stop,
                    Either3::Second(()) => IdleWake::Receive,
                    Either3::Third(()) => IdleWake::Transmit,
                }
            } else {
                match select3(stop.as_mut(), mailbox.ready(), mac.wait_rx()).await {
                    Either3::First(()) => IdleWake::Stop,
                    Either3::Second(()) => IdleWake::Transmit,
                    Either3::Third(()) => IdleWake::Receive,
                }
            };
            match wake {
                IdleWake::Stop => break,
                IdleWake::Receive => {
                    self.prefer_receive = false;
                    if let Err(error) = self.service_receive().await {
                        return match self
                            .fail_and_stop(StandaloneEspNowRunError::ReceiveService(error))
                            .await
                        {
                            Ok(report) => Ok(report),
                            Err(failure) => Err(map_off_channel_runtime_failure(failure)),
                        };
                    }
                }
                IdleWake::Transmit => {
                    self.prefer_receive = true;
                    let queued = match self.take_next_transmit() {
                        Ok(Some(queued)) => queued,
                        Ok(None) => continue,
                        Err(error) => {
                            return match self.fail_and_stop(error).await {
                                Ok(report) => Ok(report),
                                Err(failure) => Err(map_off_channel_runtime_failure(failure)),
                            };
                        }
                    };
                    let peer = self
                        .protocol
                        .as_ref()
                        .expect("ESP-NOW protocol owner exists")
                        .peers()
                        .get(queued.peer)
                        .ok();
                    let Some(peer) = peer else {
                        if let Err(error) = self.start_queued_transmit(queued, home) {
                            return match self.fail_and_stop(error).await {
                                Ok(report) => Ok(report),
                                Err(failure) => Err(map_off_channel_runtime_failure(failure)),
                            };
                        }
                        continue;
                    };
                    let EspNowPeerChannelPolicy::StandaloneFixed(peer_channel) =
                        peer.channel_policy()
                    else {
                        if let Err(error) = self.start_queued_transmit(queued, home) {
                            return match self.fail_and_stop(error).await {
                                Ok(report) => Ok(report),
                                Err(failure) => Err(map_off_channel_runtime_failure(failure)),
                            };
                        }
                        continue;
                    };

                    if matches!(peer.phy_mode(), EspNowPhyMode::LongRange(_)) {
                        self.report.tx_rejected = self.report.tx_rejected.saturating_add(1);
                        let error = SingleMpduEspNowTxError::Backend(
                            EspNowTxError::OffChannelLongRangeUnsupported {
                                channel: peer_channel,
                            },
                        );
                        if let Err(error) = self
                            .mailbox()
                            .publish(queued, EspNowTxTerminal::Rejected(error))
                        {
                            return match self
                                .fail_and_stop(StandaloneEspNowRunError::Mailbox(error))
                                .await
                            {
                                Ok(report) => Ok(report),
                                Err(failure) => Err(map_off_channel_runtime_failure(failure)),
                            };
                        }
                        continue;
                    }

                    if matches!(select(stop.as_mut(), ready(())).await, Either::First(())) {
                        if let Err(error) = self.mailbox().publish(
                            queued,
                            EspNowTxTerminal::Cancelled(EspNowTxCancelReason::StationStopped),
                        ) {
                            return Err(self.off_channel_failure(
                                StandaloneEspNowOffChannelRunError::Runtime(
                                    StandaloneEspNowRunError::Mailbox(error),
                                ),
                            ));
                        }
                        break;
                    }

                    match self
                        .service_off_channel_request(queued, peer_channel, channel, stop.as_mut())
                        .await
                    {
                        Ok(OffChannelRequestOutcome::Continue) => {}
                        Ok(OffChannelRequestOutcome::Stop) => break,
                        Err(error) => return Err(self.off_channel_failure(error)),
                    }
                }
            }
        }

        match self
            .stop_running(EspNowTxCancelReason::StationStopped)
            .await
        {
            Ok(()) => Ok(self.report),
            Err(error) => Err(self.off_channel_failure(
                StandaloneEspNowOffChannelRunError::Runtime(StandaloneEspNowRunError::Stop(error)),
            )),
        }
    }

    async fn service_off_channel_request<C, F>(
        &mut self,
        queued: EspNowQueuedTx,
        peer_channel: WifiChannel,
        channel: &mut C,
        mut stop: Pin<&mut F>,
    ) -> Result<
        OffChannelRequestOutcome,
        StandaloneEspNowOffChannelRunError<R::Error, RX::Error, C::Error>,
    >
    where
        F: Future<Output = ()>,
        C: StandaloneEspNowChannelControl<H>,
    {
        self.enter_quarantine();

        if self.tx().active() {
            self.publish_off_channel_failure(
                queued,
                EspNowOffChannelFailureStage::QuiesceHomeInterrupts,
            )?;
            return Err(StandaloneEspNowOffChannelRunError::Runtime(
                StandaloneEspNowRunError::TransmitNotIdle,
            ));
        }
        if self.interrupts().is_active()
            && let Err(error) = self.quiesce_interrupts()
        {
            self.publish_off_channel_failure(
                queued,
                EspNowOffChannelFailureStage::QuiesceHomeInterrupts,
            )?;
            return Err(StandaloneEspNowOffChannelRunError::Runtime(
                StandaloneEspNowRunError::Stop(StandaloneEspNowStopError::Interrupt(error)),
            ));
        }
        if let Err(error) = self.stop_receive().await {
            self.publish_off_channel_failure(
                queued,
                EspNowOffChannelFailureStage::StopHomeReceive,
            )?;
            return Err(StandaloneEspNowOffChannelRunError::Runtime(
                StandaloneEspNowRunError::Stop(StandaloneEspNowStopError::Receive(error)),
            ));
        }

        // Stop wins every safe state boundary. A PHY switch itself is an
        // indivisible ownership transaction and is never cancelled halfway.
        if matches!(select(stop.as_mut(), ready(())).await, Either::First(())) {
            self.mailbox()
                .publish(
                    queued,
                    EspNowTxTerminal::Cancelled(EspNowTxCancelReason::StationStopped),
                )
                .map_err(|error| {
                    StandaloneEspNowOffChannelRunError::Runtime(StandaloneEspNowRunError::Mailbox(
                        error,
                    ))
                })?;
            self.leave_quarantine_at_home();
            return Ok(OffChannelRequestOutcome::Stop);
        }

        let from = channel.current_channel();
        if let Err(error) = self.switch_channel(channel, peer_channel).await {
            self.publish_off_channel_failure(queued, EspNowOffChannelFailureStage::SwitchToPeer)?;
            return Err(StandaloneEspNowOffChannelRunError::ChannelSwitch {
                stage: EspNowOffChannelFailureStage::SwitchToPeer,
                from,
                target: peer_channel,
                error,
            });
        }
        let actual = channel.current_channel();
        if actual != peer_channel {
            self.publish_off_channel_failure(queued, EspNowOffChannelFailureStage::SwitchToPeer)?;
            return Err(
                StandaloneEspNowOffChannelRunError::ControllerChannelMismatch {
                    expected: peer_channel,
                    actual,
                },
            );
        }
        self.report.off_channel_started = self.report.off_channel_started.saturating_add(1);

        if matches!(select(stop.as_mut(), ready(())).await, Either::First(())) {
            self.mailbox()
                .publish(
                    queued,
                    EspNowTxTerminal::Cancelled(EspNowTxCancelReason::StationStopped),
                )
                .map_err(|error| {
                    StandaloneEspNowOffChannelRunError::Runtime(StandaloneEspNowRunError::Mailbox(
                        error,
                    ))
                })?;
            self.restore_home(channel, peer_channel).await?;
            self.leave_quarantine_at_home();
            return Ok(OffChannelRequestOutcome::Stop);
        }

        if let Err(activation) = self.activate_interrupts() {
            self.publish_off_channel_failure(
                queued,
                EspNowOffChannelFailureStage::ActivateTransmitInterrupts,
            )?;
            // Recovery is mandatory even though the runtime remains
            // quarantined after this failed excursion.
            self.restore_home(channel, peer_channel).await?;
            return Err(StandaloneEspNowOffChannelRunError::Runtime(
                StandaloneEspNowRunError::Activate(activation),
            ));
        }

        if let Err(error) = self.start_queued_transmit_with_publication(
            queued,
            peer_channel,
            TxCompletionPublication::AfterHomeRestore,
        ) {
            if self.interrupts().is_active()
                && let Err(quiesce) = self.quiesce_interrupts()
            {
                return Err(StandaloneEspNowOffChannelRunError::Runtime(
                    StandaloneEspNowRunError::Stop(StandaloneEspNowStopError::Interrupt(quiesce)),
                ));
            }
            self.restore_home(channel, peer_channel).await?;
            return Err(StandaloneEspNowOffChannelRunError::Runtime(error));
        }

        let mut stop_pending = false;
        while self.tx().active() {
            let wake = if stop_pending {
                let mac = self.interrupts().mac_runtime();
                let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
                match select(mac.wait_tx(), tx.wait_deadline()).await {
                    Either::First(events) => WifiTxWake::Interrupt { events },
                    Either::Second(()) => WifiTxWake::Deadline,
                }
            } else {
                let mac = self.interrupts().mac_runtime();
                let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
                match select3(stop.as_mut(), mac.wait_tx(), tx.wait_deadline()).await {
                    Either3::First(()) => {
                        stop_pending = true;
                        continue;
                    }
                    Either3::Second(events) => WifiTxWake::Interrupt { events },
                    Either3::Third(()) => WifiTxWake::Deadline,
                }
            };
            if let Err(error) = self
                .service_transmit_with_publication(wake, TxCompletionPublication::AfterHomeRestore)
                .await
            {
                if let Some(active) = self.active.take() {
                    let terminal = match error {
                        StandaloneEspNowRunError::Tx(error) => {
                            EspNowTxRuntimeFailure::TxLifecycle(error)
                        }
                        _ => EspNowTxRuntimeFailure::OffChannel(
                            EspNowOffChannelFailureStage::QuiesceTransmitInterrupts,
                        ),
                    };
                    let _ = self
                        .mailbox()
                        .publish(active, EspNowTxTerminal::RuntimeFailure(terminal));
                }
                self.enter_quarantine();
                return Err(StandaloneEspNowOffChannelRunError::Runtime(error));
            }
        }

        if let Err(error) = self.quiesce_interrupts() {
            self.publish_active_off_channel_failure(
                EspNowOffChannelFailureStage::QuiesceTransmitInterrupts,
            )?;
            return Err(StandaloneEspNowOffChannelRunError::Runtime(
                StandaloneEspNowRunError::Stop(StandaloneEspNowStopError::Interrupt(error)),
            ));
        }
        if let Err(error) = self.restore_home(channel, peer_channel).await {
            self.publish_active_off_channel_failure(EspNowOffChannelFailureStage::SwitchHome)?;
            return Err(error);
        }

        let prepare = {
            let receive = self.receive.as_mut().expect("ESP-NOW RX owner exists");
            let hardware = self
                .hardware
                .as_mut()
                .expect("ESP-NOW hardware owner exists");
            receive.prepare_next(hardware)
        };
        if let Err(error) = prepare {
            self.publish_active_off_channel_failure(
                EspNowOffChannelFailureStage::PrepareHomeReceive,
            )?;
            return Err(StandaloneEspNowOffChannelRunError::Runtime(
                StandaloneEspNowRunError::ReceivePrepare(error),
            ));
        }
        if let Err(error) = self.start_receive() {
            self.publish_active_off_channel_failure(
                EspNowOffChannelFailureStage::StartHomeReceive,
            )?;
            return Err(StandaloneEspNowOffChannelRunError::Runtime(
                StandaloneEspNowRunError::ReceiveStart(error),
            ));
        }
        if let Err(activation) = self.activate_interrupts() {
            let _ = self.stop_receive().await;
            self.publish_active_off_channel_failure(
                EspNowOffChannelFailureStage::ActivateHomeInterrupts,
            )?;
            return Err(StandaloneEspNowOffChannelRunError::Runtime(
                StandaloneEspNowRunError::Activate(activation),
            ));
        }
        self.interrupts().mac_runtime().notify_rx_handoff();
        self.finish_active()
            .map_err(StandaloneEspNowOffChannelRunError::Runtime)?;
        self.leave_quarantine_at_home();
        if stop_pending {
            Ok(OffChannelRequestOutcome::Stop)
        } else {
            Ok(OffChannelRequestOutcome::Continue)
        }
    }

    async fn switch_channel<C>(
        &mut self,
        channel: &mut C,
        target: WifiChannel,
    ) -> Result<(), C::Error>
    where
        C: StandaloneEspNowChannelControl<H>,
    {
        let hardware = self
            .hardware
            .as_mut()
            .expect("ESP-NOW hardware owner exists");
        channel.switch_channel(hardware, target).await
    }

    async fn restore_home<C>(
        &mut self,
        channel: &mut C,
        from: WifiChannel,
    ) -> Result<(), StandaloneEspNowOffChannelRunError<R::Error, RX::Error, C::Error>>
    where
        C: StandaloneEspNowChannelControl<H>,
    {
        let home = self.binding.channel;
        if let Err(error) = self.switch_channel(channel, home).await {
            return Err(StandaloneEspNowOffChannelRunError::ChannelSwitch {
                stage: EspNowOffChannelFailureStage::SwitchHome,
                from,
                target: home,
                error,
            });
        }
        let actual = channel.current_channel();
        if actual != home {
            return Err(
                StandaloneEspNowOffChannelRunError::ControllerChannelMismatch {
                    expected: home,
                    actual,
                },
            );
        }
        configure_standalone_esp_now_receive_policy(self.hardware_mut());
        self.report.home_channel_restored = self.report.home_channel_restored.saturating_add(1);
        Ok(())
    }

    fn activate_interrupts(&mut self) -> Result<(), MacInterruptEpochActivateError<R::Error>> {
        let platform = self
            .platform
            .as_ref()
            .expect("ESP-NOW platform owner exists");
        self.interrupts
            .as_mut()
            .expect("ESP-NOW interrupt owner exists")
            .activate(platform, MAC_COLD_RX_INTERRUPT_MASK)
    }

    fn quiesce_interrupts(
        &mut self,
    ) -> Result<MacInterruptEpochDrain, MacInterruptEpochQuiesceError<R::Error>> {
        let platform = self
            .platform
            .as_ref()
            .expect("ESP-NOW platform owner exists");
        let drain = self
            .interrupts
            .as_mut()
            .expect("ESP-NOW interrupt owner exists")
            .quiesce(platform)?;
        self.report.interrupt_drain = drain;
        Ok(drain)
    }

    fn publish_off_channel_failure<C>(
        &mut self,
        queued: EspNowQueuedTx,
        stage: EspNowOffChannelFailureStage,
    ) -> Result<(), StandaloneEspNowOffChannelRunError<R::Error, RX::Error, C>> {
        self.mailbox()
            .publish(
                queued,
                EspNowTxTerminal::RuntimeFailure(EspNowTxRuntimeFailure::OffChannel(stage)),
            )
            .map_err(|error| {
                StandaloneEspNowOffChannelRunError::Runtime(StandaloneEspNowRunError::Mailbox(
                    error,
                ))
            })
    }

    fn publish_active_off_channel_failure<C>(
        &mut self,
        stage: EspNowOffChannelFailureStage,
    ) -> Result<(), StandaloneEspNowOffChannelRunError<R::Error, RX::Error, C>> {
        let Some(queued) = self.active.take() else {
            return Ok(());
        };
        let _ = self.tx_mut().take_last_outcome();
        self.publish_off_channel_failure(queued, stage)
    }

    fn enter_quarantine(&mut self) {
        self.phase = ServicePhase::Quarantined;
        self.report.quarantined = true;
    }

    fn leave_quarantine_at_home(&mut self) {
        self.phase = ServicePhase::Running;
        self.report.quarantined = false;
    }

    fn off_channel_failure<C>(
        &self,
        error: StandaloneEspNowOffChannelRunError<R::Error, RX::Error, C>,
    ) -> StandaloneEspNowOffChannelRunFailure<R::Error, RX::Error, C> {
        StandaloneEspNowOffChannelRunFailure {
            error,
            report: self.report,
        }
    }

    async fn begin_running(&mut self) -> Result<(), StandaloneEspNowRunError<R::Error, RX::Error>> {
        match self.phase {
            ServicePhase::Prepared => {}
            ServicePhase::Running => {
                return Err(StandaloneEspNowRunError::AlreadyRunning);
            }
            ServicePhase::Stopped => {
                return Err(StandaloneEspNowRunError::AlreadyStopped);
            }
            ServicePhase::Faulted | ServicePhase::Quarantined => {
                return Err(StandaloneEspNowRunError::Faulted);
            }
        }
        let receive_phase = self.receive().phase();
        let receive_station = self.receive().station();
        if receive_station != self.binding.station {
            return Err(StandaloneEspNowRunError::ReceiveStationBinding {
                expected: self.binding.station,
                actual: receive_station,
            });
        }
        let receive_channel = self.receive().home_channel();
        if receive_channel != self.binding.channel {
            return Err(StandaloneEspNowRunError::ReceiveChannelBinding {
                expected: self.binding.channel,
                actual: receive_channel,
            });
        }
        if !self.receive().peer_snapshot_matches(
            self.protocol
                .as_ref()
                .expect("ESP-NOW protocol owner exists"),
        ) {
            return Err(StandaloneEspNowRunError::ReceivePeerSnapshotMismatch);
        }
        if receive_phase != RxFrontierPhase::Prepared {
            return Err(StandaloneEspNowRunError::ReceiveNotPrepared(receive_phase));
        }
        if self.tx().active() {
            return Err(StandaloneEspNowRunError::TransmitNotIdle);
        }

        configure_standalone_esp_now_receive_policy(self.hardware_mut());
        self.start_receive()
            .map_err(StandaloneEspNowRunError::ReceiveStart)?;
        let activation = {
            let platform = self
                .platform
                .as_ref()
                .expect("ESP-NOW platform owner exists");
            self.interrupts
                .as_mut()
                .expect("ESP-NOW interrupt owner exists")
                .activate(platform, MAC_COLD_RX_INTERRUPT_MASK)
        };
        if let Err(activation) = activation {
            return match self.stop_receive().await {
                Ok(()) => Err(StandaloneEspNowRunError::Activate(activation)),
                Err(receive) => {
                    self.phase = ServicePhase::Faulted;
                    Err(StandaloneEspNowRunError::ActivateReceiveStop {
                        activation,
                        receive,
                    })
                }
            };
        }
        self.phase = ServicePhase::Running;
        self.interrupts().mac_runtime().notify_rx_handoff();
        Ok(())
    }

    /// Explicit cancellation recovery for a dropped run future.
    pub async fn stop(
        &mut self,
    ) -> Result<StandaloneEspNowRunReport, StandaloneEspNowStopError<R::Error, RX::Error>> {
        match self.phase {
            ServicePhase::Prepared => {
                self.close_and_cancel(EspNowTxCancelReason::StationStopped)?;
                self.finish_mailbox(EspNowTxCancelReason::StationStopped)
                    .await?;
                self.report.duplicate_history_cleared =
                    self.receive_mut().reset_duplicate_history();
                self.phase = ServicePhase::Stopped;
                Ok(self.report)
            }
            ServicePhase::Running => {
                self.stop_running(EspNowTxCancelReason::StationStopped)
                    .await?;
                Ok(self.report)
            }
            ServicePhase::Stopped | ServicePhase::Faulted => {
                Err(StandaloneEspNowStopError::NotRunning)
            }
            ServicePhase::Quarantined => Err(StandaloneEspNowStopError::Quarantined),
        }
    }

    fn start_receive(&mut self) -> Result<(), RX::Error> {
        let receive = self.receive.as_mut().expect("ESP-NOW RX owner exists");
        let hardware = self
            .hardware
            .as_mut()
            .expect("ESP-NOW hardware owner exists");
        receive.start(hardware)
    }

    async fn service_receive(&mut self) -> Result<(), RX::Error> {
        self.report.receive_wakes = self.report.receive_wakes.saturating_add(1);
        let progress = {
            let receive = self.receive.as_mut().expect("ESP-NOW RX owner exists");
            let hardware = self
                .hardware
                .as_mut()
                .expect("ESP-NOW hardware owner exists");
            let sink = self.sink.as_mut().expect("ESP-NOW RX sink exists");
            receive.service(hardware, sink)?
        };
        record_receive(&mut self.report.receive, progress);
        if progress.service_probe_pending {
            yield_now().await;
            self.interrupts().mac_runtime().notify_rx_handoff();
        }
        Ok(())
    }

    fn start_next_transmit(&mut self) -> Result<(), StandaloneEspNowRunError<R::Error, RX::Error>> {
        let Some(queued) = self.take_next_transmit()? else {
            return Ok(());
        };
        self.start_queued_transmit(queued, self.binding.channel)
    }

    fn take_next_transmit(
        &mut self,
    ) -> Result<Option<EspNowQueuedTx>, StandaloneEspNowRunError<R::Error, RX::Error>> {
        let Some(queued) = self.mailbox().try_take() else {
            return Ok(None);
        };
        if queued.ticket.epoch() != self.mailbox().epoch() {
            self.mailbox()
                .publish(
                    queued,
                    EspNowTxTerminal::Cancelled(EspNowTxCancelReason::StaleEpoch),
                )
                .map_err(StandaloneEspNowRunError::Mailbox)?;
            return Ok(None);
        }
        Ok(Some(queued))
    }

    fn start_queued_transmit(
        &mut self,
        queued: EspNowQueuedTx,
        active_channel: WifiChannel,
    ) -> Result<(), StandaloneEspNowRunError<R::Error, RX::Error>> {
        self.start_queued_transmit_with_publication(
            queued,
            active_channel,
            TxCompletionPublication::Immediate,
        )
    }

    fn start_queued_transmit_with_publication(
        &mut self,
        queued: EspNowQueuedTx,
        active_channel: WifiChannel,
        publication: TxCompletionPublication,
    ) -> Result<(), StandaloneEspNowRunError<R::Error, RX::Error>> {
        // No ordinary transaction is live, so any coalesced TX signal belongs
        // to an earlier role/transaction and must not complete this request.
        let _ = self.interrupts().mac_runtime().try_take_tx();
        let result = match queued.request {
            EspNowQueuedRequest::V1(request) => {
                let hardware = self
                    .hardware
                    .as_mut()
                    .expect("ESP-NOW hardware owner exists");
                let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
                let protocol = self
                    .protocol
                    .as_ref()
                    .expect("ESP-NOW protocol owner exists");
                tx.start_esp_now_v1_plaintext(
                    hardware,
                    protocol,
                    &mut self.sequence,
                    request.peer(),
                    request.random_value(),
                    request.payload(),
                    active_channel,
                    self.binding.station,
                    self.binding.tx,
                )
            }
            EspNowQueuedRequest::V2(_) => {
                let mailbox = self.mailbox.as_ref().expect("ESP-NOW mailbox owner exists");
                let hardware = self
                    .hardware
                    .as_mut()
                    .expect("ESP-NOW hardware owner exists");
                let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
                let protocol = self
                    .protocol
                    .as_ref()
                    .expect("ESP-NOW protocol owner exists");
                let request = mailbox.with_v2_request(&queued, |request| {
                    tx.start_esp_now_v2_plaintext(
                        hardware,
                        protocol,
                        &mut self.sequence,
                        request.peer(),
                        request.random_value(),
                        request.payload(),
                        active_channel,
                        self.binding.station,
                        self.binding.tx,
                    )
                });
                match request {
                    Ok(result) => result,
                    Err(error) => {
                        mailbox
                            .publish(
                                queued,
                                EspNowTxTerminal::RuntimeFailure(
                                    EspNowTxRuntimeFailure::MissingV2PayloadSlot,
                                ),
                            )
                            .map_err(StandaloneEspNowRunError::Mailbox)?;
                        return Err(StandaloneEspNowRunError::Mailbox(error));
                    }
                }
            }
        };
        match result {
            Ok(WifiTxProgress::Pending) => {
                self.active = Some(queued);
                self.report.tx_started = self.report.tx_started.saturating_add(1);
            }
            Ok(WifiTxProgress::Complete) => {
                self.active = Some(queued);
                self.report.tx_started = self.report.tx_started.saturating_add(1);
                if publication == TxCompletionPublication::Immediate {
                    self.finish_active()?;
                }
            }
            Err(error) => {
                self.report.tx_rejected = self.report.tx_rejected.saturating_add(1);
                self.mailbox()
                    .publish(queued, EspNowTxTerminal::Rejected(error))
                    .map_err(StandaloneEspNowRunError::Mailbox)?;
            }
        }
        Ok(())
    }

    async fn service_transmit(
        &mut self,
        wake: WifiTxWake,
    ) -> Result<WifiTxProgress, StandaloneEspNowRunError<R::Error, RX::Error>> {
        self.service_transmit_with_publication(wake, TxCompletionPublication::Immediate)
            .await
    }

    async fn service_transmit_with_publication(
        &mut self,
        wake: WifiTxWake,
        publication: TxCompletionPublication,
    ) -> Result<WifiTxProgress, StandaloneEspNowRunError<R::Error, RX::Error>> {
        let progress = {
            let hardware = self
                .hardware
                .as_mut()
                .expect("ESP-NOW hardware owner exists");
            let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
            tx.service(hardware, wake)
                .await
                .map_err(StandaloneEspNowRunError::Tx)?
        };
        if progress == WifiTxProgress::Complete && publication == TxCompletionPublication::Immediate
        {
            self.finish_active()?;
        }
        Ok(progress)
    }

    fn finish_active(&mut self) -> Result<(), StandaloneEspNowRunError<R::Error, RX::Error>> {
        let Some(queued) = self.active.take() else {
            return Ok(());
        };
        let Some(outcome) = self.tx_mut().take_last_outcome() else {
            self.mailbox()
                .publish(
                    queued,
                    EspNowTxTerminal::RuntimeFailure(
                        EspNowTxRuntimeFailure::MissingOrdinaryTxOutcome,
                    ),
                )
                .map_err(StandaloneEspNowRunError::Mailbox)?;
            return Err(StandaloneEspNowRunError::MissingOrdinaryTxOutcome);
        };
        self.mailbox()
            .publish(queued, EspNowTxTerminal::Completed(outcome))
            .map_err(StandaloneEspNowRunError::Mailbox)?;
        self.report.tx_completed = self.report.tx_completed.saturating_add(1);
        Ok(())
    }

    fn fail_transmit(
        &mut self,
        error: SingleMpduTxError,
    ) -> Result<StandaloneEspNowRunReport, StandaloneEspNowRunFailure<R::Error, RX::Error>> {
        if let Some(queued) = self.active.take() {
            let _ = self.mailbox().publish(
                queued,
                EspNowTxTerminal::RuntimeFailure(EspNowTxRuntimeFailure::TxLifecycle(error)),
            );
        }
        let _ = self.close_and_cancel(EspNowTxCancelReason::OwnerShutdown);
        self.phase = ServicePhase::Faulted;
        Err(self.failure(StandaloneEspNowRunError::Tx(error)))
    }

    async fn fail_and_stop(
        &mut self,
        error: StandaloneEspNowRunError<R::Error, RX::Error>,
    ) -> Result<StandaloneEspNowRunReport, StandaloneEspNowRunFailure<R::Error, RX::Error>> {
        match self.stop_running(EspNowTxCancelReason::OwnerShutdown).await {
            Ok(()) => Err(self.failure(error)),
            Err(stop) => Err(self.failure(StandaloneEspNowRunError::Stop(stop))),
        }
    }

    async fn stop_running(
        &mut self,
        reason: EspNowTxCancelReason,
    ) -> Result<(), StandaloneEspNowStopError<R::Error, RX::Error>> {
        self.close_and_cancel(reason)?;
        while self.tx().active() {
            let mac = self.interrupts().mac_runtime();
            let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
            let wake = match select(mac.wait_tx(), tx.wait_deadline()).await {
                Either::First(events) => WifiTxWake::Interrupt { events },
                Either::Second(()) => WifiTxWake::Deadline,
            };
            let progress = {
                let hardware = self
                    .hardware
                    .as_mut()
                    .expect("ESP-NOW hardware owner exists");
                let tx = self.tx.as_mut().expect("ESP-NOW TX owner exists");
                tx.service(hardware, wake)
                    .await
                    .map_err(StandaloneEspNowStopError::Tx)?
            };
            if progress == WifiTxProgress::Complete {
                self.finish_active().map_err(|error| match error {
                    StandaloneEspNowRunError::Mailbox(error) => {
                        StandaloneEspNowStopError::Mailbox(error)
                    }
                    StandaloneEspNowRunError::MissingOrdinaryTxOutcome => {
                        StandaloneEspNowStopError::MissingOrdinaryTxOutcome
                    }
                    _ => StandaloneEspNowStopError::MissingOrdinaryTxOutcome,
                })?;
            }
        }
        self.finish_mailbox(reason).await?;

        if self.interrupts().is_active() {
            let platform = self
                .platform
                .as_ref()
                .expect("ESP-NOW platform owner exists");
            self.report.interrupt_drain = self
                .interrupts
                .as_mut()
                .expect("ESP-NOW interrupt owner exists")
                .quiesce(platform)
                .map_err(StandaloneEspNowStopError::Interrupt)?;
        }
        self.stop_receive()
            .await
            .map_err(StandaloneEspNowStopError::Receive)?;
        self.report.duplicate_history_cleared = self.receive_mut().reset_duplicate_history();
        self.phase = ServicePhase::Stopped;
        Ok(())
    }

    fn close_and_cancel(
        &mut self,
        reason: EspNowTxCancelReason,
    ) -> Result<u32, StandaloneEspNowStopError<R::Error, RX::Error>> {
        let mailbox = self
            .mailbox
            .as_mut()
            .ok_or(StandaloneEspNowStopError::NotRunning)?;
        mailbox.close();
        mailbox
            .cancel_pending(reason)
            .map_err(StandaloneEspNowStopError::Mailbox)
    }

    async fn finish_mailbox(
        &mut self,
        reason: EspNowTxCancelReason,
    ) -> Result<(), StandaloneEspNowStopError<R::Error, RX::Error>> {
        while self.mailbox().publishers_in_flight() != 0 {
            yield_now().await;
        }
        self.close_and_cancel(reason)?;
        let mailbox = self
            .mailbox
            .take()
            .ok_or(StandaloneEspNowStopError::NotRunning)?;
        self.report.mailbox = Some(
            mailbox
                .shutdown(reason)
                .map_err(StandaloneEspNowStopError::Mailbox)?,
        );
        Ok(())
    }

    async fn stop_receive(&mut self) -> Result<(), RX::Error> {
        while self.receive().phase() == RxFrontierPhase::Live {
            let stopped = {
                let receive = self.receive.as_mut().expect("ESP-NOW RX owner exists");
                let hardware = self
                    .hardware
                    .as_mut()
                    .expect("ESP-NOW hardware owner exists");
                receive.stop(hardware)?
            };
            if !stopped {
                yield_now().await;
            }
        }
        Ok(())
    }

    fn failure(
        &self,
        error: StandaloneEspNowRunError<R::Error, RX::Error>,
    ) -> StandaloneEspNowRunFailure<R::Error, RX::Error> {
        StandaloneEspNowRunFailure {
            error,
            report: self.report,
        }
    }

    fn hardware_mut(&mut self) -> &mut H {
        self.hardware
            .as_mut()
            .expect("ESP-NOW hardware owner exists")
    }

    fn receive(&self) -> &RX {
        self.receive.as_ref().expect("ESP-NOW RX owner exists")
    }

    fn receive_mut(&mut self) -> &mut RX {
        self.receive.as_mut().expect("ESP-NOW RX owner exists")
    }

    fn interrupts(&self) -> &InterruptEpoch<'runtime, R, M> {
        self.interrupts
            .as_ref()
            .expect("ESP-NOW interrupt owner exists")
    }

    fn tx(&self) -> &ControlTransmitter<'slot, P, E, T, TX_BUFFER_SIZE> {
        self.tx.as_ref().expect("ESP-NOW TX owner exists")
    }

    fn tx_mut(&mut self) -> &mut ControlTransmitter<'slot, P, E, T, TX_BUFFER_SIZE> {
        self.tx.as_mut().expect("ESP-NOW TX owner exists")
    }

    fn mailbox(&self) -> &EspNowTxMailboxOwner<'resources, M, TX_CAPACITY> {
        self.mailbox.as_ref().expect("ESP-NOW mailbox owner exists")
    }
}

/// Every owner returned after a successful stopped edge. A new mailbox epoch
/// and a re-prepared halted RX frontier can construct the next runtime epoch.
pub struct StandaloneEspNowStopped<
    'slot,
    'runtime,
    M: RawMutex,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
> where
    R: MacInterruptRoute,
    R::Platform: Sized,
{
    pub hardware: H,
    pub receive: RX,
    pub sink: S,
    pub interrupts: InterruptEpoch<'runtime, R, M>,
    pub platform: R::Platform,
    pub tx: ControlTransmitter<'slot, P, E, T, TX_BUFFER_SIZE>,
    pub protocol: EspNowProtocol<PEERS>,
    pub sequence: StaSequenceCounter,
    pub binding: StandaloneEspNowBinding,
    pub report: StandaloneEspNowRunReport,
}

impl<
    'slot,
    'resources,
    'runtime,
    M: RawMutex,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const TX_CAPACITY: usize,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
>
    StandaloneEspNowService<
        'slot,
        'resources,
        'runtime,
        M,
        H,
        R,
        RX,
        S,
        P,
        E,
        T,
        TX_CAPACITY,
        PEERS,
        TX_BUFFER_SIZE,
    >
where
    R: MacInterruptRoute,
    R::Platform: Sized,
{
    #[allow(clippy::result_large_err)]
    pub fn try_into_stopped(
        mut self,
    ) -> Result<
        StandaloneEspNowStopped<'slot, 'runtime, M, H, R, RX, S, P, E, T, PEERS, TX_BUFFER_SIZE>,
        Self,
    > {
        if self.phase != ServicePhase::Stopped
            || self.active.is_some()
            || self.mailbox.is_some()
            || self
                .interrupts
                .as_ref()
                .is_some_and(InterruptEpoch::is_active)
        {
            return Err(self);
        }
        Ok(StandaloneEspNowStopped {
            hardware: self.hardware.take().expect("checked hardware owner"),
            receive: self.receive.take().expect("checked RX owner"),
            sink: self.sink.take().expect("checked RX sink"),
            interrupts: self.interrupts.take().expect("checked IRQ owner"),
            platform: self.platform.take().expect("checked platform owner"),
            tx: self.tx.take().expect("checked TX owner"),
            protocol: self.protocol.take().expect("checked protocol owner"),
            sequence: self.sequence,
            binding: self.binding,
            report: self.report,
        })
    }

    fn retain_active_owners_for_reset(&mut self) {
        core::mem::forget(self.hardware.take());
        core::mem::forget(self.receive.take());
        core::mem::forget(self.sink.take());
        core::mem::forget(self.interrupts.take());
        core::mem::forget(self.platform.take());
        core::mem::forget(self.tx.take());
    }
}

impl<
    M: RawMutex,
    H,
    R,
    RX,
    S,
    P,
    E,
    T,
    const TX_CAPACITY: usize,
    const PEERS: usize,
    const TX_BUFFER_SIZE: usize,
> Drop
    for StandaloneEspNowService<
        '_,
        '_,
        '_,
        M,
        H,
        R,
        RX,
        S,
        P,
        E,
        T,
        TX_CAPACITY,
        PEERS,
        TX_BUFFER_SIZE,
    >
where
    R: MacInterruptRoute,
    R::Platform: Sized,
{
    fn drop(&mut self) {
        if matches!(
            self.phase,
            ServicePhase::Running | ServicePhase::Faulted | ServicePhase::Quarantined
        ) {
            self.retain_active_owners_for_reset();
        }
    }
}

fn map_off_channel_runtime_failure<InterruptError, ReceiveError, ChannelError>(
    failure: StandaloneEspNowRunFailure<InterruptError, ReceiveError>,
) -> StandaloneEspNowOffChannelRunFailure<InterruptError, ReceiveError, ChannelError> {
    StandaloneEspNowOffChannelRunFailure {
        error: StandaloneEspNowOffChannelRunError::Runtime(failure.error),
        report: failure.report,
    }
}

fn record_receive(total: &mut StandaloneEspNowRxProgress, progress: StandaloneEspNowRxProgress) {
    total.completed_descriptors = total
        .completed_descriptors
        .saturating_add(progress.completed_descriptors);
    total.received = total.received.saturating_add(progress.received);
    total.duplicates = total.duplicates.saturating_add(progress.duplicates);
    total.ignored = total.ignored.saturating_add(progress.ignored);
    total.rejected = total.rejected.saturating_add(progress.rejected);
    total.recycled_descriptors = total
        .recycled_descriptors
        .saturating_add(progress.recycled_descriptors);
    total.reload_pending = progress.reload_pending;
    total.service_probe_pending = progress.service_probe_pending;
}

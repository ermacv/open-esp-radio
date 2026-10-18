#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc control outcomes carry the exact reusable or faulted owner"
)]
#![expect(
    clippy::result_large_err,
    reason = "control failure returns the exact affine owner for teardown"
)]

//! Embassy delivery adapter for ESP32-S31 connected-station control.
//!
//! Protocol state, GTK/replay transactions and finite transitions live in the
//! chip STA crate. This module retains those owners with the bounded event
//! receiver, deadline wait and reorder sender needed to schedule them on Embassy.

use core::future::Future;
use oer_ieee80211_mac::block_ack::TX_BLOCK_ACK_MAX_TIDS;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_time::Timer;

pub use oer_esp32s31_ieee80211_mac::rx::ampdu::{RxReorderCommand, RxReorderCommandError};

use oer_esp32s31_ieee80211_sta::connected_rx::ConnectedRxControlEvent;
use oer_ieee80211_trace::{ControlEventKind, ControlExit, ControlMailbox, MailboxOp};

use oer_ieee80211_mac::twt::IndividualTwtFlowId;

use oer_ieee80211_sta::{
    ftm::FtmRequesterConfig,
    link_monitor::{StaBeaconLossConfig, StaBeaconMonitor},
    twt::{IndividualTwtProposal, IndividualTwtRequester, IndividualTwtRequesterConfig},
};

pub use oer_esp32s31_ieee80211_sta::{
    connected_control::{
        ConnectedControlCore, ConnectedControlError, ConnectedControlPorts,
        ConnectedControlReorder, ConnectedControlTx, ConnectedControlTxFailure,
        ConnectedControlTxKind, ConnectedDisconnectReason, ConnectedFtmRequestFrontier,
        ConnectedHeControlRuntimeEvidence, ConnectedHeControlRuntimeOutcome,
        ConnectedHeControlRuntimeRejection, ConnectedIndividualTwtRuntimeEvidence,
        ConnectedIndividualTwtRuntimeOutcome, HeNdpaRuntimeRequest, HeTriggerRuntimeRequest,
        JoinBeacon, StationTwtWakePlan,
    },
    ftm::{
        StationFtmFrontierStatus, StationFtmHardwareError, StationFtmHardwareFrontier,
        StationFtmHardwareStage, StationFtmUnsupportedStage, station_ftm_hardware_frontier,
    },
    hardware::{
        beacon_monitor::{
            StationBeaconMonitorBinding, StationHardwareBeaconMonitorEpoch,
            StationHardwareBeaconMonitorFrontier, StationHardwareBeaconMonitorStopped,
        },
        control::{
            ConnectedControlHardware, StationIndividualTwtHardwareError,
            StationIndividualTwtHardwareStage, StationIndividualTwtUnsupportedStage,
        },
    },
    modem_sleep::{ModemSleep, PmBeacon, SleepType},
};

use crate::{
    datapath::{
        DatapathControlContext, DatapathControlProgress,
        rx::reorder::{RxReorderCommandSender, try_send_rx_reorder_command},
        services::DatapathControlService,
    },
    roles::{
        concurrent::StaApRxBlockAck,
        station::{
            control_mailbox::{ConnectedControlReceiver, ConnectedManagementInput},
            power::{StationPowerBinding, StationPowerLink},
        },
    },
};

/// Finite ownership released when one connected control epoch stops.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedControlShutdown {
    pub rx_block_ack_agreements: u8,
    pub tx_block_ack_sessions: u8,
    pub discarded_events: u8,
    pub in_flight: Option<ConnectedControlTxKind>,
    /// Consumed association admission owner. Its current form proves that the
    /// automatic monitor stopped before any hardware mutation.
    pub hardware_beacon_monitor: Option<StationHardwareBeaconMonitorStopped>,
}

pub use oer_esp32s31_ieee80211_sta::connected::security::{
    ConnectedWpa2Security, ConnectedWpa2SecurityEvidence, ConnectedWpa2SecurityFailure,
};

struct EmbassyReorderSink<'sender, 'resources, M: RawMutex> {
    sender: Option<&'sender RxReorderCommandSender<'resources, M>>,
}

impl<M: RawMutex> ConnectedControlReorder for EmbassyReorderSink<'_, '_, M> {
    fn publish(&mut self, command: RxReorderCommand) -> Result<(), RxReorderCommandError> {
        let Some(sender) = self.sender else {
            return Ok(());
        };
        try_send_rx_reorder_command(sender, command)
    }
}

/// Unique Embassy event consumer around one executor-independent control core.
pub struct ConnectedControl<'resources, M: RawMutex, const CAPACITY: usize> {
    receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
    core: ConnectedControlCore,
    rx_block_ack: ConnectedRxBlockAck<'resources>,
    rx_reorder_commands: Option<RxReorderCommandSender<'resources, M>>,
    security: Option<ConnectedWpa2Security>,
    deferred_control_event: Option<ConnectedRxControlEvent>,
    power: Option<&'resources StationPowerLink<M>>,
    hardware_beacon_monitor: Option<StationHardwareBeaconMonitorEpoch>,
    unverified_management: u32,
    /// Consecutive steps that asked the scheduler to run control again.
    #[cfg(feature = "diagnostics")]
    more_streak: u32,
}

/// Consecutive `More` steps after which control is taken to spin: the
/// scheduler runs control again without awaiting, so a step that never
/// consumes its input starves every task of its executor.
#[cfg(feature = "diagnostics")]
const CONTROL_SPIN_STEPS: u32 = 200_000;

/// Robust management input one association dropped.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ConnectedManagementProtectionDrops {
    /// Frames that did not open under the temporal key or verify under the
    /// IGTK.
    pub unverified: u32,
    /// Frames too long to copy or arriving at a full lane.
    pub mailbox: u32,
}

const fn control_event_requires_active(event: ConnectedRxControlEvent) -> bool {
    matches!(
        event,
        ConnectedRxControlEvent::BlockAck(_)
            | ConnectedRxControlEvent::IndividualTwt(_)
            | ConnectedRxControlEvent::UnprotectedDisconnect(_)
            | ConnectedRxControlEvent::SaQuery(_)
    )
}

const fn he_control_event_requires_active(event: ConnectedRxControlEvent) -> bool {
    matches!(
        event,
        ConnectedRxControlEvent::Trigger {
            schedule: Ok(_),
            ..
        } | ConnectedRxControlEvent::Ndpa {
            addressed_to_station: true,
            ..
        }
    )
}

enum ConnectedRxBlockAck<'resources> {
    Local(StaApRxBlockAck),
    Shared(&'resources StaApRxBlockAck),
}

impl ConnectedRxBlockAck<'_> {
    const fn sessions(&self) -> &StaApRxBlockAck {
        match self {
            Self::Local(sessions) => sessions,
            Self::Shared(sessions) => sessions,
        }
    }
}

impl<'resources, M: RawMutex, const CAPACITY: usize> ConnectedControl<'resources, M, CAPACITY> {
    /// The owner of the station TSF writes of this association.
    pub const fn station_tsf(&self) -> &oer_esp32s31_ieee80211::station_tsf::StationTsf {
        self.core.station_tsf()
    }

    pub fn new(
        receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
        peer: [u8; 6],
        he_enabled: bool,
        tx_block_ack: oer_ieee80211_mac::block_ack::TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS>,
        tsf_epoch: u32,
    ) -> Self {
        Self {
            receiver,
            core: ConnectedControlCore::new(peer, he_enabled, tx_block_ack, tsf_epoch),
            rx_block_ack: ConnectedRxBlockAck::Local(StaApRxBlockAck::new()),
            rx_reorder_commands: None,
            security: None,
            deferred_control_event: None,
            power: None,
            hardware_beacon_monitor: None,
            unverified_management: 0,
            #[cfg(feature = "diagnostics")]
            more_streak: 0,
        }
    }

    pub fn new_shared(
        receiver: ConnectedControlReceiver<'resources, M, CAPACITY>,
        peer: [u8; 6],
        he_enabled: bool,
        tx_block_ack: oer_ieee80211_mac::block_ack::TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS>,
        rx_block_ack: &'resources StaApRxBlockAck,
        tsf_epoch: u32,
    ) -> Self {
        Self {
            receiver,
            core: ConnectedControlCore::new(peer, he_enabled, tx_block_ack, tsf_epoch),
            rx_block_ack: ConnectedRxBlockAck::Shared(rx_block_ack),
            rx_reorder_commands: None,
            security: None,
            deferred_control_event: None,
            power: None,
            hardware_beacon_monitor: None,
            unverified_management: 0,
            #[cfg(feature = "diagnostics")]
            more_streak: 0,
        }
    }

    /// Install the association's WPA2 security. When it protects
    /// management frames, `random` draws the first transaction identifier of
    /// every SA Query procedure.
    pub fn install_wpa2_security(
        &mut self,
        mut security: ConnectedWpa2Security,
        random: fn() -> u32,
    ) -> Result<(), ConnectedWpa2Security> {
        if self.security.is_some() {
            return Err(security);
        }
        if security.management_protection().is_some() {
            self.core.enable_management_protection(random);
        }
        self.security = Some(security);
        Ok(())
    }

    pub fn take_wpa2_security(&mut self) -> Option<ConnectedWpa2Security> {
        self.security.take()
    }

    pub const fn wpa2_security(&self) -> Option<&ConnectedWpa2Security> {
        self.security.as_ref()
    }

    pub fn set_rx_reorder_commands(&mut self, commands: RxReorderCommandSender<'resources, M>) {
        self.rx_reorder_commands = Some(commands);
    }

    pub fn set_rx_block_ack_maximum_window(
        &mut self,
        maximum_window: u16,
    ) -> Result<(), oer_esp32s31_ieee80211_mac::rx::ampdu::RxBlockAckSessionsError> {
        match &mut self.rx_block_ack {
            ConnectedRxBlockAck::Local(sessions) => {
                *sessions = StaApRxBlockAck::with_maximum_window(maximum_window)?;
            }
            ConnectedRxBlockAck::Shared(sessions) => {
                if sessions.maximum_window() != maximum_window {
                    return Err(
                        oer_esp32s31_ieee80211_mac::rx::ampdu::RxBlockAckSessionsError::InvalidWindow(
                            maximum_window,
                        ),
                    );
                }
            }
        }
        Ok(())
    }

    pub fn enable_beacon_loss(&mut self, config: StaBeaconLossConfig) {
        self.core.enable_beacon_loss(config);
    }

    /// Bind one non-clone automatic-monitor admission owner to this connected
    /// association. The portable software deadline remains authoritative.
    pub fn enable_hardware_beacon_monitor_frontier(
        &mut self,
        binding: StationBeaconMonitorBinding,
        policy: StaBeaconLossConfig,
    ) -> Result<(), StationHardwareBeaconMonitorEpoch> {
        let epoch = StationHardwareBeaconMonitorEpoch::new(binding, policy);
        if self.hardware_beacon_monitor.is_some() {
            return Err(epoch);
        }
        self.hardware_beacon_monitor = Some(epoch);
        Ok(())
    }

    /// Last one-shot hardware admission result for this association.
    pub const fn hardware_beacon_monitor_frontier(
        &self,
    ) -> Option<StationHardwareBeaconMonitorFrontier> {
        match self.hardware_beacon_monitor.as_ref() {
            Some(epoch) => epoch.frontier(),
            None => None,
        }
    }

    pub const fn hardware_beacon_monitor_binding(&self) -> Option<StationBeaconMonitorBinding> {
        match self.hardware_beacon_monitor.as_ref() {
            Some(epoch) => Some(epoch.binding()),
            None => None,
        }
    }

    /// Start power management for this association over its agent link.
    pub fn enable_power_management(
        &mut self,
        sleep_type: SleepType,
        join_beacon: JoinBeacon,
        binding: StationPowerBinding<'resources, M>,
    ) {
        let coex = binding
            .link
            .coex()
            .expect("a bound power link reads its coexistence state");
        self.core
            .enable_power_management(sleep_type, join_beacon, coex);
        self.power = Some(binding.link);
    }

    pub fn enable_individual_twt_requester(&mut self, config: IndividualTwtRequesterConfig) {
        self.core.enable_individual_twt_requester(config);
    }

    /// Evaluate the production FTM frontier without publishing a frame.
    pub fn evaluate_ftm_request_frontier(
        &self,
        config: FtmRequesterConfig,
        now: oer_time::Instant,
    ) -> Result<ConnectedFtmRequestFrontier, ConnectedControlError> {
        self.core.evaluate_ftm_request_frontier(config, now)
    }

    /// Report the reviewed FTM source frontier without touching MMIO.
    pub const fn ftm_hardware_frontier(&self) -> StationFtmHardwareFrontier {
        station_ftm_hardware_frontier()
    }

    pub fn queue_individual_twt_setup(
        &mut self,
        proposal: IndividualTwtProposal,
        now: oer_time::Instant,
    ) -> Result<(), ConnectedControlError> {
        self.core.queue_individual_twt_setup(proposal, now)
    }

    pub fn queue_individual_twt_teardown<H: ConnectedControlHardware>(
        &mut self,
        hardware: &mut H,
        flow_id: IndividualTwtFlowId,
        now: oer_time::Instant,
    ) -> Result<(), ConnectedControlError> {
        self.core
            .queue_individual_twt_teardown(hardware, flow_id, now)
    }

    pub fn set_he_trigger_based(
        &mut self,
        config: Option<oer_esp32s31_ieee80211_mac::tx::HeTriggerBasedTxConfig>,
    ) {
        self.core.set_he_trigger_based(config);
    }

    pub fn queue_initial_tx_block_ack(&mut self, attempt_limit: u8) {
        self.core.queue_initial_tx_block_ack(attempt_limit);
    }

    pub const fn rx_block_ack(&self) -> &StaApRxBlockAck {
        self.rx_block_ack.sessions()
    }

    pub const fn tx_block_ack(
        &self,
    ) -> &oer_ieee80211_mac::block_ack::TxBlockAckOriginator<TX_BLOCK_ACK_MAX_TIDS> {
        self.core.tx_block_ack()
    }

    pub const fn last_event(
        &self,
    ) -> Option<oer_esp32s31_ieee80211_sta::connected_rx::ConnectedRxControlEvent> {
        self.core.last_event()
    }

    pub const fn last_tx_failure(&self) -> Option<ConnectedControlTxFailure> {
        self.core.last_tx_failure()
    }

    pub const fn tx_in_flight(&self) -> bool {
        self.core.tx_in_flight()
    }

    pub const fn last_expired_tid(&self) -> Option<u8> {
        self.core.last_expired_tid()
    }

    pub const fn stale_tx_block_ack_responses(&self) -> u32 {
        self.core.stale_tx_block_ack_responses()
    }

    pub const fn last_stale_tx_block_ack_token(&self) -> Option<u8> {
        self.core.last_stale_tx_block_ack_token()
    }

    pub const fn he_control_runtime_evidence(&self) -> ConnectedHeControlRuntimeEvidence {
        self.core.he_control_runtime_evidence()
    }

    pub const fn individual_twt_runtime_evidence(&self) -> ConnectedIndividualTwtRuntimeEvidence {
        self.core.individual_twt_runtime_evidence()
    }

    pub const fn individual_twt_requester(&self) -> Option<&IndividualTwtRequester> {
        self.core.individual_twt_requester()
    }

    /// The next individual-TWT wake at the station TSF now, with the TSF
    /// generation it belongs to (see [`StationTwtWakePlan`]).
    pub fn individual_twt_wake_plan<H: ConnectedControlHardware>(
        &self,
        hardware: &mut H,
        wake_guard: oer_ieee80211_mac::tsf::TsfDuration,
    ) -> Result<Option<StationTwtWakePlan>, ConnectedControlError> {
        self.core.individual_twt_wake_plan(hardware, wake_guard)
    }

    pub fn dropped_he_observations(&self) -> u32 {
        self.receiver.dropped_he_observations()
    }

    pub const fn beacon_monitor(&self) -> Option<&StaBeaconMonitor> {
        self.core.beacon_monitor()
    }

    pub const fn beacon_lost(&self) -> bool {
        self.core.beacon_lost()
    }

    pub const fn power_management(&self) -> &ModemSleep {
        self.core.power_management()
    }

    pub fn shutdown<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
    ) -> Result<ConnectedControlShutdown, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        self.receiver.set_power_save_data_armed(false);
        let shutdown = self
            .rx_block_ack
            .sessions()
            .with_sessions(|rx_block_ack| self.core.shutdown(hardware, tx, rx_block_ack))?;
        // The stop's air releases and RF wake reach the agent, which performs
        // them before the association's radio resources return.
        self.forward_power_commands()?;
        let hardware_beacon_monitor = self
            .hardware_beacon_monitor
            .take()
            .map(StationHardwareBeaconMonitorEpoch::stop);
        let mut discarded_events = 0_u8;
        while self.receiver.try_receive().is_some() {
            discarded_events = discarded_events.saturating_add(1);
        }
        while self.receiver.try_receive_security().is_some() {
            discarded_events = discarded_events.saturating_add(1);
        }
        while self.receiver.try_receive_management().is_some() {
            discarded_events = discarded_events.saturating_add(1);
        }
        if self.deferred_control_event.take().is_some() {
            discarded_events = discarded_events.saturating_add(1);
        }
        Ok(ConnectedControlShutdown {
            rx_block_ack_agreements: shutdown.rx_block_ack_agreements,
            tx_block_ack_sessions: shutdown.tx_block_ack_sessions,
            discarded_events,
            in_flight: shutdown.in_flight,
            hardware_beacon_monitor,
        })
    }

    /// Whether a controlled stop must still run control: a transmission
    /// completes, or the leaving station's Deauthentication is unsent.
    pub const fn leave_pending(&self) -> bool {
        self.core.tx_in_flight() || self.core.leave_pending()
    }

    pub fn has_immediate_work(&self) -> bool {
        self.deferred_control_event.is_some()
            || self.receiver.overflowed()
            || self
                .security
                .as_ref()
                .is_some_and(ConnectedWpa2Security::tx_in_flight)
            || self.power.is_some_and(StationPowerLink::has_input)
            || self.core.has_immediate_work(self.control_event_ready())
    }

    /// Whether a received event can be consumed now. Events leading to a
    /// frame wait while power management holds the TX queues, and must not
    /// keep the service spinning meanwhile.
    fn control_event_ready(&self) -> bool {
        if self.power_holds_tx() {
            self.deferred_control_event.is_none() && !self.receiver.is_empty()
        } else {
            !self.receiver.is_empty()
        }
    }

    /// Whether frames other than power management's Null must wait: the
    /// station is outside its Wi-Fi slice, its RF sleeps, or the agent has
    /// not yet performed a command power management relies on.
    fn power_holds_tx(&self) -> bool {
        self.core.power_blocks_tx() || self.power.is_some_and(StationPowerLink::outstanding)
    }

    /// Whether a network or ESP-NOW frame may be published now.
    pub fn admits_frames(&self) -> bool {
        !self.power_holds_tx() && self.core.admits_network_tx()
    }

    /// Hand every queued power command to the agent.
    fn forward_power_commands(&mut self) -> Result<(), ConnectedControlError> {
        let Some(link) = self.power else {
            return Ok(());
        };
        while let Some(command) = self.core.take_power_command() {
            link.send(command)
                .map_err(|_| ConnectedControlError::PowerCommandOverflow)?;
        }
        Ok(())
    }

    /// Move the agent's inputs into the control core.
    fn collect_power_inputs(&mut self) -> Result<(), ConnectedControlError> {
        let Some(link) = self.power else {
            return Ok(());
        };
        if let Some(failure) = link.failure() {
            let _ = failure;
            return Err(ConnectedControlError::PowerAgentFailed);
        }
        if let Some(coex) = link.coex() {
            self.core.set_power_coex(coex);
        }
        if link.take_tbtt() {
            self.core.observe_tbtt();
        }
        if let Some(phase) = link.take_phase() {
            self.core.observe_coex_phase(phase);
        }
        if let Some(end) = link.take_preemption() {
            self.core.observe_preemption_end(end);
        }
        Ok(())
    }

    /// Earliest role-local control deadline. Reading it does not require the
    /// ordinary/A-MPDU publication capability.
    pub fn next_alarm_deadline(&self) -> Option<oer_time::Instant> {
        self.core.next_alarm_deadline()
    }

    /// Paired-runtime wait which deliberately owns no physical TX resource.
    /// `timer` reads the same monotonic time as the station's transmitter, so
    /// this preserves the standalone deadline epoch without lending DMA to a
    /// sleeping station role.
    pub async fn wait_ready_without_tx(&mut self, timer: &impl Timer) {
        if self.has_immediate_work() {
            return;
        }
        let power = self.power;
        let deadline = self.next_alarm_deadline();
        let receiver = &self.receiver;
        wait_control_input(receiver, power, async move {
            match deadline {
                Some(deadline) => timer.wait_until(deadline).await,
                None => core::future::pending().await,
            }
        })
        .await;
    }

    /// Wait without consuming the event that made control work ready.
    pub async fn wait_ready<'a, X>(&'a mut self, tx: &'a mut X)
    where
        X: ConnectedControlTx + oer_time::Timer + 'a,
    {
        if self.has_immediate_work() {
            return;
        }
        let power = self.power;
        let deadline = self.core.next_alarm_deadline();
        let receiver = &self.receiver;
        wait_control_input(receiver, power, async move {
            match deadline {
                Some(deadline) => Timer::wait_until(&*tx, deadline).await,
                None => core::future::pending().await,
            }
        })
        .await;
    }

    /// Turn one robust management input into the control event it carries:
    /// open or verify a protected frame under the association's keys, or
    /// pass an unprotected disconnect on to the SA Query procedure.
    fn open_management(
        &mut self,
        input: ConnectedManagementInput,
    ) -> Option<ConnectedRxControlEvent> {
        match input {
            ConnectedManagementInput::UnprotectedDisconnect(disconnect) => {
                Some(ConnectedRxControlEvent::UnprotectedDisconnect(disconnect))
            }
            ConnectedManagementInput::Protected(mut frame) => {
                let protection = self
                    .security
                    .as_mut()
                    .and_then(ConnectedWpa2Security::management_protection)?;
                match protection.receive(&mut frame) {
                    Ok(event) => event,
                    Err(_) => {
                        self.unverified_management = self.unverified_management.saturating_add(1);
                        None
                    }
                }
            }
        }
    }

    /// Robust management frames that failed to open or verify, and robust
    /// management input the mailbox dropped.
    pub fn management_protection_drops(&self) -> ConnectedManagementProtectionDrops {
        ConnectedManagementProtectionDrops {
            unverified: self.unverified_management,
            mailbox: self.receiver.dropped_management(),
        }
    }

    fn service_core_step<H, X>(
        &mut self,
        hardware: &mut H,
        tx: &mut X,
        event: Option<ConnectedRxControlEvent>,
        control_event_pending: bool,
        context: DatapathControlContext,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware,
        X: ConnectedControlTx,
    {
        if let Some(event) = &event {
            oer_trace::emit(&ControlMailbox {
                event: ControlEventKind::from(event),
                op: MailboxOp::Applied,
            });
        }
        let mut reorder = EmbassyReorderSink {
            sender: self.rx_reorder_commands.as_ref(),
        };
        let result = self.rx_block_ack.sessions().with_sessions(|rx_block_ack| {
            self.core.service_step(
                ConnectedControlPorts {
                    hardware,
                    tx,
                    reorder: &mut reorder,
                    rx_block_ack,
                },
                event,
                control_event_pending,
                context,
            )
        });
        let exiting = matches!(&result, Ok(DatapathControlProgress::Exit(_)));
        self.receiver
            .set_power_save_data_armed(!exiting && self.core.wants_power_save_data());
        let result = result?;
        self.forward_power_commands()?;
        Ok(result)
    }

    pub fn service<'a, H, X>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
    ) -> impl Future<
        Output = Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>,
    > + 'a
    where
        H: ConnectedControlHardware + 'a,
        X: ConnectedControlTx + 'a,
    {
        self.service_with_context(hardware, tx, DatapathControlContext::IDLE)
    }

    pub async fn service_with_context<'a, H, X>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: DatapathControlContext,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware + 'a,
        X: ConnectedControlTx + 'a,
    {
        let progress = self.service_step(hardware, tx, context).await;
        if let Ok(DatapathControlProgress::Exit(reason)) = progress {
            oer_trace::emit(&ControlExit {
                reason: reason.into(),
            });
            oer_trace::freeze(
                <ControlExit as oer_trace::Event>::KIND,
                ControlExit::POST_TRIGGER_ENTRIES,
            );
        }
        #[cfg(feature = "diagnostics")]
        self.observe_spin(&progress, oer_time::Clock::now(&*tx));
        progress
    }

    #[cfg(feature = "diagnostics")]
    fn observe_spin(
        &mut self,
        progress: &Result<
            DatapathControlProgress<ConnectedDisconnectReason>,
            ConnectedControlError,
        >,
        now: oer_time::Instant,
    ) {
        if !matches!(progress, Ok(DatapathControlProgress::More)) {
            self.more_streak = 0;
            return;
        }
        self.more_streak += 1;
        if self.more_streak == CONTROL_SPIN_STEPS {
            panic!(
                "station control spins: now={} deferred={:?} mailbox_empty={} core_tx={} \
                 outstanding={} power={:?}",
                now.as_micros(),
                self.deferred_control_event,
                self.receiver.is_empty(),
                self.core.tx_in_flight(),
                self.power.is_some_and(StationPowerLink::outstanding),
                self.core.power_debug(),
            );
        }
    }

    async fn service_step<'a, H, X>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: DatapathControlContext,
    ) -> Result<DatapathControlProgress<ConnectedDisconnectReason>, ConnectedControlError>
    where
        H: ConnectedControlHardware + 'a,
        X: ConnectedControlTx + 'a,
    {
        if let Some(epoch) = self.hardware_beacon_monitor.as_mut()
            && epoch.frontier().is_none()
        {
            let snapshot = hardware.station_beacon_monitor_readback();
            let _ = epoch.evaluate_once(snapshot);
        }
        self.collect_power_inputs()?;
        // The shared ordinary-TX completion always precedes newly queued
        // RX work. Security and the generic control core can never both
        // own that transaction.
        if let Some(security) = self.security.as_mut()
            && security.tx_in_flight()
        {
            return Ok(security.complete_tx(tx));
        }

        if self.core.tx_in_flight() {
            let queued_control_pending =
                self.deferred_control_event.is_some() || !self.receiver.is_empty();
            return self.service_core_step(hardware, tx, None, queued_control_pending, context);
        }

        // A controlled stop sends the leaving station's Deauthentication
        // before the epoch ends; queued RX work no longer matters.
        if context.stop_pending {
            return self.service_core_step(hardware, tx, None, false, context);
        }

        if self.receiver.overflowed() {
            return Ok(DatapathControlProgress::Exit(
                ConnectedDisconnectReason::ControlMailboxOverflow,
            ));
        }

        // A power input the core performs leaves queued RX work in the
        // mailbox for the next step, which the core then yields to it.
        if self.core.power_input_first(tx, context) {
            let queued_control_pending =
                self.deferred_control_event.is_some() || !self.receiver.is_empty();
            return self.service_core_step(hardware, tx, None, queued_control_pending, context);
        }

        // Security processing can publish EAPOL immediately, so its frames
        // wait while power management holds the TX queues. Beacons are
        // handled below meanwhile: they drive power management itself.
        let holds_tx = self.power_holds_tx();

        // A peer disconnect keeps terminal priority once TX ownership is
        // free. GTK rekey then precedes non-terminal BlockAck work.
        if let Some(event) = self.receiver.try_receive_terminal() {
            return self.service_core_step(
                hardware,
                tx,
                Some(event),
                !self.receiver.is_empty(),
                context,
            );
        }
        if !holds_tx && let Some(frame) = self.receiver.try_receive_security() {
            let Some(security) = self.security.as_mut() else {
                return Ok(DatapathControlProgress::Exit(
                    ConnectedDisconnectReason::GroupKeyHandshakeFailed,
                ));
            };
            return Ok(security.process(hardware, tx, frame).await);
        }
        if (!holds_tx || self.deferred_control_event.is_none())
            && let Some(input) = self.receiver.try_receive_management()
        {
            let Some(event) = self.open_management(input) else {
                return Ok(DatapathControlProgress::More);
            };
            if holds_tx && control_event_requires_active(event) {
                self.deferred_control_event = Some(event);
                return self.service_core_step(hardware, tx, None, false, context);
            }
            return self.service_core_step(
                hardware,
                tx,
                Some(event),
                !self.receiver.is_empty(),
                context,
            );
        }
        let deferred = if holds_tx {
            None
        } else {
            self.deferred_control_event.take()
        };
        let event = deferred
            .or_else(|| self.receiver.try_receive_power_save_data())
            .or_else(|| {
                if holds_tx && self.deferred_control_event.is_some() {
                    None
                } else {
                    self.receiver.try_receive_control()
                }
            })
            .or_else(|| self.receiver.try_receive_he_observation());
        if holds_tx
            && event.is_some_and(|event| {
                control_event_requires_active(event)
                    || (self.core.he_trigger_runtime_enabled()
                        && he_control_event_requires_active(event))
            })
        {
            self.deferred_control_event = event;
            return self.service_core_step(hardware, tx, None, false, context);
        }
        self.service_core_step(
            hardware,
            tx,
            event,
            self.deferred_control_event.is_some() || !self.receiver.is_empty(),
            context,
        )
    }
}

impl<'resources, M, H, X, const CAPACITY: usize> DatapathControlService<H, X>
    for ConnectedControl<'resources, M, CAPACITY>
where
    M: RawMutex,
    H: ConnectedControlHardware,
    X: ConnectedControlTx + oer_time::Timer,
{
    type Error = ConnectedControlError;
    type Exit = ConnectedDisconnectReason;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        ConnectedControl::service_with_context(self, hardware, tx, context)
    }

    fn ready(&self, tx: &X, now: oer_time::Instant) -> bool {
        self.has_immediate_work()
            || (self.core.power_management().is_started() && tx.has_network_tx_report())
            || self
                .next_alarm_deadline()
                .is_some_and(|deadline| deadline <= now)
    }

    fn required_before_network_tx(&self) -> bool {
        self.core.network_tx_needs_offer(DatapathControlContext {
            network_tx_pending: true,
            stop_pending: false,
        })
    }

    fn admits_network_tx(&self) -> bool {
        self.admits_frames()
    }

    fn required_before_stop(&self) -> bool {
        self.leave_pending()
    }

    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a {
        ConnectedControl::wait_ready(self, tx)
    }
}

/// Wait for a received event, a power input or `deadline`.
async fn wait_control_input<M: RawMutex, const CAPACITY: usize>(
    receiver: &ConnectedControlReceiver<'_, M, CAPACITY>,
    power: Option<&StationPowerLink<M>>,
    deadline: impl Future<Output = ()>,
) {
    let power = async move {
        match power {
            Some(link) => link.wait().await,
            None => core::future::pending().await,
        }
    };
    let _ = embassy_futures::select::select3(receiver.ready(), power, deadline).await;
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;

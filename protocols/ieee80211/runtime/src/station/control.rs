//! Application control, lossless callback events, and the neutral network end.

use super::contract::{Command, Notice};
use super::{ServiceCommand, ServiceOutput, ServiceOutputSink};
use core::{
    cell::Cell,
    fmt,
    future::Future,
    task::{Context, Poll},
};
use embassy_futures::select::{Either, select};
use oer_bridges_ieee80211_ethernet::qos::classify_wmm;
use oer_ieee80211_core_roles_sta::data::{EthernetFrame, TxMsdu};
use oer_memory::{WakingSpscReceiver, WakingSpscSender};
use oer_network_port::{LinkSnapshot, LinkState, NetDevice, TransmitError, TransmitRefused};
use oer_sync::PriorityWaker;
use oer_time::connected_tasks::{
    ConnectedTaskControlError, ConnectedTaskControlResources, ConnectedTaskController,
    ConnectedTaskEndpoint, ConnectedTaskGroup,
};

/// Latest product state. Events and command owners still reach the callback once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Status {
    pub link: LinkSnapshot,
    pub notice: Option<Notice>,
    pub stopped: bool,
}

/// One station's shared state. Keep it for the complete Runner/Control lifetime.
pub struct State {
    status: Cell<Status>,
    tasks: ConnectedTaskControlResources<()>,
    link_waker: PriorityWaker,
    policy: Cell<super::reconnect::Policy>,
    policy_waker: PriorityWaker,
}

impl State {
    pub const fn new() -> Self {
        Self {
            status: Cell::new(Status {
                link: LinkSnapshot::DOWN,
                notice: None,
                stopped: false,
            }),
            tasks: ConnectedTaskControlResources::new(),
            link_waker: PriorityWaker::new(),
            policy: Cell::new(super::reconnect::Policy::new(
                super::ReconnectPolicy::DISABLED,
            )),
            policy_waker: PriorityWaker::new(),
        }
    }
    pub(super) fn split(
        &self,
    ) -> Result<(ConnectedTaskController<'_, ()>, RunnerControl<'_>), ConnectedTaskControlError>
    {
        let (task, endpoint) = self.tasks.split()?;
        self.status.set(Status {
            link: LinkSnapshot::DOWN,
            notice: None,
            stopped: false,
        });
        Ok((
            task,
            RunnerControl {
                state: self,
                endpoint: Some(endpoint),
            },
        ))
    }
    pub(super) fn events<O>(&self, output: O) -> Events<'_, O> {
        Events {
            state: self,
            output,
        }
    }
    pub(super) fn epoch(&self, epoch: u64) {
        let mut status = self.status.get();
        status.link.epoch = epoch;
        self.status.set(status);
    }
}
impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// A context-local read-only projection of Control's existing product state.
#[derive(Clone, Copy)]
pub struct StatusReader<'c> {
    state: &'c State,
}
impl StatusReader<'_> {
    pub fn status(&self) -> Status {
        let mut status = self.state.status.get();
        status.stopped |= self.state.tasks.is_poisoned();
        status
    }
}

/// Why a command remains owned by its caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlError {
    QueueFull,
    Stopped,
}

#[derive(Debug)]
pub struct ControlRefused<K> {
    pub command: ServiceCommand<K>,
    pub reason: ControlError,
}

/// Construction refused without consuming the original command producer.
pub struct ControlCreateRefused<'c, K, const DEPTH: usize> {
    pub commands: WakingSpscSender<'c, ServiceCommand<K>, DEPTH>,
    pub reason: ConnectedTaskControlError,
}
impl<K, const DEPTH: usize> fmt::Debug for ControlCreateRefused<'_, K, DEPTH> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(f)
    }
}

/// Sole command producer and cooperative stop capability.
pub struct Control<'c, K, const DEPTH: usize> {
    commands: WakingSpscSender<'c, ServiceCommand<K>, DEPTH>,
    state: &'c State,
    task: ConnectedTaskController<'c, ()>,
}

/// The Runner's affine side of the control connection.
pub struct RunnerControl<'c> {
    pub(super) state: &'c State,
    endpoint: Option<ConnectedTaskEndpoint<'c, ()>>,
}

impl<'c> RunnerControl<'c> {
    pub(super) async fn wait_for_stop(&self) {
        match &self.endpoint {
            Some(endpoint) => endpoint.wait_for_stop().await,
            None => core::future::pending().await,
        }
    }
    pub(super) fn policy_deadline(&self) -> Option<oer_time::Instant> {
        self.state.policy.get().deadline()
    }
    pub(super) fn policy_ready(&self, now: oer_time::Instant) -> bool {
        self.state
            .policy
            .get()
            .ready(now, self.state.status.get().link.state == LinkState::Up)
    }
    pub(super) fn policy_register(&self, cx: &Context<'_>) {
        self.state.policy_waker.register(cx.waker());
    }
    pub(super) fn policy_command<K>(
        &self,
        now: oer_time::Instant,
    ) -> Result<Option<ServiceCommand<K>>, super::reconnect::DeadlineOverflow> {
        let mut policy = self.state.policy.get();
        let result = policy.command(now, self.state.status.get().link.state == LinkState::Up);
        self.state.policy.set(policy);
        result
    }
    pub(super) fn complete(&mut self) {
        let mut status = self.state.status.get();
        status.stopped = true;
        status.link.state = LinkState::Down;
        self.state.status.set(status);
        self.state.link_waker.wake();
        if let Some(endpoint) = self.endpoint.take() {
            // A duplicate completion refuses its unit owner and poisons reuse;
            // no radio owner is released through this notification.
            let _completion = endpoint.complete(());
        }
    }
}

impl<'c, K, const DEPTH: usize> Control<'c, K, DEPTH> {
    pub fn new(
        commands: WakingSpscSender<'c, ServiceCommand<K>, DEPTH>,
        state: &'c State,
    ) -> Result<(Self, RunnerControl<'c>), ControlCreateRefused<'c, K, DEPTH>> {
        match state.split() {
            Ok((task, runner)) => Ok((
                Self {
                    commands,
                    state,
                    task,
                },
                runner,
            )),
            Err(reason) => Err(ControlCreateRefused { commands, reason }),
        }
    }
    pub fn status_reader(&self) -> StatusReader<'c> {
        StatusReader { state: self.state }
    }
    pub fn status(&self) -> Status {
        self.status_reader().status()
    }
    /// All events, including affine refusals and command terminals, are
    /// delivered directly to this concrete recipient. There is no event queue
    /// whose overflow could discard an owner or coalesce a lifecycle notice.
    pub fn events<O>(&self, output: O) -> Events<'c, O> {
        self.state.events(output)
    }
    pub fn configure_reconnect(&mut self, policy: super::ReconnectPolicy) {
        self.state.policy.set(super::reconnect::Policy::new(policy));
        self.state.policy_waker.wake();
    }
    /// Disconnect once, then retire Runner after the link is down. Stop is
    /// independent of command queue capacity and never schedules a reconnect.
    pub fn stop_gracefully(&mut self) {
        let mut policy = self.state.policy.get();
        policy.stop();
        self.state.policy.set(policy);
        self.state.policy_waker.wake();
    }
    fn admitted(&self, command: Option<Command>) {
        if let Some(command) = command {
            let mut policy = self.state.policy.get();
            policy.admitted(command);
            self.state.policy.set(policy);
        }
    }
    pub fn try_command(&mut self, command: ServiceCommand<K>) -> Result<(), ControlRefused<K>> {
        if self.status().stopped || self.state.tasks.is_poisoned() {
            return Err(ControlRefused {
                command,
                reason: ControlError::Stopped,
            });
        }
        let kind = match &command {
            ServiceCommand::Station(request) => Some(request.command()),
            _ => None,
        };
        self.commands
            .try_send(command)
            .map_err(|error| ControlRefused {
                command: error.0,
                reason: ControlError::QueueFull,
            })?;
        self.admitted(kind);
        Ok(())
    }
    pub async fn command(&mut self, command: ServiceCommand<K>) -> Result<(), ControlRefused<K>> {
        if self.status().stopped || self.state.tasks.is_poisoned() {
            return Err(ControlRefused {
                command,
                reason: ControlError::Stopped,
            });
        }
        let kind = match &command {
            ServiceCommand::Station(request) => Some(request.command()),
            _ => None,
        };
        let result = match select(self.commands.ready(), self.task.wait_stopped()).await {
            Either::First(permit) => {
                permit.send(command);
                Ok(())
            }
            Either::Second(()) => Err(ControlRefused {
                command,
                reason: ControlError::Stopped,
            }),
        };
        if result.is_ok() {
            self.admitted(kind);
        }
        result
    }
    /// Consume the completed control epoch and recover its sole producer.
    /// A pending result retains the entire control, including partial returns.
    pub fn try_finish(self) -> Result<WakingSpscSender<'c, ServiceCommand<K>, DEPTH>, Self> {
        let Self {
            commands,
            state,
            task,
        } = self;
        match task.try_finish() {
            Ok(()) => Ok(commands),
            Err(task) => Err(Self {
                commands,
                state,
                task,
            }),
        }
    }
    pub async fn connect(&mut self, correlation: u64) -> Result<(), ControlRefused<K>> {
        self.command(ServiceCommand::Station(super::Request::new(
            correlation,
            Command::Connect,
        )))
        .await
    }
    pub async fn disconnect(&mut self, correlation: u64) -> Result<(), ControlRefused<K>> {
        self.command(ServiceCommand::Station(super::Request::new(
            correlation,
            Command::Disconnect,
        )))
        .await
    }
    pub async fn scan(&mut self, correlation: u64) -> Result<(), ControlRefused<K>> {
        self.command(ServiceCommand::Station(super::Request::new(
            correlation,
            Command::BackgroundScan,
        )))
        .await
    }
    pub fn stop(&mut self) {
        self.task.request_stop();
    }
    pub async fn wait_stopped(&mut self) {
        self.task.wait_stopped().await;
    }
}

pub struct Events<'c, O> {
    state: &'c State,
    output: O,
}
impl<K, O: ServiceOutputSink<K>> ServiceOutputSink<K> for Events<'_, O> {
    fn emit(&mut self, output: ServiceOutput<K>) {
        if let ServiceOutput::Notice(notice) = &output {
            let mut status = self.state.status.get();
            status.notice = Some(*notice);
            status.link.state = match notice {
                Notice::Connected { .. } => LinkState::Up,
                Notice::Disconnected(_) => LinkState::Down,
            };
            self.state.status.set(status);
            self.state.link_waker.wake();
        }
        let mut policy = self.state.policy.get();
        policy.observe(&output, self.state.status.get().link.state == LinkState::Up);
        self.state.policy.set(policy);
        self.state.policy_waker.wake();
        self.output.emit(output);
    }
}

/// The Wi-Fi implementation of the neutral Ethernet contract.
pub struct Device<'c, const TX: usize, const RX: usize> {
    transmit: WakingSpscSender<'c, TxMsdu, TX>,
    receive: WakingSpscReceiver<'c, EthernetFrame, RX>,
    state: &'c State,
    observed: LinkSnapshot,
}
impl<'c, const TX: usize, const RX: usize> Device<'c, TX, RX> {
    pub fn new(
        transmit: WakingSpscSender<'c, TxMsdu, TX>,
        receive: WakingSpscReceiver<'c, EthernetFrame, RX>,
        state: &'c State,
    ) -> Self {
        Self {
            transmit,
            receive,
            state,
            observed: LinkSnapshot::DOWN,
        }
    }
}
impl<const TX: usize, const RX: usize> NetDevice for Device<'_, TX, RX> {
    type RxFrame = EthernetFrame;
    fn link(&self) -> LinkSnapshot {
        self.state.status.get().link
    }
    fn poll_link(&mut self, cx: &mut Context<'_>) -> Poll<LinkSnapshot> {
        self.state.link_waker.register(cx.waker());
        let link = self.link();
        if link == self.observed {
            Poll::Pending
        } else {
            self.observed = link;
            Poll::Ready(link)
        }
    }
    fn receive_queue_len(&self) -> usize {
        self.receive.len()
    }
    fn poll_receive_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        core::pin::pin!(self.receive.ready()).poll(cx)
    }
    fn try_receive(&mut self) -> Option<Self::RxFrame> {
        self.receive.try_receive()
    }
    fn receive_with<R>(&mut self, use_frame: impl FnOnce(&Self::RxFrame) -> R) -> Option<R> {
        self.receive.try_receive_with(|frame| use_frame(frame))
    }
    fn poll_transmit_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.state.link_waker.register(cx.waker());
        if self.link().state == LinkState::Down {
            return Poll::Pending;
        }
        core::pin::pin!(self.transmit.ready()).poll(cx).map(|_| ())
    }
    fn try_transmit<F: oer_network_port::EthernetFrame>(
        &mut self,
        frame: F,
    ) -> Result<(), TransmitRefused<F>> {
        let result = if self.link().state == LinkState::Down {
            Err(TransmitError::LinkDown)
        } else {
            frame
                .view()
                .map_err(TransmitError::InvalidLength)
                .and_then(|view| {
                    // The frame requests a priority from its VLAN or IP
                    // marking; the station decides the final one.
                    let requested = classify_wmm(view.ether_type, view.payload);
                    TxMsdu::from_network(view, requested.user_priority)
                        .map_err(|_| {
                            TransmitError::InvalidLength(
                                oer_network_port::FrameLengthError::TooLong,
                            )
                        })
                        .and_then(|packet| {
                            self.transmit
                                .try_send(packet)
                                .map_err(|_| TransmitError::QueueFull)
                        })
                })
        };
        result.map_err(|reason| TransmitRefused { frame, reason })
    }
}

#[cfg(test)]
mod tests;

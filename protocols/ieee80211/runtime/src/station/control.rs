//! Application control, lossless callback events, and the neutral network end.

use super::Request;
use super::contract::{Command, Notice};
pub use operation::{Operation, OperationEnd, UnreadOperation};

mod operation;
use super::{AcrossHarts, Notifies, SameHart, ServiceCommand, ServiceOutput, ServiceOutputSink};
use super::{CopiedMark, RX_COPIED_DEPTH, Received};
use core::{
    cell::Cell,
    fmt,
    future::Future,
    task::{Context, Poll},
};
use embassy_futures::select::{Either, select};
use oer_portable_memory::{QueueNotify, WakingSpscReceiver, WakingSpscSender};
use oer_portable_sync::PriorityWaker;
use oer_portable_time::connected_tasks::{
    ConnectedTaskControlError, ConnectedTaskControlResources, ConnectedTaskController,
    ConnectedTaskEndpoint, ConnectedTaskGroup,
};
use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
use oer_protocols_ieee80211_core_roles_sta::machine::StaProfile;
use oer_protocols_network_port::{LinkSnapshot, LinkState, NetDevice};

/// Latest product state. Events and command owners still reach the callback once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Status {
    pub link: LinkSnapshot,
    pub notice: Option<Notice>,
    pub stopped: bool,
}

/// One station's shared state. Keep it for the complete lifecycle/Control
/// lifetime. `N` names how its link tells the network device.
pub struct State<N: Notifies = SameHart> {
    status: Cell<Status>,
    tasks: ConnectedTaskControlResources<()>,
    /// The link the network device reads, possibly on another hart.
    network: super::NetworkLink<N::NetworkWaker>,
    policy: Cell<super::reconnect::Policy>,
    policy_waker: PriorityWaker,
    operations: operation::Operations,
    /// The deadline the last stop request named.
    stop_deadline: Cell<Option<oer_portable_time::Instant>>,
    /// The profile of the one Connect in the queue, published in the same
    /// poll as that request; the lifecycle takes it with the request.
    profile: Cell<Option<StaProfile>>,
}

impl State {
    pub const fn new() -> Self {
        Self::with_network(super::NetworkLink::new())
    }
}

impl<N: QueueNotify + Copy + 'static, S: QueueNotify + Copy + 'static> State<AcrossHarts<N, S>> {
    /// A state whose network device runs on another hart: `link` raises
    /// that device's executor signal when the link changes.
    pub const fn across_harts(link: N) -> Self {
        Self::with_network(super::NetworkLink::across_harts(link))
    }
}

impl<N: Notifies> State<N> {
    const fn with_network(network: super::NetworkLink<N::NetworkWaker>) -> Self {
        Self {
            status: Cell::new(Status {
                link: LinkSnapshot::DOWN,
                notice: None,
                stopped: false,
            }),
            tasks: ConnectedTaskControlResources::new(),
            network,
            policy: Cell::new(super::reconnect::Policy::new(
                super::ReconnectPolicy::DISABLED,
            )),
            policy_waker: PriorityWaker::new(),
            operations: operation::Operations::new(),
            stop_deadline: Cell::new(None),
            profile: Cell::new(None),
        }
    }

    /// The link the network device reads.
    pub fn network_link(&self) -> &super::NetworkLink<N::NetworkWaker> {
        &self.network
    }

    /// Set the status, publishing its link to the network device.
    fn set_status(&self, status: Status) {
        self.status.set(status);
        self.network.publish(status.link);
    }
    /// Publish `link` as the station's, as its service does.
    #[cfg(test)]
    pub(crate) fn set_link(&self, link: LinkSnapshot) {
        let mut status = self.status.get();
        status.link = link;
        self.set_status(status);
    }
    /// The service retired or its lifecycle was lost.
    fn ended(&self) -> bool {
        self.status.get().stopped || self.tasks.is_poisoned()
    }
    pub(super) fn split(
        &self,
    ) -> Result<(ConnectedTaskController<'_, ()>, RunnerControl<'_, N>), ControlCreateError> {
        self.operations
            .check_epoch()
            .map_err(ControlCreateError::Unread)?;
        let (task, endpoint) = self.tasks.split().map_err(ControlCreateError::Task)?;
        // A new epoch's requests never wait for an ended epoch's terminals;
        // an operation of the ended epoch reads its retired result.
        self.operations.begin_epoch();
        // An ended epoch's unserved profile erases itself here.
        drop(self.profile.take());
        // An ended epoch's stop ended with it: the new epoch's policy keeps
        // only the configuration, and no stop deadline is pending.
        self.policy.set(self.policy.get().next_epoch());
        self.stop_deadline.set(None);
        self.set_status(Status {
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
    pub(super) fn events<O>(&self, output: O) -> Events<'_, O, N> {
        Events {
            state: self,
            output,
        }
    }
    pub(crate) fn epoch(&self, epoch: u64) {
        let mut status = self.status.get();
        status.link.epoch = epoch;
        self.set_status(status);
    }
}
impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

/// A context-local read-only projection of Control's existing product state.
pub struct StatusReader<'c, N: Notifies = SameHart> {
    state: &'c State<N>,
}
impl<N: Notifies> Clone for StatusReader<'_, N> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<N: Notifies> Copy for StatusReader<'_, N> {}
impl<N: Notifies> StatusReader<'_, N> {
    pub fn status(&self) -> Status {
        let mut status = self.state.status.get();
        status.stopped |= self.state.tasks.is_poisoned();
        status
    }
}

/// A command a control producer may send itself. Station requests are
/// admitted only through [`Control::connect`], [`Control::disconnect`] and
/// [`Control::scan`], whose permission they hold until their terminal.
#[derive(Debug)]
pub enum ControlCommand<K> {
    /// A fact of the access point beside the station.
    AccessPoint(super::contract::AccessPointFact<K>),
    /// The access point beside the station is wanted.
    WantAccessPoint,
    /// Stop: the lifecycle stops the station and the radio, and the actor
    /// retires its installation.
    Stop,
}
impl<K> From<ControlCommand<K>> for ServiceCommand<K> {
    fn from(command: ControlCommand<K>) -> Self {
        match command {
            ControlCommand::AccessPoint(fact) => Self::AccessPoint(fact),
            ControlCommand::WantAccessPoint => Self::WantAccessPoint,
            ControlCommand::Stop => Self::Stop,
        }
    }
}

/// Why a command remains owned by its caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlError {
    QueueFull,
    Stopped,
}

#[derive(Debug)]
pub struct ControlRefused<C> {
    pub command: C,
    pub reason: ControlError,
}

/// Why a new control epoch was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlCreateError {
    /// The task connection refused the split.
    Task(ConnectedTaskControlError),
    /// An earlier epoch's operation has not read its retired result, and
    /// the ending epoch's operation of the same kind would need its place.
    Unread(UnreadOperation),
}

/// Construction refused without consuming the original command producer.
pub struct ControlCreateRefused<'c, K, const DEPTH: usize> {
    pub commands: WakingSpscSender<'c, ServiceCommand<K>, DEPTH>,
    pub reason: ControlCreateError,
}
impl<K, const DEPTH: usize> fmt::Debug for ControlCreateRefused<'_, K, DEPTH> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.reason.fmt(f)
    }
}

/// Sole command producer and cooperative stop capability.
pub struct Control<'c, K, const DEPTH: usize, N: Notifies = SameHart> {
    commands: WakingSpscSender<'c, ServiceCommand<K>, DEPTH>,
    state: &'c State<N>,
    task: ConnectedTaskController<'c, ()>,
}

/// The lifecycle's affine side of the control connection.
pub struct RunnerControl<'c, N: Notifies = SameHart> {
    pub(crate) state: &'c State<N>,
    endpoint: Option<ConnectedTaskEndpoint<'c, ()>>,
}

impl<'c, N: Notifies> RunnerControl<'c, N> {
    /// Publish `link`, as the lifecycle does, for a host composition that
    /// plays the lifecycle itself.
    #[cfg(feature = "test-support")]
    pub fn publish_link_for_test(&self, link: oer_protocols_network_port::LinkSnapshot) {
        self.state.set_status(Status {
            link,
            ..self.state.status.get()
        });
    }

    /// The deadline the stop request named, once one did.
    pub(crate) fn stop_deadline(&self) -> Option<oer_portable_time::Instant> {
        self.state.stop_deadline.get()
    }
    /// The profile published with the Connect just received, if its
    /// producer named one; a policy reconnect has none.
    pub(crate) fn take_profile(&self) -> Option<StaProfile> {
        self.state.profile.take()
    }
    /// Keep a retired station's network for the next installation's first
    /// Connect, a policy reconnect's included. A profile an application
    /// connect published meanwhile is newer and stays.
    pub(crate) fn keep_profile(&self, retired: Option<StaProfile>) {
        let published = self.state.profile.take();
        self.state.profile.set(published.or(retired));
    }
    pub(crate) async fn wait_for_stop(&self) {
        match &self.endpoint {
            Some(endpoint) => endpoint.wait_for_stop().await,
            None => core::future::pending().await,
        }
    }
    fn linked(&self) -> bool {
        self.state.status.get().link.state == LinkState::Up
    }
    /// Whether the policy's next station request can take its permission;
    /// while another request holds it, the policy waits for its release.
    fn policy_admissible(&self) -> bool {
        self.state
            .policy
            .get()
            .wanted(self.linked())
            .is_none_or(|command| self.state.operations.is_free(command))
    }
    pub(crate) fn policy_deadline(&self) -> Option<oer_portable_time::Instant> {
        if !self.policy_admissible() {
            return None;
        }
        self.state.policy.get().deadline()
    }
    pub(crate) fn policy_register(&self, cx: &Context<'_>) {
        self.state.policy_waker.register(cx.waker());
        self.state.operations.register_released(cx);
    }
    /// Whether the policy has a command at `now`, looked at after
    /// [`Self::policy_register`]: a wake of the policy that came before the
    /// registration found no waker.
    pub(crate) fn policy_pending(&self, now: oer_portable_time::Instant) -> bool {
        self.policy_admissible() && self.state.policy.get().pending(now, self.linked())
    }
    /// The policy's next command; a station request takes its kind's
    /// permission, the one admission it shares with operations.
    pub(crate) fn policy_command<K>(
        &self,
        now: oer_portable_time::Instant,
    ) -> Result<Option<ServiceCommand<K>>, super::reconnect::DeadlineOverflow> {
        if !self.policy_admissible() {
            return Ok(None);
        }
        let mut policy = self.state.policy.get();
        let result = policy.command(now, self.linked());
        if let Ok(Some(ServiceCommand::Station(request))) = &result
            && !self
                .state
                .operations
                .admit_unobserved(request.command(), request.correlation())
        {
            // Checked admissible above, in the same poll: unreachable, and
            // the policy's state stays as it was.
            return Ok(None);
        }
        self.state.policy.set(policy);
        result
    }
    pub(crate) fn complete(&mut self) {
        let mut status = self.state.status.get();
        status.stopped = true;
        status.link.state = LinkState::Down;
        self.state.set_status(status);
        self.state.operations.service_ended();
        drop(self.state.profile.take());
        if let Some(endpoint) = self.endpoint.take() {
            // A duplicate completion refuses its unit owner and poisons reuse;
            // no radio owner is released through this notification.
            let _completion = endpoint.complete(());
        }
    }
}

impl<N: Notifies> Drop for RunnerControl<'_, N> {
    fn drop(&mut self) {
        // A lost lifecycle poisons the epoch; its waiting operations read that.
        self.state.operations.service_ended();
    }
}

impl<'c, K, const DEPTH: usize, N: Notifies> Control<'c, K, DEPTH, N> {
    pub fn new(
        commands: WakingSpscSender<'c, ServiceCommand<K>, DEPTH>,
        state: &'c State<N>,
    ) -> Result<(Self, RunnerControl<'c, N>), ControlCreateRefused<'c, K, DEPTH>> {
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
    pub fn status_reader(&self) -> StatusReader<'c, N> {
        StatusReader { state: self.state }
    }
    pub fn status(&self) -> Status {
        self.status_reader().status()
    }
    /// All events, including affine refusals and command terminals, are
    /// delivered directly to this concrete recipient. There is no event queue
    /// whose overflow could discard an owner or coalesce a lifecycle notice.
    pub fn events<O>(&self, output: O) -> Events<'c, O, N> {
        self.state.events(output)
    }
    /// Install a fresh reconnect policy; it becomes active at the next
    /// admitted connect, which names the network it reconnects to.
    pub fn configure_reconnect(&mut self, policy: super::ReconnectPolicy) {
        self.state.policy.set(super::reconnect::Policy::new(policy));
        self.state.policy_waker.wake();
    }
    /// Disconnect once, then retire the lifecycle after the link is down. Stop is
    /// independent of command queue capacity and never schedules a reconnect.
    /// Disconnect, then stop; the network must return the station's lent
    /// receive buffers by `deadline` ([`Self::stop`]).
    pub fn stop_gracefully(&mut self, deadline: oer_portable_time::Instant) {
        self.state.stop_deadline.set(Some(deadline));
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
    pub fn try_command(
        &mut self,
        command: ControlCommand<K>,
    ) -> Result<(), ControlRefused<ControlCommand<K>>> {
        if self.status().stopped || self.state.tasks.is_poisoned() {
            return Err(ControlRefused {
                command,
                reason: ControlError::Stopped,
            });
        }
        match self.commands.try_reserve() {
            Some(permit) => {
                permit.send(command.into());
                Ok(())
            }
            None => Err(ControlRefused {
                command,
                reason: ControlError::QueueFull,
            }),
        }
    }
    pub async fn command(
        &mut self,
        command: ControlCommand<K>,
    ) -> Result<(), ControlRefused<ControlCommand<K>>> {
        self.send(command).await
    }
    /// Send one command once the queue has room; an admitted station
    /// request updates the policy's intent. The command stays the caller's
    /// until it is published.
    async fn send<C: Into<ServiceCommand<K>>>(
        &mut self,
        command: C,
    ) -> Result<(), ControlRefused<C>> {
        self.send_with(command, None).await
    }
    /// [`Self::send`], publishing `profile` in the same poll as the command,
    /// so a cancelled send leaves no profile behind. A refused or cancelled
    /// send drops the profile, whose secrets erase themselves.
    async fn send_with<C: Into<ServiceCommand<K>>>(
        &mut self,
        command: C,
        profile: Option<StaProfile>,
    ) -> Result<(), ControlRefused<C>> {
        if self.status().stopped || self.state.tasks.is_poisoned() {
            return Err(ControlRefused {
                command,
                reason: ControlError::Stopped,
            });
        }
        match select(self.commands.ready(), self.task.wait_stopped()).await {
            Either::First(permit) => {
                let command = command.into();
                let kind = match &command {
                    ServiceCommand::Station(request) => Some(request.command()),
                    _ => None,
                };
                if profile.is_some() {
                    self.state.profile.set(profile);
                }
                permit.send(command);
                self.admitted(kind);
                Ok(())
            }
            Either::Second(()) => Err(ControlRefused {
                command,
                reason: ControlError::Stopped,
            }),
        }
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
    /// Admit a connect to the network `profile` names once the previous
    /// connect's terminal returned its permission; the operation resolves to
    /// this request's terminal. The station takes the profile only while
    /// idle and keeps it for later reconnections; a refused, stopped or
    /// cancelled connect drops it, erasing its secrets.
    pub async fn connect(&mut self, profile: StaProfile) -> Result<Operation<'c, N>, ControlError> {
        self.operation(Command::Connect, Some(profile)).await
    }
    /// Admit a disconnect request, as [`Self::connect`].
    pub async fn disconnect(&mut self) -> Result<Operation<'c, N>, ControlError> {
        self.operation(Command::Disconnect, None).await
    }
    /// Admit a background scan request, as [`Self::connect`].
    pub async fn scan(&mut self) -> Result<Operation<'c, N>, ControlError> {
        self.operation(Command::BackgroundScan, None).await
    }
    async fn operation(
        &mut self,
        command: Command,
        profile: Option<StaProfile>,
    ) -> Result<Operation<'c, N>, ControlError> {
        if self.state.ended() {
            return Err(ControlError::Stopped);
        }
        let operations = &self.state.operations;
        let correlation = match select(
            core::future::poll_fn(|cx| operations.poll_free(command, cx)),
            self.task.wait_stopped(),
        )
        .await
        {
            Either::First(correlation) => correlation,
            Either::Second(()) => return Err(ControlError::Stopped),
        };
        self.send_with(
            ServiceCommand::Station(Request::new(correlation, command)),
            profile,
        )
        .await
        .map_err(|refused| refused.reason)?;
        // Sent and recorded in the same poll: no terminal can precede it.
        self.state.operations.admitted(command, correlation);
        Ok(Operation::new(self.state, command, correlation))
    }
    /// Stop the service: the network must return the station's lent
    /// receive buffers by `deadline`, or the port fails to retire.
    pub fn stop(&mut self, deadline: oer_portable_time::Instant) {
        self.state.stop_deadline.set(Some(deadline));
        self.task.request_stop();
    }
    pub async fn wait_stopped(&mut self) {
        self.task.wait_stopped().await;
    }
}

pub struct Events<'c, O, N: Notifies = SameHart> {
    state: &'c State<N>,
    output: O,
}
impl<K, O: ServiceOutputSink<K>, N: Notifies> ServiceOutputSink<K> for Events<'_, O, N> {
    fn emit(&mut self, output: ServiceOutput<K>) {
        if let ServiceOutput::Notice(notice) = &output {
            let mut status = self.state.status.get();
            status.notice = Some(*notice);
            status.link.state = match notice {
                Notice::Connected { .. } => LinkState::Up,
                Notice::Disconnected(_) => LinkState::Down,
            };
            self.state.set_status(status);
        }
        let mut policy = self.state.policy.get();
        policy.observe(&output, self.state.status.get().link.state == LinkState::Up);
        self.state.policy.set(policy);
        self.state.policy_waker.wake();
        self.state.operations.observe(&output);
        self.output.emit(output);
    }
}

/// The Wi-Fi implementation of the neutral Ethernet contract's link and
/// receive half. `L` is the port's lent receive buffer
/// ([`DatapathPort::LentRx`](crate::datapath::DatapathPort::LentRx)).
pub struct Device<'c, L, const RX: usize, N: Notifies = SameHart> {
    receive: WakingSpscReceiver<'c, Received<L>, RX, N::Network, N::Station>,
    copied: WakingSpscReceiver<'c, EthernetFrame, RX_COPIED_DEPTH, N::Network, N::Station>,
    link: &'c super::NetworkLink<N::NetworkWaker>,
    /// The link last observed.
    observed: LinkSnapshot,
}
impl<'c, L, const RX: usize, N: Notifies> Device<'c, L, RX, N> {
    /// The network end over the station's receive channels and `state`'s
    /// link.
    pub fn new(
        receive: WakingSpscReceiver<'c, Received<L>, RX, N::Network, N::Station>,
        copied: WakingSpscReceiver<'c, EthernetFrame, RX_COPIED_DEPTH, N::Network, N::Station>,
        state: &'c State<N>,
    ) -> Self {
        Self {
            receive,
            copied,
            link: state.network_link(),
            observed: LinkSnapshot::DOWN,
        }
    }
}
impl<L: oer_protocols_ieee80211_contracts_port::LentRxBuffer, const RX: usize, N: Notifies>
    NetDevice for Device<'_, L, RX, N>
{
    type RxFrame = Received<L>;
    fn link(&self) -> LinkSnapshot {
        self.link.snapshot()
    }
    /// Ready at a new publication of the link word.
    fn poll_link(&mut self, cx: &mut Context<'_>) -> Poll<LinkSnapshot> {
        self.link.register(cx);
        let published = self.link.snapshot();
        if published == self.observed {
            Poll::Pending
        } else {
            self.observed = published;
            Poll::Ready(published)
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
    fn read_copied<R>(
        &mut self,
        _: CopiedMark,
        read: impl FnOnce(oer_protocols_network_port::EthernetFrameView<'_>) -> R,
    ) -> Result<R, oer_protocols_network_port::FrameLengthError> {
        // Rare (an A-MSDU, or a buffer the backend kept): the copy leaves
        // its queue by value.
        let mut copy = self
            .copied
            .try_receive()
            .ok_or(oer_protocols_network_port::FrameLengthError::MissingCopy)?;
        oer_protocols_network_port::EthernetFrame::view(&mut copy).map(read)
    }
}

#[cfg(test)]
mod tests;

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
use oer_portable_memory::{
    QueueNotify, SlotReceiver, SlotSender, WakingSpscReceiver, WakingSpscSender,
};
use oer_portable_sync::PriorityWaker;
use oer_portable_time::connected_tasks::{
    ConnectedTaskControlError, ConnectedTaskControlResources, ConnectedTaskController,
    ConnectedTaskEndpoint, ConnectedTaskGroup,
};
use oer_protocols_bridges_ieee80211_ethernet::qos::classify_wmm;
use oer_protocols_ieee80211_core_roles_sta::data::{EthernetFrame, MAX_ETHERNET_PAYLOAD_LEN};
use oer_protocols_ieee80211_core_roles_sta::machine::StaProfile;
use oer_protocols_network_port::{
    LinkSnapshot, LinkState, NetDevice, TransmitError, TransmitRefused,
};

/// Latest product state. Events and command owners still reach the callback once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Status {
    pub link: LinkSnapshot,
    pub notice: Option<Notice>,
    pub stopped: bool,
}

/// One station's shared state. Keep it for the complete Runner/Control
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
    /// poll as that request; the runner takes it with the request.
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
    /// The service retired or its runner was lost.
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
    pub(super) fn epoch(&self, epoch: u64) {
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
    /// Stop: the service disables the port and returns its owner.
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

/// The Runner's affine side of the control connection.
pub struct RunnerControl<'c, N: Notifies = SameHart> {
    pub(super) state: &'c State<N>,
    endpoint: Option<ConnectedTaskEndpoint<'c, ()>>,
}

impl<'c, N: Notifies> RunnerControl<'c, N> {
    /// The deadline the stop request named, once one did.
    pub(super) fn stop_deadline(&self) -> Option<oer_portable_time::Instant> {
        self.state.stop_deadline.get()
    }
    /// The profile published with the Connect just received, if its
    /// producer named one; a policy reconnect has none.
    pub(super) fn take_profile(&self) -> Option<StaProfile> {
        self.state.profile.take()
    }
    pub(super) async fn wait_for_stop(&self) {
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
    pub(super) fn policy_deadline(&self) -> Option<oer_portable_time::Instant> {
        if !self.policy_admissible() {
            return None;
        }
        self.state.policy.get().deadline()
    }
    pub(super) fn policy_ready(&self, now: oer_portable_time::Instant) -> bool {
        self.policy_admissible() && self.state.policy.get().ready(now, self.linked())
    }
    pub(super) fn policy_register(&self, cx: &Context<'_>) {
        self.state.policy_waker.register(cx.waker());
        self.state.operations.register_released(cx);
    }
    /// The policy's next command; a station request takes its kind's
    /// permission, the one admission it shares with operations.
    pub(super) fn policy_command<K>(
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
    pub(super) fn complete(&mut self) {
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
        // A lost runner poisons the epoch; its waiting operations read that.
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
    /// Disconnect once, then retire Runner after the link is down. Stop is
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

/// The network's memory the station's data MSDUs come in, as the network
/// end holds it: free memory for its next frames, and the memory the
/// station gives back once an MSDU ended.
pub trait TxSupply {
    /// The memory the station holds for one MSDU
    /// ([`StationPort::TxMemory`](super::StationPort::TxMemory)).
    type Memory;
    /// Whether free memory takes a frame now.
    fn available(&self) -> bool;
    /// Write `frame` into free memory where the data machine reads it
    /// ([`MSDU_ETHERNET_OFFSET`](oer_protocols_ieee80211_core_roles_sta::data::MSDU_ETHERNET_OFFSET)),
    /// and hand the memory over for the station.
    ///
    /// # Errors
    ///
    /// No free memory ([`TransmitError::QueueFull`]), or a frame the memory
    /// does not hold ([`TransmitError::InvalidLength`]); the memory stays free.
    fn write(
        &mut self,
        frame: oer_protocols_network_port::EthernetFrameView<'_>,
    ) -> Result<Self::Memory, TransmitError>;
    /// Memory the station gave back: free again.
    fn give(&mut self, memory: Self::Memory);
}

/// The Wi-Fi implementation of the neutral Ethernet contract. `L` is the
/// backend's lent receive buffer ([`StationPort::LentRx`](super::StationPort::LentRx)),
/// `P` the network stack's packet a transmission hands over and `S` the
/// memory its frame goes into for the station.
pub struct Device<'c, L, P, S: TxSupply, const TX: usize, const RX: usize, N: Notifies = SameHart> {
    /// The frames for the station, each in the network's memory, where the
    /// station writes its MPDU header in place.
    transmit: SlotSender<'c, Option<super::Outgoing<S::Memory>>, TX, N::Station, N::Network>,
    /// The memory the station gave back, free for the next frames.
    returned: SlotReceiver<'c, Option<S::Memory>, TX, N::Network, N::Station>,
    supply: S,
    receive: WakingSpscReceiver<'c, Received<L>, RX, N::Network, N::Station>,
    copied: WakingSpscReceiver<'c, EthernetFrame, RX_COPIED_DEPTH, N::Network, N::Station>,
    link: &'c super::NetworkLink<N::NetworkWaker>,
    observed: LinkSnapshot,
    packet: core::marker::PhantomData<fn(P)>,
}
/// The network end over the station's channels and `state`'s link.
impl<'c, L, P, S: TxSupply, const TX: usize, const RX: usize, N: Notifies>
    Device<'c, L, P, S, TX, RX, N>
{
    pub fn new(
        transmit: SlotSender<'c, Option<super::Outgoing<S::Memory>>, TX, N::Station, N::Network>,
        returned: SlotReceiver<'c, Option<S::Memory>, TX, N::Network, N::Station>,
        supply: S,
        receive: WakingSpscReceiver<'c, Received<L>, RX, N::Network, N::Station>,
        copied: WakingSpscReceiver<'c, EthernetFrame, RX_COPIED_DEPTH, N::Network, N::Station>,
        state: &'c State<N>,
    ) -> Self {
        Self {
            transmit,
            returned,
            supply,
            receive,
            copied,
            link: state.network_link(),
            observed: LinkSnapshot::DOWN,
            packet: core::marker::PhantomData,
        }
    }
    /// Take back every memory the station returned, here on the network's
    /// executor, and register for the next return.
    fn reclaim(&mut self, cx: &mut Context<'_>) {
        loop {
            while let Ok(Some(memory)) = self.returned.try_receive_with(Option::take) {
                self.supply.give(memory);
            }
            if core::pin::pin!(self.returned.ready()).poll(cx).is_pending() {
                return;
            }
        }
    }
}
impl<
    L: oer_protocols_ieee80211_contracts_port::LentRxBuffer,
    P: oer_protocols_network_port::EthernetFrame,
    S: TxSupply,
    const TX: usize,
    const RX: usize,
    N: Notifies,
> NetDevice for Device<'_, L, P, S, TX, RX, N>
{
    type RxFrame = Received<L>;
    type TxFrame = P;
    fn link(&self) -> LinkSnapshot {
        self.link.snapshot()
    }
    fn poll_link(&mut self, cx: &mut Context<'_>) -> Poll<LinkSnapshot> {
        // The bridge polls the link on every turn: returned memory comes
        // back here even while the stack has nothing to send.
        self.reclaim(cx);
        self.link.register(cx);
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
    fn read_copied<R>(
        &mut self,
        _: CopiedMark,
        read: impl FnOnce(oer_protocols_network_port::EthernetFrameView<'_>) -> R,
    ) -> Result<R, oer_protocols_network_port::FrameLengthError> {
        // Rare (an A-MSDU, or a buffer the backend kept): the copy leaves
        // its queue by value.
        let copy = self
            .copied
            .try_receive()
            .ok_or(oer_protocols_network_port::FrameLengthError::MissingCopy)?;
        oer_protocols_network_port::EthernetFrame::view(&copy).map(read)
    }
    fn poll_transmit_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.link.register(cx);
        if self.link().state == LinkState::Down {
            return Poll::Pending;
        }
        // Free memory comes back with the station's returns.
        self.reclaim(cx);
        if !self.supply.available() {
            return Poll::Pending;
        }
        core::pin::pin!(self.transmit.ready()).poll(cx)
    }
    fn try_transmit(&mut self, frame: P) -> Result<(), TransmitRefused<P>> {
        let checked = if self.link().state == LinkState::Down {
            Err(TransmitError::LinkDown)
        } else if self.transmit.free_capacity() == 0 {
            Err(TransmitError::QueueFull)
        } else {
            frame
                .view()
                .map_err(TransmitError::InvalidLength)
                .and_then(|view| {
                    if view.payload.len() > MAX_ETHERNET_PAYLOAD_LEN {
                        return Err(TransmitError::InvalidLength(
                            oer_protocols_network_port::FrameLengthError::TooLong,
                        ));
                    }
                    // The frame requests a priority from its VLAN or IP
                    // marking; the station decides the final one.
                    let priority = classify_wmm(view.ether_type, view.payload).user_priority;
                    // Header and payload: at most 14 + MAX_ETHERNET_PAYLOAD_LEN.
                    let length = view
                        .payload
                        .len()
                        .checked_add(14)
                        .and_then(|length| u16::try_from(length).ok())
                        .ok_or(TransmitError::InvalidLength(
                            oer_protocols_network_port::FrameLengthError::TooLong,
                        ))?;
                    let memory = self.supply.write(view)?;
                    Ok(super::Outgoing {
                        memory,
                        length,
                        priority,
                    })
                })
        };
        let outgoing = match checked {
            Ok(outgoing) => outgoing,
            Err(reason) => return Err(TransmitRefused { frame, reason }),
        };
        // The frame is in the network's memory: the packet itself is done
        // here, on the network's executor.
        drop(frame);
        let mut outgoing = Some(outgoing);
        let _sent = self.transmit.try_send_with(|slot| {
            *slot = outgoing.take();
            Ok::<_, core::convert::Infallible>(())
        });
        if let Some(outgoing) = outgoing {
            // The room was checked above: a broken queue keeps the memory
            // quarantined rather than reused under the station.
            let _quarantined = core::mem::ManuallyDrop::new(outgoing);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

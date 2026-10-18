#![expect(
    clippy::result_large_err,
    reason = "mailbox preparation failures retain the concrete no-alloc resource owner"
)]

//! Bounded handoff from borrowed RX dispatch to the connected control owner.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_futures::select::{select, select6};
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender, TrySendError};
use oer_esp32s31_ieee80211_sta::connected::management_protection::ProtectedManagementFrame;
use oer_esp32s31_ieee80211_sta::connected_rx::{
    ConnectedRxControlEvent, ConnectedRxEvent, ConnectedRxSink,
};
use oer_ieee80211_mac::station::StaDisconnect;
use oer_ieee80211_rsn::{OwnedEapolFrame, RsnInterface};
use oer_ieee80211_trace::{ControlEventKind, ControlMailbox, MailboxOp};

const EAPOL_ETHERTYPE: u16 = 0x888e;

pub(super) use oer_esp32s31_ieee80211_sta::connected::security::ConnectedSecurityFrame;

/// Robust management input of an association that protects its management
/// frames. Both are unauthenticated until control opens the frame or an SA
/// Query confirms the disconnect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedManagementInput {
    Protected(ProtectedManagementFrame),
    UnprotectedDisconnect(StaDisconnect),
}

/// Capacity of the robust management lane.
const MANAGEMENT_CAPACITY: usize = 2;

/// Explicit observer for profiles that intentionally ignore control-plane
/// events. Production association/BlockAck state should supply a real sink.
pub struct IgnoreConnectedControl;

impl ConnectedRxSink for IgnoreConnectedControl {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {}
}

/// Fixed synchronous queue used by executor-independent composition tests.
/// Overflow is explicit evidence; it never allocates or silently overwrites an
/// older action.
pub struct ConnectedControlQueue<const CAPACITY: usize> {
    events: [Option<ConnectedRxControlEvent>; CAPACITY],
    head: usize,
    tail: usize,
    len: usize,
    dropped: u32,
}

fn scheduled_connected_control(event: ConnectedRxEvent<'_>) -> Option<ConnectedRxControlEvent> {
    match event.control()? {
        // `Esp32s31ConnectedControl` consumes these as semantic state. HE
        // Trigger/NDPA uses its own single-slot lane so it cannot starve a
        // beacon or ADDBA/DELBA transition in this bounded mailbox.
        event @ (ConnectedRxControlEvent::Beacon(_)
        | ConnectedRxControlEvent::ProbeResponse
        | ConnectedRxControlEvent::BlockAck(_)
        | ConnectedRxControlEvent::IndividualTwt(_)) => Some(event),
        event @ ConnectedRxControlEvent::PeerDisconnect(_) => Some(event),
        ConnectedRxControlEvent::Trigger { .. }
        | ConnectedRxControlEvent::Ndpa { .. }
        | ConnectedRxControlEvent::UnprotectedDisconnect(_)
        | ConnectedRxControlEvent::SaQuery(_)
        | ConnectedRxControlEvent::PowerSaveData(_) => None,
    }
}

fn scheduled_he_observation(event: ConnectedRxEvent<'_>) -> Option<ConnectedRxControlEvent> {
    match event.control()? {
        event
        @ (ConnectedRxControlEvent::Trigger { .. } | ConnectedRxControlEvent::Ndpa { .. }) => {
            Some(event)
        }
        ConnectedRxControlEvent::Beacon(_)
        | ConnectedRxControlEvent::ProbeResponse
        | ConnectedRxControlEvent::BlockAck(_)
        | ConnectedRxControlEvent::IndividualTwt(_)
        | ConnectedRxControlEvent::PeerDisconnect(_)
        | ConnectedRxControlEvent::UnprotectedDisconnect(_)
        | ConnectedRxControlEvent::SaQuery(_)
        | ConnectedRxControlEvent::PowerSaveData(_) => None,
    }
}

/// Capacity of the power-save data lane.
const POWER_SAVE_DATA_CAPACITY: usize = 4;

impl<const CAPACITY: usize> ConnectedControlQueue<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            events: [None; CAPACITY],
            head: 0,
            tail: 0,
            len: 0,
            dropped: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn dropped(&self) -> u32 {
        self.dropped
    }

    pub fn pop(&mut self) -> Option<ConnectedRxControlEvent> {
        if self.len == 0 || CAPACITY == 0 {
            return None;
        }
        let event = self.events[self.head].take()?;
        self.head = (self.head + 1) % CAPACITY;
        self.len -= 1;
        Some(event)
    }
}

impl<const CAPACITY: usize> ConnectedRxSink for ConnectedControlQueue<CAPACITY> {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let Some(event) = scheduled_connected_control(event) else {
            return;
        };
        if CAPACITY == 0 || self.len == CAPACITY {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.events[self.tail] = Some(event);
        self.tail = (self.tail + 1) % CAPACITY;
        self.len += 1;
    }
}

impl<const CAPACITY: usize> Default for ConnectedControlQueue<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Static Embassy mailbox for owned connected control events.
pub struct ConnectedControlResources<M: RawMutex, const CAPACITY: usize> {
    channel: Channel<M, ConnectedRxControlEvent, CAPACITY>,
    terminal: Channel<M, ConnectedRxControlEvent, 1>,
    he_observation: Channel<M, ConnectedRxControlEvent, 1>,
    /// Authenticated connected EAPOL such as Group Message 1. Losing one is a
    /// functional protocol overflow and remains fail-closed.
    security: Channel<M, ConnectedSecurityFrame, 1>,
    /// Best-effort plaintext EAPOL admitted only as a duplicate-M3 candidate.
    /// Keeping this separate prevents unauthenticated traffic from occupying
    /// the protected security lane or poisoning ordered-control overflow.
    unprotected_security: Channel<M, ConnectedSecurityFrame, 1>,
    /// Data frames of the access point while the station advertises power
    /// save. Power management reads them to time its sleep; a full lane
    /// coalesces, as the next frame or timer carries the same decision.
    power_save_data: Channel<M, ConnectedRxControlEvent, POWER_SAVE_DATA_CAPACITY>,
    power_save_data_armed: AtomicBool,
    /// Robust management frames control opens and unprotected disconnects
    /// it confirms. They are peer input anyone can forge, so a full lane
    /// drops rather than invalidating the ordered control stream.
    management: Channel<M, ConnectedManagementInput, MANAGEMENT_CAPACITY>,
    dropped_management: AtomicU32,
    overflowed: AtomicBool,
    dropped_he_observations: AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedControlResources<M, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            channel: Channel::new(),
            terminal: Channel::new(),
            he_observation: Channel::new(),
            security: Channel::new(),
            unprotected_security: Channel::new(),
            power_save_data: Channel::new(),
            power_save_data_armed: AtomicBool::new(false),
            management: Channel::new(),
            dropped_management: AtomicU32::new(0),
            overflowed: AtomicBool::new(false),
            dropped_he_observations: AtomicU32::new(0),
        }
    }

    /// Split one station epoch into a receive-dispatch publisher and its
    /// scheduler-side consumer.
    ///
    /// Embassy channels support recreating their lightweight endpoints. The
    /// station lifecycle owner must nevertheless keep epochs disjoint: it
    /// stops the RX protocol task and drops the connected-control consumer
    /// before calling `split` for a later association. Accepting `&self`
    /// permits that sequential reuse for statically located resources without
    /// manufacturing a second channel allocation.
    pub fn split(
        &self,
    ) -> (
        ConnectedControlPublisher<'_, M, CAPACITY>,
        ConnectedControlReceiver<'_, M, CAPACITY>,
    ) {
        let resources: &Self = self;
        resources.overflowed.store(false, Ordering::Release);
        resources
            .power_save_data_armed
            .store(false, Ordering::Release);
        while resources.power_save_data.try_receive().is_ok() {}
        while resources.management.try_receive().is_ok() {}
        resources.dropped_management.store(0, Ordering::Release);
        resources
            .dropped_he_observations
            .store(0, Ordering::Release);
        (
            ConnectedControlPublisher {
                sender: resources.channel.sender(),
                terminal: resources.terminal.sender(),
                he_observation: resources.he_observation.sender(),
                security: resources.security.sender(),
                unprotected_security: resources.unprotected_security.sender(),
                power_save_data: resources.power_save_data.sender(),
                power_save_data_armed: &resources.power_save_data_armed,
                management: resources.management.sender(),
                dropped_management: &resources.dropped_management,
                overflowed: &resources.overflowed,
                dropped_he_observations: &resources.dropped_he_observations,
            },
            ConnectedControlReceiver {
                receiver: resources.channel.receiver(),
                terminal: resources.terminal.receiver(),
                he_observation: resources.he_observation.receiver(),
                security: resources.security.receiver(),
                unprotected_security: resources.unprotected_security.receiver(),
                power_save_data: resources.power_save_data.receiver(),
                power_save_data_armed: &resources.power_save_data_armed,
                management: resources.management.receiver(),
                dropped_management: &resources.dropped_management,
                overflowed: &resources.overflowed,
                dropped_he_observations: &resources.dropped_he_observations,
            },
        )
    }
}

impl<M: RawMutex, const CAPACITY: usize> Default for ConnectedControlResources<M, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// RX-dispatch capability; it can publish but cannot consume or execute a
/// control action.
#[derive(Clone, Copy)]
pub struct ConnectedControlPublisher<'resources, M: RawMutex, const CAPACITY: usize> {
    sender: Sender<'resources, M, ConnectedRxControlEvent, CAPACITY>,
    terminal: Sender<'resources, M, ConnectedRxControlEvent, 1>,
    he_observation: Sender<'resources, M, ConnectedRxControlEvent, 1>,
    security: Sender<'resources, M, ConnectedSecurityFrame, 1>,
    unprotected_security: Sender<'resources, M, ConnectedSecurityFrame, 1>,
    power_save_data: Sender<'resources, M, ConnectedRxControlEvent, POWER_SAVE_DATA_CAPACITY>,
    power_save_data_armed: &'resources AtomicBool,
    management: Sender<'resources, M, ConnectedManagementInput, MANAGEMENT_CAPACITY>,
    dropped_management: &'resources AtomicU32,
    overflowed: &'resources AtomicBool,
    dropped_he_observations: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedRxSink
    for ConnectedControlPublisher<'_, M, CAPACITY>
{
    fn wants_power_save_data(&self) -> bool {
        self.power_save_data_armed.load(Ordering::Acquire)
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::UnprotectedEapol { source, payload } = event {
            if let Ok(frame) = OwnedEapolFrame::try_copy(RsnInterface::Station, source, payload) {
                // This is unauthenticated peer input until connected WPA2
                // verifies its MIC and exact completed-M3 commitment. Full is
                // therefore a peer-local coalescing drop, never authority to
                // invalidate the ordered control stream or protected lane.
                let _ = self
                    .unprotected_security
                    .try_send(ConnectedSecurityFrame::Unprotected(frame));
            }
            return;
        }
        if let ConnectedRxEvent::Ethernet { frame, .. } = event
            && frame.ether_type == EAPOL_ETHERTYPE
        {
            let result =
                OwnedEapolFrame::try_copy(RsnInterface::Station, frame.source, frame.payload)
                    .ok()
                    .map(|frame| {
                        self.security
                            .try_send(ConnectedSecurityFrame::Protected(frame))
                    });
            if !matches!(result, Some(Ok(()))) {
                self.overflowed.store(true, Ordering::Release);
            }
            return;
        }
        let management = match event {
            ConnectedRxEvent::ProtectedManagement { frame, group } => Some(
                ProtectedManagementFrame::try_copy(frame, group)
                    .map(ConnectedManagementInput::Protected),
            ),
            ConnectedRxEvent::UnprotectedDisconnect(disconnect) => Some(Some(
                ConnectedManagementInput::UnprotectedDisconnect(disconnect),
            )),
            _ => None,
        };
        if let Some(input) = management {
            if input.is_none_or(|input| self.management.try_send(input).is_err()) {
                self.dropped_management.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        if let ConnectedRxEvent::PowerSaveData(data) = event {
            if self.power_save_data_armed.load(Ordering::Acquire) {
                let _ = self
                    .power_save_data
                    .try_send(ConnectedRxControlEvent::PowerSaveData(data));
            }
            return;
        }
        if let Some(event) = scheduled_he_observation(event) {
            if let Err(TrySendError::Full(_)) = self.he_observation.try_send(event) {
                self.dropped_he_observations.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        let Some(event) = scheduled_connected_control(event) else {
            return;
        };
        let kind = ControlEventKind::from(&event);
        let result = if matches!(event, ConnectedRxControlEvent::PeerDisconnect(_)) {
            self.terminal.try_send(event)
        } else {
            self.sender.try_send(event)
        };
        oer_trace::emit(&ControlMailbox {
            event: kind,
            op: if result.is_ok() {
                MailboxOp::Published
            } else {
                MailboxOp::Overflowed
            },
        });
        if let Err(TrySendError::Full(_)) = result {
            self.overflowed.store(true, Ordering::Release);
        }
    }
}

/// Scheduler-side control capability; it cannot publish borrowed RX data.
pub struct ConnectedControlReceiver<'resources, M: RawMutex, const CAPACITY: usize> {
    receiver: Receiver<'resources, M, ConnectedRxControlEvent, CAPACITY>,
    terminal: Receiver<'resources, M, ConnectedRxControlEvent, 1>,
    he_observation: Receiver<'resources, M, ConnectedRxControlEvent, 1>,
    security: Receiver<'resources, M, ConnectedSecurityFrame, 1>,
    unprotected_security: Receiver<'resources, M, ConnectedSecurityFrame, 1>,
    power_save_data: Receiver<'resources, M, ConnectedRxControlEvent, POWER_SAVE_DATA_CAPACITY>,
    power_save_data_armed: &'resources AtomicBool,
    management: Receiver<'resources, M, ConnectedManagementInput, MANAGEMENT_CAPACITY>,
    dropped_management: &'resources AtomicU32,
    overflowed: &'resources AtomicBool,
    dropped_he_observations: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> ConnectedControlReceiver<'_, M, CAPACITY> {
    pub fn try_receive_terminal(&self) -> Option<ConnectedRxControlEvent> {
        self.terminal.try_receive().ok()
    }

    pub fn try_receive_control(&self) -> Option<ConnectedRxControlEvent> {
        self.receiver.try_receive().ok()
    }

    pub fn try_receive_he_observation(&self) -> Option<ConnectedRxControlEvent> {
        self.he_observation.try_receive().ok()
    }

    /// Publish data frames of the access point to power management, or
    /// stop and discard the queued ones.
    pub fn set_power_save_data_armed(&self, armed: bool) {
        self.power_save_data_armed.store(armed, Ordering::Release);
        if !armed {
            while self.power_save_data.try_receive().is_ok() {}
        }
    }

    pub fn try_receive_management(&self) -> Option<ConnectedManagementInput> {
        self.management.try_receive().ok()
    }

    /// Robust management input dropped because it did not fit its copy or
    /// its lane was full.
    pub fn dropped_management(&self) -> u32 {
        self.dropped_management.load(Ordering::Acquire)
    }

    pub fn try_receive_power_save_data(&self) -> Option<ConnectedRxControlEvent> {
        self.power_save_data.try_receive().ok()
    }

    pub fn try_receive(&self) -> Option<ConnectedRxControlEvent> {
        if let Some(event) = self.try_receive_terminal() {
            return Some(event);
        }
        self.try_receive_power_save_data()
            .or_else(|| self.try_receive_control())
            .or_else(|| self.try_receive_he_observation())
    }

    pub(super) fn try_receive_security(&self) -> Option<ConnectedSecurityFrame> {
        // Authenticated connected traffic always precedes the best-effort
        // duplicate-M3 candidate lane, regardless of arrival order.
        self.security
            .try_receive()
            .ok()
            .or_else(|| self.unprotected_security.try_receive().ok())
    }

    pub async fn ready(&self) {
        select(
            select6(
                self.terminal.ready_to_receive(),
                self.security.ready_to_receive(),
                self.unprotected_security.ready_to_receive(),
                self.power_save_data.ready_to_receive(),
                self.receiver.ready_to_receive(),
                self.he_observation.ready_to_receive(),
            ),
            self.management.ready_to_receive(),
        )
        .await;
    }

    pub fn len(&self) -> usize {
        self.terminal.len()
            + self.security.len()
            + self.unprotected_security.len()
            + self.power_save_data.len()
            + self.receiver.len()
            + self.he_observation.len()
            + self.management.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn security_pending(&self) -> bool {
        !self.security.is_empty() || !self.unprotected_security.is_empty()
    }

    /// Whether this mailbox epoch has lost any semantic control event.
    ///
    /// This is functional protocol state, not a diagnostic counter. Once set,
    /// the connected owner must fail closed and may only clear it by returning
    /// every endpoint and starting a fresh split epoch.
    pub fn overflowed(&self) -> bool {
        self.overflowed.load(Ordering::Acquire)
    }

    /// Number of HE control events coalesced by the single-slot runtime lane.
    /// Losing one does not invalidate Association/BlockAck state; the lane is
    /// intentionally best-effort because a later dequeue could not recover an
    /// already missed response window.
    pub fn dropped_he_observations(&self) -> u32 {
        self.dropped_he_observations.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests;

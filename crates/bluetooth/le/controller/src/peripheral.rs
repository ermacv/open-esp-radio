//! The peripheral connections created by connection indications, one per
//! slot.
//!
//! A connection opens the backend's connection, reports LE Connection
//! Complete and then listens at every connection event: the first across the
//! transmit window the indication set, later ones at the anchor the last
//! received packet fixed, widened for clock drift. The next event is planned
//! when the previous one ended; events that can no longer be admitted in time,
//! or that would find no room in the Host output for their receptions, are
//! skipped. While events are skipped for lack of room the supervision timeout
//! still runs against the current time, so a Host that withholds its credits
//! loses the connection with Disconnection Complete `0x08`. Peripheral latency is not used: the peripheral listens at every
//! event it plans.
//!
//! Between events at most one PDU waits for the central's acknowledgement.
//! The next one is chosen in this order: an encryption procedure response,
//! a Link Layer control response or request, then the next fragment of the
//! Host ACL packet in progress, as long as the effective transmit length of the
//! Data Length Update allows; an encrypted fragment leaves room for its MIC.
//! While encryption is being started, paused or restarted only its own
//! responses and a required termination may be sent. A new connection starts
//! the Data Length Update when the Host suggested a longer default.
//!
//! Received packets are authenticated first, then dispatched to the control
//! procedures or delivered to the Host as ACL data. A Channel Map Update or
//! Connection Update waits for the event to end and then enters the Link
//! Layer's pending instant.
//!
//! The connection ends when the central's `LL_TERMINATE_IND` arrives, when the
//! central acknowledged a local `LL_TERMINATE_IND`, when the supervision
//! timeout passes without a received packet, when six events pass without
//! establishment, at once with `0x3D` and without a terminate when a received
//! packet fails its MIC or the encryption procedure, or on Reset. The event in progress is cancelled, the
//! backend's connection is closed and Disconnection Complete follows, except
//! after Reset.

mod receive;
mod timing;

use bt_hci::{
    data::AclPacket,
    param::{AddrKind, BdAddr, ClockAccuracy, ConnHandle, Error as HciError, Status},
};
use oer_bluetooth_hci::{
    LeControllerAclPacket, LeHostAclPacket, LeRandomSource, LeReadRemoteFeaturesCommand,
    LeReadRemoteVersionInformationCommand,
};
use oer_bluetooth_ll::{
    LeDeviceAddressKind,
    connection::{
        LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS, LeChannelSelectionAlgorithm, LeConnectionTiming,
        LeFollowingEvent, LeLegacyConnectionRequest, LePeripheralConnection,
        LePeripheralConnectionEventCompleted, LePeripheralConnectionEventDelta,
        LePeripheralConnectionEventInFlight, LePeripheralConnectionEventPeerActivity,
        LePeripheralConnectionEventPrepared, LePeripheralConnectionState,
    },
    control::{
        LeChannelMapUpdate, LeConnectionUpdate, LePeripheralControl, LePeripheralControlError,
        LePeripheralReceive, LeRemoteFeaturesAdmission, LeRemoteFeaturesResult,
        LeRemoteVersionAdmission, LeRemoteVersionResult, LeVersionInformation,
    },
    data_length::{LE_DATA_LENGTH_MAXIMUM_OCTETS, LeDataLength, LeDataLengths},
    security::{
        LE_ACL_MIC_BYTES, LeLongTermKey, LePeripheralEncryptionProcedure,
        LePeripheralEncryptionRandom, LePeripheralEncryptionTermination,
    },
};
use oer_bluetooth_radio::LeRadio;
use oer_bluetooth_radio::{
    AccessAddress, ConnectionConfiguration, ConnectionEvent, ConnectionId, CrcInit, DataChannel,
    DataPdu, DataPduKind, EventId, EventResult, LeCapabilities, LeInstant, LePhy, LeWindow,
    OutsideEpoch, RadioDuration, RadioOutcome, RadioRequest, RadioTiming, Supported, TxPower,
};

use oer_bluetooth_trace::PeripheralEventOp as Op;

use crate::planning::{
    EpochExhausted, PlanningCalculation as C, PlanningOperation as O, PlanningRole as R,
};

use crate::{RadioWork, advertising::ConnectionIndication, coexistence};

/// Host events one connection event can produce: its receptions plus the
/// procedure events that follow them.
pub(crate) const EVENT_OUTPUT: usize = 8;
/// Largest data channel payload the Data Length Extension allows.
const LL_PAYLOAD: usize = LE_DATA_LENGTH_MAXIMUM_OCTETS as usize;
/// Largest data PDU payload a connection transmits: the Data Length
/// Extension maximum and an encrypted PDU's MIC.
pub(crate) const LL_DATA_PAYLOAD: usize = LL_PAYLOAD + LE_ACL_MIC_BYTES;
/// Scheduling priority of the first event and of later events (vendor
/// first-event transform and recurring baseline).
const FIRST_PRIORITY: u8 = 13;
const PRIORITY: u8 = 8;
/// Link Layer procedure response timeout (Core Vol 6 Part B 5.2).
const PROCEDURE_TIMEOUT: RadioDuration<LeRadio> = RadioDuration::from_micros(40_000_000);
/// Events tried per plan before the connection waits for the next call.
const PLAN_ATTEMPTS: usize = 64;
/// Connection Terminated due to MIC Failure.
const MIC_FAILURE: u8 = 0x3d;

/// What the connection reports to the Host.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PeripheralEvent {
    Connected {
        peer_kind: AddrKind,
        peer: BdAddr,
        interval_units: u16,
        latency: u16,
        timeout_units: u16,
        accuracy: ClockAccuracy,
    },
    ConnectionFailed(Status),
    Disconnected(Status),
    ConnectionUpdated {
        interval_units: u16,
        latency: u16,
        timeout_units: u16,
    },
    RemoteFeatures(Status, [u8; 8]),
    RemoteVersion(Status, LeVersionInformation),
    LongTermKeyRequest {
        random: [u8; 8],
        diversifier: u16,
    },
    EncryptionChanged(Status, bool),
    KeyRefreshed(Status),
    DataLengthChanged(LeDataLengths),
    CompletedPackets(u16),
}

/// A connection's report to the Host, with the handle it names.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HostEvent {
    pub(crate) handle: ConnHandle,
    pub(crate) event: PeripheralEvent,
}

/// The PDU waiting for the central's acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pending {
    Security,
    Control,
    Acl,
}

/// The Link Layer connection between and during events.
enum Link {
    /// Before the first event.
    Created(LePeripheralConnection),
    InFlight(LePeripheralConnectionEventInFlight),
    Completed(LePeripheralConnectionEventCompleted),
    /// Transiently moved out.
    Moved,
}

#[derive(Clone, Copy, Debug)]
struct Event {
    id: EventId,
    reservation: LeWindow,
    /// From the phase's reference to the event's nominal anchor.
    since_reference: RadioDuration<LeRadio>,
    transmit_window: RadioDuration<LeRadio>,
}

#[derive(Clone, Copy, Debug)]
struct Closing {
    /// `None` after Reset: no Host event follows.
    reason: Option<Status>,
    cancel_sent: bool,
}

struct Connection {
    id: ConnectionId,
    /// A more urgent connection's event took the place of this one's event
    /// in progress; its cancel is sent.
    preempted: bool,
    handle: ConnHandle,
    request: LeLegacyConnectionRequest,
    created_at: LeInstant,
    link: Link,
    phase: timing::Phase,
    /// The last anchor a received packet established.
    last_activity: Option<LeInstant>,
    event: Option<Event>,
    /// The event whose request is at the backend.
    submitting: Option<(LePeripheralConnectionEventPrepared, Event)>,
    opened: bool,
    open_sent: bool,
    control: LePeripheralControl,
    security: LePeripheralEncryptionProcedure,
    /// Since when a procedure awaits the central.
    procedure_since: Option<LeInstant>,
    pending: Option<Pending>,
    /// The transmission whose request is at the backend.
    transmitting: Option<(Pending, usize)>,
    host_acl: Option<LeHostAclPacket>,
    /// The next received encrypted data PDU has its MIC corrupted.
    #[cfg(feature = "diagnostic-mic-fault")]
    corrupt_next_mic: bool,
    channel_map: Option<LeChannelMapUpdate>,
    connection_update: Option<LeConnectionUpdate>,
    peer_termination: Option<u8>,
    closing: Option<Closing>,
    tx: [u8; LL_DATA_PAYLOAD],
    rx: [u8; receive::PDU_CAPACITY],
}

/// Queue of Host events.
struct Events {
    items: [Option<HostEvent>; 16],
    head: usize,
    len: usize,
}

impl Events {
    const fn new() -> Self {
        Self {
            items: [None; 16],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, handle: ConnHandle, event: PeripheralEvent) {
        assert!(
            self.len < self.items.len(),
            "the Host event queue is drained per call"
        );
        self.items[(self.head + self.len) % self.items.len()] = Some(HostEvent { handle, event });
        self.len += 1;
    }

    fn pop(&mut self) -> Option<HostEvent> {
        if self.len == 0 {
            return None;
        }
        let event = self.items[self.head].take();
        self.head = (self.head + 1) % self.items.len();
        self.len -= 1;
        event
    }
}

/// One connection slot: an accepted indication until its first window is
/// validated, then its connection.
struct Slot {
    /// Accepted indication and its exact LL owner, before first-window
    /// validation.
    opening: Option<(ConnectionIndication, LePeripheralConnection)>,
    connection: Option<Connection>,
    /// The submission count when the slot's last event was submitted, which
    /// orders slots that wait equally long.
    served: u64,
}

impl Slot {
    const fn new() -> Self {
        Self {
            opening: None,
            connection: None,
            served: 0,
        }
    }

    const fn is_active(&self) -> bool {
        self.opening.is_some() || self.connection.is_some()
    }

    /// The Host-visible connection.
    fn live(&self) -> Option<&Connection> {
        self.connection
            .as_ref()
            .filter(|connection| connection.opened && connection.closing.is_none())
    }

    fn live_mut(&mut self) -> Option<&mut Connection> {
        self.connection
            .as_mut()
            .filter(|connection| connection.opened && connection.closing.is_none())
    }

    /// Whether the slot has work for the radio. `room` tells whether the
    /// Host output can take one event's receptions.
    fn wants_radio(&self, room: bool) -> bool {
        if self.opening.is_some() {
            return true;
        }
        let Some(connection) = &self.connection else {
            return false;
        };
        if connection.submitting.is_some() || connection.transmitting.is_some() {
            return false;
        }
        if !connection.open_sent {
            return true;
        }
        if let Some(closing) = connection.closing {
            return connection.event.is_none() || !closing.cancel_sent;
        }
        // Without room an established connection still wants the radio: the
        // request pass checks its supervision timeout against the current
        // time instead of planning an event.
        connection.event.is_none()
            && (room
                || connection.last_activity.is_some()
                || connection.pending.is_none() && connection.has_transmission())
    }
}

/// The connections' next request, which the caller checks against its
/// capabilities: a transmission borrows its PDU from its connection through
/// [`Peripheral::transmission`].
pub(crate) enum PeripheralWork {
    /// Withdraw an event.
    Cancel(EventId),
    /// Open a connection.
    Open(ConnectionConfiguration),
    /// Close a connection.
    Close(ConnectionId),
    /// Schedule a connection event, placed by the caller.
    Event(ConnectionEvent),
    Transmit {
        index: usize,
        kind: DataPduKind,
        length: usize,
    },
}

/// What a slot asks of the backend next, chosen before its request borrows
/// the slot.
#[derive(Clone, Copy, Debug)]
enum Work {
    Open,
    Cancel(EventId),
    Close,
    Transmit(Pending, usize),
}

// CAPABILITY: bluetooth-peripheral, bluetooth-peripheral-latency, bluetooth-establishment-supervision-timeout, bluetooth-missed-event-recovery, bluetooth-termination, bluetooth-ll-data-pdu-tx, bluetooth-sn-nesn-retransmission-duplicate-suppression-and-empty-pdu-acknowledgments
/// The peripheral connections, one per slot of `N`. A slot's index is its
/// connection's identity at the backend; the Host's connection handle is
/// allocated apart and not reused at once, so a command still in flight to
/// a closed connection never reaches the next one.
pub(crate) struct Peripheral<const N: usize> {
    slots: [Slot; N],
    events: Events,
    local_version: Option<LeVersionInformation>,
    /// The Host's suggested transmit length for new connections.
    suggested_data_length: LeDataLength,
    /// The request at the backend and the slot it is for.
    requested: Option<(usize, RequestKind)>,
    /// Data received for the Host, taken after every outcome.
    received: Option<LeControllerAclPacket>,
    /// Connection events submitted so far.
    submissions: u64,
    /// The next connection handle to allocate.
    next_handle: u16,
}

impl<const N: usize> Peripheral<N> {
    pub(crate) const fn new(local_version: Option<LeVersionInformation>) -> Self {
        const {
            assert!(N > 0 && N <= 255, "1 to 255 connection slots");
        }
        Self {
            slots: [const { Slot::new() }; N],
            events: Events::new(),
            local_version,
            suggested_data_length: LeDataLength::MINIMUM,
            requested: None,
            received: None,
            submissions: 0,
            next_handle: 0,
        }
    }

    /// Whether a connection exists or is still closing.
    pub(crate) fn is_active(&self) -> bool {
        self.slots.iter().any(Slot::is_active)
    }

    /// Whether a connection indication finds a slot.
    pub(crate) fn has_free_slot(&self) -> bool {
        self.slots.iter().any(|slot| !slot.is_active())
    }

    /// The slot of the connection `handle`.
    fn slot_of(&self, handle: ConnHandle) -> Option<usize> {
        self.slots.iter().position(|slot| {
            slot.connection
                .as_ref()
                .is_some_and(|connection| connection.handle == handle)
        })
    }

    fn live_mut(&mut self, handle: ConnHandle) -> Option<&mut Connection> {
        let slot = self.slot_of(handle)?;
        self.slots[slot].live_mut()
    }

    /// Whether `handle` names a Host-visible connection.
    pub(crate) fn is_live(&self, handle: ConnHandle) -> bool {
        self.slot_of(handle)
            .is_some_and(|slot| self.slots[slot].live().is_some())
    }

    /// A handle no connection holds, after the last one allocated: the
    /// largest valid handle is 0x0EFF.
    fn allocate_handle(&mut self) -> ConnHandle {
        loop {
            let handle = ConnHandle::new(self.next_handle);
            self.next_handle = if self.next_handle == 0x0eff {
                0
            } else {
                self.next_handle + 1
            };
            if self.slot_of(handle).is_none() {
                return handle;
            }
        }
    }

    /// The Host's suggested transmit length for new connections.
    pub(crate) const fn suggested_data_length(&self) -> LeDataLength {
        self.suggested_data_length
    }

    /// Suggest the transmit length of later connections; Reset restores
    /// the minimum.
    pub(crate) fn suggest_data_length(&mut self, transmit: LeDataLength) {
        self.suggested_data_length = transmit;
    }

    /// Ask the live connection `handle` to send up to `transmit`.
    pub(crate) fn set_data_length(
        &mut self,
        handle: ConnHandle,
        transmit: LeDataLength,
    ) -> Admission {
        let Some(connection) = self.live_mut(handle) else {
            return Admission::UnknownConnection;
        };
        connection.control.set_data_length(transmit);
        Admission::Accepted
    }

    pub(crate) fn take_event(&mut self) -> Option<HostEvent> {
        self.events.pop()
    }

    /// Data received for the Host by the last outcome.
    pub(crate) fn take_received(&mut self) -> Option<LeControllerAclPacket> {
        self.received.take()
    }

    /// Create a connection from an accepted indication, in a free slot:
    /// connectable advertising runs only while one is free.
    pub(crate) fn open(&mut self, indication: ConnectionIndication) {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| !slot.is_active())
            .expect("connectable advertising runs only while a connection slot is free");
        let connection = LePeripheralConnection::from_request(
            indication.request,
            LeChannelSelectionAlgorithm::AlgorithmTwo,
        );
        slot.opening = Some((indication, connection));
    }

    fn finish_open(
        &mut self,
        index: usize,
        anchor: LeInstant,
        transmit_window: RadioDuration<LeRadio>,
    ) {
        let (indication, connection) = self.slots[index]
            .opening
            .take()
            .expect("validated pending indication");
        let request = indication.request;
        crate::trace::emit(|| oer_bluetooth_trace::PeripheralOpened {
            // The low word of the radio time.
            first_anchor_low: anchor.as_micros() as u32,
            interval_units: request.timing().interval_units(),
        });
        let id = ConnectionId::new(u8::try_from(index).expect("at most 255 connection slots"));
        let handle = self.allocate_handle();
        self.slots[index].connection = Some(Connection {
            id,
            preempted: false,
            handle,
            request,
            created_at: indication.at,
            link: Link::Created(connection),
            phase: timing::Phase::at(anchor, transmit_window),
            last_activity: None,
            event: None,
            submitting: None,
            opened: false,
            open_sent: false,
            control: LePeripheralControl::new()
                .with_local_version(self.local_version)
                .with_data_length(self.suggested_data_length),
            security: LePeripheralEncryptionProcedure::new(),
            procedure_since: None,
            pending: None,
            transmitting: None,
            host_acl: None,
            #[cfg(feature = "diagnostic-mic-fault")]
            corrupt_next_mic: false,
            channel_map: None,
            connection_update: None,
            peer_termination: None,
            closing: None,
            tx: [0; LL_DATA_PAYLOAD],
            rx: [0; receive::PDU_CAPACITY],
        });
    }

    /// End every connection without a Host event, as Reset does.
    pub(crate) fn abort(&mut self) {
        for slot in &mut self.slots {
            slot.opening = None;
            if let Some(connection) = &mut slot.connection {
                connection.closing = Some(Closing {
                    reason: None,
                    cancel_sent: false,
                });
            }
        }
    }

    /// Whether a connection has work for the radio. `room` tells whether
    /// the Host output can take one event's receptions.
    pub(crate) fn wants_radio(&self, room: bool) -> bool {
        self.slots.iter().any(|slot| slot.wants_radio(room))
    }

    /// Connection events submitted and not ended yet.
    pub(crate) fn events_in_flight(&self) -> usize {
        self.slots
            .iter()
            .filter_map(|slot| slot.connection.as_ref())
            .filter(|connection| connection.event.is_some() || connection.submitting.is_some())
            .count()
    }

    /// The reservations of the events in progress.
    pub(crate) fn busy(&self) -> [Option<LeWindow>; N] {
        core::array::from_fn(|index| {
            self.slots[index]
                .connection
                .as_ref()
                .and_then(|connection| connection.event)
                .map(|event| event.reservation)
        })
    }

    /// The reservations of the connections' next events at their nominal
    /// anchors, which other roles leave free.
    pub(crate) fn planned(
        &self,
        timing: RadioTiming,
    ) -> Result<[Option<LeWindow>; N], EpochExhausted> {
        let mut planned = [None; N];
        for (window, slot) in planned.iter_mut().zip(&self.slots) {
            if let Some(connection) = &slot.connection {
                *window = connection.planned(timing)?;
            }
        }
        Ok(planned)
    }

    /// The next radio request: the slots' control work in slot order, then
    /// the connection event of the slot closest to its supervision timeout,
    /// the one served longest ago first among equals. An event that would
    /// overlap another connection's event in progress is skipped as missed.
    pub(crate) fn next_request(
        &mut self,
        ids: &mut u32,
        now: LeInstant,
        timing: RadioTiming,
        room: bool,
    ) -> Result<Option<PeripheralWork>, EpochExhausted> {
        for index in 0..N {
            self.validate_opening(index, timing)?;
            self.prepare(index, now, room);
        }
        if let Some((index, work)) = (0..N).find_map(|index| Some((index, self.work(index)?))) {
            return Ok(Some(self.submit(index, work)));
        }
        if !room {
            return Ok(None);
        }
        let earliest = crate::planning::earliest(now, timing, R::Peripheral)?;
        let mut order: [usize; N] = core::array::from_fn(|index| index);
        order.sort_unstable_by_key(|&index| {
            let slot = &self.slots[index];
            (
                slot.connection
                    .as_ref()
                    .map_or(u64::MAX, Connection::supervision_deadline),
                slot.served,
                index,
            )
        });
        // The most urgent connection waiting for its event takes the place
        // of a less urgent one's event in its way.
        let urgency = |slot: &Slot| {
            (
                slot.connection
                    .as_ref()
                    .map_or(u64::MAX, Connection::supervision_deadline),
                slot.served,
            )
        };
        if let Some(&waiting) = order.iter().find(|&&index| {
            self.slots[index]
                .connection
                .as_ref()
                .is_some_and(Connection::waits_for_event)
        }) {
            let connection = self.slots[waiting]
                .connection
                .as_ref()
                .expect("the slot holds the connection");
            if let Some(planned) = connection.planned(timing)? {
                let victim = (0..N).find(|&other| {
                    other != waiting
                        && urgency(&self.slots[other]) > urgency(&self.slots[waiting])
                        && self.slots[other].connection.as_ref().is_some_and(|c| {
                            !c.preempted
                                && c.closing.is_none()
                                && c.event
                                    .is_some_and(|event| event.reservation.overlaps(planned))
                        })
                });
                if let Some(victim) = victim {
                    let connection = self.slots[victim]
                        .connection
                        .as_mut()
                        .expect("the victim holds the connection");
                    connection.preempted = true;
                    let id = connection
                        .event
                        .expect("the victim's event is in progress")
                        .id;
                    self.requested = Some((victim, RequestKind::Cancel));
                    return Ok(Some(PeripheralWork::Cancel(id)));
                }
            }
        }
        // A cancelled event still holds the radio until its end arrives;
        // the connection that took its place waits for it instead of
        // missing its own event.
        if self.slots.iter().any(|slot| {
            slot.connection
                .as_ref()
                .is_some_and(|connection| connection.preempted && connection.event.is_some())
        }) {
            return Ok(None);
        }
        for index in order {
            let Some(connection) = self.slots[index].connection.as_ref() else {
                continue;
            };
            if !connection.waits_for_event() {
                continue;
            }
            let busy = self.busy();
            let blocked = busy
                .iter()
                .enumerate()
                .filter(|&(other, _)| other != index)
                .filter_map(|(_, window)| *window);
            let connection = self.slots[index]
                .connection
                .as_mut()
                .expect("the slot holds the connection");
            let Some((prepared, plan, reservation)) = connection.plan(earliest, timing, blocked)?
            else {
                if connection.closing.is_some() {
                    self.requested = Some((index, RequestKind::Close));
                    return Ok(Some(PeripheralWork::Close(connection.id)));
                }
                continue;
            };
            self.submissions += 1;
            self.slots[index].served = self.submissions;
            self.requested = Some((index, RequestKind::Event));
            let connection = self.slots[index]
                .connection
                .as_mut()
                .expect("the slot holds the connection");
            return Ok(Some(PeripheralWork::Event(connection.submit_event(
                ids,
                prepared,
                plan,
                reservation,
            ))));
        }
        Ok(None)
    }

    /// Validate the first window of the slot's accepted indication and
    /// create its connection.
    fn validate_opening(
        &mut self,
        index: usize,
        timing: RadioTiming,
    ) -> Result<(), EpochExhausted> {
        let Some((indication, _)) = &self.slots[index].opening else {
            return Ok(());
        };
        let wire = indication.request.timing();
        // The packet's end, then the start of the first transmit window.
        let anchor = indication
            .at
            .checked_add(RadioDuration::from_micros(
                u64::from(LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS)
                    + u64::from(wire.first_window_start_micros()),
            ))
            .ok_or(error(O::ConnectionIndication, C::FirstAnchor))?;
        let transmit_window =
            RadioDuration::from_micros(u64::from(wire.window_size_units()) * 1_250);
        let plan =
            timing::first(anchor, transmit_window, timing.connection).map_err(|mut cause| {
                cause.operation = O::ConnectionIndication;
                cause
            })?;
        timing
            .reservation(plan.window)
            .map_err(|OutsideEpoch| error(O::ConnectionIndication, C::Reservation))?;
        self.finish_open(index, anchor, transmit_window);
        Ok(())
    }

    /// Settle the slot's state before its work is chosen: a connection that
    /// closes before it reached the backend goes at once, procedures time
    /// out, and without room for an event the supervision timeout still
    /// runs against `now`.
    fn prepare(&mut self, index: usize, now: LeInstant, room: bool) {
        let slot = &mut self.slots[index];
        let Some(connection) = slot.connection.as_mut() else {
            return;
        };
        if connection.closing.is_some() && !connection.open_sent {
            // Nothing reached the backend yet.
            slot.connection = None;
            return;
        }
        if !connection.open_sent
            || connection.closing.is_some()
            || connection.event.is_some()
            || connection.submitting.is_some()
            || connection.transmitting.is_some()
        {
            return;
        }
        connection.check_procedures(now, &mut self.events);
        if connection.closing.is_none()
            && !room
            && connection.supervision_lost(now, connection.current_timing())
        {
            connection.closing = Some(Closing {
                reason: Some(HciError::CONN_TIMEOUT.to_status()),
                cancel_sent: false,
            });
        }
    }

    /// The slot's work other than its next event.
    fn work(&mut self, index: usize) -> Option<Work> {
        let connection = self.slots[index].connection.as_mut()?;
        if connection.submitting.is_some() || connection.transmitting.is_some() {
            return None;
        }
        if !connection.open_sent {
            return Some(Work::Open);
        }
        if let Some(closing) = connection.closing {
            return match connection.event {
                Some(event) if !closing.cancel_sent => Some(Work::Cancel(event.id)),
                Some(_) => None,
                None => Some(Work::Close),
            };
        }
        if connection.event.is_some() || connection.pending.is_some() {
            return None;
        }
        let (pending, length) = connection.prepare_transmission()?;
        Some(Work::Transmit(pending, length))
    }

    /// The request of the slot's `work`.
    fn submit(&mut self, index: usize, work: Work) -> PeripheralWork {
        let connection = self.slots[index]
            .connection
            .as_mut()
            .expect("work belongs to a connection");
        let id = connection.id;
        match work {
            Work::Open => {
                connection.open_sent = true;
                self.requested = Some((index, RequestKind::Open));
                let access_address = connection.request.access_address().value().to_le_bytes();
                PeripheralWork::Open(ConnectionConfiguration {
                    connection: id,
                    access_address: AccessAddress(access_address),
                    crc_init: CrcInit(connection.request.crc_initialization().wire_bytes()),
                    created_at: connection.created_at,
                    tx_power: TxPower::from_dbm(0),
                    phy: LePhy::Le1M,
                })
            }
            Work::Cancel(event) => {
                if let Some(closing) = &mut connection.closing {
                    closing.cancel_sent = true;
                }
                self.requested = Some((index, RequestKind::Cancel));
                PeripheralWork::Cancel(event)
            }
            Work::Close => {
                self.requested = Some((index, RequestKind::Close));
                PeripheralWork::Close(id)
            }
            Work::Transmit(pending, length) => {
                connection.transmitting = Some((pending, length));
                let kind = match pending {
                    Pending::Security | Pending::Control => DataPduKind::Control,
                    Pending::Acl => connection.acl_kind(),
                };
                let length = connection.tx_length(pending, length);
                self.requested = Some((index, RequestKind::Transmit));
                PeripheralWork::Transmit {
                    index,
                    kind,
                    length,
                }
            }
        }
    }

    /// The transmission [`PeripheralWork::Transmit`] chose, borrowing its
    /// PDU from the connection.
    pub(crate) fn transmission<K: LeCapabilities>(
        &self,
        index: usize,
        kind: DataPduKind,
        length: usize,
    ) -> RadioWork<'_, K> {
        let connection = self.slots[index]
            .connection
            .as_ref()
            .expect("a transmission belongs to a connection");
        let pdu = DataPdu::new(kind, &connection.tx[..length]).expect("a legacy data PDU fits");
        RadioWork::Submit(RadioRequest::Transmit {
            connection: connection.id,
            pdu: Supported::new(pdu).expect(
                "K carries every data PDU the core builds: LeController checks it at compile time",
            ),
        })
    }

    /// The backend's answer to the last request of a connection.
    pub(crate) fn request_done(&mut self, accepted: bool) {
        let Some((index, request)) = self.requested.take() else {
            return;
        };
        let slot = &mut self.slots[index];
        let Some(connection) = &mut slot.connection else {
            return;
        };
        let handle = connection.handle;
        match request {
            RequestKind::Open => {
                if accepted {
                    connection.opened = true;
                    let request = connection.request;
                    let timing = request.timing();
                    let initiator = request.initiator();
                    self.events.push(
                        handle,
                        PeripheralEvent::Connected {
                            peer_kind: match initiator.kind() {
                                LeDeviceAddressKind::Public => AddrKind::PUBLIC,
                                LeDeviceAddressKind::Random => AddrKind::RANDOM,
                            },
                            peer: BdAddr::new(initiator.wire_bytes()),
                            interval_units: timing.interval_units(),
                            latency: timing.peripheral_latency(),
                            timeout_units: timing.supervision_timeout_units(),
                            accuracy: clock_accuracy(request.sleep_clock_accuracy().encoded()),
                        },
                    );
                } else {
                    // The opening the trace recorded closes with the reason
                    // the Host gets.
                    let reason = HciError::CONN_REJECTED_LIMITED_RESOURCES.to_status();
                    crate::trace::emit(|| oer_bluetooth_trace::PeripheralClosed {
                        reason: Some(reason.into_inner()),
                    });
                    slot.connection = None;
                    self.events
                        .push(handle, PeripheralEvent::ConnectionFailed(reason));
                }
            }
            RequestKind::Cancel => {}
            RequestKind::Close => {
                let reason = connection.closing.and_then(|closing| closing.reason);
                crate::trace::emit(|| oer_bluetooth_trace::PeripheralClosed {
                    reason: reason.map(Status::into_inner),
                });
                connection.close_procedures(&mut self.events);
                slot.connection = None;
                if let Some(reason) = reason {
                    self.events
                        .push(handle, PeripheralEvent::Disconnected(reason));
                }
            }
            RequestKind::Transmit => {
                let (pending, length) = connection
                    .transmitting
                    .take()
                    .expect("a transmission was requested");
                if accepted {
                    connection.enqueued(pending, length);
                    connection.pending = Some(pending);
                }
            }
            RequestKind::Event => {
                let (prepared, event) = connection
                    .submitting
                    .take()
                    .expect("an event was requested");
                let in_flight = prepared.into_submitted();
                trace_event(
                    if accepted { Op::Accepted } else { Op::Refused },
                    in_flight.event_counter(),
                    event.id,
                );
                connection.phase.since_reference = event.since_reference;
                connection.phase.transmit_window = event.transmit_window;
                if accepted {
                    connection.link = Link::InFlight(in_flight);
                    connection.event = Some(event);
                } else {
                    // A refused event is a missed one.
                    connection.link = Link::Completed(
                        in_flight.complete(LePeripheralConnectionEventPeerActivity::Missed),
                    );
                    connection.after_event(&mut self.events);
                }
            }
        }
    }

    /// Account one backend observation.
    pub(crate) fn outcome(
        &mut self,
        outcome: RadioOutcome<'_>,
        random: Option<&dyn LeRandomSource>,
    ) {
        for slot in &mut self.slots {
            let Some(connection) = &mut slot.connection else {
                continue;
            };
            match outcome {
                RadioOutcome::Received { id, pdu } if connection.owns(id) => {
                    self.received =
                        connection.receive(pdu.pdu, random, self.local_version, &mut self.events);
                }
                RadioOutcome::TransmitAcknowledged(id) if id == connection.id => {
                    connection.acknowledged(&mut self.events);
                }
                RadioOutcome::EventEnded { id, result } if connection.owns(id) => {
                    connection.event = None;
                    connection.preempted = false;
                    let Link::InFlight(in_flight) =
                        core::mem::replace(&mut connection.link, Link::Moved)
                    else {
                        unreachable!("an event in progress is in flight")
                    };
                    let activity = match result {
                        EventResult::Executed {
                            anchor: Some(anchor),
                        } => {
                            connection.phase =
                                timing::Phase::at(anchor, RadioDuration::from_micros(0));
                            connection.last_activity = Some(anchor);
                            LePeripheralConnectionEventPeerActivity::Observed
                        }
                        // An aborted event proves no peer packet.
                        EventResult::Executed { anchor: None }
                        | EventResult::NotExecuted
                        | EventResult::Aborted => LePeripheralConnectionEventPeerActivity::Missed,
                    };
                    trace_event(
                        match activity {
                            LePeripheralConnectionEventPeerActivity::Observed => Op::Observed,
                            LePeripheralConnectionEventPeerActivity::Missed => Op::Missed,
                        },
                        in_flight.event_counter(),
                        id,
                    );
                    connection.link = Link::Completed(in_flight.complete(activity));
                    connection.after_event(&mut self.events);
                }
                _ => continue,
            }
            return;
        }
    }

    /// Host Disconnect of `handle`.
    pub(crate) fn disconnect(&mut self, handle: ConnHandle, reason: u8) -> bool {
        match self.live_mut(handle) {
            Some(connection) => {
                connection.control.request_host_termination(reason);
                true
            }
            None => false,
        }
    }

    /// The connection `handle` while it is not closing; the Host may still
    /// address one that has not reported its completion.
    fn addressed(&mut self, handle: ConnHandle) -> Option<&mut Connection> {
        let slot = self.slot_of(handle)?;
        self.slots[slot]
            .connection
            .as_mut()
            .filter(|c| c.closing.is_none())
    }

    /// Host LE Read Remote Features: whether the request was admitted.
    pub(crate) fn read_remote_features(
        &mut self,
        command: &LeReadRemoteFeaturesCommand,
    ) -> Admission {
        let handle = command.handle();
        let Some(connection) = self.addressed(handle) else {
            return Admission::UnknownConnection;
        };
        let features = match connection.control.admit_remote_feature_request() {
            LeRemoteFeaturesAdmission::Cached(features) => Some(features),
            LeRemoteFeaturesAdmission::Admitted => {
                connection.control.activate_remote_feature_request()
            }
            LeRemoteFeaturesAdmission::Busy => return Admission::Disallowed,
        };
        if let Some(features) = features {
            self.events.push(
                handle,
                PeripheralEvent::RemoteFeatures(Status::SUCCESS, features),
            );
        }
        Admission::Accepted
    }

    /// Host Read Remote Version Information.
    pub(crate) fn read_remote_version(
        &mut self,
        command: &LeReadRemoteVersionInformationCommand,
    ) -> Admission {
        let handle = command.handle();
        let Some(connection) = self.addressed(handle) else {
            return Admission::UnknownConnection;
        };
        let version = match connection.control.admit_remote_version_request() {
            LeRemoteVersionAdmission::Cached(version) => Some(version),
            LeRemoteVersionAdmission::Admitted => {
                connection.control.activate_remote_version_request()
            }
            LeRemoteVersionAdmission::Busy | LeRemoteVersionAdmission::LocalIdentityUnavailable => {
                return Admission::Disallowed;
            }
        };
        if let Some(version) = version {
            self.events.push(
                handle,
                PeripheralEvent::RemoteVersion(Status::SUCCESS, version),
            );
        }
        Admission::Accepted
    }

    /// Host LE Long Term Key Request Reply or Negative Reply for `handle`.
    pub(crate) fn long_term_key(&mut self, handle: ConnHandle, key: Option<[u8; 16]>) -> Admission {
        let Some(connection) = self.addressed(handle) else {
            return Admission::UnknownConnection;
        };
        let result = match key {
            Some(key) => connection
                .security
                .provide_long_term_key(LeLongTermKey::new(key))
                .map_err(|_| ()),
            None => connection.security.reject_long_term_key().map_err(|_| ()),
        };
        match result {
            Ok(()) => Admission::Accepted,
            Err(()) => Admission::Disallowed,
        }
    }

    /// Arm one received-MIC corruption on the connection `handle`, encrypted
    /// or not yet.
    #[cfg(feature = "diagnostic-mic-fault")]
    pub(crate) fn arm_mic_corruption(&mut self, handle: ConnHandle) -> Status {
        if !self.is_live(handle) {
            return HciError::UNKNOWN_CONN_IDENTIFIER.to_status();
        }
        match self.addressed(handle) {
            Some(connection) => {
                connection.corrupt_next_mic = true;
                Status::SUCCESS
            }
            None => HciError::CMD_DISALLOWED.to_status(),
        }
    }

    /// The handles whose connection still holds a Host ACL packet and
    /// takes no other now.
    pub(crate) fn acl_busy(&self) -> [Option<ConnHandle>; N] {
        core::array::from_fn(|index| {
            self.slots[index]
                .connection
                .as_ref()
                .filter(|connection| connection.host_acl.is_some())
                .map(|connection| connection.handle)
        })
    }

    /// Take one Host ACL packet. A packet no connection can carry is
    /// discarded.
    pub(crate) fn acl(&mut self, packet: AclPacket<'_>, capacity: u16) {
        let handle = packet.handle();
        let Some(connection) = self.live_mut(handle) else {
            return;
        };
        if let Ok(packet) = LeHostAclPacket::copy_from(packet, Some(handle), capacity) {
            connection.host_acl = Some(packet);
        }
    }
}

/// Which request of the connection is at the backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestKind {
    Open,
    Cancel,
    Close,
    Transmit,
    Event,
}

/// How a Host command about the connection was admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Admission {
    Accepted,
    Disallowed,
    UnknownConnection,
}

impl Connection {
    /// The reservation of the next event at its nominal anchor, which other
    /// roles leave free.
    fn planned(&self, timing: RadioTiming) -> Result<Option<LeWindow>, EpochExhausted> {
        if self.closing.is_some() {
            return Ok(None);
        }
        // The event one interval after the last one, as the Link Layer will
        // prepare it: an update's instant moves it by the updated window
        // offset and listens across the updated transmit window.
        let (previous, following, transmit_window) = match (&self.link, self.event) {
            // The first event is the next one.
            (Link::Created(_), _) => {
                let plan = timing::first(
                    self.phase.reference,
                    self.phase.transmit_window,
                    timing.connection,
                )
                .map_err(|mut cause| {
                    cause.operation = O::FutureReservation;
                    cause
                })?;
                return timing
                    .reservation(plan.window)
                    .map(Some)
                    .map_err(|OutsideEpoch| error(O::FutureReservation, C::Reservation));
            }
            (Link::InFlight(in_flight), Some(event)) => (
                event.since_reference,
                in_flight.following_event(),
                event.transmit_window,
            ),
            (Link::Completed(completed), _) => (
                self.phase.since_reference,
                completed.following_event(),
                self.phase.transmit_window,
            ),
            (Link::InFlight(_), None) | (Link::Moved, _) => {
                unreachable!(
                    "an event in flight is outstanding, and a link moves only within planning"
                )
            }
        };
        let (span, transmit_window) = match following {
            LeFollowingEvent::Unchanged(timing) => {
                (u64::from(timing.interval_micros()), transmit_window)
            }
            // An interval of at most 4 s and an offset of at most 82 s.
            LeFollowingEvent::Instant { previous, updated } => (
                u64::from(previous.interval_micros())
                    + u64::from(updated.window_offset_units()) * 1_250,
                RadioDuration::from_micros(u64::from(updated.window_size_units()) * 1_250),
            ),
        };
        // A span past `u64::MAX` cannot lie in the epoch either.
        let since_reference = previous
            .checked_add(RadioDuration::from_micros(span))
            .ok_or(error(O::FutureReservation, C::Recurrence))?;
        let plan = timing::recurring(
            self.phase.reference,
            since_reference,
            transmit_window,
            self.request.sleep_clock_accuracy().worst_case_ppm(),
            timing.connection,
        )?;
        timing
            .reservation(plan.window)
            .map(Some)
            .map_err(|OutsideEpoch| error(O::FutureReservation, C::Reservation))
    }

    /// Whether the connection's next event is due: it is open, not closing,
    /// and has no request or event outstanding.
    fn waits_for_event(&self) -> bool {
        self.open_sent
            && self.closing.is_none()
            && self.event.is_none()
            && self.submitting.is_none()
            && self.transmitting.is_none()
    }

    /// When the supervision timeout ends the connection without another
    /// received packet, in radio microseconds; the earliest wins a conflict.
    fn supervision_deadline(&self) -> u64 {
        let since = self.last_activity.unwrap_or(self.phase.reference);
        since.as_micros().saturating_add(u64::from(
            self.current_timing().supervision_timeout_micros(),
        ))
    }

    /// The request of a planned event.
    fn submit_event(
        &mut self,
        ids: &mut u32,
        prepared: LePeripheralConnectionEventPrepared,
        plan: timing::Plan,
        reservation: LeWindow,
    ) -> ConnectionEvent {
        let first = prepared.first_transmit_window_micros().is_some();
        let coexistence = coexistence::peripheral_connection_level(
            prepared.event_counter(),
            prepared.state().supervision_anchor_event_counter(),
            prepared.timing().interval_micros(),
            self.local_procedure_pending(),
        );
        let channel = DataChannel::new(prepared.channel().get()).expect("a data channel index");
        let interval = RadioDuration::from_micros(u64::from(prepared.timing().interval_micros()));
        let id = crate::controller::allocate_event(ids);
        trace_event(Op::Requested, prepared.event_counter(), id);
        let event = Event {
            id,
            reservation,
            since_reference: plan.since_reference,
            transmit_window: plan.transmit_window,
        };
        self.submitting = Some((prepared, event));
        ConnectionEvent {
            id,
            connection: self.id,
            channel,
            window: plan.window,
            interval,
            timing: plan.timing,
            priority: if first { FIRST_PRIORITY } else { PRIORITY },
            coexistence,
        }
    }

    fn owns(&self, id: EventId) -> bool {
        self.event.is_some_and(|event| event.id == id)
    }

    fn has_transmission(&self) -> bool {
        self.security.pending_response().is_some()
            || self.control.pending_response().is_some()
            || self.host_acl.is_some()
    }

    /// Choose the next PDU and write it to `tx`: its kind and plaintext
    /// length.
    fn prepare_transmission(&mut self) -> Option<(Pending, usize)> {
        if let Some(response) = self.security.pending_response() {
            let bytes = response.as_bytes();
            self.tx[..bytes.len()].copy_from_slice(bytes);
            return Some((Pending::Security, bytes.len()));
        }
        if let Some(response) = self.control.pending_response() {
            let bytes = response.as_bytes();
            if self.security.blocks_unrelated_transmission()
                && !self.security.permits_control_pdu(bytes)
            {
                return None;
            }
            let length = bytes.len();
            self.tx[..length].copy_from_slice(bytes);
            return Some((Pending::Control, length));
        }
        if self.security.blocks_unrelated_transmission() {
            return None;
        }
        let fragment = self.host_acl.as_ref()?.next_fragment(self.acl_payload())?;
        let length = fragment.payload().len();
        self.tx[..length].copy_from_slice(fragment.payload());
        Some((Pending::Acl, length))
    }

    /// The longest ACL fragment the effective transmit length allows; an
    /// encrypted fragment leaves room for its MIC in the air time.
    fn acl_payload(&self) -> usize {
        let mic = if self.security.is_active() {
            LE_ACL_MIC_BYTES as u16
        } else {
            0
        };
        usize::from(self.control.data_lengths().transmit.payload_octets(mic))
    }

    fn acl_kind(&self) -> DataPduKind {
        let continuing = self
            .host_acl
            .as_ref()
            .and_then(|packet| packet.next_fragment(self.acl_payload()))
            .is_some_and(|fragment| fragment.is_continuing());
        if continuing {
            DataPduKind::Continuation
        } else {
            DataPduKind::Start
        }
    }

    /// The payload length on air: encrypted packets carry a MIC. Security
    /// responses arrive already encrypted when they must be.
    fn tx_length(&mut self, pending: Pending, length: usize) -> usize {
        if pending == Pending::Security {
            return length;
        }
        let header = match pending {
            Pending::Acl => match self.acl_kind() {
                DataPduKind::Continuation => 0x01,
                _ => 0x02,
            },
            _ => 0x03,
        };
        match self.security.active_encryption() {
            Some(encryption) => encryption
                .encrypt_new_packet(header, &mut self.tx, length)
                .unwrap_or(length),
            None => length,
        }
    }

    fn enqueued(&mut self, pending: Pending, length: usize) {
        match pending {
            Pending::Security => {
                let _ = self.security.response_enqueued();
            }
            Pending::Control => self.control.response_enqueued(),
            Pending::Acl => {
                if let Some(packet) = &mut self.host_acl {
                    packet.fragment_enqueued(length);
                }
            }
        }
    }

    fn acknowledged(&mut self, events: &mut Events) {
        match self.pending.take() {
            Some(Pending::Security) => self.security.observe_transmission_completion(true),
            Some(Pending::Control) => self.control.observe_transmission_completion(true),
            Some(Pending::Acl) => {
                if let Some(packet) = &mut self.host_acl {
                    packet.observe_fragment_completion(true);
                    if packet.is_complete() {
                        self.host_acl = None;
                        events.push(self.handle, PeripheralEvent::CompletedPackets(1));
                    }
                }
            }
            None => {}
        }
        self.poll_procedures(events);
    }

    fn receive(
        &mut self,
        pdu: &[u8],
        random: Option<&dyn LeRandomSource>,
        local_version: Option<LeVersionInformation>,
        events: &mut Events,
    ) -> Option<LeControllerAclPacket> {
        let mut received = None;
        #[cfg(feature = "diagnostic-mic-fault")]
        let mut corrupted = [0; receive::PDU_CAPACITY];
        #[cfg(feature = "diagnostic-mic-fault")]
        let pdu = match self
            .corrupt_next_mic
            .then(|| crate::diagnostic::corrupt_data_mic(pdu, &mut corrupted))
            .flatten()
        {
            Some(corrupted) if self.security.is_active() => {
                self.corrupt_next_mic = false;
                corrupted
            }
            _ => pdu,
        };
        let decoded = receive::decode(&mut self.security, pdu, &mut self.rx, || {
            let source = random?;
            let diversifier = source.random_bytes().ok()?;
            let vector = source.random_bytes().ok()?;
            Some(LePeripheralEncryptionRandom::new(
                diversifier,
                [vector[0], vector[1], vector[2], vector[3]],
            ))
        });
        if let Some(decoded) = decoded {
            match self.control.receive(decoded, local_version) {
                Ok(LePeripheralReceive::Data(fragment)) => {
                    received = LeControllerAclPacket::copy_from_ll(
                        self.handle,
                        fragment.is_continuing(),
                        fragment.payload(),
                    );
                }
                Ok(LePeripheralReceive::Control) => {}
                Ok(LePeripheralReceive::ChannelMapUpdate(update)) => {
                    self.channel_map = Some(update);
                }
                Ok(LePeripheralReceive::ConnectionUpdate(update)) => {
                    self.connection_update = Some(update);
                }
                Err(LePeripheralControlError::PeerTermination { reason }) => {
                    self.peer_termination = Some(reason);
                }
                Err(error) => {
                    if let Some(reason) = error.termination_reason() {
                        self.control.request_local_termination(reason);
                    }
                }
            }
        }
        self.poll_procedures(events);
        received
    }

    /// Report procedure results and required terminations.
    fn poll_procedures(&mut self, events: &mut Events) {
        if let Some(request) = self.security.take_long_term_key_request() {
            events.push(
                self.handle,
                PeripheralEvent::LongTermKeyRequest {
                    random: request.random_number(),
                    diversifier: request.encrypted_diversifier(),
                },
            );
        }
        if self.security.take_encryption_enabled() {
            events.push(
                self.handle,
                PeripheralEvent::EncryptionChanged(Status::SUCCESS, true),
            );
        }
        if self.security.take_encryption_refreshed() {
            events.push(self.handle, PeripheralEvent::KeyRefreshed(Status::SUCCESS));
        }
        match self.security.termination() {
            Some(LePeripheralEncryptionTermination::Terminate(reason)) => {
                self.control.request_local_termination(reason);
            }
            Some(LePeripheralEncryptionTermination::Exit) if self.closing.is_none() => {
                self.closing = Some(Closing {
                    reason: Some(Status::new(MIC_FAILURE)),
                    cancel_sent: false,
                });
            }
            Some(LePeripheralEncryptionTermination::Exit) | None => {}
        }
        if let Some(result) = self.control.take_remote_features_result() {
            events.push(self.handle, features_event(result));
        }
        if let Some(result) = self.control.take_remote_version_result() {
            events.push(self.handle, version_event(result));
        }
        if let Some(lengths) = self.control.take_data_length_change() {
            events.push(self.handle, PeripheralEvent::DataLengthChanged(lengths));
        }
    }

    /// Procedure timeouts, checked before planning.
    fn check_procedures(&mut self, now: LeInstant, events: &mut Events) {
        if !self.local_procedure_pending() {
            self.procedure_since = None;
            return;
        }
        let since = self.procedure_since.unwrap_or(now);
        // A procedure that started after `now` has not run out.
        if !now
            .checked_duration_since(since)
            .is_some_and(|elapsed| elapsed >= PROCEDURE_TIMEOUT)
        {
            self.procedure_since = Some(since);
            return;
        }
        self.procedure_since = None;
        if !self.security.is_idle() && !self.security.is_active() {
            self.control.request_local_termination(
                HciError::LMP_LL_RESPONSE_TIMEOUT.to_status().into_inner(),
            );
        } else {
            self.control.expire_local_procedure();
            self.poll_procedures(events);
        }
    }

    /// Whether the supervision timeout of `timing` passed between the last
    /// peer activity and `at`; activity after `at` keeps the link.
    fn supervision_lost(&self, at: LeInstant, timing: LeConnectionTiming) -> bool {
        let timeout = RadioDuration::from_micros(u64::from(timing.supervision_timeout_micros()));
        self.last_activity.is_some_and(|last| {
            at.checked_duration_since(last)
                .is_some_and(|elapsed| elapsed > timeout)
        })
    }

    /// The connection parameters in force: the Link Layer's, which a
    /// Connection Update changes at its instant, and the indication's until
    /// the first event completes.
    fn current_timing(&self) -> LeConnectionTiming {
        match &self.link {
            Link::Completed(completed) => completed.timing(),
            _ => self.request.timing(),
        }
    }

    /// Whether a local procedure awaits the central.
    fn local_procedure_pending(&self) -> bool {
        !self.security.is_idle() && !self.security.is_active()
            || self.control.local_feature_request_transmitted()
            || self.control.local_version_request_transmitted()
            || self.control.local_data_length_request_transmitted()
    }

    fn close_procedures(&mut self, events: &mut Events) {
        self.control.close_remote_feature_request();
        self.control.close_remote_version_request();
        self.poll_procedures(events);
        if self.host_acl.take().is_some() {
            // The Host's credit returns with the connection.
        }
    }

    /// Apply what the ended event decided.
    fn after_event(&mut self, events: &mut Events) {
        let Link::Completed(completed) = &mut self.link else {
            return;
        };
        if let Some(transition) = completed.connection_timing_transition()
            && transition.host_parameters_changed()
        {
            let timing = transition.updated();
            events.push(
                self.handle,
                PeripheralEvent::ConnectionUpdated {
                    interval_units: timing.interval_units(),
                    latency: timing.peripheral_latency(),
                    timeout_units: timing.supervision_timeout_units(),
                },
            );
        }
        let mut failure = None;
        if let Some(update) = self.channel_map.take()
            && let Err(error) =
                completed.schedule_channel_map_update(update.channel_map(), update.instant())
        {
            failure = LePeripheralControlError::ChannelMapUpdate(error).termination_reason();
        }
        if let Some(update) = self.connection_update.take()
            && let Err(error) =
                completed.schedule_connection_update(update.timing(), update.instant())
        {
            failure = LePeripheralControlError::ConnectionUpdate(error).termination_reason();
        }
        if let Some(reason) = failure {
            self.control.request_local_termination(reason);
        }
        let established = completed.connection_state() != LePeripheralConnectionState::Created;
        let reason = if let Some(reason) = self.peer_termination {
            Some(Status::new(reason))
        } else if self.control.local_termination_acknowledged() {
            self.control
                .local_termination_completion_reason()
                .map(Status::new)
        } else if !established && completed.establishment_failed() {
            Some(HciError::CONN_FAILED_SYNCHRONIZATION_TIMEOUT.to_status())
        } else {
            None
        };
        if let Some(reason) = reason
            && self.closing.is_none()
        {
            self.closing = Some(Closing {
                reason: Some(reason),
                cancel_sent: false,
            });
        }
        self.poll_procedures(events);
    }

    /// Plan the next event no earlier than `earliest`; an event whose
    /// reservation overlaps a `blocked` one, another connection's event in
    /// progress, is missed.
    fn plan(
        &mut self,
        earliest: LeInstant,
        timing: RadioTiming,
        blocked: impl Iterator<Item = LeWindow> + Clone,
    ) -> Result<Option<(LePeripheralConnectionEventPrepared, timing::Plan, LeWindow)>, EpochExhausted>
    {
        match core::mem::replace(&mut self.link, Link::Moved) {
            Link::Created(connection) => {
                let prepared = connection.prepare_event();
                // The first anchor is the phase's reference.
                let geometry = timing::first(
                    self.phase.reference,
                    self.phase.transmit_window,
                    timing.connection,
                )
                .and_then(|plan| {
                    timing
                        .reservation(plan.window)
                        .map(|reservation| (plan, reservation))
                        .map_err(|OutsideEpoch| error(O::Event, C::Reservation))
                });
                let (plan, reservation) = match geometry {
                    Ok(geometry) => geometry,
                    Err(cause) => {
                        self.link = Link::Created(prepared.cancel());
                        return Err(cause);
                    }
                };
                if plan.window.start() >= earliest
                    && !blocked.clone().any(|window| window.overlaps(reservation))
                {
                    return Ok(Some((prepared, plan, reservation)));
                }
                // A legitimate missed first window advances the LL policy.
                self.link = Link::Completed(
                    prepared
                        .into_submitted()
                        .complete(LePeripheralConnectionEventPeerActivity::Missed),
                );
                self.plan(earliest, timing, blocked)
            }
            Link::Completed(completed) => self.plan_recurring(completed, earliest, timing, blocked),
            link => {
                self.link = link;
                Ok(None)
            }
        }
    }

    fn plan_recurring(
        &mut self,
        mut completed: LePeripheralConnectionEventCompleted,
        earliest: LeInstant,
        timing: RadioTiming,
        blocked: impl Iterator<Item = LeWindow> + Clone,
    ) -> Result<Option<(LePeripheralConnectionEventPrepared, timing::Plan, LeWindow)>, EpochExhausted>
    {
        let peer_ppm = self.request.sleep_clock_accuracy().worst_case_ppm();
        let mut delta = 1_u16;
        for _ in 0..PLAN_ATTEMPTS {
            let step = LePeripheralConnectionEventDelta::new(delta)
                .expect("a validated nonzero event distance");
            let provisional = completed.prepare_recurring_event(step);
            let geometry = (|| {
                let (since_reference, transmit_window) = self.span_of(&provisional)?;
                let anchor = self
                    .phase
                    .reference
                    .checked_add(since_reference)
                    .ok_or(error(O::Event, C::Recurrence))?;
                if self.supervision_lost(anchor, provisional.timing()) {
                    return Ok(RecurringCandidate::Lost(HciError::CONN_TIMEOUT.to_status()));
                }
                if provisional.connection_state() == LePeripheralConnectionState::Created
                    && provisional.event_counter() >= 6
                {
                    return Ok(RecurringCandidate::Lost(
                        HciError::CONN_FAILED_SYNCHRONIZATION_TIMEOUT.to_status(),
                    ));
                }
                let plan = timing::recurring(
                    self.phase.reference,
                    since_reference,
                    transmit_window,
                    peer_ppm,
                    timing.connection,
                )?;
                let reservation = timing
                    .reservation(plan.window)
                    .map_err(|OutsideEpoch| error(O::Event, C::Reservation))?;
                if plan.window.start() >= earliest {
                    if blocked.clone().any(|window| window.overlaps(reservation)) {
                        // Another connection holds the radio: this event is
                        // missed, the next one tried.
                        return Ok(match delta.checked_add(1) {
                            Some(next) => RecurringCandidate::Late(next),
                            None => RecurringCandidate::Lost(HciError::CONN_TIMEOUT.to_status()),
                        });
                    }
                    return Ok(RecurringCandidate::Ready(plan, reservation));
                }
                let interval = RadioDuration::<LeRadio>::from_micros(u64::from(
                    provisional.timing().interval_micros(),
                ));
                // The decoded wire interval is nonzero. A late window may
                // have its nominal anchor ahead of earliest because of widening.
                let behind = earliest
                    .checked_duration_since(plan.anchor)
                    .map_or(0, RadioDuration::as_micros);
                let skip = (behind / interval.as_micros()).max(1);
                // More than 65,535 intervals behind outlasts every supervision
                // timeout, at most 32 s over intervals of at least 7.5 ms.
                Ok(
                    match u64::from(delta)
                        .checked_add(skip)
                        .and_then(|next| u16::try_from(next).ok())
                    {
                        Some(next) => RecurringCandidate::Late(next),
                        None => RecurringCandidate::Lost(HciError::CONN_TIMEOUT.to_status()),
                    },
                )
            })();
            match geometry {
                Ok(RecurringCandidate::Ready(plan, reservation)) => {
                    return Ok(Some((provisional.commit(), plan, reservation)));
                }
                Ok(RecurringCandidate::Lost(reason)) => {
                    self.link = Link::Completed(provisional.cancel());
                    self.closing = Some(Closing {
                        reason: Some(reason),
                        cancel_sent: false,
                    });
                    return Ok(None);
                }
                Ok(RecurringCandidate::Late(next)) => {
                    completed = provisional.cancel();
                    delta = next;
                }
                Err(cause) => {
                    self.link = Link::Completed(provisional.cancel());
                    return Err(cause);
                }
            }
        }
        self.link = Link::Completed(completed);
        Ok(None)
    }

    /// From the phase's reference to the anchor of `provisional`, and the
    /// transmit window still uncertain there.
    fn span_of(
        &self,
        provisional: &oer_bluetooth_ll::connection::LePeripheralConnectionRecurringEventProvisional,
    ) -> Result<(RadioDuration<LeRadio>, RadioDuration<LeRadio>), EpochExhausted> {
        let delta = provisional.delta().get();
        // Intervals are at most 4 s and counts at most 65,535, so each
        // product and their sum stays far below `u64::MAX`.
        let (span, transmit_window) = match (
            provisional.connection_timing_transition(),
            provisional.connection_timing_intervals(),
        ) {
            (Some(transition), Some(intervals)) => {
                let previous = u64::from(transition.previous().interval_micros());
                let updated = transition.updated();
                (
                    previous * u64::from(intervals.before)
                        + u64::from(updated.window_offset_units()) * 1_250
                        + u64::from(updated.interval_micros()) * u64::from(intervals.after),
                    RadioDuration::from_micros(u64::from(updated.window_size_units()) * 1_250),
                )
            }
            _ => (
                u64::from(provisional.timing().interval_micros()) * u64::from(delta),
                self.phase.transmit_window,
            ),
        };
        let since_reference = self
            .phase
            .since_reference
            .checked_add(RadioDuration::from_micros(span))
            .ok_or(error(O::Event, C::Recurrence))?;
        Ok((since_reference, transmit_window))
    }
}

fn features_event(result: LeRemoteFeaturesResult) -> PeripheralEvent {
    match result {
        LeRemoteFeaturesResult::Success(features) => {
            PeripheralEvent::RemoteFeatures(Status::SUCCESS, features)
        }
        LeRemoteFeaturesResult::Unsupported => PeripheralEvent::RemoteFeatures(
            HciError::UNSUPPORTED_REMOTE_FEATURE.to_status(),
            [0; 8],
        ),
        LeRemoteFeaturesResult::Rejected { reason } => {
            PeripheralEvent::RemoteFeatures(Status::new(reason), [0; 8])
        }
        LeRemoteFeaturesResult::ResponseTimeout => {
            PeripheralEvent::RemoteFeatures(HciError::LMP_LL_RESPONSE_TIMEOUT.to_status(), [0; 8])
        }
        LeRemoteFeaturesResult::ConnectionClosed => {
            PeripheralEvent::RemoteFeatures(HciError::UNKNOWN_CONN_IDENTIFIER.to_status(), [0; 8])
        }
    }
}

fn version_event(result: LeRemoteVersionResult) -> PeripheralEvent {
    let none = LeVersionInformation::new(0, 0, 0);
    match result {
        LeRemoteVersionResult::Success(version) => {
            PeripheralEvent::RemoteVersion(Status::SUCCESS, version)
        }
        LeRemoteVersionResult::Unsupported => {
            PeripheralEvent::RemoteVersion(HciError::UNSUPPORTED_REMOTE_FEATURE.to_status(), none)
        }
        LeRemoteVersionResult::Rejected { reason } => {
            PeripheralEvent::RemoteVersion(Status::new(reason), none)
        }
        LeRemoteVersionResult::ResponseTimeout => {
            PeripheralEvent::RemoteVersion(HciError::LMP_LL_RESPONSE_TIMEOUT.to_status(), none)
        }
        LeRemoteVersionResult::ConnectionClosed => {
            PeripheralEvent::RemoteVersion(HciError::UNKNOWN_CONN_IDENTIFIER.to_status(), none)
        }
    }
}

const fn clock_accuracy(encoded: u8) -> ClockAccuracy {
    match encoded {
        0 => ClockAccuracy::Ppm500,
        1 => ClockAccuracy::Ppm250,
        2 => ClockAccuracy::Ppm150,
        3 => ClockAccuracy::Ppm100,
        4 => ClockAccuracy::Ppm75,
        5 => ClockAccuracy::Ppm50,
        6 => ClockAccuracy::Ppm30,
        _ => ClockAccuracy::Ppm20,
    }
}

enum RecurringCandidate {
    Ready(timing::Plan, LeWindow),
    Late(u16),
    Lost(Status),
}

/// Record one step of a connection event.
fn trace_event(op: Op, counter: u16, id: EventId) {
    crate::trace::emit(|| oer_bluetooth_trace::PeripheralEvent {
        op,
        counter,
        id: id.get(),
    });
}

fn error(operation: O, calculation: C) -> EpochExhausted {
    EpochExhausted::at(R::Peripheral, operation, calculation)
}

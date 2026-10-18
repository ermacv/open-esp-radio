//! Lowering of radio requests into the role pools and the executor.

use core::{convert::Infallible, num::NonZeroU64};

use crate::coexistence::{self, CoexistenceProfile};
use oer_bluetooth_radio::{
    AcceptListChange, AcceptListDevice, AdvertisingChannel, AdvertisingConfiguration,
    AdvertisingEvent, AdvertisingReception, AdvertisingSetId, CancelError, ConnectionAllowances,
    ConnectionConfiguration, ConnectionEvent, ConnectionEventTiming, ConnectionId, DataPduKind,
    EventId, EventResult, LeCapabilities, LeConnectionCapabilities, LeInstant, LePhys, LeRadio,
    LeRadioCapabilities, LeWindow, LegacyAdvertisingTypes, LinkAcknowledgement,
    NonZeroRadioDuration, OutsideEpoch, RadioDuration, RadioOutcome, RadioRequest, RadioTiming,
    ReceivedPdu, RequestError, ScanFilterPolicy, ScanType, ScanWindow, ScannerConfiguration,
    ScannerId, TestPhy, TestReceive, TestReport, TestTransmit, TxPower,
};
use oer_esp32s31_bluetooth::{
    ControllerSchedulerEpoch, ControllerTimeSample,
    scheduler::{
        SchedulerExecutor, SchedulerHardwareView, SchedulerIdleInsertion, SchedulerNotStopped,
        SchedulerObservation, SchedulerRawWindow, SchedulerSoftwareConfig, SchedulerStep,
        SchedulerTimingPolicy, SchedulerTransactionFault,
    },
};
use oer_esp32s31_bluetooth_memory::{
    BLUETOOTH_LE_RX_PAYLOAD_CAPACITY, BlePhyLe1MPacketStartCalibration,
    DirectionFindingWorkspaceLink, DtmPool, DtmReceiverEventPhase, DtmRole,
    DtmSchedulerItemCompletionStatus, DtmSchedulerItemEventType, DtmSchedulerReceiverPhy,
    DtmSchedulerTransmitterPhy, LeDeviceTable, LeDeviceTableError, LeDeviceTablePublication,
    LeFilterAcceptListDevice, LeRxChain, LeRxOutcome, LeRxSource, LeTxPower, LegacyAdvertisingPool,
    LegacyAdvertisingPrimaryChannelPlan, LegacyConnectableAdvIndPacketInput,
    LegacyConnectableAdvertisingMemoryInput, LegacyConnectableAdvertisingOwnAddress,
    LegacyConnectableAdvertisingPool, LegacyConnectableScanResponsePacketInput,
    LegacyScanEventTiming, LegacyScanFilterPolicy, LegacyScanPool, LegacyScanPrimaryChannel,
    LegacyScanResetConfig, LegacyScanSchedulerWindow, LegacyScanStartSelection, LegacyScanType,
    LegacyScanWindowTicks, PeripheralConnectionCapturedAnchorAvailability,
    PeripheralConnectionDataChannel, PeripheralConnectionEventSpan, PeripheralConnectionFirstEvent,
    PeripheralConnectionIdentity, PeripheralConnectionPool, PeripheralConnectionReceiveTime,
    PeripheralConnectionReceiveWait, PeripheralConnectionRecurringEvent,
    PeripheralConnectionRecurringReceiveWait, PeripheralConnectionSchedulerItemCompletionStatus,
    PeripheralConnectionSchedulerPriority, PeripheralConnectionSchedulerWindow,
    PeripheralConnectionTransmitPduKind, SchedulerItemCompletionStatus, SchedulerItemId,
    SchedulerItemSpace, SchedulerRoleInstance, SchedulerRoleKind,
};
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerReset, BluetoothControllerTimeScale, BluetoothSchedulerStopped,
};

/// Reservations may end at most this far after the current time; wrapping
/// controller time orders windows only within a half-range.
const MAX_AHEAD_MICROS: u64 = 1 << 30;
/// The longest first-event wait the short receive-wait form holds: its
/// 65,534 microseconds less the 61-microsecond PHY allowance
/// (`PeripheralConnectionReceiveWait`).
const FIRST_RECEIVE_WAIT_MAX_MICROS: u64 = 65_534 - 61;
/// The longest recurring receive wait the long form holds, in two-microsecond
/// units (`PeripheralConnectionRecurringReceiveWait`).
const RECEIVE_WAIT_MAX_MICROS: u64 = 131_070;
/// The longest recurring receive wait the short form holds to the
/// microsecond.
const RECEIVE_WAIT_SHORT_MAX_MICROS: u64 = 65_534;
/// The positional DTM hardware profile of the reviewed standalone build.
const DTM_REVIEWED_CONFIG: u8 = 3;
/// The largest LE Test payload.
const DTM_MAX_PAYLOAD: usize = 255;

/// The pools and receive chains of one radio.
pub struct BluetoothRadioMemory<
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
> {
    /// Non-connectable advertising sets.
    pub legacy: LegacyAdvertisingPool<LEGACY>,
    /// Response-capable advertising sets.
    pub connectable: LegacyConnectableAdvertisingPool<CONNECTABLE>,
    /// Passive scanners.
    pub scanners: LegacyScanPool<SCANNERS>,
    /// Peripheral connections.
    pub connections: PeripheralConnectionPool<CONNECTIONS>,
    /// The Direct Test Mode instance.
    pub dtm: DtmPool<1>,
    /// The global scanning receive chain.
    pub scanning: LeRxChain<SCAN_PACKETS>,
    /// The global non-scanning receive chain.
    pub non_scanning: LeRxChain<RX_PACKETS>,
    /// The controller-global direction-finding workspace.
    pub direction_finding: DirectionFindingWorkspaceLink,
    /// The device table that holds the filter accept list.
    pub device_table: LeDeviceTable,
}

/// Receives the outcomes of one entry. PDUs are lent only for the call.
pub trait BluetoothRadioSink {
    /// Deliver one outcome.
    fn outcome(&mut self, outcome: RadioOutcome<'_>);

    /// The radio faulted for `fault` and schedules nothing until a
    /// Controller reset: its port is poisoned with this cause.
    fn fault(&mut self, fault: RadioFault);
}

/// Why the radio stopped scheduling: the cause its port is poisoned with.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioFault {
    /// Hardware reported a result the radio cannot interpret.
    UnsupportedHardwareResult,
    /// A receive or transmit structure was found inconsistent.
    MemoryInconsistency,
    /// Hardware executed events out of their scheduled order.
    OutOfOrderCompletion,
}

/// The radio's state is unknown: a fault was reported to its sink, now or
/// earlier, and it admits nothing until a Controller reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioFaulted;

/// Why one admission did not take place.
enum Admit {
    /// The world refused it; nothing changed.
    Refused(RequestError),
    /// The radio's own structures disagree: a fault.
    Fault(RadioFault),
    /// The radio faulted before.
    Faulted,
}

impl From<RequestError> for Admit {
    fn from(error: RequestError) -> Self {
        Self::Refused(error)
    }
}

/// Hardware work the caller performs next.
#[derive(Debug)]
pub enum RadioStep<const ITEMS: usize> {
    /// Nothing to do.
    Idle,
    /// The scheduler is idle: publish this head and start the scheduler.
    Start(SchedulerIdleInsertion),
    /// Stop the scheduler and hand the stopped receipt to
    /// [`BluetoothRadio::enter_stopped`]: a test event is published only as
    /// the head of an idle scheduler, and a listed test event leaves its list
    /// only after a stop.
    StopScheduler,
    /// The first event of a test session is ready on an idle scheduler:
    /// disable the BLE PHY ETM route, keep its owner until
    /// [`RadioStep::LeaveTest`], then publish the head and start the
    /// scheduler.
    EnterTest(SchedulerIdleInsertion),
    /// The test session ended: restore the BLE PHY ETM route with the owner
    /// taken at [`RadioStep::EnterTest`].
    LeaveTest,
    /// Perform the actions of a list transaction and deliver the awaited
    /// observation to [`BluetoothRadio::advance`].
    Transaction(SchedulerStep<SchedulerItemId, ITEMS>),
}

/// One event of an instance.
#[derive(Clone, Copy, Debug)]
struct Event {
    id: EventId,
    /// Items with the executor or its insertion.
    listed: u8,
    /// Items waiting for insertion.
    pending: u8,
    executed: bool,
    /// An item was released after a scheduler stop without a completion
    /// status, though its start had passed: the hardware may have run it.
    unconfirmed: bool,
    cancel: bool,
    /// DTM receiver event.
    receiver: bool,
    /// Scanner item.
    item: u8,
}

impl Event {
    const fn new(id: EventId, items: u8) -> Self {
        Self {
            id,
            listed: 0,
            pending: items,
            executed: false,
            unconfirmed: false,
            cancel: false,
            receiver: false,
            item: 0,
        }
    }

    const fn settled(&self) -> bool {
        self.listed == 0 && self.pending == 0
    }
}

/// One configured role instance.
struct Slot<Id> {
    id: Id,
    instance: SchedulerRoleInstance,
    event: Option<Event>,
}

/// Connection facts the first event needs.
#[derive(Clone, Copy)]
struct ConnectionFacts {
    created_at: LeInstant,
    tx_power: TxPower,
}

// Source-owned S31 connection-event policy. The provenance of each term is
// the recurring-event section of
// docs/vendor/esp32s31/bluetooth-peripheral-connection.md:
// the 16-us widening jitter is the Controller configuration's default
// `ble_ll_jitter_usecs`, the 10-us receive guard is the private-options
// default, the 2-us receive tail and the 1-us boundary are CPU-time ticks, and
// 1,074 us is the minimum LE 1M event duration. A recurring event ends
// 1,074 us after its widened anchor less the preparation lead; the first
// event ends 1,074 us plus the boundary after its transmit window, with the
// 16-us first-event uncertainty on each side.
const WIDENING_JITTER_MICROS: u32 = 16;
const RECEIVE_GUARD_MICROS: u32 = 10;
const RECEIVE_TAIL_MICROS: u32 = 2;
const BOUNDARY_GUARD_MICROS: u32 = 1;
const FIRST_EVENT_GUARD_MICROS: u32 = 16;
/// SOURCE: pinned `libble_app.a[ble_28.o]` table
/// `sym_ble_Qw8LKJo0HxALvv5KCN4X`, which `r_ble_ll_conn_get_min_dura_required`
/// (`r_sym_ble_IXD9YD54AdqppMHisVZw`), `ble_ll_conn_created`
/// (`r_sym_ble_62UX9ux9YcuUoIf4gPAp`) and the move body
/// (`r_sym_ble_JqbtnypGd2wyQym9wmK4`) index by PHY; LE 1M is its first entry.
const LE_1M_EVENT_MICROS: u32 = 1_074;

/// SOURCE: pinned `libble_app.a[ble_3.o]` connection-event setup
/// `r_sym_ble_rsCCyH2B22gdYkN4LOOJ`, which installs the link-state event span
/// from the connection interval: the whole interval up to 7,499 us, otherwise
/// the interval less 2,000 us plus the peripheral's window widening. Its
/// third branch, for an exchange of maximum-length packets that does not fit
/// the interval less 2,150 us, cannot occur on LE 1M above 7,499 us.
const WHOLE_INTERVAL_SPAN_MICROS: u32 = 7_499;
const LONG_INTERVAL_MARGIN_MICROS: u32 = 2_000;

/// The Controller span of one connection event in microseconds.
fn connection_event_span(
    interval: RadioDuration<LeRadio>,
    widening: RadioDuration<LeRadio>,
) -> RadioDuration<LeRadio> {
    if interval <= RadioDuration::from_micros(u64::from(WHOLE_INTERVAL_SPAN_MICROS)) {
        return interval;
    }
    interval
        .checked_sub(RadioDuration::from_micros(u64::from(
            LONG_INTERVAL_MARGIN_MICROS,
        )))
        .and_then(|span| span.checked_add(widening))
        .expect("a long interval exceeds its margin and a widening fits the epoch")
}

const fn connection_allowances(
    preparation_lead_micros: u32,
    local_sleep_clock_ppm: u16,
) -> ConnectionAllowances {
    ConnectionAllowances {
        local_sleep_clock_ppm,
        widening_jitter: RadioDuration::from_micros(WIDENING_JITTER_MICROS as u64),
        receive_guard: RadioDuration::from_micros(RECEIVE_GUARD_MICROS as u64),
        receive_tail: RadioDuration::from_micros(RECEIVE_TAIL_MICROS as u64),
        boundary_guard: NonZeroRadioDuration::from_micros(
            NonZeroU64::new(BOUNDARY_GUARD_MICROS as u64).unwrap(),
        ),
        first_event_guard: RadioDuration::from_micros(FIRST_EVENT_GUARD_MICROS as u64),
        event_length: RadioDuration::from_micros(
            // The only constructible scheduler profile has a 137-us lead, below 1074 us.
            (LE_1M_EVENT_MICROS - preparation_lead_micros) as u64,
        ),
        first_event_length: RadioDuration::from_micros(
            (LE_1M_EVENT_MICROS + BOUNDARY_GUARD_MICROS) as u64,
        ),
    }
}

/// Portable microseconds the radio epoch keeps free at each end: one full
/// range of the 32-bit controller-time projection.
///
/// A capture lies within the signed half range of the 2^31-microsecond raw
/// counter around the latest controller-time sample, so with this margin
/// below the first instant and above the last one every capture and its
/// packet-start correction are representable.
/// Being a multiple of 2^32, it keeps an instant's low word equal to the
/// controller microseconds it projects.
const EPOCH_MARGIN: u64 = 1 << 32;

// The margin covers the half range of a capture around a sample, and keeps
// an instant's low word equal to the controller microseconds it projects.
const _: () = assert!(EPOCH_MARGIN >= 1 << 31 && EPOCH_MARGIN.is_multiple_of(1 << 32));

/// The last instant of the radio epoch.
const EPOCH_END: u64 = u64::MAX - EPOCH_MARGIN;

/// The monotonic radio epoch over the controller scheduler epoch.
///
/// Invariant: `EPOCH_MARGIN <= now <= EPOCH_END`. The epoch starts
/// [`EPOCH_MARGIN`] above zero, and an observation that would pass
/// [`EPOCH_END`] is refused as exhausted.
struct RadioClock {
    epoch: ControllerSchedulerEpoch,
    now: u64,
}

impl RadioClock {
    fn new(epoch: ControllerSchedulerEpoch, sample: &ControllerTimeSample) -> Self {
        Self {
            now: EPOCH_MARGIN + u64::from(epoch.project_without_reanchor(sample)),
            epoch,
        }
    }

    /// Advance to `sample`; an instant past [`EPOCH_END`] leaves the clock
    /// unchanged and reports the epoch exhausted.
    fn observe(&mut self, sample: &ControllerTimeSample) -> Result<(), OutsideEpoch> {
        let micros = self.epoch.project_without_reanchor(sample);
        let delta = micros.wrapping_sub(self.now as u32) as i32;
        if delta > 0 {
            // `now <= EPOCH_END` leaves room for any 32-bit step.
            let now = self.now + delta as u64;
            if now > EPOCH_END {
                return Err(OutsideEpoch);
            }
            self.now = now;
        }
        self.epoch = self.epoch.reanchor(sample);
        Ok(())
    }

    fn raw(&self, micros: u64) -> u32 {
        self.epoch.raw_ticks_for_micros(micros as u32)
    }

    /// The raw time of `raw` after conversion to scheduler microseconds and
    /// back, as the DTM event bodies derive the sequence start.
    fn round_trip(&self, raw: u32) -> u32 {
        self.epoch
            .raw_ticks_for_micros(self.epoch.project_capture(raw))
    }

    /// `duration` in controller ticks; every duration a placed request
    /// holds lies within its horizon, which the ticks represent.
    fn duration(&self, duration: RadioDuration<LeRadio>) -> u32 {
        self.epoch
            .raw_duration_ticks(duration)
            .expect("a placed duration lies within the horizon")
    }

    /// The instant of a capture, placed by its signed half-range distance
    /// from the latest sample. The epoch margins keep it representable.
    fn instant(&self, raw_capture: u32) -> LeInstant {
        let micros = self.epoch.project_capture(raw_capture);
        let delta = micros.wrapping_sub(self.now as u32) as i32;
        let distance = u64::from(delta.unsigned_abs());
        LeInstant::from_micros(if delta >= 0 {
            self.now + distance
        } else {
            self.now - distance
        })
    }

    /// The on-air start of an LE 1M packet from its receive timestamp.
    fn packet_start(&self, raw_capture: u32) -> LeInstant {
        let delay = BlePhyLe1MPacketStartCalibration::le_1m().capture_delay_micros();
        // A capture lies at least `EPOCH_MARGIN / 2` above zero.
        LeInstant::from_micros(self.instant(raw_capture).as_micros() - u64::from(delay))
    }
}

/// The scheduler policy and sleep-clock accuracy a radio is built with,
/// known when the image is built.
pub trait BluetoothRadioConfig: 'static {
    /// The scheduler policy.
    const SCHEDULER: SchedulerSoftwareConfig;
    /// The board's worst-case sleep-clock accuracy, in parts per million.
    const LOCAL_SLEEP_CLOCK_PPM: u16;
}

/// The capabilities of a radio with configuration `C` and these pools: the
/// capabilities marker of its port, computed by
/// [`BluetoothRadio::capabilities`] from the same timing the radio reserves
/// with.
pub struct BluetoothCapabilities<
    C,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
>(core::marker::PhantomData<fn() -> C>, Infallible);

impl<
    C: BluetoothRadioConfig,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> LeCapabilities
    for BluetoothCapabilities<
        C,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
    >
{
    const CAPS: LeRadioCapabilities = BluetoothRadio::<
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
    >::capabilities(BluetoothRadio::<
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
    >::radio_timing(
        C::SCHEDULER, C::LOCAL_SLEEP_CLOCK_PPM
    ));
}

impl<
    C,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> core::fmt::Debug
    for BluetoothCapabilities<
        C,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
    >
{
    fn fmt(&self, _: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.1 {}
    }
}

impl<
    C,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> PartialEq
    for BluetoothCapabilities<
        C,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
    >
{
    fn eq(&self, _: &Self) -> bool {
        match self.1 {}
    }
}

/// The contract over the executor and the role pools.
// CAPABILITY: bluetooth-le-1m-phy, bluetooth-le-2m-phy, bluetooth-le-coded-s-8-125-kbit-s, bluetooth-le-coded-s-2-500-kbit-s
pub struct BluetoothRadio<
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> {
    memory:
        BluetoothRadioMemory<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS>,
    executor: SchedulerExecutor<SchedulerItemId, ITEMS>,
    pending: [Option<(SchedulerItemId, SchedulerRawWindow)>; ITEMS],
    inserting: Option<SchedulerItemId>,
    legacy: [Option<Slot<AdvertisingSetId>>; LEGACY],
    connectable: [Option<Slot<AdvertisingSetId>>; CONNECTABLE],
    scanners: [Option<Slot<ScannerId>>; SCANNERS],
    connections: [Option<(Slot<ConnectionId>, ConnectionFacts)>; CONNECTIONS],
    dtm: Option<Slot<()>>,
    mode: SchedulerMode,
    clock: RadioClock,
    timing: RadioTiming,
    policy: SchedulerTimingPolicy,
    faulted: bool,
    /// A finished list zero was observed while a list transaction refused
    /// the completion pass; the pass runs when the transaction finishes.
    deferred_completion: bool,
    coexistence: CoexistenceProfile,
}

/// Who owns the scheduler: every role, or a Direct Test Mode session, which
/// owns the Link Layer exclusively (Core Specification Vol 6, Part F).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SchedulerMode {
    Shared,
    Test(TestPhase),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestPhase {
    /// The session holds its instance; no event was published yet and the
    /// BLE PHY ETM route is live.
    Opened,
    /// A test event was published; the route stays disabled until the
    /// session ends.
    Running,
    /// The scheduler must stop before the session continues in the phase
    /// it names.
    Stopping(TestResume),
    /// Test End released the instance; the route restore is due.
    Restoring,
}

/// The phase a stopped test session continues in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TestResume {
    Opened,
    Running,
}

impl TestResume {
    const fn phase(self) -> TestPhase {
        match self {
            Self::Opened => TestPhase::Opened,
            Self::Running => TestPhase::Running,
        }
    }
}

impl TestPhase {
    /// The phase a stop returns to, from a phase that owns a live session.
    const fn resume(self) -> TestResume {
        match self {
            Self::Running | Self::Stopping(TestResume::Running) => TestResume::Running,
            Self::Opened | Self::Stopping(TestResume::Opened) | Self::Restoring => {
                TestResume::Opened
            }
        }
    }
}

impl<
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> BluetoothRadio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>
{
    /// What the radio serves with these pools.
    ///
    /// Advertising, scanning and the peripheral connection run on LE 1M; the
    /// Direct Test Mode transmitter and receiver also run on LE 2M and LE
    /// Coded. The hardware keeps SN and NESN, retransmits the queued PDU
    /// until the peer acknowledges it and answers with empty PDUs, so the
    /// connection's acknowledgement is [`LinkAcknowledgement::Hardware`]. A
    /// data PDU payload, MIC included, fills at most one receive allocation.
    /// `timing` is [`Self::radio_timing`] of the epoch's scheduler policy.
    pub const fn capabilities(timing: RadioTiming) -> LeRadioCapabilities {
        LeRadioCapabilities {
            legacy_advertising: LegacyAdvertisingTypes {
                nonconnectable: LEGACY > 0,
                responsive: CONNECTABLE > 0,
            },
            passive_scanning: SCANNERS > 0,
            active_scanning: SCANNERS > 0,
            filter_accept_list: true,
            peripheral_connection: if CONNECTIONS > 0 {
                Some(LeConnectionCapabilities {
                    max_data_payload: BLUETOOTH_LE_RX_PAYLOAD_CAPACITY as u8,
                    link_acknowledgement: LinkAcknowledgement::Hardware,
                    max_first_receive_wait: RadioDuration::from_micros(
                        FIRST_RECEIVE_WAIT_MAX_MICROS,
                    ),
                    max_receive_wait: RadioDuration::from_micros(RECEIVE_WAIT_MAX_MICROS),
                })
            } else {
                None
            },
            direct_test_mode: true,
            phys: LePhys::LE_1M,
            test_phys: LePhys::ALL,
            min_tx_power_dbm: LeTxPower::LOWEST_DBM,
            max_ahead: RadioDuration::from_micros(MAX_AHEAD_MICROS),
            timing,
        }
    }

    /// The timing a planner keeps reservations apart by, for the scheduler
    /// policy `config` and a sleep clock of `local_sleep_clock_ppm`.
    pub const fn radio_timing(
        config: SchedulerSoftwareConfig,
        local_sleep_clock_ppm: u16,
    ) -> RadioTiming {
        RadioTiming {
            preparation_lead: RadioDuration::from_micros(config.preparation_lead_micros() as u64),
            admission_guard: RadioDuration::from_micros(config.late_start_guard_micros() as u64),
            connection: connection_allowances(
                config.preparation_lead_micros(),
                local_sleep_clock_ppm,
            ),
        }
    }

    /// Take the memory of an initialized scheduler epoch. `sample` is the
    /// first live controller-time sample of the epoch and
    /// `local_sleep_clock_ppm` the board's worst-case sleep-clock accuracy.
    pub fn new(
        memory: BluetoothRadioMemory<
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
        >,
        config: SchedulerSoftwareConfig,
        scale: BluetoothControllerTimeScale,
        sample: &ControllerTimeSample,
        local_sleep_clock_ppm: u16,
    ) -> Self {
        let epoch = ControllerSchedulerEpoch::from_first_live_update(sample, scale);
        Self {
            memory,
            executor: SchedulerExecutor::new(),
            pending: [None; ITEMS],
            inserting: None,
            legacy: [const { None }; LEGACY],
            connectable: [const { None }; CONNECTABLE],
            scanners: [const { None }; SCANNERS],
            connections: [const { None }; CONNECTIONS],
            dtm: None,
            mode: SchedulerMode::Shared,
            clock: RadioClock::new(epoch, sample),
            timing: Self::radio_timing(config, local_sleep_clock_ppm),
            policy: SchedulerTimingPolicy::from_scheduler_config(config, scale),
            faulted: false,
            deferred_completion: false,
            coexistence: CoexistenceProfile::Standalone,
        }
    }

    /// Follow the antenna's sharing: from the next event on, the events
    /// carry the coexistence priorities of `profile`, and connections opened
    /// while it is shared are protected.
    pub fn set_coexistence(&mut self, profile: CoexistenceProfile) {
        self.coexistence = profile;
    }

    /// The timing a planner uses to keep reservations apart.
    pub const fn timing(&self) -> RadioTiming {
        self.timing
    }

    /// The current radio time.
    pub const fn now(&self) -> LeInstant {
        LeInstant::from_micros(self.clock.now)
    }

    /// Advance the radio time with one live controller-time sample.
    ///
    /// # Errors
    ///
    /// The sample lies past the last instant of the radio epoch; the radio
    /// time stays where it was. Nothing timed may be admitted any more, and
    /// the work already admitted still settles.
    pub fn observe_time(&mut self, sample: &ControllerTimeSample) -> Result<(), OutsideEpoch> {
        self.clock.observe(sample)
    }

    /// Place the radio time at the last instant of its epoch, so that the
    /// next advancing sample exhausts it; for host models only.
    #[cfg(any(test, all(feature = "test-support", not(target_arch = "riscv32"))))]
    #[doc(hidden)]
    pub fn reach_epoch_end_for_validation(&mut self) {
        self.clock.now = EPOCH_END;
    }

    /// The pools and chains, for publication by the lifecycle owner.
    pub const fn memory(
        &self,
    ) -> &BluetoothRadioMemory<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS>
    {
        &self.memory
    }

    /// The item memory of every pool.
    pub fn item_space(&self) -> SchedulerItemSpace<'_> {
        space(&self.memory)
    }

    /// Take the pools and chains back after the Controller of this epoch was
    /// reset.
    ///
    /// The owner ends every admitted event first
    /// ([`Self::cancel_all_stopped`]); configured roles hold no event then.
    /// The pools, withheld items included, and the receive chains return to
    /// their allocation-time image, ready for the next epoch.
    pub fn into_memory(
        self,
        reset: &BluetoothControllerReset,
    ) -> BluetoothRadioMemory<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS>
    {
        let mut memory = self.memory;
        memory.legacy.reset_after_controller_reset(reset);
        memory.connectable.reset_after_controller_reset(reset);
        memory.scanners.reset_after_controller_reset(reset);
        memory.connections.reset_after_controller_reset(reset);
        memory.dtm.reset_after_controller_reset(reset);
        memory.scanning.reset_after_controller_reset(reset);
        memory.non_scanning.reset_after_controller_reset(reset);
        memory
    }

    /// Whether a fault stopped the radio: it reported a [`RadioFault`] to
    /// its sink and schedules nothing until a Controller reset. The runtime refuses requests to a faulted radio as poisoned
    /// before it asks [`Self::request`].
    pub const fn is_faulted(&self) -> bool {
        self.faulted
    }

    /// Admit one request of a backend whose capabilities are `K`: every
    /// value it carries is supported and every window placed within their
    /// horizon, so the radio refuses only what the world decides (time, the
    /// epoch) and what its pools and lists hold now. A disagreement of its
    /// own structures is a fault, reported to `sink` and returned as
    /// [`RadioFaulted`]; a faulted radio ([`Self::is_faulted`]) admits
    /// nothing and returns it again.
    pub fn request<K: LeCapabilities>(
        &mut self,
        request: RadioRequest<'_, K>,
        sink: &mut impl BluetoothRadioSink,
    ) -> Result<Result<(), RequestError>, RadioFaulted> {
        // The core placed the request with the reservation this radio uses.
        debug_assert_eq!(K::CAPS.timing, self.timing);
        let kind = crate::trace::request_kind(&request);
        let start = match &request {
            RadioRequest::Advertise(event) => Some(event.get().anchor),
            RadioRequest::Scan(window) => Some(window.get().window.start()),
            RadioRequest::ConnectionEvent(event) => Some(event.get().window.start()),
            RadioRequest::TestTransmit(test) => Some(test.get().window.start()),
            RadioRequest::TestReceive(test) => Some(test.get().window.start()),
            _ => None,
        };
        let now = self.now();
        let result = match self.admit(request) {
            Ok(()) => Ok(Ok(())),
            Err(Admit::Refused(error)) => Ok(Err(error)),
            Err(Admit::Fault(fault)) => {
                self.fault(fault, sink);
                Err(RadioFaulted)
            }
            Err(Admit::Faulted) => Err(RadioFaulted),
        };
        crate::trace::emit(|| oer_bluetooth_trace::RoleRequest {
            kind,
            answer: crate::trace::answer(result),
            // The lead to a window past the radio time, within 2^32 us.
            lead_micros: start
                .and_then(|start| start.checked_duration_since(now))
                .and_then(|lead| u32::try_from(lead.as_micros()).ok()),
        });
        result
    }

    fn admit<K: LeCapabilities>(&mut self, request: RadioRequest<'_, K>) -> Result<(), Admit> {
        if self.faulted {
            return Err(Admit::Faulted);
        }
        // A test session owns the Link Layer: no other role schedules air
        // activity until Test End.
        if matches!(self.mode, SchedulerMode::Test(_))
            && matches!(
                request,
                RadioRequest::Advertise(_)
                    | RadioRequest::Scan(_)
                    | RadioRequest::OpenConnection(_)
                    | RadioRequest::ConnectionEvent(_)
                    | RadioRequest::Transmit { .. }
            )
        {
            return Err(RequestError::Busy.into());
        }
        match request {
            RadioRequest::ConfigureAdvertising(configuration) => {
                self.configure_advertising(*configuration.get())
            }
            RadioRequest::Advertise(event) => self.advertise(*event.get()),
            RadioRequest::RemoveAdvertising(set) => Ok(self.remove_advertising(set)?),
            RadioRequest::ConfigureScanner(configuration) => {
                self.configure_scanner(*configuration.get())
            }
            RadioRequest::Scan(window) => self.scan(*window.get()),
            RadioRequest::RemoveScanner(scanner) => Ok(self.remove_scanner(scanner)?),
            RadioRequest::OpenConnection(configuration) => {
                self.open_connection(*configuration.get())
            }
            RadioRequest::ConnectionEvent(event) => self.connection_event(*event.get()),
            RadioRequest::Transmit { connection, pdu } => {
                let pdu = *pdu.get();
                let kind = match pdu.kind() {
                    DataPduKind::Continuation => {
                        PeripheralConnectionTransmitPduKind::DataContinuation
                    }
                    DataPduKind::Start => PeripheralConnectionTransmitPduKind::DataStartOrComplete,
                    DataPduKind::Control => PeripheralConnectionTransmitPduKind::Control,
                };
                self.transmit(connection, kind, pdu.payload())
            }
            RadioRequest::CloseConnection(connection) => Ok(self.close_connection(connection)?),
            RadioRequest::TestTransmit(test) => self.test_transmit(*test.get()),
            RadioRequest::TestReceive(test) => self.test_receive(*test.get()),
            RadioRequest::EndTest => Ok(self.end_test()?),
            RadioRequest::FilterAcceptList(change) => Ok(self.change_accept_list(*change.get())?),
        }
    }

    /// Hardware's view of the device table; the caller publishes it after
    /// every accepted list change.
    pub fn device_table_publication(&self) -> LeDeviceTablePublication {
        self.memory.device_table.publication()
    }

    /// The next hardware work: a pending cancellation first, then pending
    /// insertions. Call it while no transaction is waiting.
    pub fn drive(
        &mut self,
        view: SchedulerHardwareView,
        sink: &mut impl BluetoothRadioSink,
    ) -> RadioStep<ITEMS> {
        if self.faulted || self.inserting.is_some() {
            return RadioStep::Idle;
        }
        match self.mode {
            SchedulerMode::Test(TestPhase::Restoring) => {
                self.mode = SchedulerMode::Shared;
                return RadioStep::LeaveTest;
            }
            SchedulerMode::Test(TestPhase::Stopping(_)) => return RadioStep::StopScheduler,
            SchedulerMode::Shared | SchedulerMode::Test(_) => {}
        }
        let mut cancelled = [None; ITEMS];
        let mut count = 0;
        for (id, _) in self.executor.list().iter() {
            if self.event_of(id).is_some_and(|event| event.cancel) {
                cancelled[count] = Some(id);
                count += 1;
            }
        }
        if count > 0 {
            let ids: [SchedulerItemId; ITEMS] =
                core::array::from_fn(|index| cancelled[index].unwrap_or(cancelled[0].unwrap()));
            let mut items = space(&self.memory);
            match self.executor.begin_cancel(
                &mut items,
                &view.busy,
                view.hardware_head,
                &ids[..count],
            ) {
                Ok(step) => return self.after_step(step, sink),
                Err(_) => return RadioStep::Idle,
            }
        }
        let Some((id, window)) = self.pending[0] else {
            // An idle scheduler with listed events, after a stop or a
            // cancellation, restarts at the first unexecuted one.
            let items = space(&self.memory);
            return match self.executor.restart_if_idle(&items, &view.busy) {
                Some(insertion) => RadioStep::Start(insertion),
                None => RadioStep::Idle,
            };
        };
        // The vendor test event bodies stop the scheduler
        // (`r_sym_bt_74l62ZLsZuXg67pPHSd7`), publish the item as the list-zero
        // head and run it; a test event never enters a running list.
        if view.busy.is_busy()
            && id.kind() == SchedulerRoleKind::DirectTestMode
            && let SchedulerMode::Test(phase) = self.mode
        {
            self.mode = SchedulerMode::Test(TestPhase::Stopping(phase.resume()));
            return RadioStep::StopScheduler;
        }
        if !view.busy.is_busy() {
            let mut head = None;
            let mut inserted = [None; ITEMS];
            let mut count = 0;
            let mut items = space(&self.memory);
            for &(id, window) in self.pending.iter().flatten() {
                match self
                    .executor
                    .submit_idle(&mut items, &view.busy, id, window)
                {
                    Ok(insertion) => {
                        head = Some(insertion);
                        inserted[count] = Some(id);
                        count += 1;
                    }
                    Err(_) => break,
                }
            }
            let mut test = false;
            for id in inserted.into_iter().flatten() {
                self.pop_pending();
                self.mark_listed(id);
                test |= id.kind() == SchedulerRoleKind::DirectTestMode;
            }
            return match head {
                // The vendor event bodies disable the route on every test
                // event without CTE (`r_sym_ble_YpJTETFGhduIAMkkBKjc`); it
                // stays disabled until Test End reinitializes the PHY.
                Some(head) if test && self.mode == SchedulerMode::Test(TestPhase::Opened) => {
                    self.mode = SchedulerMode::Test(TestPhase::Running);
                    RadioStep::EnterTest(head)
                }
                Some(head) => RadioStep::Start(head),
                None => RadioStep::Idle,
            };
        }
        let items = space(&self.memory);
        match self
            .executor
            .begin_live_insertion(&items, &view.busy, id, window)
        {
            Ok(step) => {
                self.pop_pending();
                self.inserting = Some(id);
                RadioStep::Transaction(step)
            }
            Err(_) => RadioStep::Idle,
        }
    }

    /// Deliver the observation a transaction awaited.
    pub fn advance(
        &mut self,
        observation: SchedulerObservation,
        sink: &mut impl BluetoothRadioSink,
    ) -> Result<RadioStep<ITEMS>, SchedulerTransactionFault> {
        let mut items = space(&self.memory);
        match self.executor.advance(&mut items, observation) {
            Ok(step) => Ok(self.after_step(step, sink)),
            Err(fault) => {
                match fault {
                    SchedulerTransactionFault::UnexpectedObservation
                    | SchedulerTransactionFault::NoTransaction => {}
                    SchedulerTransactionFault::UnsupportedExecutionLockResult
                    | SchedulerTransactionFault::ExecutionModifyRejected => {
                        if let Some(id) = self.inserting.take() {
                            // The insertion ended without linking the item.
                            if let Some(event) = self.event_mut(id) {
                                event.listed += 1;
                            }
                            self.settle(id, None, sink);
                        }
                        self.fault(RadioFault::UnsupportedHardwareResult, sink);
                    }
                    SchedulerTransactionFault::UnsupportedSkipResult => {
                        self.withhold_detached();
                        self.fault(RadioFault::UnsupportedHardwareResult, sink);
                    }
                }
                Err(fault)
            }
        }
    }

    /// Take the items that hardware finished and end their events.
    ///
    /// A list transaction in progress defers the pass to its end: the
    /// finished-list observation that called it raises no second wake.
    pub fn complete(&mut self, sink: &mut impl BluetoothRadioSink) {
        let mut items = space(&self.memory);
        let Ok(completion) = self.executor.take_completed(&mut items) else {
            self.deferred_completion = true;
            return;
        };
        self.deferred_completion = false;
        let out_of_order = completion.out_of_order();
        for (id, status) in completion.iter() {
            self.settle(id, Some(status), sink);
        }
        if out_of_order {
            self.fault(RadioFault::OutOfOrderCompletion, sink);
        }
    }

    /// Hand over the receipt of a stopped scheduler.
    pub fn enter_stopped(
        &mut self,
        stopped: BluetoothSchedulerStopped,
    ) -> Result<(), oer_esp32s31_bluetooth::scheduler::SchedulerStopRejected> {
        self.executor.enter_stopped(stopped)?;
        if let SchedulerMode::Test(TestPhase::Stopping(resume)) = self.mode {
            self.mode = SchedulerMode::Test(resume.phase());
        }
        Ok(())
    }

    /// The stopped receipt the executor holds.
    pub const fn stopped(&self) -> Option<&BluetoothSchedulerStopped> {
        self.executor.stopped()
    }

    /// Consume the stopped receipt against a fresh time sample.
    ///
    /// Listed events whose start no longer passes the late-start guard are
    /// cancelled and end as not executed. The next [`Self::drive`] restarts
    /// the idle scheduler at the first remaining event. The caller observes
    /// `sample` first ([`Self::observe_time`]); the late-start guard reads the
    /// raw sample, so an exhausted radio epoch does not keep the scheduler
    /// stopped.
    ///
    /// # Errors
    ///
    /// The radio holds no stopped receipt.
    pub fn resume(&mut self, sample: &ControllerTimeSample) -> Result<(), SchedulerNotStopped> {
        let items = space(&self.memory);
        self.executor.resume(&items)?;
        let mut passed = [None; ITEMS];
        let mut count = 0;
        for (id, window) in self.executor.list().iter() {
            if items.completion_status(id).is_none()
                && !self.policy.initial_deadline_is_open(sample, window.start())
            {
                passed[count] = Some(id);
                count += 1;
            }
        }
        for id in passed.into_iter().flatten() {
            if let Some(event) = self.event_mut(id) {
                event.cancel = true;
            }
        }
        Ok(())
    }

    fn after_step(
        &mut self,
        step: SchedulerStep<SchedulerItemId, ITEMS>,
        sink: &mut impl BluetoothRadioSink,
    ) -> RadioStep<ITEMS> {
        if matches!(
            step.next(),
            oer_esp32s31_bluetooth::scheduler::SchedulerNext::Finished
        ) && let Some(id) = self.inserting.take()
        {
            self.mark_listed(id);
        }
        for (id, status) in step.released().iter() {
            self.settle(id, status, sink);
        }
        if matches!(
            step.next(),
            oer_esp32s31_bluetooth::scheduler::SchedulerNext::Finished
        ) && self.deferred_completion
        {
            self.complete(sink);
        }
        RadioStep::Transaction(step)
    }

    fn fault(&mut self, fault: RadioFault, sink: &mut impl BluetoothRadioSink) {
        self.faulted = true;
        sink.fault(fault);
    }

    /// Keep the items of cancelled events that left the executor without
    /// release: hardware may still reach them.
    fn withhold_detached(&mut self) {
        let mut detached = [None; ITEMS];
        let mut count = 0;
        for kind in [
            SchedulerRoleKind::LegacyAdvertising,
            SchedulerRoleKind::ConnectableAdvertising,
            SchedulerRoleKind::LegacyScanning,
            SchedulerRoleKind::PeripheralConnection,
            SchedulerRoleKind::DirectTestMode,
        ] {
            for item in 0..3 {
                for instance in 0..ITEMS {
                    let Some(id) = self.item_id(kind, instance, item) else {
                        continue;
                    };
                    if self.listed_in_pool(id)
                        && !self.executor.list().contains(id)
                        && self.event_of(id).is_some_and(|event| event.cancel)
                        && count < ITEMS
                    {
                        detached[count] = Some(id);
                        count += 1;
                    }
                }
            }
        }
        for id in detached.into_iter().flatten() {
            let _ = match id.kind() {
                SchedulerRoleKind::LegacyAdvertising => self.memory.legacy.withhold(id),
                SchedulerRoleKind::ConnectableAdvertising => self.memory.connectable.withhold(id),
                SchedulerRoleKind::LegacyScanning => self.memory.scanners.withhold(id),
                SchedulerRoleKind::PeripheralConnection => self.memory.connections.withhold(id),
                SchedulerRoleKind::DirectTestMode => self.memory.dtm.withhold(id),
            };
        }
    }

    fn item_id(
        &self,
        kind: SchedulerRoleKind,
        instance: usize,
        item: usize,
    ) -> Option<SchedulerItemId> {
        let handle = match kind {
            SchedulerRoleKind::LegacyAdvertising => self
                .legacy
                .iter()
                .flatten()
                .find(|slot| slot.instance.index() == instance)?
                .instance
                .item(item),
            SchedulerRoleKind::ConnectableAdvertising => self
                .connectable
                .iter()
                .flatten()
                .find(|slot| slot.instance.index() == instance)?
                .instance
                .item(item),
            SchedulerRoleKind::LegacyScanning => self
                .scanners
                .iter()
                .flatten()
                .find(|slot| slot.instance.index() == instance)?
                .instance
                .item(item),
            SchedulerRoleKind::PeripheralConnection => self
                .connections
                .iter()
                .flatten()
                .find(|(slot, _)| slot.instance.index() == instance)?
                .0
                .instance
                .item(item),
            SchedulerRoleKind::DirectTestMode => {
                let slot = self
                    .dtm
                    .as_ref()
                    .filter(|slot| slot.instance.index() == instance)?;
                slot.instance.item(item)
            }
        };
        Some(handle)
    }

    fn listed_in_pool(&self, id: SchedulerItemId) -> bool {
        match id.kind() {
            SchedulerRoleKind::LegacyAdvertising => self.memory.legacy.is_listed(id),
            SchedulerRoleKind::ConnectableAdvertising => self.memory.connectable.is_listed(id),
            SchedulerRoleKind::LegacyScanning => self.memory.scanners.is_listed(id),
            SchedulerRoleKind::PeripheralConnection => self.memory.connections.is_listed(id),
            SchedulerRoleKind::DirectTestMode => self.memory.dtm.is_listed(id),
        }
    }

    fn pop_pending(&mut self) {
        self.pending.rotate_left(1);
        self.pending[ITEMS - 1] = None;
    }

    fn push_pending(&mut self, id: SchedulerItemId, window: SchedulerRawWindow) {
        let slot = self
            .pending
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("admission checked the queue capacity");
        *slot = Some((id, window));
    }

    fn mark_listed(&mut self, id: SchedulerItemId) {
        if let Some(event) = self.event_mut(id) {
            event.pending -= 1;
            event.listed += 1;
        }
    }

    fn pending_free(&self) -> usize {
        self.pending.iter().filter(|slot| slot.is_none()).count()
    }

    fn event_of(&self, id: SchedulerItemId) -> Option<Event> {
        let instance = id.instance();
        match id.kind() {
            SchedulerRoleKind::LegacyAdvertising => find(&self.legacy, instance)?.event,
            SchedulerRoleKind::ConnectableAdvertising => find(&self.connectable, instance)?.event,
            SchedulerRoleKind::LegacyScanning => find(&self.scanners, instance)?.event,
            SchedulerRoleKind::PeripheralConnection => {
                self.connections
                    .iter()
                    .flatten()
                    .find(|(slot, _)| slot.instance.index() == instance)?
                    .0
                    .event
            }
            SchedulerRoleKind::DirectTestMode => self.dtm.as_ref()?.event,
        }
    }

    fn event_mut(&mut self, id: SchedulerItemId) -> Option<&mut Event> {
        let instance = id.instance();
        match id.kind() {
            SchedulerRoleKind::LegacyAdvertising => {
                find_mut(&mut self.legacy, instance)?.event.as_mut()
            }
            SchedulerRoleKind::ConnectableAdvertising => {
                find_mut(&mut self.connectable, instance)?.event.as_mut()
            }
            SchedulerRoleKind::LegacyScanning => {
                find_mut(&mut self.scanners, instance)?.event.as_mut()
            }
            SchedulerRoleKind::PeripheralConnection => self
                .connections
                .iter_mut()
                .flatten()
                .find(|(slot, _)| slot.instance.index() == instance)?
                .0
                .event
                .as_mut(),
            SchedulerRoleKind::DirectTestMode => self.dtm.as_mut()?.event.as_mut(),
        }
    }

    /// Admission of the reservation of `air` for one item. The core placed
    /// it within the horizon of an instant no later than this radio's time,
    /// with the reservation this radio uses; only the time since then
    /// refuses it, as [`RequestError::Late`].
    fn reserve(
        &self,
        anchor: LeInstant,
        duration: RadioDuration<LeRadio>,
    ) -> Result<SchedulerRawWindow, RequestError> {
        let now = LeInstant::from_micros(self.clock.now);
        let air = LeWindow::new(anchor, duration).map_err(|_| RequestError::EpochExhausted)?;
        // A valid air window keeps its end, so only an early start fails.
        let reservation = self
            .timing
            .reservation(air)
            .map_err(|OutsideEpoch| RequestError::Late)?;
        let guarded_now = now
            .checked_add(self.timing.admission_guard)
            .ok_or(RequestError::EpochExhausted)?;
        if reservation.start() < guarded_now {
            return Err(RequestError::Late);
        }
        debug_assert!(
            reservation
                .end()
                .checked_duration_since(now)
                .is_some_and(|horizon| horizon <= RadioDuration::from_micros(MAX_AHEAD_MICROS)),
            "a placed reservation ends within the horizon"
        );
        Ok(SchedulerRawWindow::from_projected_scheduler_window(
            self.clock.raw(reservation.start().as_micros()),
            self.clock.raw(reservation.end().as_micros()),
        )
        .expect("a reservation within the horizon is a scheduler window"))
    }

    /// Check that `window` is free of listed and pending reservations: the
    /// core keeps its events apart with the reservation this radio uses.
    fn free(&self, window: SchedulerRawWindow) {
        debug_assert!(
            self.executor
                .list()
                .iter()
                .map(|(_, listed)| listed)
                .chain(self.pending.iter().flatten().map(|(_, pending)| *pending))
                .all(|other| !overlaps(window, other)),
            "the core never overlaps its events"
        );
    }

    fn check_capacity(&self, items: usize) -> Result<(), RequestError> {
        if self.pending_free() < items || self.executor.list().len() + items > ITEMS {
            return Err(RequestError::Busy);
        }
        Ok(())
    }

    fn configure_advertising(
        &mut self,
        configuration: AdvertisingConfiguration<'_>,
    ) -> Result<(), Admit> {
        if find_id(&self.legacy, configuration.set).is_some()
            || find_id(&self.connectable, configuration.set).is_some()
        {
            return Err(RequestError::AlreadyConfigured.into());
        }
        let tx_power = le_tx_power(configuration.tx_power);
        let scan_response = match configuration.reception {
            AdvertisingReception::None => None,
            AdvertisingReception::ScanResponse(scan_response) => Some(Some(scan_response)),
            AdvertisingReception::Report => Some(None),
        };
        match scan_response {
            None => {
                let index = free_slot(&self.legacy)?;
                let pool = &mut self.memory.legacy;
                let instance = pool.acquire().ok_or(RequestError::NoInstance)?;
                let result = pool
                    .prepare_packet(&instance, configuration.pdu.bytes())
                    .and_then(|()| pool.reset_link_state(&instance, tx_power));
                if result.is_err() {
                    let _ = pool.release(instance);
                    return Err(Admit::Fault(RadioFault::MemoryInconsistency));
                }
                self.legacy[index] = Some(Slot {
                    id: configuration.set,
                    instance,
                    event: None,
                });
            }
            Some(scan_response) => {
                let index = free_slot(&self.connectable)?;
                let pdu = configuration.pdu.bytes();
                let adv_ind =
                    LegacyConnectableAdvIndPacketInput::try_from_encoded_extent(pdu, pdu[1])
                        .map_err(|_| Admit::Fault(RadioFault::MemoryInconsistency))?;
                let scan_response = scan_response
                    .map(|response| {
                        let response = response.bytes();
                        LegacyConnectableScanResponsePacketInput::try_from_encoded_extent(
                            response,
                            response[1],
                        )
                    })
                    .transpose()
                    .map_err(|_| Admit::Fault(RadioFault::MemoryInconsistency))?;
                // TxAdd selects the advertiser address type.
                let own_address = if pdu[0] & 0x40 == 0 {
                    LegacyConnectableAdvertisingOwnAddress::Public
                } else {
                    let mut address = [0; 6];
                    address.copy_from_slice(&pdu[2..8]);
                    LegacyConnectableAdvertisingOwnAddress::Random(address)
                };
                let pool = &mut self.memory.connectable;
                let instance = pool.acquire().ok_or(RequestError::NoInstance)?;
                let input = LegacyConnectableAdvertisingMemoryInput::new(
                    adv_ind,
                    scan_response,
                    own_address,
                );
                if pool
                    .prepare(&instance, input, &self.memory.non_scanning, tx_power)
                    .is_err()
                {
                    let _ = pool.release(instance);
                    return Err(Admit::Fault(RadioFault::MemoryInconsistency));
                }
                self.connectable[index] = Some(Slot {
                    id: configuration.set,
                    instance,
                    event: None,
                });
            }
        }
        Ok(())
    }

    fn advertise(&mut self, event: AdvertisingEvent) -> Result<(), Admit> {
        if let Some(index) = find_id(&self.legacy, event.set) {
            return self.advertise_legacy(index, event);
        }
        if let Some(index) = find_id(&self.connectable, event.set) {
            return self.advertise_connectable(index, event);
        }
        Err(RequestError::Unknown.into())
    }

    fn advertise_legacy(&mut self, index: usize, event: AdvertisingEvent) -> Result<(), Admit> {
        let slot = self.legacy[index].as_ref().expect("the set is configured");
        if slot.event.is_some() {
            return Err(RequestError::Busy.into());
        }
        let count = event.channels.len();
        let spacing = event.channel_spacing;
        let mut windows = [None; 3];
        for (position, window) in windows.iter_mut().enumerate().take(count) {
            let (_, anchor) = event
                .channel_anchor(position)
                .expect("a placed event's channels lie in the epoch")
                .expect("the channel count holds the position");
            let lead = self.timing.preparation_lead;
            let window_ = self.reserve(
                anchor,
                spacing
                    .checked_sub(lead)
                    .expect("a supported event's spacing exceeds the preparation lead"),
            )?;
            self.free(window_);
            *window = Some(window_);
        }
        self.check_capacity(count)?;
        let plan = LegacyAdvertisingPrimaryChannelPlan::new(
            event.channels.contains(AdvertisingChannel::Channel37),
            event.channels.contains(AdvertisingChannel::Channel38),
            event.channels.contains(AdvertisingChannel::Channel39),
        )
        .expect("a channel set is non-empty");
        let first = windows[0].expect("an event has a channel");
        let raw_duration = self.clock.duration(spacing);
        let slot = self.legacy[index].as_mut().expect("the set is configured");
        let prepared = self
            .memory
            .legacy
            .prepare_event(
                &slot.instance,
                plan,
                first.start(),
                raw_duration,
                coexistence::advertising_priorities(self.coexistence, event.coexistence),
            )
            .map_err(|_| Admit::Fault(RadioFault::MemoryInconsistency))?;
        let mut submitted = [None; 3];
        for (item, start, end) in prepared.items() {
            let id = self
                .memory
                .legacy
                .submit(&slot.instance, item)
                .expect("the event prepared the item");
            submitted[item] = Some((
                id,
                SchedulerRawWindow::from_projected_scheduler_window(start, end)
                    .expect("admission checked the window"),
            ));
        }
        slot.event = Some(Event::new(event.id, count as u8));
        for (id, window) in submitted.into_iter().flatten() {
            self.push_pending(id, window);
        }
        Ok(())
    }

    fn advertise_connectable(
        &mut self,
        index: usize,
        event: AdvertisingEvent,
    ) -> Result<(), Admit> {
        let slot = self.connectable[index]
            .as_ref()
            .expect("the set is configured");
        if slot.event.is_some() {
            return Err(RequestError::Busy.into());
        }
        let count = event.channels.len();
        let spacing = event.channel_spacing;
        let lead = self.timing.preparation_lead;
        let mut windows = [None; 3];
        for (position, window) in windows.iter_mut().enumerate().take(count) {
            let (_, anchor) = event
                .channel_anchor(position)
                .expect("a placed event's channels lie in the epoch")
                .expect("the channel count holds the position");
            let window_ = self.reserve(
                anchor,
                spacing
                    .checked_sub(lead)
                    .expect("a supported event's spacing exceeds the preparation lead"),
            )?;
            self.free(window_);
            *window = Some(window_);
        }
        self.check_capacity(count)?;
        let plan = LegacyAdvertisingPrimaryChannelPlan::new(
            event.channels.contains(AdvertisingChannel::Channel37),
            event.channels.contains(AdvertisingChannel::Channel38),
            event.channels.contains(AdvertisingChannel::Channel39),
        )
        .expect("a channel set is non-empty");
        let first = windows[0].expect("an event has a channel");
        let raw_duration = self.clock.duration(spacing);
        let lead_ticks = self.policy.sequence_lead_raw_delta();
        let slot = self.connectable[index]
            .as_mut()
            .expect("the set is configured");
        let prepared = self
            .memory
            .connectable
            .prepare_event(
                &slot.instance,
                plan,
                first.start(),
                raw_duration,
                lead_ticks,
                coexistence::advertising_priorities(self.coexistence, event.coexistence),
            )
            .map_err(|_| Admit::Fault(RadioFault::MemoryInconsistency))?;
        let mut submitted = [None; 3];
        for (item, start, end) in prepared.items() {
            let id = self
                .memory
                .connectable
                .submit(&slot.instance, item)
                .expect("the event prepared the item");
            submitted[item] = Some((
                id,
                SchedulerRawWindow::from_projected_scheduler_window(start, end)
                    .expect("admission checked the window"),
            ));
        }
        slot.event = Some(Event::new(event.id, count as u8));
        for (id, window) in submitted.into_iter().flatten() {
            self.push_pending(id, window);
        }
        Ok(())
    }

    fn remove_advertising(&mut self, set: AdvertisingSetId) -> Result<(), RequestError> {
        if let Some(index) = find_id(&self.legacy, set) {
            if self.legacy[index]
                .as_ref()
                .is_some_and(|slot| slot.event.is_some())
            {
                return Err(RequestError::Busy);
            }
            let slot = self.legacy[index].take().expect("the set is configured");
            if let Err(failure) = self.memory.legacy.release(slot.instance) {
                self.legacy[index] = Some(Slot {
                    instance: failure.instance,
                    ..slot
                });
                return Err(RequestError::Busy);
            }
            return Ok(());
        }
        if let Some(index) = find_id(&self.connectable, set) {
            if self.connectable[index]
                .as_ref()
                .is_some_and(|slot| slot.event.is_some())
            {
                return Err(RequestError::Busy);
            }
            let slot = self.connectable[index]
                .take()
                .expect("the set is configured");
            if let Err(failure) = self.memory.connectable.release(slot.instance) {
                self.connectable[index] = Some(Slot {
                    instance: failure.instance,
                    ..slot
                });
                return Err(RequestError::Busy);
            }
            return Ok(());
        }
        Err(RequestError::Unknown)
    }

    fn configure_scanner(&mut self, configuration: ScannerConfiguration) -> Result<(), Admit> {
        if find_id(&self.scanners, configuration.scanner).is_some() {
            return Err(RequestError::AlreadyConfigured.into());
        }
        let tx_power = le_tx_power(configuration.tx_power);
        let index = free_slot(&self.scanners)?;
        let pool = &mut self.memory.scanners;
        let instance = pool.acquire().ok_or(RequestError::NoInstance)?;
        let scan_type = match configuration.scan_type {
            ScanType::Passive => LegacyScanType::Passive,
            ScanType::Active => LegacyScanType::Active,
        };
        let filter_policy = match configuration.filter_policy {
            ScanFilterPolicy::AcceptAll => LegacyScanFilterPolicy::AcceptAll,
            ScanFilterPolicy::AcceptListOnly => LegacyScanFilterPolicy::AcceptListOnly,
        };
        let config = LegacyScanResetConfig::le_1m_public(tx_power, scan_type, filter_policy);
        if pool
            .reset(&instance, &self.memory.scanning, config)
            .is_err()
        {
            let _ = pool.release(instance);
            return Err(Admit::Fault(RadioFault::MemoryInconsistency));
        }
        self.scanners[index] = Some(Slot {
            id: configuration.scanner,
            instance,
            event: None,
        });
        Ok(())
    }

    fn change_accept_list(&mut self, change: AcceptListChange) -> Result<(), RequestError> {
        let device = |device: AcceptListDevice| LeFilterAcceptListDevice {
            random: device.random,
            address: device.address,
        };
        let table = &mut self.memory.device_table;
        match change {
            AcceptListChange::Add(entry) => table.add(device(entry)),
            AcceptListChange::Remove(entry) => table.remove(device(entry)),
            AcceptListChange::Clear => {
                table.clear();
                Ok(())
            }
        }
        .map_err(|error| match error {
            LeDeviceTableError::Full => RequestError::ListFull,
            LeDeviceTableError::NotFound => RequestError::NotListed,
        })
    }

    fn scan(&mut self, scan: ScanWindow) -> Result<(), Admit> {
        let index = find_id(&self.scanners, scan.scanner).ok_or(RequestError::Unknown)?;
        if self.scanners[index]
            .as_ref()
            .is_some_and(|slot| slot.event.is_some())
        {
            return Err(RequestError::Busy.into());
        }
        let anchor = scan.window.start();
        let duration = scan.window.duration();
        let window = self.reserve(anchor, duration)?;
        self.free(window);
        self.check_capacity(1)?;
        // The pinned scan restart (`r_sym_ble_M0sTWGzdUqAUyXoK849F`) ends
        // the item at most 32768 microseconds after the anchor and records
        // the whole window length in the link state.
        let item_end = anchor
            .checked_add(duration.min(RadioDuration::from_micros(u64::from(SCAN_EVENT_MAX_MICROS))))
            .expect("a placed window's end lies in the epoch");
        let item_end = self.clock.raw(item_end.as_micros());
        let window_ticks = LegacyScanWindowTicks::new(self.clock.duration(duration))
            .expect("a placed window is non-empty and within the horizon");
        let slot = self.scanners[index]
            .as_mut()
            .expect("the scanner is configured");
        let prepared = self
            .memory
            .scanners
            .prepare_event(
                &slot.instance,
                scan_channel(scan.channel),
                LegacyScanEventTiming {
                    window: LegacyScanSchedulerWindow::from_controller_ticks(
                        window.start(),
                        item_end,
                    )
                    .expect("admission checked the window"),
                    window_ticks,
                    raw_sequence_lead: self.policy.sequence_lead_raw_delta(),
                },
                LegacyScanStartSelection::Requested,
                coexistence::passive_scan_priorities(self.coexistence),
            )
            .map_err(|_| Admit::Fault(RadioFault::MemoryInconsistency))?;
        let id = self
            .memory
            .scanners
            .submit(&slot.instance, prepared.item())
            .expect("the window prepared the item");
        let mut event = Event::new(scan.id, 1);
        event.item = prepared.item() as u8;
        slot.event = Some(event);
        self.push_pending(id, window);
        Ok(())
    }

    fn remove_scanner(&mut self, scanner: ScannerId) -> Result<(), RequestError> {
        let index = find_id(&self.scanners, scanner).ok_or(RequestError::Unknown)?;
        if self.scanners[index]
            .as_ref()
            .is_some_and(|slot| slot.event.is_some())
        {
            return Err(RequestError::Busy);
        }
        let slot = self.scanners[index]
            .take()
            .expect("the scanner is configured");
        if let Err(failure) = self.memory.scanners.release(slot.instance) {
            self.scanners[index] = Some(Slot {
                instance: failure.instance,
                ..slot
            });
            return Err(RequestError::Busy);
        }
        Ok(())
    }

    fn open_connection(&mut self, configuration: ConnectionConfiguration) -> Result<(), Admit> {
        if self
            .connections
            .iter()
            .flatten()
            .any(|(slot, _)| slot.id == configuration.connection)
        {
            return Err(RequestError::AlreadyConfigured.into());
        }
        let index = self
            .connections
            .iter()
            .position(Option::is_none)
            .ok_or(RequestError::NoInstance)?;
        let pool = &mut self.memory.connections;
        let instance = pool.acquire().ok_or(RequestError::NoInstance)?;
        let identity = PeripheralConnectionIdentity::new(
            configuration.access_address.0,
            configuration.crc_init.0,
        );
        if pool.prepare_identity(&instance, identity).is_err() {
            let _ = pool.release(instance);
            return Err(Admit::Fault(RadioFault::MemoryInconsistency));
        }
        self.connections[index] = Some((
            Slot {
                id: configuration.connection,
                instance,
                event: None,
            },
            ConnectionFacts {
                created_at: configuration.created_at,
                tx_power: configuration.tx_power,
            },
        ));
        Ok(())
    }

    fn connection_index(&self, connection: ConnectionId) -> Result<usize, RequestError> {
        self.connections
            .iter()
            .position(|entry| {
                entry
                    .as_ref()
                    .is_some_and(|(slot, _)| slot.id == connection)
            })
            .ok_or(RequestError::Unknown)
    }

    fn connection_event(&mut self, event: ConnectionEvent) -> Result<(), Admit> {
        let index = self.connection_index(event.connection)?;
        let (slot, facts) = self.connections[index]
            .as_ref()
            .expect("the connection is open");
        if slot.event.is_some() {
            return Err(RequestError::Busy.into());
        }
        let facts = *facts;
        let window = self.reserve(event.window.start(), event.window.duration())?;
        self.free(window);
        self.check_capacity(1)?;
        let channel = PeripheralConnectionDataChannel::new(event.channel.index())
            .expect("a data channel index is valid");
        let raw_window = PeripheralConnectionSchedulerWindow::new(window.start(), window.end())
            .expect("admission checked the window");
        let widening = match event.timing {
            ConnectionEventTiming::First { timing_guard, .. } => timing_guard,
            ConnectionEventTiming::Recurring { widening, .. } => widening,
        };
        let span = PeripheralConnectionEventSpan::new(
            self.clock
                .duration(connection_event_span(event.interval, widening)),
        )
        .expect("a connection interval spans at least one tick");
        let priority = PeripheralConnectionSchedulerPriority::new(event.priority)
            .expect("a supported event's priority is at most 15");
        let coexistence = coexistence::connection_priorities(self.coexistence, event.coexistence);
        let protection = coexistence::connection_protection(self.coexistence);
        let lead = self.policy.sequence_lead_raw_delta();
        let receive_time = PeripheralConnectionReceiveTime::from_controller_ticks(
            self.clock.raw(facts.created_at.as_micros()),
        );
        let workspace = self.memory.direction_finding;
        let (slot, _) = self.connections[index]
            .as_mut()
            .expect("the connection is open");
        let prepared = match event.timing {
            ConnectionEventTiming::First {
                transmit_window,
                timing_guard,
            } => {
                let receive_wait = PeripheralConnectionReceiveWait::new(
                    transmit_window.as_micros(),
                    timing_guard.as_micros(),
                )
                .expect("a supported first event's wait fits the short form");
                self.memory.connections.prepare_first_event(
                    &slot.instance,
                    PeripheralConnectionFirstEvent {
                        channel,
                        receive_time,
                        event_span: span,
                        window: raw_window,
                        receive_wait,
                        default_tx_power: le_tx_power(facts.tx_power),
                        priority,
                        coexistence,
                        protection,
                        raw_sequence_lead: lead,
                    },
                    workspace,
                )
            }
            ConnectionEventTiming::Recurring { receive_wait, .. } => {
                let receive_wait =
                    PeripheralConnectionRecurringReceiveWait::new(recurring_wait(receive_wait))
                        .expect("a supported recurring wait fits the long form");
                self.memory.connections.prepare_recurring_event(
                    &slot.instance,
                    PeripheralConnectionRecurringEvent {
                        channel,
                        event_span: span,
                        window: raw_window,
                        receive_wait,
                        priority,
                        coexistence,
                        raw_sequence_lead: lead,
                    },
                )
            }
        }
        .map_err(|_| Admit::Fault(RadioFault::MemoryInconsistency))?;
        let id = self
            .memory
            .connections
            .submit(&slot.instance, prepared.item())
            .expect("the event prepared the item");
        slot.event = Some(Event::new(event.id, 1));
        self.push_pending(id, window);
        Ok(())
    }

    fn transmit(
        &mut self,
        connection: ConnectionId,
        kind: PeripheralConnectionTransmitPduKind,
        payload: &[u8],
    ) -> Result<(), Admit> {
        let index = self.connection_index(connection)?;
        let (slot, _) = self.connections[index]
            .as_ref()
            .expect("the connection is open");
        if slot.event.is_some() {
            return Err(RequestError::Busy.into());
        }
        match self
            .memory
            .connections
            .enqueue_transmission(&slot.instance, kind, payload)
        {
            Ok(true) => Ok(()),
            Ok(false) => Err(RequestError::Busy.into()),
            Err(_) => Err(Admit::Fault(RadioFault::MemoryInconsistency)),
        }
    }

    fn close_connection(&mut self, connection: ConnectionId) -> Result<(), RequestError> {
        let index = self.connection_index(connection)?;
        if self.connections[index]
            .as_ref()
            .is_some_and(|(slot, _)| slot.event.is_some())
        {
            return Err(RequestError::Busy);
        }
        let (slot, facts) = self.connections[index]
            .take()
            .expect("the connection is open");
        if let Err(failure) = self.memory.connections.release(slot.instance) {
            self.connections[index] = Some((
                Slot {
                    instance: failure.instance,
                    ..slot
                },
                facts,
            ));
            return Err(RequestError::Busy);
        }
        Ok(())
    }

    fn dtm_instance(&mut self) -> Result<(), RequestError> {
        match &self.dtm {
            Some(slot) if slot.event.is_some() => Err(RequestError::Busy),
            Some(_) => Ok(()),
            None => {
                if self.mode != SchedulerMode::Shared {
                    return Err(RequestError::Busy);
                }
                let instance = self.memory.dtm.acquire().ok_or(RequestError::NoInstance)?;
                self.mode = SchedulerMode::Test(TestPhase::Opened);
                self.dtm = Some(Slot {
                    id: (),
                    instance,
                    event: None,
                });
                Ok(())
            }
        }
    }

    fn test_transmit(&mut self, test: TestTransmit<'_>) -> Result<(), Admit> {
        if test.payload.len() > DTM_MAX_PAYLOAD {
            return Err(Admit::Fault(RadioFault::MemoryInconsistency));
        }
        let tx_power = le_tx_power(test.tx_power);
        let phy = match test.phy {
            TestPhy::Le1M => DtmSchedulerTransmitterPhy::Le1M,
            TestPhy::Le2M => DtmSchedulerTransmitterPhy::Le2M,
            TestPhy::LeCodedS8 => DtmSchedulerTransmitterPhy::LeCodedS8,
            TestPhy::LeCodedS2 => DtmSchedulerTransmitterPhy::LeCodedS2,
        };
        let window = self.reserve(test.window.start(), test.window.duration())?;
        self.free(window);
        self.check_capacity(1)?;
        self.dtm_instance()?;
        let mut payload = [0; DTM_MAX_PAYLOAD];
        payload[..test.payload.len()].copy_from_slice(test.payload);
        let slot = self.dtm.as_ref().expect("the test holds its instance");
        self.memory
            .dtm
            .prepare_tx_packet(
                &slot.instance,
                test.payload_type.value(),
                test.payload.len() as u8,
                &payload,
            )
            .map_err(|_| Admit::Fault(RadioFault::MemoryInconsistency))?;
        self.schedule_test(
            test.id,
            window,
            test.channel.rf_channel(),
            DtmSchedulerItemEventType::Transmitter(phy),
            tx_power,
        )
    }

    fn test_receive(&mut self, test: TestReceive) -> Result<(), Admit> {
        let tx_power = le_tx_power(test.tx_power);
        let phy = match test.phy {
            TestPhy::Le1M => DtmSchedulerReceiverPhy::Le1M,
            TestPhy::Le2M => DtmSchedulerReceiverPhy::Le2M,
            TestPhy::LeCodedS8 | TestPhy::LeCodedS2 => DtmSchedulerReceiverPhy::LeCoded,
        };
        let phase = if test.recurring {
            DtmReceiverEventPhase::Recurring
        } else {
            DtmReceiverEventPhase::Initial
        };
        let window = self.reserve(test.window.start(), test.window.duration())?;
        self.free(window);
        self.check_capacity(1)?;
        self.dtm_instance()?;
        self.schedule_test(
            test.id,
            window,
            test.channel.rf_channel(),
            DtmSchedulerItemEventType::Receiver { phase, phy },
            tx_power,
        )
    }

    fn schedule_test(
        &mut self,
        id: EventId,
        window: SchedulerRawWindow,
        rf_channel: u8,
        event_type: DtmSchedulerItemEventType,
        tx_power: LeTxPower,
    ) -> Result<(), Admit> {
        // Every event after the first of a session is a recurring event.
        let recurring = matches!(self.mode, SchedulerMode::Test(phase) if phase.resume() == TestResume::Running);
        let sequence_start = self.clock.round_trip(window.start());
        let role = event_type.role();
        let slot = self.dtm.as_mut().expect("the test holds its instance");
        self.memory
            .dtm
            .prepare_event(&slot.instance, |seed| {
                let current = seed.words();
                let link_state = current.link_state().apply_reset(
                    Some(seed.tx_header_head_projection()),
                    Some(seed.rx_header_tail_projection()),
                    tx_power,
                    DTM_REVIEWED_CONFIG,
                    role,
                );
                let item = current
                    .scheduler_item()
                    .apply_event(rf_channel * 2, event_type, window.start(), window.end())
                    .apply_sequence_start(sequence_start);
                let item = if recurring {
                    item.apply_recurring_power(link_state)
                } else {
                    item
                };
                Ok::<_, Infallible>(oer_esp32s31_bluetooth_memory::DtmPositionalEventWords::new(
                    link_state, item,
                ))
            })
            .map_err(|_| Admit::Fault(RadioFault::MemoryInconsistency))?;
        let item = self
            .memory
            .dtm
            .submit(&slot.instance, 0)
            .expect("the event prepared the item");
        let mut event = Event::new(id, 1);
        event.receiver = matches!(role, DtmRole::Receiver);
        slot.event = Some(event);
        self.push_pending(item, window);
        Ok(())
    }

    fn end_test(&mut self) -> Result<(), RequestError> {
        match self.dtm.take() {
            None => Err(RequestError::Unknown),
            Some(slot) if slot.event.is_some() => {
                self.dtm = Some(slot);
                Err(RequestError::Busy)
            }
            Some(slot) => match self.memory.dtm.release(slot.instance) {
                Ok(()) => {
                    // Vendor Test End reinitializes the PHY, which routes
                    // the channel again (`r_ble_phy_init`).
                    self.mode = match self.mode {
                        SchedulerMode::Test(phase) if phase.resume() == TestResume::Running => {
                            SchedulerMode::Test(TestPhase::Restoring)
                        }
                        _ => SchedulerMode::Shared,
                    };
                    Ok(())
                }
                Err(failure) => {
                    self.dtm = Some(Slot {
                        instance: failure.instance,
                        ..slot
                    });
                    Err(RequestError::Busy)
                }
            },
        }
    }

    /// Withdraw the scheduled event `id`; its end follows, as the port's
    /// cancellation states. A faulted radio runs nothing.
    ///
    /// # Errors
    ///
    /// No event with this identifier is scheduled.
    pub fn cancel(
        &mut self,
        id: EventId,
        sink: &mut impl BluetoothRadioSink,
    ) -> Result<(), CancelError> {
        if self.faulted {
            return Err(CancelError::NotRunning);
        }
        // Items waiting for insertion leave the queue at once.
        let mut found = None;
        for index in 0..ITEMS {
            let Some((item, _)) = self.pending[index] else {
                continue;
            };
            if self.event_of(item).is_some_and(|event| event.id == id) {
                found = Some(item);
            }
        }
        let owner = match found {
            Some(item) => Some(item),
            None => self.listed_item_of(id),
        };
        let Some(owner) = owner else {
            return Err(CancelError::NotRunning);
        };
        // The vendor ends a test by stopping the scheduler rather than
        // cancelling its running event (`sym_dtm_NsbldBIeGraE2wg0AVy7`).
        if found.is_none()
            && owner.kind() == SchedulerRoleKind::DirectTestMode
            && self.executor.stopped().is_none()
            && let SchedulerMode::Test(phase) = self.mode
        {
            self.mode = SchedulerMode::Test(TestPhase::Stopping(phase.resume()));
        }
        let mut dropped = [None; ITEMS];
        let mut count = 0;
        for slot in self.pending.iter_mut() {
            if let Some((item, _)) = *slot
                && item.kind() == owner.kind()
                && item.instance() == owner.instance()
            {
                dropped[count] = Some(item);
                count += 1;
                *slot = None;
            }
        }
        compact(&mut self.pending);
        for item in dropped.into_iter().flatten() {
            if let Some(event) = self.event_mut(item) {
                event.pending -= 1;
                event.listed += 1;
            }
            self.settle(item, None, sink);
        }
        if let Some(event) = self.event_mut(owner) {
            event.cancel = true;
        }
        Ok(())
    }

    /// Withdraw every scheduled event, as a port's `Disable` does; each
    /// one's end follows.
    pub fn cancel_all(&mut self, sink: &mut impl BluetoothRadioSink) {
        while let Some(id) = self.next_uncancelled() {
            // An event in a slot has items with the executor or waiting
            // for insertion, so its cancellation is admitted.
            if self.cancel(id, sink).is_err() {
                debug_assert!(false, "a scheduled event without items");
                return;
            }
        }
    }

    /// Withdraw every scheduled event of a stopped scheduler, as a port's
    /// `Disable` does; each one's end follows.
    ///
    /// The stopped receipt proves that the hardware no longer runs any item,
    /// so each listed item leaves with what it recorded: an event with a
    /// completion status ends with it, and one whose start lay ahead of
    /// `sample`, taken after the stop, ends as not executed. An item without
    /// a status whose start had passed may have run before the stop: its
    /// event ends [`EventResult::Aborted`].
    ///
    /// # Errors
    ///
    /// The radio holds no stopped receipt; nothing changed.
    pub fn cancel_all_stopped(
        &mut self,
        sample: &ControllerTimeSample,
        sink: &mut impl BluetoothRadioSink,
    ) -> Result<(), SchedulerNotStopped> {
        if self.executor.stopped().is_none() {
            return Err(SchedulerNotStopped);
        }
        let items = space(&self.memory);
        let mut passed = [None; ITEMS];
        let mut count = 0;
        for (id, window) in self.executor.list().iter() {
            if items.completion_status(id).is_none()
                && !self.policy.initial_deadline_is_open(sample, window.start())
            {
                passed[count] = Some(id);
                count += 1;
            }
        }
        for id in passed.into_iter().flatten() {
            if let Some(event) = self.event_mut(id) {
                event.unconfirmed = true;
            }
        }
        self.cancel_all(sink);
        Ok(())
    }

    /// A scheduled event no cancellation withdrew yet.
    fn next_uncancelled(&self) -> Option<EventId> {
        let events = self
            .legacy
            .iter()
            .chain(&self.connectable)
            .flatten()
            .map(|slot| slot.event)
            .chain(self.scanners.iter().flatten().map(|slot| slot.event))
            .chain(
                self.connections
                    .iter()
                    .flatten()
                    .map(|(slot, _)| slot.event),
            )
            .chain(self.dtm.iter().map(|slot| slot.event));
        events
            .flatten()
            .find(|event| !event.cancel)
            .map(|event| event.id)
    }

    fn listed_item_of(&self, id: EventId) -> Option<SchedulerItemId> {
        self.executor
            .list()
            .iter()
            .map(|(item, _)| item)
            .chain(self.inserting)
            .find(|item| self.event_of(*item).is_some_and(|event| event.id == id))
    }

    /// Return one item to its pool and end its event when it was the last.
    fn settle(
        &mut self,
        id: SchedulerItemId,
        status: Option<SchedulerItemCompletionStatus>,
        sink: &mut impl BluetoothRadioSink,
    ) {
        let pool_result = match id.kind() {
            SchedulerRoleKind::LegacyAdvertising => self.memory.legacy.retire(id),
            SchedulerRoleKind::ConnectableAdvertising => self.memory.connectable.retire(id),
            SchedulerRoleKind::LegacyScanning => self.memory.scanners.retire(id),
            SchedulerRoleKind::PeripheralConnection => self.memory.connections.retire(id),
            SchedulerRoleKind::DirectTestMode => self.memory.dtm.retire(id),
        };
        debug_assert!(pool_result.is_ok(), "a released item was listed");
        let Some(event) = self.event_mut(id) else {
            return;
        };
        event.listed -= 1;
        event.executed |= status.is_some();
        if !event.settled() {
            return;
        }
        let event = *event;
        self.finish(id, event, sink);
    }

    fn finish(&mut self, id: SchedulerItemId, event: Event, sink: &mut impl BluetoothRadioSink) {
        let mut anchor = None;
        let mut counts = PduCounts::default();
        match id.kind() {
            SchedulerRoleKind::LegacyAdvertising => {
                let slot =
                    find_mut(&mut self.legacy, id.instance()).expect("the set is configured");
                let finished = self.memory.legacy.finish_event(&slot.instance);
                slot.event = None;
                accounted(Ok::<(), ()>(()), finished, &mut self.faulted, sink);
            }
            SchedulerRoleKind::ConnectableAdvertising => {
                let slot =
                    find_mut(&mut self.connectable, id.instance()).expect("the set is configured");
                let source = self.memory.connectable.receive_source(&slot.instance);
                let finished = self.memory.connectable.finish_event(&slot.instance);
                slot.event = None;
                if let Some(source) = accounted(source, finished, &mut self.faulted, sink) {
                    drain_chain(
                        &mut self.memory.non_scanning,
                        source,
                        &self.clock,
                        event.id,
                        sink,
                        &mut self.faulted,
                        &mut counts,
                    );
                }
            }
            SchedulerRoleKind::LegacyScanning => {
                let slot =
                    find_mut(&mut self.scanners, id.instance()).expect("the scanner is configured");
                let source = self
                    .memory
                    .scanners
                    .receive_source(&slot.instance, usize::from(event.item));
                let finished = self.memory.scanners.finish_event(&slot.instance);
                slot.event = None;
                if let Some(source) = accounted(source, finished, &mut self.faulted, sink) {
                    drain_chain(
                        &mut self.memory.scanning,
                        source,
                        &self.clock,
                        event.id,
                        sink,
                        &mut self.faulted,
                        &mut counts,
                    );
                }
            }
            SchedulerRoleKind::PeripheralConnection => {
                let (slot, _) = self
                    .connections
                    .iter_mut()
                    .flatten()
                    .find(|(slot, _)| slot.instance.index() == id.instance())
                    .expect("the connection is open");
                let pool = &mut self.memory.connections;
                let finished = pool.finish_event(&slot.instance);
                if let Some(result) = accounted(finished, Ok::<(), ()>(()), &mut self.faulted, sink)
                {
                    if let PeripheralConnectionCapturedAnchorAvailability::Available(captured) =
                        result.capture
                    {
                        anchor = Some(
                            self.clock
                                .packet_start(captured.wrapping_controller_ticks()),
                        );
                    }
                    if result.status == PeripheralConnectionSchedulerItemCompletionStatus::Aborted {
                        anchor = None;
                    }
                }
                slot.event = None;
                loop {
                    match pool.receive(&slot.instance) {
                        Ok(Some(LeRxOutcome::Received(pdu))) => {
                            let captured_at = Some(
                                self.clock
                                    .packet_start(pdu.captured_time().wrapping_controller_ticks()),
                            );
                            counts.received = counts.received.saturating_add(1);
                            sink.outcome(RadioOutcome::Received {
                                id: event.id,
                                pdu: ReceivedPdu {
                                    pdu: pdu.as_bytes(),
                                    rssi_dbm: pdu.rssi_dbm(),
                                    captured_at,
                                },
                            })
                        }
                        Ok(Some(LeRxOutcome::Discarded)) => {
                            counts.discarded = counts.discarded.saturating_add(1);
                        }
                        Ok(None) => break,
                        Err(_) => {
                            self.faulted = true;
                            sink.fault(RadioFault::MemoryInconsistency);
                            break;
                        }
                    }
                }
                if pool.reclaim_transmission(&slot.instance) == Ok(true) {
                    sink.outcome(RadioOutcome::TransmitAcknowledged(slot.id));
                }
            }
            SchedulerRoleKind::DirectTestMode => {
                let slot = self.dtm.as_mut().expect("the test holds its instance");
                match self.memory.dtm.finish_event(&slot.instance, event.receiver) {
                    Ok(result) => {
                        if event.receiver {
                            let report = match result.received {
                                None => TestReport::Nothing,
                                Some(Ok(projection)) => TestReport::Received {
                                    rssi_dbm: projection.rssi().controller_value(),
                                },
                                Some(Err(_)) => TestReport::Failed,
                            };
                            if result.status != DtmSchedulerItemCompletionStatus::Aborted {
                                sink.outcome(RadioOutcome::TestReport {
                                    id: event.id,
                                    report,
                                });
                            }
                        }
                    }
                    Err(_) => {
                        self.faulted = true;
                        sink.fault(RadioFault::MemoryInconsistency);
                    }
                }
                slot.event = None;
            }
        }
        let result = if event.executed {
            EventResult::Executed { anchor }
        } else if event.unconfirmed {
            EventResult::Aborted
        } else {
            EventResult::NotExecuted
        };
        crate::trace::emit(|| oer_bluetooth_trace::RoleEventEnd {
            id: event.id.get(),
            result: match result {
                EventResult::Executed { anchor: Some(_) } => {
                    oer_bluetooth_trace::EndResult::ExecutedWithAnchor
                }
                EventResult::Executed { anchor: None } => oer_bluetooth_trace::EndResult::Executed,
                EventResult::NotExecuted => oer_bluetooth_trace::EndResult::NotExecuted,
                EventResult::Aborted => oer_bluetooth_trace::EndResult::Aborted,
            },
            received: counts.received,
            discarded: counts.discarded,
        });
        sink.outcome(RadioOutcome::EventEnded {
            id: event.id,
            result,
        });
    }
}

fn space<
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
>(
    memory: &BluetoothRadioMemory<
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
    >,
) -> SchedulerItemSpace<'_> {
    SchedulerItemSpace::new()
        .with(&memory.legacy)
        .with(&memory.connectable)
        .with(&memory.scanners)
        .with(&memory.connections)
        .with(&memory.dtm)
}

/// The receive source of an ending event, when its pool both named it and
/// finished the event. A refusal of either is an ownership inconsistency:
/// the event's receptions cannot be accounted, so the role faults instead of
/// dropping them silently.
fn accounted<S, E, F>(
    source: Result<S, E>,
    finished: Result<(), F>,
    faulted: &mut bool,
    sink: &mut impl BluetoothRadioSink,
) -> Option<S> {
    if source.is_err() || finished.is_err() {
        *faulted = true;
        sink.fault(RadioFault::MemoryInconsistency);
    }
    source.ok()
}

/// The PDUs an ending event reported and discarded, for its trace; each
/// count stops at 255.
#[derive(Default)]
struct PduCounts {
    received: u8,
    discarded: u8,
}

fn drain_chain<const PACKETS: usize>(
    chain: &mut LeRxChain<PACKETS>,
    source: LeRxSource,
    clock: &RadioClock,
    id: EventId,
    sink: &mut impl BluetoothRadioSink,
    faulted: &mut bool,
    counts: &mut PduCounts,
) {
    loop {
        match chain.take(source) {
            Ok(Some(LeRxOutcome::Received(pdu))) => {
                counts.received = counts.received.saturating_add(1);
                // A received PDU holds its two-octet header.
                if let [header, length, ..] = *pdu.as_bytes() {
                    crate::trace::emit(|| oer_bluetooth_trace::RolePdu {
                        id: id.get(),
                        header,
                        length,
                        rssi_dbm: pdu.rssi_dbm(),
                    });
                }
                let captured_at =
                    Some(clock.packet_start(pdu.captured_time().wrapping_controller_ticks()));
                sink.outcome(RadioOutcome::Received {
                    id,
                    pdu: ReceivedPdu {
                        pdu: pdu.as_bytes(),
                        rssi_dbm: pdu.rssi_dbm(),
                        captured_at,
                    },
                });
            }
            Ok(Some(LeRxOutcome::Discarded)) => {
                counts.discarded = counts.discarded.saturating_add(1);
            }
            Ok(None) => return,
            Err(_) => {
                *faulted = true;
                sink.fault(RadioFault::MemoryInconsistency);
                return;
            }
        }
    }
}

/// Longest scanner item the pinned restart schedules, in microseconds.
const SCAN_EVENT_MAX_MICROS: u32 = 0x8000;

/// The provider level of `power`, which a supported request holds at or
/// above the lowest one.
fn le_tx_power(power: TxPower) -> LeTxPower {
    LeTxPower::from_dbm(power.dbm()).expect("a supported power is at or above the lowest level")
}

/// The recurring receive wait the hardware holds for `wait`: above the short
/// form's range the long form counts two-microsecond units, so an odd wait
/// listens one microsecond longer rather than shorter.
fn recurring_wait(wait: RadioDuration<LeRadio>) -> u64 {
    let micros = wait.as_micros();
    if micros > RECEIVE_WAIT_SHORT_MAX_MICROS {
        micros.next_multiple_of(2)
    } else {
        micros
    }
}

fn overlaps(a: SchedulerRawWindow, b: SchedulerRawWindow) -> bool {
    (b.end().wrapping_sub(a.start()) as i32) > 0 && (a.end().wrapping_sub(b.start()) as i32) > 0
}

fn compact<T: Copy, const N: usize>(slots: &mut [Option<T>; N]) {
    let mut next = 0;
    for index in 0..N {
        if let Some(value) = slots[index] {
            slots[index] = None;
            slots[next] = Some(value);
            next += 1;
        }
    }
}

fn find<Id>(slots: &[Option<Slot<Id>>], instance: usize) -> Option<&Slot<Id>> {
    slots
        .iter()
        .flatten()
        .find(|slot| slot.instance.index() == instance)
}

fn find_mut<Id>(slots: &mut [Option<Slot<Id>>], instance: usize) -> Option<&mut Slot<Id>> {
    slots
        .iter_mut()
        .flatten()
        .find(|slot| slot.instance.index() == instance)
}

fn find_id<Id: Eq + Copy>(slots: &[Option<Slot<Id>>], id: Id) -> Option<usize> {
    slots
        .iter()
        .position(|slot| slot.as_ref().is_some_and(|slot| slot.id == id))
}

fn free_slot<Id>(slots: &[Option<Slot<Id>>]) -> Result<usize, RequestError> {
    slots
        .iter()
        .position(Option::is_none)
        .ok_or(RequestError::NoInstance)
}

const fn scan_channel(channel: AdvertisingChannel) -> LegacyScanPrimaryChannel {
    match channel {
        AdvertisingChannel::Channel37 => LegacyScanPrimaryChannel::Channel37,
        AdvertisingChannel::Channel38 => LegacyScanPrimaryChannel::Channel38,
        AdvertisingChannel::Channel39 => LegacyScanPrimaryChannel::Channel39,
    }
}

#[cfg(test)]
mod tests;

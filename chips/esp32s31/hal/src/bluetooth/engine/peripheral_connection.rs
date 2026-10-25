//! Peripheral connection instances.
//!
//! One instance holds the connection link state, the scheduler context, two
//! scheduler items forming the vendor's private free chain, the TX sentinel
//! pair with one control or ACL packet, and a private receive chain. As
//! `r_ble_lll_conn_use_rxbuf_from_link_state` does, the connection receives
//! outside the global lists: its link state selects no RX class and link-state
//! `+0x08` carries the hardware receive cursor into the private chain.
//!
//! The link state persists across events. Each event takes the free head
//! item, and finishing the event returns it.

#![forbid(unsafe_code)]

mod values;

use crate::layouts::*;

pub use values::{
    PeripheralConnectionCapturedAnchorAvailability, PeripheralConnectionCapturedAnchorTime,
    PeripheralConnectionDataChannel, PeripheralConnectionEventSpan, PeripheralConnectionIdentity,
    PeripheralConnectionReceiveTime, PeripheralConnectionReceiveWait,
    PeripheralConnectionRecurringReceiveWait, PeripheralConnectionSchedulerItemCompletionStatus,
    PeripheralConnectionSchedulerPriority, PeripheralConnectionSchedulerWindow,
    PeripheralConnectionTransmitPduKind,
};

use crate::bluetooth::engine::{
    coexistence::{ConnectionCoexistencePriorities, PeripheralConnectionCoexistenceProtection},
    direction_finding_workspace::DirectionFindingWorkspaceLink,
    le_rx_chain::{LeRxNodes, LeRxRing},
    le_rx_packet::LeRxOutcome,
    le_tx_packet::{
        LeTxBufferHeaderStorage, LeTxPacketAddress, LeTxPacketPrepareError, LeTxPacketStorage,
    },
    le_tx_power::LeTxPower,
    scheduler_context::SchedulerContextStorage,
    scheduler_item::{SchedulerItemCompletionStatus, SchedulerItemHeader},
    scheduler_pool::{
        SchedulerPoolError, SchedulerRoleInstance, SchedulerRoleKind, SchedulerRolePool,
        SchedulerRoleStorage, sealed,
    },
    sram_link::{ControllerSramLink, SchedulerItemLink},
};

/// Bytes retained by one connection link-state allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES: usize = BleLinkState::SIZE;
/// Bytes retained by one connection scheduler-item allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES: usize = BleSchedulerItem::SIZE;
/// Scheduler items retained by one connection allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT: usize = 2;
/// Bytes retained by the initially empty transmit queue sentinel.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES: usize = BleBufferHeader::SIZE;
/// Packets in the private receive chain: two writable successors of the
/// node that hardware may still hold.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_RX_PACKETS: usize = 3;

const ITEMS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT;
const EVENT_ITEM: usize = ITEMS - 1;
const RX_PACKETS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_RX_PACKETS;
/// The longest data PDU payload on air: 251 octets of data plus the MIC.
const DATA_PDU_PAYLOAD_BYTES: usize = 255;
const TX_PACKET_BYTES: usize =
    crate::bluetooth::engine::BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + DATA_PDU_PAYLOAD_BYTES;
// Bits 19:14 hold the coexistence protection; the vendor sets bits 21:20
// with it.
// The two connection lanes of the item priority word.
// Common allocation sets both bits; the connection role retains them.
// Standalone module default after common allocation and connection-role masks.
// Product policy for the dedicated radio, not the vendor coexistence default.
/// SOURCE: pinned `libble_app.a` first-event `r_sym_ble_tPr7egUaNHmqfcieCA5O`
/// (`r_ble_lll_conn_slave_new`) and recurring `r_sym_ble_rsCCyH2B22gdYkN4LOOJ`
/// (`ble_lll_conn_reschedule_event`) write the link-state priority nibble into
/// item `+0x18` bits 3:0 and a fixed 7 into bits 7:4.

#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
struct LinkStateStorage {
    layout: BleLinkState,
}

impl LinkStateStorage {
    const fn new() -> Self {
        Self {
            layout: BleLinkState::zeroed(),
        }
    }

    fn initialize(&self, binding: &PeripheralConnectionBinding) {
        crate::bluetooth::engine::shared_layout::clear_link_state(&self.layout);
        self.set_scheduler_head(binding.items[EVENT_ITEM]);
        let sentinel = binding.tx_sentinel.controller_address();
        self.layout
            .tx_head()
            .write(BleLinkStateTxHead::from_bits(sentinel));
        self.layout
            .tx_tail()
            .write(BleLinkStateTxTail::from_bits(sentinel));
    }

    fn set_scheduler_head(&self, head: SchedulerItemLink) {
        self.layout
            .scheduler_head()
            .write(BleLinkStateSchedulerHead::ZERO.with_address(Some(head.reference())));
    }

    /// Mirror the private chain's software head, tail and spare header.
    fn install_receive_endpoints(&self, (head, tail, spare): (u32, u32, u32)) {
        self.layout
            .rx_head()
            .write(BleLinkStateRxHead::from_bits(head));
        self.layout
            .rx_tail()
            .write(BleLinkStateRxTail::from_bits(tail));
        self.layout
            .rx_reserve()
            .write(BleLinkStateRxReserve::from_bits(spare));
    }

    fn prepare_identity(&self, identity: PeripheralConnectionIdentity) {
        self.layout
            .crc_initialization()
            .write(BleLinkStateCrcInitialization::from_bits(
                u32::from_le_bytes(identity.crc_initialization_word()),
            ));
        self.layout
            .access_address()
            .write(BleLinkStateAccessAddress::from_bits(u32::from_le_bytes(
                identity.access_address_wire_bytes(),
            )));
    }

    fn prepare_event_profile(
        &self,
        receive_head: ControllerSramLink<BleBufferHeader>,
        transmit_sentinel: ControllerSramLink<BleBufferHeader>,
        event: &PeripheralConnectionFirstEvent,
    ) {
        let tx = CONNECTION_TX_PATH;
        self.layout.tx_path().write(
            BleLinkStateTxPath::ZERO
                .with_header(Some(transmit_sentinel.reference()))
                .expect("a bound link is encodable")
                .with_max_tx_octets(tx.max_tx_octets())
                .with_queue_ready(tx.queue_ready())
                .with_valid(tx.valid()),
        );
        self.layout.rx_path().write(
            BleLinkStateRxPath::ZERO
                .with_header(Some(receive_head.reference()))
                .expect("a bound link is encodable")
                .with_unconsumed_limit(255),
        );
        self.layout.control_policy().write(CONNECTION_CONTROL);
        self.layout
            .packet_policy()
            .write(BleLinkStatePacketPolicy::ZERO);
        self.layout
            .word_001c_opaque()
            .write(BleLinkStateWord001cOpaque::ZERO);
        self.layout.rx_list().write(BleLinkStateRxList::ZERO);
        self.layout
            .crc_initialization()
            .write(self.layout.crc_initialization().read().with_ready(true));
        let sequence = match event.protection {
            Some(protection) => CONNECTION_SEQUENCE
                .with_coexistence_protection(protection.value())
                .expect("validated protection fits")
                .with_bits_20_21_opaque(3)
                .expect("reviewed image fits"),
            None => CONNECTION_SEQUENCE,
        };
        self.layout.sequence_policy().write(sequence);
        self.layout
            .direction_finding()
            .write(CONNECTION_RADIO_POLICY);
        self.layout.event().write(
            self.layout
                .event()
                .read()
                .with_power_index(event.default_tx_power.index())
                .with_priority(event.priority.value()),
        );
        self.layout
            .receive_context()
            .write(BleLinkStateReceiveContext::from_bits(
                event.receive_time.wrapping_controller_ticks(),
            ));
        self.layout
            .event_span()
            .write(BleLinkStateEventSpan::ZERO.with_value(event.event_span.ticks()));
    }

    fn prepare_recurring_event_profile(
        &self,
        event_span: PeripheralConnectionEventSpan,
        priority: PeripheralConnectionSchedulerPriority,
    ) {
        self.layout
            .event()
            .write(self.layout.event().read().with_priority(priority.value()));
        self.layout
            .event_span()
            .write(BleLinkStateEventSpan::from_bits(event_span.ticks()));
    }

    fn install_direction_finding_workspace(&self, workspace: DirectionFindingWorkspaceLink) {
        let original = self.layout.direction_finding().read();
        self.layout.direction_finding().write(
            BleLinkStateDirectionFinding::ZERO
                .with_configuration(Some(workspace.link_state_configuration()))
                .expect("a bound link is encodable")
                .with_common_radio_policy(original.common_radio_policy())
                .expect("retained radio policy fits")
                .with_ready(true)
                .with_bit_31_opaque(original.bit_31_opaque()),
        );
        self.layout.direction_finding_policy().write(
            self.layout
                .direction_finding_policy()
                .read()
                .with_bits_19_20_opaque(3)
                .expect("disabled policy fits")
                .with_bits_21_30_opaque(0)
                .expect("zero fits"),
        );
    }

    fn power_index(&self) -> u8 {
        self.layout.event().read().power_index()
    }

    fn receive_time(&self) -> PeripheralConnectionReceiveTime {
        PeripheralConnectionReceiveTime::from_controller_ticks(
            self.layout.receive_context().read().bits(),
        )
    }

    fn identity(&self) -> PeripheralConnectionIdentity {
        let crc = self.layout.crc_initialization().read().bits().to_le_bytes();
        PeripheralConnectionIdentity::new(
            self.layout.access_address().read().bits().to_le_bytes(),
            [crc[0], crc[1], crc[2]],
        )
    }
}

#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
struct ItemStorage {
    layout: BleSchedulerItem,
}

impl ItemStorage {
    const fn new() -> Self {
        Self {
            layout: BleSchedulerItem::zeroed(),
        }
    }

    fn header(&self) -> SchedulerItemHeader<'_> {
        SchedulerItemHeader::new(&self.layout)
    }

    /// The vendor connection allocator's image, linked to the next free item.
    fn initialize(
        &self,
        binding: &PeripheralConnectionBinding,
        next_free: Option<SchedulerItemLink>,
    ) {
        crate::bluetooth::engine::shared_layout::clear_scheduler_item(&self.layout);
        self.link_free(next_free);
        self.layout
            .word_001c_opaque()
            .write(CONNECTION_ALLOCATION_FLAGS);
        self.layout
            .allocation_number()
            .write(BleSchedulerItemAllocationNumber::from_bits(u32::from(
                binding.number,
            )));
        self.layout.context().write(
            BleSchedulerItemContext::ZERO
                .with_address(Some(binding.scheduler_context.reference()))
                .expect("a bound link is encodable"),
        );
        self.layout.link_state().write(
            BleSchedulerItemLinkState::ZERO
                .with_address(Some(binding.link_state.reference()))
                .expect("a bound link is encodable")
                .with_kind(2)
                .expect("connection prefix fits"),
        );
        self.layout.control().write(CONNECTION_ITEM_CONTROL);
    }

    fn link_free(&self, next_free: Option<SchedulerItemLink>) {
        self.layout.hardware_next().write(
            BleSchedulerItemHardwareNext::ZERO
                .with_address(next_free.map(ControllerSramLink::reference))
                .expect("a bound link is encodable")
                .with_bits_20_21_opaque(3)
                .expect("allocation prefix fits"),
        );
    }

    fn prepare_priority_and_channel(
        &self,
        channel: PeripheralConnectionDataChannel,
        priority: PeripheralConnectionSchedulerPriority,
        coexistence: ConnectionCoexistencePriorities,
    ) {
        self.layout.coexistence_priority().write(
            self.layout
                .coexistence_priority()
                .read()
                .with_lane_0(coexistence.event.value())
                .expect("validated priority fits")
                .with_lane_1(coexistence.base.value())
                .expect("validated priority fits"),
        );
        self.layout
            .context()
            .write(self.layout.context().read().with_event_ready(true));
        self.layout.channel_and_priority().write(
            self.layout
                .channel_and_priority()
                .read()
                .with_frequency(channel.frequency_image())
                .expect("data channel fits")
                .with_arbitration_priority(priority.value() % 16)
                .expect("projected priority fits")
                .with_priority_copy(7)
                .expect("reviewed image fits"),
        );
    }

    /// Current r_btdm_sched_calc_seq_time: the sequencer starts after the
    /// admitted lead and retains the complete window duration.
    fn prepare_window(&self, window: PeripheralConnectionSchedulerWindow, raw_sequence_lead: u32) {
        let header = self.header();
        header.set_status(0);
        header.set_raw_start(window.start());
        header.set_raw_end(window.end());
        header.clear_event_byte();
        header.set_sequence(window.start(), window.end(), raw_sequence_lead);
    }

    fn prepare_first_event(&self, power_index: u8, event: &PeripheralConnectionFirstEvent) {
        self.prepare_priority_and_channel(event.channel, event.priority, event.coexistence);
        self.layout.rate_and_power().write(
            self.layout
                .rate_and_power()
                .read()
                .with_power_index(power_index)
                .with_phy_code_28_29(0)
                .expect("zero fits")
                .with_phy_code_30_31(0)
                .expect("zero fits"),
        );
        self.layout
            .receive_wait()
            .write(CONNECTION_WAIT_SHORT.with_value(event.receive_wait.total_micros() as u16));
        self.prepare_window(event.window, event.raw_sequence_lead);
    }

    fn prepare_recurring_event(&self, event: &PeripheralConnectionRecurringEvent) {
        self.prepare_priority_and_channel(event.channel, event.priority, event.coexistence);
        let total_micros = event.receive_wait.total_micros();
        let wait = if total_micros == 0 {
            BleSchedulerItemReceiveWait::ZERO.with_value(1)
        } else if total_micros < u32::from(u16::MAX) {
            CONNECTION_WAIT_SHORT.with_value(total_micros as u16)
        } else {
            CONNECTION_WAIT_LONG.with_value((total_micros / 2) as u16)
        };
        self.layout.receive_wait().write(wait);
        self.prepare_window(event.window, event.raw_sequence_lead);
    }

    fn result(&self) -> PeripheralConnectionEventResult {
        let header = self.header();
        let Some(recorded) = header.completion_status() else {
            return PeripheralConnectionEventResult {
                status: PeripheralConnectionSchedulerItemCompletionStatus::Aborted,
                capture: PeripheralConnectionCapturedAnchorAvailability::Absent,
            };
        };
        let capture = if self.layout.status().read().capture_available() {
            PeripheralConnectionCapturedAnchorAvailability::Available(
                PeripheralConnectionCapturedAnchorTime::from_controller_sram_word(
                    self.layout.captured_anchor().read().bits(),
                ),
            )
        } else {
            PeripheralConnectionCapturedAnchorAvailability::Absent
        };
        PeripheralConnectionEventResult {
            status: match recorded {
                SchedulerItemCompletionStatus::Zero => {
                    PeripheralConnectionSchedulerItemCompletionStatus::Zero
                }
                SchedulerItemCompletionStatus::NonZero(_) => {
                    PeripheralConnectionSchedulerItemCompletionStatus::NonZero
                }
            },
            capture,
        }
    }
}

/// Controller-SRAM graph of one peripheral connection instance.
#[repr(C)]
#[derive(bytemuck::Zeroable)]
pub struct PeripheralConnectionStorage {
    link_state: LinkStateStorage,
    scheduler_context: SchedulerContextStorage,
    items: [ItemStorage; ITEMS],
    tx_sentinel: LeTxBufferHeaderStorage,
    tx_successor: LeTxBufferHeaderStorage,
    tx_packet: LeTxPacketStorage<TX_PACKET_BYTES>,
    rx: LeRxNodes<RX_PACKETS>,
}

/// Addresses and connection number of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct PeripheralConnectionBinding {
    link_state: ControllerSramLink<BleLinkState>,
    scheduler_context: ControllerSramLink<BleSchedulerContext>,
    items: [SchedulerItemLink; ITEMS],
    tx_sentinel: ControllerSramLink<BleBufferHeader>,
    tx_successor: ControllerSramLink<BleBufferHeader>,
    tx_packet: LeTxPacketAddress<TX_PACKET_BYTES>,
    rx: LeRxRing<RX_PACKETS>,
    number: u16,
}

#[doc(hidden)]
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum PeripheralConnectionPhase {
    Empty,
    Identity,
    Active { event: bool },
}

/// CPU-side state of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct PeripheralConnectionState {
    phase: PeripheralConnectionPhase,
    rx: Option<LeRxRing<RX_PACKETS>>,
    tx_current_second: bool,
    tx_pending: bool,
}

impl sealed::Sealed for PeripheralConnectionStorage {}

impl SchedulerRoleStorage for PeripheralConnectionStorage {
    const NUMBERS: usize = 1;
    const KIND: SchedulerRoleKind = SchedulerRoleKind::PeripheralConnection;
    const ITEMS: usize = ITEMS;
    const NEW: Self = Self {
        link_state: LinkStateStorage::new(),
        scheduler_context: SchedulerContextStorage::new(),
        items: [const { ItemStorage::new() }; ITEMS],
        tx_sentinel: LeTxBufferHeaderStorage::new(),
        tx_successor: LeTxBufferHeaderStorage::new(),
        tx_packet: LeTxPacketStorage::new(),
        rx: LeRxNodes::new(),
    };
    type Binding = PeripheralConnectionBinding;
    type State = PeripheralConnectionState;
    const INITIAL_STATE: PeripheralConnectionState = PeripheralConnectionState {
        phase: PeripheralConnectionPhase::Empty,
        rx: None,
        tx_current_second: false,
        tx_pending: false,
    };

    fn addresses<D: crate::bluetooth::engine::ControllerDmaSpace>(
        &self,
        placement: &crate::bluetooth::engine::Placement<D>,
        number: u16,
    ) -> PeripheralConnectionBinding {
        PeripheralConnectionBinding {
            link_state: ControllerSramLink::new(placement.reference(&self.link_state.layout)),
            scheduler_context: ControllerSramLink::new(
                placement.reference(self.scheduler_context.layout()),
            ),
            tx_sentinel: ControllerSramLink::new(placement.reference(&self.tx_sentinel.layout)),
            tx_successor: ControllerSramLink::new(placement.reference(&self.tx_successor.layout)),
            items: core::array::from_fn(|index| {
                ControllerSramLink::new(placement.reference(&self.items[index].layout))
            }),
            tx_packet: LeTxPacketAddress::from_placed(placement, &self.tx_packet),
            rx: LeRxRing::addresses(&self.rx, placement),
            number,
        }
    }

    fn item_layout(&self, item: usize) -> &BleSchedulerItem {
        &self.items[item].layout
    }

    fn item_link(binding: &PeripheralConnectionBinding, item: usize) -> SchedulerItemLink {
        binding.items[item]
    }

    fn reinitialize(&mut self, binding: &PeripheralConnectionBinding) -> PeripheralConnectionState {
        self.scheduler_context.clear();
        self.items[0].initialize(binding, None);
        self.items[1].initialize(binding, Some(binding.items[0]));
        self.link_state.initialize(binding);
        self.tx_sentinel.initialize_empty_cursor();
        self.tx_successor.initialize_empty_cursor();
        self.tx_packet.clear();
        let mut rx = binding.rx;
        rx.view(&self.rx).initialize();
        PeripheralConnectionState {
            rx: Some(rx),
            ..Self::INITIAL_STATE
        }
    }

    fn admits(state: &PeripheralConnectionState, item: usize) -> bool {
        item == EVENT_ITEM && state.phase == PeripheralConnectionPhase::Active { event: true }
    }
}

/// Pool of peripheral connection instances.
pub type PeripheralConnectionPool<'pool, 'epoch, const N: usize> =
    SchedulerRolePool<'pool, 'epoch, PeripheralConnectionStorage, N>;

/// Why a connection operation was refused. Nothing changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionError {
    Pool(SchedulerPoolError),
    /// The operation does not follow the instance's preparation state.
    State,
    /// The private receive chain refused to continue; the vendor asserts.
    Receive(crate::bluetooth::engine::LeRxChainError),
    /// The packet does not fit the transmit allocation.
    Transmit(LeTxPacketPrepareError),
}

/// Fields of the first connection event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionFirstEvent {
    pub channel: PeripheralConnectionDataChannel,
    pub receive_time: PeripheralConnectionReceiveTime,
    pub event_span: PeripheralConnectionEventSpan,
    pub window: PeripheralConnectionSchedulerWindow,
    pub receive_wait: PeripheralConnectionReceiveWait,
    pub default_tx_power: LeTxPower,
    pub priority: PeripheralConnectionSchedulerPriority,
    pub coexistence: ConnectionCoexistencePriorities,
    /// Coexistence protection of the connection; none leaves it clear.
    pub protection: Option<PeripheralConnectionCoexistenceProtection>,
    /// The accepted scheduler preparation lead in controller ticks.
    pub raw_sequence_lead: u32,
}

/// Fields of one software-widened recurring connection event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionRecurringEvent {
    pub channel: PeripheralConnectionDataChannel,
    pub event_span: PeripheralConnectionEventSpan,
    pub window: PeripheralConnectionSchedulerWindow,
    pub receive_wait: PeripheralConnectionRecurringReceiveWait,
    pub priority: PeripheralConnectionSchedulerPriority,
    pub coexistence: ConnectionCoexistencePriorities,
    pub raw_sequence_lead: u32,
}

/// Item of one prepared connection event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionEvent {
    item: usize,
    window: PeripheralConnectionSchedulerWindow,
}

impl PeripheralConnectionEvent {
    pub const fn item(&self) -> usize {
        self.item
    }

    pub const fn raw_window(&self) -> (u32, u32) {
        (self.window.start(), self.window.end())
    }
}

/// What the event item recorded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralConnectionEventResult {
    pub status: PeripheralConnectionSchedulerItemCompletionStatus,
    pub capture: PeripheralConnectionCapturedAnchorAvailability,
}

impl<'pool, 'epoch, const N: usize> PeripheralConnectionPool<'pool, 'epoch, N> {
    /// Install the Access Address and CRCInit.
    pub fn prepare_identity(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        identity: PeripheralConnectionIdentity,
    ) -> Result<(), PeripheralConnectionError> {
        let cpu = self
            .cpu(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        if cpu.state.phase != PeripheralConnectionPhase::Empty {
            return Err(PeripheralConnectionError::State);
        }
        cpu.graph.link_state.prepare_identity(identity);
        cpu.state.phase = PeripheralConnectionPhase::Identity;
        Ok(())
    }

    /// Lower the first event, join the global direction-finding workspace
    /// and take the event item from the free chain.
    pub(crate) fn prepare_first_event(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        event: PeripheralConnectionFirstEvent,
        workspace: DirectionFindingWorkspaceLink,
    ) -> Result<PeripheralConnectionEvent, PeripheralConnectionError> {
        let cpu = self
            .cpu(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        if cpu.state.phase != PeripheralConnectionPhase::Identity {
            return Err(PeripheralConnectionError::State);
        }
        let rx = cpu
            .state
            .rx
            .expect("an acquired instance has its receive ring");
        let graph = &cpu.graph;
        graph.link_state.install_receive_endpoints(rx.snapshot());
        graph
            .link_state
            .prepare_event_profile(rx.head_link(), cpu.binding.tx_sentinel, &event);
        graph.items[EVENT_ITEM].prepare_first_event(graph.link_state.power_index(), &event);
        graph
            .link_state
            .install_direction_finding_workspace(workspace);
        take_event_item(graph, cpu.binding);
        cpu.state.phase = PeripheralConnectionPhase::Active { event: true };
        Ok(PeripheralConnectionEvent {
            item: EVENT_ITEM,
            window: event.window,
        })
    }

    /// Lower one recurring event; identity, packet history and the receive
    /// cursor stay.
    pub fn prepare_recurring_event(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        event: PeripheralConnectionRecurringEvent,
    ) -> Result<PeripheralConnectionEvent, PeripheralConnectionError> {
        let cpu = self
            .cpu(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        if cpu.state.phase != (PeripheralConnectionPhase::Active { event: false }) {
            return Err(PeripheralConnectionError::State);
        }
        let graph = &cpu.graph;
        graph
            .link_state
            .prepare_recurring_event_profile(event.event_span, event.priority);
        graph.items[EVENT_ITEM].prepare_recurring_event(&event);
        take_event_item(graph, cpu.binding);
        cpu.state.phase = PeripheralConnectionPhase::Active { event: true };
        Ok(PeripheralConnectionEvent {
            item: EVENT_ITEM,
            window: event.window,
        })
    }

    /// Read the event item's result and return it to the free chain.
    pub fn finish_event(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> Result<PeripheralConnectionEventResult, PeripheralConnectionError> {
        let cpu = self
            .cpu(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        if cpu.state.phase != (PeripheralConnectionPhase::Active { event: true }) {
            return Err(PeripheralConnectionError::State);
        }
        let item = &cpu.graph.items[EVENT_ITEM];
        let result = item.result();
        item.link_free(Some(cpu.binding.items[EVENT_ITEM - 1]));
        item.header().set_status(0);
        cpu.graph
            .link_state
            .set_scheduler_head(cpu.binding.items[EVENT_ITEM]);
        cpu.state.phase = PeripheralConnectionPhase::Active { event: false };
        Ok(result)
    }

    /// Take the oldest completed packet from the private receive chain.
    pub fn receive(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> Result<Option<LeRxOutcome>, PeripheralConnectionError> {
        let cpu = self
            .cpu(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        let rx = cpu
            .state
            .rx
            .as_mut()
            .expect("an acquired instance has its receive ring");
        let outcome = rx
            .view(&cpu.graph.rx)
            .take(None, true)
            .map_err(PeripheralConnectionError::Receive)?;
        cpu.graph
            .link_state
            .install_receive_endpoints(rx.snapshot());
        Ok(outcome)
    }

    /// Latest hardware receive time; the creation seed until a valid
    /// reception updates it.
    pub fn receive_time(
        &self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> Result<PeripheralConnectionReceiveTime, PeripheralConnectionError> {
        let (graph, _, _) = self
            .shared(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        Ok(graph.link_state.receive_time())
    }

    pub fn identity(
        &self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> Result<PeripheralConnectionIdentity, PeripheralConnectionError> {
        let (graph, _, _) = self
            .shared(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        Ok(graph.link_state.identity())
    }

    /// Whether one new reliable packet can be queued.
    pub fn can_enqueue_transmission(
        &self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> bool {
        self.shared(instance)
            .is_ok_and(|(_, _, state)| !state.tx_pending)
    }

    /// Queue one packet. `Ok(false)` keeps the pending packet and its
    /// retransmission state. The payload stays owned until the peer
    /// acknowledges it.
    pub fn enqueue_transmission(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        kind: PeripheralConnectionTransmitPduKind,
        payload: &[u8],
    ) -> Result<bool, PeripheralConnectionError> {
        let cpu = self
            .cpu(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        if cpu.state.phase == PeripheralConnectionPhase::Empty {
            return Err(PeripheralConnectionError::State);
        }
        if cpu.state.tx_pending {
            return Ok(false);
        }
        let graph = &mut *cpu.graph;
        graph
            .tx_packet
            .prepare_pdu(kind.llid(), payload)
            .map_err(PeripheralConnectionError::Transmit)?;
        let (current, next, next_address) = if cpu.state.tx_current_second {
            (
                &graph.tx_successor,
                &graph.tx_sentinel,
                cpu.binding.tx_sentinel,
            )
        } else {
            (
                &graph.tx_sentinel,
                &graph.tx_successor,
                cpu.binding.tx_successor,
            )
        };
        next.initialize_bound_tx(cpu.binding.tx_packet);
        next.mark_complete_connection_packet(kind.llid());
        current.link_successor(next_address);
        graph
            .link_state
            .layout
            .tx_tail()
            .write(BleLinkStateTxTail::ZERO.with_address(Some(next_address.reference())));
        cpu.state.tx_pending = true;
        Ok(true)
    }

    /// Reclaim an acknowledged packet while the hardware keeps the completed
    /// header as its cursor. A scheduler completion alone does not complete
    /// a queued transmission.
    pub fn reclaim_transmission(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
    ) -> Result<bool, PeripheralConnectionError> {
        let cpu = self
            .cpu(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        if !cpu.state.tx_pending {
            return Ok(false);
        }
        let (header, address) = if cpu.state.tx_current_second {
            (&cpu.graph.tx_sentinel, cpu.binding.tx_sentinel)
        } else {
            (&cpu.graph.tx_successor, cpu.binding.tx_successor)
        };
        if !header.transmission_completed() {
            return Ok(false);
        }
        // The completed node remains the hardware current cursor. Only its
        // packet allocation is released, matching get_txed_buffer's tail case.
        header.release_completed_packet();
        cpu.graph
            .link_state
            .layout
            .tx_head()
            .write(BleLinkStateTxHead::ZERO.with_address(Some(address.reference())));
        cpu.state.tx_current_second = !cpu.state.tx_current_second;
        cpu.state.tx_pending = false;
        Ok(true)
    }
}

/// Detach the event item from the free chain; the executor links it.
fn take_event_item(graph: &PeripheralConnectionStorage, binding: &PeripheralConnectionBinding) {
    graph.items[EVENT_ITEM].header().link_hardware_next(None);
    graph
        .link_state
        .set_scheduler_head(binding.items[EVENT_ITEM - 1]);
}

#[cfg(test)]
mod tests;

/// SOURCE: pinned `ble_3.o` connection allocation/reset and event bodies in the connection reference.
const CONNECTION_TX_PATH: BleLinkStateTxPath = BleLinkStateTxPath::ZERO
    .with_max_tx_octets(0xfb)
    .with_queue_ready(true)
    .with_valid(true);

/// SOURCE: pinned `ble_3.o` connection allocation/reset and event bodies in the connection reference.
const CONNECTION_CONTROL: BleLinkStateControlPolicy = BleLinkStateControlPolicy::ZERO
    .with_bits_20_23_opaque(0x2)
    .expect("reviewed field image fits")
    .with_bits_24_27_opaque(0x2)
    .expect("reviewed field image fits")
    .with_bit_31_opaque(true);

/// SOURCE: pinned `ble_3.o` connection allocation/reset and event bodies in the connection reference.
const CONNECTION_SEQUENCE: BleLinkStateSequencePolicy = BleLinkStateSequencePolicy::ZERO
    .with_bits_9_12_opaque(0xf)
    .expect("reviewed field image fits");

/// SOURCE: pinned `ble_3.o` connection allocation/reset and event bodies in the connection reference.
const CONNECTION_RADIO_POLICY: BleLinkStateDirectionFinding = BleLinkStateDirectionFinding::ZERO
    .with_common_radio_policy(0x3)
    .expect("reviewed field image fits");

/// SOURCE: pinned `ble_3.o` connection allocation/reset and event bodies in the connection reference.
const CONNECTION_WAIT_SHORT: BleSchedulerItemReceiveWait = BleSchedulerItemReceiveWait::ZERO
    .with_bits_16_20_opaque(0xf)
    .expect("reviewed field image fits");

/// SOURCE: pinned `ble_3.o` connection allocation/reset and event bodies in the connection reference.
const CONNECTION_WAIT_LONG: BleSchedulerItemReceiveWait = BleSchedulerItemReceiveWait::ZERO
    .with_bits_16_20_opaque(0x1f)
    .expect("reviewed field image fits");

/// SOURCE: pinned `ble_3.o` connection allocation/reset and event bodies in the connection reference.
const CONNECTION_ALLOCATION_FLAGS: BleSchedulerItemWord001cOpaque =
    BleSchedulerItemWord001cOpaque::from_bits(0xe7df7fff);

/// SOURCE: pinned `ble_3.o` connection allocation/reset and event bodies in the connection reference.
const CONNECTION_ITEM_CONTROL: BleSchedulerItemControl =
    BleSchedulerItemControl::ZERO.with_kind(0x3);

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

use vcell::VolatileCell;

pub use values::{
    PeripheralConnectionCapturedAnchorAvailability, PeripheralConnectionCapturedAnchorTime,
    PeripheralConnectionDataChannel, PeripheralConnectionEventSpan, PeripheralConnectionIdentity,
    PeripheralConnectionReceiveTime, PeripheralConnectionReceiveWait,
    PeripheralConnectionRecurringReceiveWait, PeripheralConnectionSchedulerItemCompletionStatus,
    PeripheralConnectionSchedulerPriority, PeripheralConnectionSchedulerWindow,
    PeripheralConnectionTransmitPduKind,
};

use crate::{
    coexistence::{
        ConnectionCoexistencePriorities, PeripheralConnectionCoexistenceProtection, lanes_image,
    },
    direction_finding_workspace::DirectionFindingWorkspaceLink,
    le_rx_chain::{LeRxChainBindError, LeRxNodes, LeRxRing},
    le_rx_packet::LeRxOutcome,
    le_tx_packet::{
        LeTxBufferHeaderStorage, LeTxPacketAddress, LeTxPacketPrepareError, LeTxPacketStorage,
    },
    le_tx_power::LeTxPower,
    link_state_event::{LinkStateEventWord, item_with_le_1m_power},
    scheduler_context::SchedulerContextStorage,
    scheduler_item::{SchedulerItemCompletionStatus, SchedulerItemHeader},
    scheduler_pool::{
        SchedulerPoolBindError, SchedulerPoolError, SchedulerRoleInstance, SchedulerRoleKind,
        SchedulerRolePool, SchedulerRoleStorage, sealed,
    },
    sram_link::ControllerSramLinkAddress,
};

/// Bytes retained by one connection link-state allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES: usize = 0x84;
/// Bytes retained by one connection scheduler-item allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Scheduler items retained by one connection allocation.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT: usize = 2;
/// Bytes retained by the initially empty transmit queue sentinel.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES: usize = 0x18;
/// Packets in the private receive chain: two writable successors of the
/// node that hardware may still hold.
pub const BLUETOOTH_PERIPHERAL_CONNECTION_RX_PACKETS: usize = 3;

const ITEMS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT;
const EVENT_ITEM: usize = ITEMS - 1;
const RX_PACKETS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_RX_PACKETS;
const LINK_STATE_WORDS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES / 4;
const SCHEDULER_ITEM_WORDS: usize = BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES / 4;
/// The longest data PDU payload on air: 251 octets of data plus the MIC.
const DATA_PDU_PAYLOAD_BYTES: usize = 255;
const TX_PACKET_BYTES: usize = crate::BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + DATA_PDU_PAYLOAD_BYTES;

const LINK_STATE_SCHEDULER_HEAD: usize = 0x64 / 4;
const LINK_STATE_RX_HEAD: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD: usize = 0x6c / 4;
const LINK_STATE_RX_TAIL: usize = 0x70 / 4;
const LINK_STATE_TX_TAIL: usize = 0x74 / 4;
const LINK_STATE_RX_RESERVE: usize = 0x78 / 4;
const LINK_STATE_TX_PATH: usize = 0;
const LINK_STATE_CRC_INITIALIZATION: usize = 0x2c / 4;
const LINK_STATE_ACCESS_ADDRESS: usize = 0x38 / 4;
const LINK_STATE_RX_PATH: usize = 2;
const LINK_STATE_CONTROL_POLICY: usize = 3;
const LINK_STATE_PACKET_FLAGS: usize = 0x14 / 4;
const LINK_STATE_RECEIVE_TIME: usize = 0x18 / 4;
const LINK_STATE_PACKET_HISTORY: usize = 0x1c / 4;
const LINK_STATE_PACKET_CONTROL: usize = 0x20 / 4;
const LINK_STATE_PACKET_SEQUENCE: usize = 0x30 / 4;
const LINK_STATE_EVENT_SPAN: usize = 0x34 / 4;
const LINK_STATE_COMMON_RADIO_AND_DIRECTION_FINDING_CONFIGURATION: usize = 0x50 / 4;
const LINK_STATE_DIRECTION_FINDING_POLICY: usize = 0x54 / 4;
const LINK_STATE_EVENT_PRIORITY: usize = 0x60 / 4;
const LINK_STATE_TX_PATH_VALID: u32 = 1 << 31;
const LINK_STATE_TX_QUEUE_READY: u32 = 1 << 28;
const LINK_STATE_SUPPORTED_MAX_TX_OCTETS: u32 = 251;
const LINK_STATE_RX_UNCONSUMED_LIMIT: u32 = 0xff;
const LINK_STATE_CONTROL_POLICY_ACTIVE: u32 = 1 << 31;
const LINK_STATE_BASELINE_CONTROL_POLICY: u32 = 2;
const LINK_STATE_CRC_CONTEXT_READY: u32 = 1 << 31;
const LINK_STATE_PACKET_SEQUENCE_BASELINE: u32 = 0x1e00;
// Bits 19:14 hold the coexistence protection; the vendor sets bits 21:20
// with it.
const LINK_STATE_COEXISTENCE_PROTECTION_SHIFT: u32 = 14;
const LINK_STATE_COEXISTENCE_PROTECTED: u32 = 0x0030_0000;
// The two connection lanes of the item priority word.
const CONNECTION_LANES_MASK: u32 = 0x3ff;
const LINK_STATE_COMMON_RADIO_POLICY_BASELINE: u32 = 3;
const LINK_STATE_DIRECTION_FINDING_RETAINED_POLICY: u32 = 0xbf00_0000;
const LINK_STATE_DIRECTION_FINDING_CONFIGURATION_READY: u32 = 1 << 30;
const LINK_STATE_DIRECTION_FINDING_POLICY_RETAINED: u32 = 0x8007_ffff;
const LINK_STATE_DIRECTION_FINDING_DISABLED_BASELINE: u32 = 0x0018_0000;

const SCHEDULER_ITEM_CONTEXT: usize = 1;
const SCHEDULER_ITEM_LINK_STATE: usize = 2;
const SCHEDULER_ITEM_CONTEXT_STATE: usize = 1;
const SCHEDULER_ITEM_RATE_AND_POWER: usize = 0x14 / 4;
const SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY: usize = 0x18 / 4;
const SCHEDULER_ITEM_ALLOCATION_FLAGS: usize = 0x1c / 4;
const SCHEDULER_ITEM_ALLOCATION_NUMBER: usize = 0x20 / 4;
const SCHEDULER_ITEM_RADIO_REQUEST_PRIORITIES: usize = 0x24 / 4;
const SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION: usize = 0x2c / 4;
const SCHEDULER_ITEM_CAPTURED_ANCHOR: usize = 0x34 / 4;
// Common allocation sets both bits; the connection role retains them.
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0030_0000;
// Standalone module default after common allocation and connection-role masks.
const SCHEDULER_ITEM_PERIPHERAL_ALLOCATION_FLAGS: u32 = 0xe7df_7fff;
// Product policy for the dedicated radio, not the vendor coexistence default.
const SCHEDULER_ITEM_PERIPHERAL_PREFIX: u32 = 0x0020_0000;
const SCHEDULER_ITEM_CONNECTION_CLASS: u32 = 3 << 8;
const SCHEDULER_ITEM_CONTEXT_READY: u32 = 1 << 31;
const SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY_MASK: u32 = 0x0000_7fff;
/// SOURCE: pinned `libble_app.a` first-event `r_sym_ble_tPr7egUaNHmqfcieCA5O`
/// (`r_ble_lll_conn_slave_new`) and recurring `r_sym_ble_rsCCyH2B22gdYkN4LOOJ`
/// (`ble_lll_conn_reschedule_event`) write the link-state priority nibble into
/// item `+0x18` bits 3:0 and a fixed 7 into bits 7:4.
const SCHEDULER_ITEM_CONNECTION_PRIORITY_HIGH_NIBBLE: u32 = 0x70;
const SCHEDULER_ITEM_PRIORITY_NIBBLE_MASK: u32 = 0x0f;
const SCHEDULER_ITEM_RECEIVE_WAIT_SHORT_MODE: u32 = 0x000f_0000;
const SCHEDULER_ITEM_RECEIVE_WAIT_LONG_MODE: u32 = 0x001f_0000;
const SCHEDULER_ITEM_RECEIVE_WAIT_ZERO_IMAGE: u32 = 1;
const SCHEDULER_ITEM_CAPTURE_AVAILABLE: u32 = 1 << 11;

#[repr(C, align(4))]
struct LinkStateStorage {
    words: [VolatileCell<u32>; LINK_STATE_WORDS],
}

impl LinkStateStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; LINK_STATE_WORDS],
        }
    }

    fn initialize(&self, binding: &PeripheralConnectionBinding) {
        for word in &self.words {
            word.set(0);
        }
        self.set_scheduler_head(binding.items[EVENT_ITEM]);
        let sentinel = binding.tx_sentinel.controller_address().address();
        self.words[LINK_STATE_TX_HEAD].set(sentinel);
        self.words[LINK_STATE_TX_TAIL].set(sentinel);
    }

    fn set_scheduler_head(&self, head: ControllerSramLinkAddress) {
        self.words[LINK_STATE_SCHEDULER_HEAD].set(head.controller_address().address());
    }

    /// Mirror the private chain's software head, tail and spare header.
    fn install_receive_endpoints(&self, (head, tail, spare): (u32, u32, u32)) {
        self.words[LINK_STATE_RX_HEAD].set(head);
        self.words[LINK_STATE_RX_TAIL].set(tail);
        self.words[LINK_STATE_RX_RESERVE].set(spare);
    }

    fn prepare_identity(&self, identity: PeripheralConnectionIdentity) {
        self.words[LINK_STATE_CRC_INITIALIZATION]
            .set(u32::from_le_bytes(identity.crc_initialization_word()));
        self.words[LINK_STATE_ACCESS_ADDRESS]
            .set(u32::from_le_bytes(identity.access_address_wire_bytes()));
    }

    fn prepare_event_profile(
        &self,
        receive_head: ControllerSramLinkAddress,
        transmit_sentinel: ControllerSramLinkAddress,
        event: &PeripheralConnectionFirstEvent,
    ) {
        self.words[LINK_STATE_TX_PATH].set(
            LINK_STATE_TX_PATH_VALID
                | LINK_STATE_TX_QUEUE_READY
                | (LINK_STATE_SUPPORTED_MAX_TX_OCTETS << 20)
                | transmit_sentinel.compressed_image(),
        );
        // use_rxbuf_from_link_state: the private consumer starts at the
        // software head.
        self.words[LINK_STATE_RX_PATH]
            .set((LINK_STATE_RX_UNCONSUMED_LIMIT << 20) | receive_head.compressed_image());
        self.words[LINK_STATE_CONTROL_POLICY].set(
            LINK_STATE_CONTROL_POLICY_ACTIVE
                | (LINK_STATE_BASELINE_CONTROL_POLICY << 20)
                | (LINK_STATE_BASELINE_CONTROL_POLICY << 24),
        );
        self.words[LINK_STATE_PACKET_FLAGS].set(0);
        self.words[LINK_STATE_PACKET_HISTORY].set(0);
        // No RX class: the connection receives through its private chain.
        self.words[LINK_STATE_PACKET_CONTROL].set(0);
        self.words[LINK_STATE_CRC_INITIALIZATION]
            .set(self.words[LINK_STATE_CRC_INITIALIZATION].get() | LINK_STATE_CRC_CONTEXT_READY);
        self.words[LINK_STATE_PACKET_SEQUENCE].set(match event.protection {
            Some(protection) => {
                LINK_STATE_PACKET_SEQUENCE_BASELINE
                    | LINK_STATE_COEXISTENCE_PROTECTED
                    | (u32::from(protection.value()) << LINK_STATE_COEXISTENCE_PROTECTION_SHIFT)
            }
            None => LINK_STATE_PACKET_SEQUENCE_BASELINE,
        });
        self.words[LINK_STATE_COMMON_RADIO_AND_DIRECTION_FINDING_CONFIGURATION]
            .set(LINK_STATE_COMMON_RADIO_POLICY_BASELINE << 24);
        // SOURCE: pinned `libble_app.a[ble_3.o]::r_sym_ble_bgOSnaHsEjrTC0mkupqH`
        // (`r_ble_lll_conn_reset_link_state`) stores the transmit-power index
        // in byte `+0x61`, next to the priority byte `+0x60`, and not
        // in `+0x04`.
        let event_word = LinkStateEventWord::from_word(self.words[LINK_STATE_EVENT_PRIORITY].get())
            .with_power(event.default_tx_power)
            .with_priority(event.priority.value());
        self.words[LINK_STATE_EVENT_PRIORITY].set(event_word.word());
        self.words[LINK_STATE_RECEIVE_TIME].set(event.receive_time.wrapping_controller_ticks());
        self.words[LINK_STATE_EVENT_SPAN].set(event.event_span.ticks());
    }

    fn prepare_recurring_event_profile(
        &self,
        event_span: PeripheralConnectionEventSpan,
        priority: PeripheralConnectionSchedulerPriority,
    ) {
        let event_word = LinkStateEventWord::from_word(self.words[LINK_STATE_EVENT_PRIORITY].get())
            .with_priority(priority.value());
        self.words[LINK_STATE_EVENT_PRIORITY].set(event_word.word());
        self.words[LINK_STATE_EVENT_SPAN].set(event_span.ticks());
    }

    fn install_direction_finding_workspace(&self, workspace: DirectionFindingWorkspaceLink) {
        let configuration =
            self.words[LINK_STATE_COMMON_RADIO_AND_DIRECTION_FINDING_CONFIGURATION].get();
        self.words[LINK_STATE_COMMON_RADIO_AND_DIRECTION_FINDING_CONFIGURATION].set(
            (configuration & LINK_STATE_DIRECTION_FINDING_RETAINED_POLICY)
                | LINK_STATE_DIRECTION_FINDING_CONFIGURATION_READY
                | workspace.compressed_link_state_configuration(),
        );
        let policy = self.words[LINK_STATE_DIRECTION_FINDING_POLICY].get();
        self.words[LINK_STATE_DIRECTION_FINDING_POLICY].set(
            (policy & LINK_STATE_DIRECTION_FINDING_POLICY_RETAINED)
                | LINK_STATE_DIRECTION_FINDING_DISABLED_BASELINE,
        );
    }

    fn power_index(&self) -> u8 {
        LinkStateEventWord::from_word(self.words[LINK_STATE_EVENT_PRIORITY].get()).power_index()
    }

    fn receive_time(&self) -> PeripheralConnectionReceiveTime {
        PeripheralConnectionReceiveTime::from_controller_ticks(
            self.words[LINK_STATE_RECEIVE_TIME].get(),
        )
    }

    fn identity(&self) -> PeripheralConnectionIdentity {
        let crc = self.words[LINK_STATE_CRC_INITIALIZATION]
            .get()
            .to_le_bytes();
        PeripheralConnectionIdentity::new(
            self.words[LINK_STATE_ACCESS_ADDRESS].get().to_le_bytes(),
            [crc[0], crc[1], crc[2]],
        )
    }
}

#[repr(C, align(4))]
struct ItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl ItemStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
        }
    }

    fn header(&self) -> SchedulerItemHeader<'_, [VolatileCell<u32>; SCHEDULER_ITEM_WORDS]> {
        SchedulerItemHeader::new(&self.words)
    }

    /// The vendor connection allocator's image, linked to the next free item.
    fn initialize(
        &self,
        binding: &PeripheralConnectionBinding,
        next_free: Option<ControllerSramLinkAddress>,
    ) {
        for word in &self.words {
            word.set(0);
        }
        self.link_free(next_free);
        self.words[SCHEDULER_ITEM_ALLOCATION_FLAGS].set(SCHEDULER_ITEM_PERIPHERAL_ALLOCATION_FLAGS);
        self.words[SCHEDULER_ITEM_ALLOCATION_NUMBER].set(u32::from(binding.number));
        self.words[SCHEDULER_ITEM_CONTEXT].set(binding.scheduler_context.compressed_image());
        self.words[SCHEDULER_ITEM_LINK_STATE]
            .set(SCHEDULER_ITEM_PERIPHERAL_PREFIX | binding.link_state.compressed_image());
        self.header().set_control(SCHEDULER_ITEM_CONNECTION_CLASS);
    }

    fn link_free(&self, next_free: Option<ControllerSramLinkAddress>) {
        self.header().set_hardware_next_word(
            SCHEDULER_ITEM_ALLOCATION_PREFIX
                | next_free.map_or(0, ControllerSramLinkAddress::compressed_image),
        );
    }

    fn prepare_priority_and_channel(
        &self,
        channel: PeripheralConnectionDataChannel,
        priority: PeripheralConnectionSchedulerPriority,
        coexistence: ConnectionCoexistencePriorities,
    ) {
        self.words[SCHEDULER_ITEM_RADIO_REQUEST_PRIORITIES].set(
            (self.words[SCHEDULER_ITEM_RADIO_REQUEST_PRIORITIES].get() & !CONNECTION_LANES_MASK)
                | lanes_image(&[coexistence.event, coexistence.base]),
        );
        self.words[SCHEDULER_ITEM_CONTEXT_STATE]
            .set(self.words[SCHEDULER_ITEM_CONTEXT_STATE].get() | SCHEDULER_ITEM_CONTEXT_READY);
        let priority = u32::from(priority.value());
        self.words[SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY].set(
            (self.words[SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY].get()
                & !SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY_MASK)
                | (u32::from(channel.frequency_image()) << 8)
                | (priority & SCHEDULER_ITEM_PRIORITY_NIBBLE_MASK)
                | SCHEDULER_ITEM_CONNECTION_PRIORITY_HIGH_NIBBLE,
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
        self.words[SCHEDULER_ITEM_RATE_AND_POWER].set(item_with_le_1m_power(
            self.words[SCHEDULER_ITEM_RATE_AND_POWER].get(),
            power_index,
        ));
        self.words[SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION]
            .set(SCHEDULER_ITEM_RECEIVE_WAIT_SHORT_MODE | event.receive_wait.total_micros());
        self.prepare_window(event.window, event.raw_sequence_lead);
    }

    fn prepare_recurring_event(&self, event: &PeripheralConnectionRecurringEvent) {
        self.prepare_priority_and_channel(event.channel, event.priority, event.coexistence);
        let total_micros = event.receive_wait.total_micros();
        let receive_wait_image = if total_micros == 0 {
            SCHEDULER_ITEM_RECEIVE_WAIT_ZERO_IMAGE
        } else if total_micros < u32::from(u16::MAX) {
            SCHEDULER_ITEM_RECEIVE_WAIT_SHORT_MODE | total_micros
        } else {
            SCHEDULER_ITEM_RECEIVE_WAIT_LONG_MODE | (total_micros >> 1)
        };
        self.words[SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION].set(receive_wait_image);
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
        let capture = if header.status() & SCHEDULER_ITEM_CAPTURE_AVAILABLE != 0 {
            PeripheralConnectionCapturedAnchorAvailability::Available(
                PeripheralConnectionCapturedAnchorTime::from_controller_sram_word(
                    self.words[SCHEDULER_ITEM_CAPTURED_ANCHOR].get(),
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
    link_state: ControllerSramLinkAddress,
    scheduler_context: ControllerSramLinkAddress,
    items: [ControllerSramLinkAddress; ITEMS],
    tx_sentinel: ControllerSramLinkAddress,
    tx_successor: ControllerSramLinkAddress,
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
    const KIND: SchedulerRoleKind = SchedulerRoleKind::PeripheralConnection;
    const ITEMS: usize = ITEMS;
    const NUMBERS: usize = 1;
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

    fn bind(base: u32, number: u16) -> Result<PeripheralConnectionBinding, SchedulerPoolBindError> {
        let link = |offset: usize| {
            ControllerSramLinkAddress::new(base + offset as u32)
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)
        };
        let items = core::mem::offset_of!(Self, items);
        let item = core::mem::size_of::<ItemStorage>();
        let rx =
            LeRxRing::bind(base + core::mem::offset_of!(Self, rx) as u32).map_err(|error| {
                match error {
                    LeRxChainBindError::InvalidAddress(error) => {
                        SchedulerPoolBindError::InvalidBase(error)
                    }
                    _ => SchedulerPoolBindError::ZeroCompressedLink,
                }
            })?;
        Ok(PeripheralConnectionBinding {
            link_state: link(core::mem::offset_of!(Self, link_state))?,
            scheduler_context: link(core::mem::offset_of!(Self, scheduler_context))?,
            items: [link(items)?, link(items + item)?],
            tx_sentinel: link(core::mem::offset_of!(Self, tx_sentinel))?,
            tx_successor: link(core::mem::offset_of!(Self, tx_successor))?,
            tx_packet: LeTxPacketAddress::new(base + core::mem::offset_of!(Self, tx_packet) as u32)
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)?,
            rx,
            number,
        })
    }

    fn item_words(&self, item: usize) -> &[VolatileCell<u32>] {
        &self.items[item].words
    }

    fn item_link(binding: &PeripheralConnectionBinding, item: usize) -> ControllerSramLinkAddress {
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
pub type PeripheralConnectionPool<const N: usize> =
    SchedulerRolePool<PeripheralConnectionStorage, N>;

/// Why a connection operation was refused. Nothing changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeripheralConnectionError {
    Pool(SchedulerPoolError),
    /// The operation does not follow the instance's preparation state.
    State,
    /// The private receive chain refused to continue; the vendor asserts.
    Receive(crate::LeRxChainError),
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

impl<const N: usize> PeripheralConnectionPool<N> {
    /// Install the Access Address and CRCInit.
    pub fn prepare_identity(
        &mut self,
        instance: &SchedulerRoleInstance,
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
    pub fn prepare_first_event(
        &mut self,
        instance: &SchedulerRoleInstance,
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
        instance: &SchedulerRoleInstance,
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
        instance: &SchedulerRoleInstance,
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
        instance: &SchedulerRoleInstance,
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
        instance: &SchedulerRoleInstance,
    ) -> Result<PeripheralConnectionReceiveTime, PeripheralConnectionError> {
        let (graph, _, _) = self
            .shared(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        Ok(graph.link_state.receive_time())
    }

    pub fn identity(
        &self,
        instance: &SchedulerRoleInstance,
    ) -> Result<PeripheralConnectionIdentity, PeripheralConnectionError> {
        let (graph, _, _) = self
            .shared(instance)
            .map_err(PeripheralConnectionError::Pool)?;
        Ok(graph.link_state.identity())
    }

    /// Whether one new reliable packet can be queued.
    pub fn can_enqueue_transmission(&self, instance: &SchedulerRoleInstance) -> bool {
        self.shared(instance)
            .is_ok_and(|(_, _, state)| !state.tx_pending)
    }

    /// Queue one packet. `Ok(false)` keeps the pending packet and its
    /// retransmission state. The payload stays owned until the peer
    /// acknowledges it.
    pub fn enqueue_transmission(
        &mut self,
        instance: &SchedulerRoleInstance,
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
        graph.link_state.words[LINK_STATE_TX_TAIL].set(next_address.controller_address().address());
        cpu.state.tx_pending = true;
        Ok(true)
    }

    /// Reclaim an acknowledged packet while the hardware keeps the completed
    /// header as its cursor. A scheduler completion alone does not complete
    /// a queued transmission.
    pub fn reclaim_transmission(
        &mut self,
        instance: &SchedulerRoleInstance,
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
        cpu.graph.link_state.words[LINK_STATE_TX_HEAD].set(address.controller_address().address());
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

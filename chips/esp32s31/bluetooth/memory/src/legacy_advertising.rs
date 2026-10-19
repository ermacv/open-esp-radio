//! Non-connectable legacy advertising instances.
//!
//! One instance holds the advertising link state, the scheduler context, one
//! scheduler item per primary channel and the TX header and packet. A
//! prepared PDU and the restricted reset persist across events; each event
//! lowers one item per selected channel, and the scheduler executor inserts
//! the items one by one. After every item returns, the event is finished and
//! the items regain their allocation-time image.

#![forbid(unsafe_code)]

use vcell::VolatileCell;

use crate::{
    coexistence::{AdvertisingCoexistencePriorities, LANES_MASK, lanes_image},
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, LeTxBufferHeaderStorage, LeTxPacketAddress,
        LeTxPacketPrepareError, LeTxPacketPreparedLength,
    },
    legacy_advertising_event_image::{
        LegacyAdvertisingItemPosition, LegacyAdvertisingLinkStateWords,
        LegacyAdvertisingOwnAddress, LegacyAdvertisingPduError,
        LegacyAdvertisingPrimaryChannelPlan, LegacyAdvertisingSchedulerItemWords,
    },
    legacy_advertising_tx_packet::LegacyAdvertisingTxPacketStorage,
    scheduler_context::SchedulerContextStorage,
    scheduler_item::SchedulerItemHeader,
    scheduler_pool::{
        SchedulerPoolBindError, SchedulerPoolError, SchedulerRoleInstance, SchedulerRoleKind,
        SchedulerRolePool, SchedulerRoleStorage, sealed,
    },
    sram_link::ControllerSramLinkAddress,
};

/// Bytes reserved for the advertising link-state object.
pub const BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES: usize = 0x84;
/// Bytes reserved for one advertising scheduler item.
pub const BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Maximum primary-channel items in one legacy advertising event.
pub const BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY: usize = 3;
/// Maximum payload after the two-byte legacy advertising PDU header.
pub const BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES: usize = 37;
/// Complete controller TX allocation for the maximum legacy advertising PDU.
pub const BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES;

const ITEMS: usize = BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY;
const LINK_STATE_WORDS: usize = BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES / 4;
const SCHEDULER_ITEM_WORDS: usize = BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES / 4;

const LINK_STATE_SCHEDULER_HEAD_OFFSET: usize = 0x64 / 4;
const LINK_STATE_RX_HEAD_OFFSET: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD_OFFSET: usize = 0x6c / 4;
const LINK_STATE_RX_TAIL_OFFSET: usize = 0x70 / 4;
const LINK_STATE_TX_TAIL_OFFSET: usize = 0x74 / 4;
const LINK_STATE_RX_SWAP_RESERVE_OFFSET: usize = 0x78 / 4;
const LINK_STATE_ALLOCATION_CONFIG_OFFSET: usize = 0x30 / 4;
const LINK_STATE_ALLOCATION_CONFIG_IMAGE: u32 = 0x0000_1e00;

const LINK_STATE_WORD_00_OFFSET: usize = 0;
const LINK_STATE_WORD_04_OFFSET: usize = 1;
const LINK_STATE_WORD_08_OFFSET: usize = 2;
const LINK_STATE_WORD_0C_OFFSET: usize = 3;
const LINK_STATE_WORD_14_OFFSET: usize = 0x14 / 4;
const LINK_STATE_WORD_18_OFFSET: usize = 0x18 / 4;
const LINK_STATE_WORD_24_OFFSET: usize = 0x24 / 4;
const LINK_STATE_WORD_2C_OFFSET: usize = 0x2c / 4;
const LINK_STATE_WORD_30_OFFSET: usize = 0x30 / 4;
const LINK_STATE_WORD_34_OFFSET: usize = 0x34 / 4;
const LINK_STATE_WORD_38_OFFSET: usize = 0x38 / 4;
const LINK_STATE_WORD_3C_OFFSET: usize = 0x3c / 4;
const LINK_STATE_WORD_40_OFFSET: usize = 0x40 / 4;
const LINK_STATE_WORD_50_OFFSET: usize = 0x50 / 4;
const LINK_STATE_WORD_60_OFFSET: usize = 0x60 / 4;

const SCHEDULER_ITEM_CONTEXT_OFFSET: usize = 1;
const SCHEDULER_ITEM_LINK_STATE_OFFSET: usize = 0x08 / 4;
const SCHEDULER_ITEM_ALLOCATION_NUMBER_OFFSET: usize = 0x20 / 4;
const SCHEDULER_ITEM_COEX_PRIORITIES_OFFSET: usize = 0x24 / 4;
const SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE: u32 = 0x0010_0000;
const SCHEDULER_ITEM_LINK_STATE_PREFIX_IMAGE: u32 = 0x0060_0000;
const SCHEDULER_ITEM_WORD_14_OFFSET: usize = 0x14 / 4;
const SCHEDULER_ITEM_WORD_18_OFFSET: usize = 0x18 / 4;

type AdvertisingTxPacketAddress = LeTxPacketAddress<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>;
type AdvertisingTxPacketLength =
    LeTxPacketPreparedLength<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>;

/// Opaque advertising link-state allocation.
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

    fn initialize(
        &self,
        first_item: ControllerSramLinkAddress,
        tx_header: ControllerSramLinkAddress,
    ) {
        for word in &self.words {
            word.set(0);
        }
        self.words[LINK_STATE_SCHEDULER_HEAD_OFFSET].set(first_item.controller_address().address());
        self.words[LINK_STATE_RX_HEAD_OFFSET].set(0);
        self.words[LINK_STATE_TX_HEAD_OFFSET].set(tx_header.controller_address().address());
        self.words[LINK_STATE_RX_TAIL_OFFSET].set(0);
        self.words[LINK_STATE_TX_TAIL_OFFSET].set(tx_header.controller_address().address());
        self.words[LINK_STATE_RX_SWAP_RESERVE_OFFSET].set(0);
        self.words[LINK_STATE_ALLOCATION_CONFIG_OFFSET].set(LINK_STATE_ALLOCATION_CONFIG_IMAGE);
    }

    fn reviewed_words(&self) -> LegacyAdvertisingLinkStateWords {
        LegacyAdvertisingLinkStateWords {
            word_00: self.words[LINK_STATE_WORD_00_OFFSET].get(),
            word_04: self.words[LINK_STATE_WORD_04_OFFSET].get(),
            word_08: self.words[LINK_STATE_WORD_08_OFFSET].get(),
            word_0c: self.words[LINK_STATE_WORD_0C_OFFSET].get(),
            word_14: self.words[LINK_STATE_WORD_14_OFFSET].get(),
            word_18: self.words[LINK_STATE_WORD_18_OFFSET].get(),
            word_24: self.words[LINK_STATE_WORD_24_OFFSET].get(),
            crc_init_word_2c: self.words[LINK_STATE_WORD_2C_OFFSET].get(),
            word_30: self.words[LINK_STATE_WORD_30_OFFSET].get(),
            word_34: self.words[LINK_STATE_WORD_34_OFFSET].get(),
            access_address_word_38: self.words[LINK_STATE_WORD_38_OFFSET].get(),
            word_3c: self.words[LINK_STATE_WORD_3C_OFFSET].get(),
            word_40: self.words[LINK_STATE_WORD_40_OFFSET].get(),
            word_50: self.words[LINK_STATE_WORD_50_OFFSET].get(),
            word_60: self.words[LINK_STATE_WORD_60_OFFSET].get(),
        }
    }

    fn write_reviewed_words(&self, words: LegacyAdvertisingLinkStateWords) {
        self.words[LINK_STATE_WORD_00_OFFSET].set(words.word_00);
        self.words[LINK_STATE_WORD_04_OFFSET].set(words.word_04);
        self.words[LINK_STATE_WORD_08_OFFSET].set(words.word_08);
        self.words[LINK_STATE_WORD_0C_OFFSET].set(words.word_0c);
        self.words[LINK_STATE_WORD_14_OFFSET].set(words.word_14);
        self.words[LINK_STATE_WORD_18_OFFSET].set(words.word_18);
        self.words[LINK_STATE_WORD_24_OFFSET].set(words.word_24);
        self.words[LINK_STATE_WORD_2C_OFFSET].set(words.crc_init_word_2c);
        self.words[LINK_STATE_WORD_30_OFFSET].set(words.word_30);
        self.words[LINK_STATE_WORD_34_OFFSET].set(words.word_34);
        self.words[LINK_STATE_WORD_38_OFFSET].set(words.access_address_word_38);
        self.words[LINK_STATE_WORD_3C_OFFSET].set(words.word_3c);
        self.words[LINK_STATE_WORD_40_OFFSET].set(words.word_40);
        self.words[LINK_STATE_WORD_50_OFFSET].set(words.word_50);
        self.words[LINK_STATE_WORD_60_OFFSET].set(words.word_60);
    }

    fn scheduler_head(&self) -> u32 {
        self.words[LINK_STATE_SCHEDULER_HEAD_OFFSET].get()
    }

    fn set_scheduler_head(&self, first_item: Option<ControllerSramLinkAddress>) {
        self.words[LINK_STATE_SCHEDULER_HEAD_OFFSET]
            .set(first_item.map_or(0, |item| item.controller_address().address()));
    }
}

/// One advertising scheduler-item allocation.
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

    /// The vendor allocator's image: allocation prefix, context and
    /// link-state links and the advertising instance number.
    fn initialize(
        &self,
        scheduler_context: ControllerSramLinkAddress,
        link_state: ControllerSramLinkAddress,
        number: u16,
    ) {
        for word in &self.words {
            word.set(0);
        }
        self.header()
            .set_hardware_next_word(SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE);
        self.words[SCHEDULER_ITEM_CONTEXT_OFFSET].set(scheduler_context.compressed_image());
        self.words[SCHEDULER_ITEM_LINK_STATE_OFFSET]
            .set(SCHEDULER_ITEM_LINK_STATE_PREFIX_IMAGE | link_state.compressed_image());
        self.words[SCHEDULER_ITEM_ALLOCATION_NUMBER_OFFSET].set(u32::from(number));
    }

    fn reviewed_words(&self) -> LegacyAdvertisingSchedulerItemWords {
        LegacyAdvertisingSchedulerItemWords {
            word_00: self.header().hardware_next_word(),
            word_04: self.words[SCHEDULER_ITEM_CONTEXT_OFFSET].get(),
            word_14: self.words[SCHEDULER_ITEM_WORD_14_OFFSET].get(),
            word_18: self.words[SCHEDULER_ITEM_WORD_18_OFFSET].get(),
            word_38: self.header().status(),
            raw_start_word_44: self.header().raw_start(),
            raw_end_word_48: self.header().raw_end(),
            word_4c: self.header().control(),
        }
    }

    fn write_reviewed_words(&self, words: LegacyAdvertisingSchedulerItemWords) {
        self.header().set_hardware_next_word(words.word_00);
        self.words[SCHEDULER_ITEM_CONTEXT_OFFSET].set(words.word_04);
        self.words[SCHEDULER_ITEM_WORD_14_OFFSET].set(words.word_14);
        self.words[SCHEDULER_ITEM_WORD_18_OFFSET].set(words.word_18);
        self.header().set_status(words.word_38);
        self.header().set_raw_start(words.raw_start_word_44);
        self.header().set_raw_end(words.raw_end_word_48);
        self.header().set_control(words.word_4c);
    }
}

/// Controller-SRAM graph of one legacy advertising instance.
#[repr(C)]
pub struct LegacyAdvertisingStorage {
    link_state: LinkStateStorage,
    scheduler_context: SchedulerContextStorage,
    items: [ItemStorage; ITEMS],
    tx_header: LeTxBufferHeaderStorage,
    tx_packet: LegacyAdvertisingTxPacketStorage<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>,
}

/// Addresses of one instance's graph.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct LegacyAdvertisingBinding {
    link_state: ControllerSramLinkAddress,
    scheduler_context: ControllerSramLinkAddress,
    items: [ControllerSramLinkAddress; ITEMS],
    tx_header: ControllerSramLinkAddress,
    tx_packet: AdvertisingTxPacketAddress,
    number: u16,
}

/// Preparation state of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum LegacyAdvertisingState {
    Empty,
    Packet(AdvertisingTxPacketLength),
    Reset(AdvertisingTxPacketLength),
    Event {
        length: AdvertisingTxPacketLength,
        items: u8,
    },
}

impl sealed::Sealed for LegacyAdvertisingStorage {}

impl SchedulerRoleStorage for LegacyAdvertisingStorage {
    const KIND: SchedulerRoleKind = SchedulerRoleKind::LegacyAdvertising;
    const ITEMS: usize = ITEMS;
    const NUMBERS: usize = 1;
    const NEW: Self = Self {
        link_state: LinkStateStorage::new(),
        scheduler_context: SchedulerContextStorage::new(),
        items: [const { ItemStorage::new() }; ITEMS],
        tx_header: LeTxBufferHeaderStorage::new(),
        tx_packet: LegacyAdvertisingTxPacketStorage::new(),
    };
    type Binding = LegacyAdvertisingBinding;
    type State = LegacyAdvertisingState;
    const INITIAL_STATE: LegacyAdvertisingState = LegacyAdvertisingState::Empty;

    fn bind(base: u32, number: u16) -> Result<LegacyAdvertisingBinding, SchedulerPoolBindError> {
        let link = |offset: usize| {
            ControllerSramLinkAddress::new(base + offset as u32)
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)
        };
        let items = core::mem::offset_of!(Self, items);
        let item = core::mem::size_of::<ItemStorage>();
        Ok(LegacyAdvertisingBinding {
            link_state: link(core::mem::offset_of!(Self, link_state))?,
            scheduler_context: link(core::mem::offset_of!(Self, scheduler_context))?,
            items: [link(items)?, link(items + item)?, link(items + 2 * item)?],
            tx_header: link(core::mem::offset_of!(Self, tx_header))?,
            tx_packet: AdvertisingTxPacketAddress::new(
                base + core::mem::offset_of!(Self, tx_packet) as u32,
            )
            .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)?,
            number,
        })
    }

    fn item_words(&self, item: usize) -> &[VolatileCell<u32>] {
        &self.items[item].words
    }

    fn item_link(binding: &LegacyAdvertisingBinding, item: usize) -> ControllerSramLinkAddress {
        binding.items[item]
    }

    fn reinitialize(&mut self, binding: &LegacyAdvertisingBinding) -> Self::State {
        self.scheduler_context.clear();
        self.link_state
            .initialize(binding.items[0], binding.tx_header);
        self.initialize_items(binding);
        self.tx_header.initialize_bound_tx(binding.tx_packet);
        self.tx_packet.clear();
        LegacyAdvertisingState::Empty
    }

    fn admits(state: &LegacyAdvertisingState, item: usize) -> bool {
        matches!(state, LegacyAdvertisingState::Event { items, .. } if item < usize::from(*items))
    }
}

impl LegacyAdvertisingStorage {
    fn initialize_items(&self, binding: &LegacyAdvertisingBinding) {
        for item in &self.items {
            item.initialize(
                binding.scheduler_context,
                binding.link_state,
                binding.number,
            );
        }
    }
}

/// Pool of legacy advertising instances.
pub type LegacyAdvertisingPool<const N: usize> = SchedulerRolePool<LegacyAdvertisingStorage, N>;

/// Why an advertising operation was refused. Nothing changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyAdvertisingError {
    Pool(SchedulerPoolError),
    /// The operation does not follow the instance's preparation state.
    State,
    Packet(LeTxPacketPrepareError),
    Pdu(LegacyAdvertisingPduError),
    /// The link state no longer names the first item as its free head.
    SchedulerHeadMismatch,
}

/// Windows of the items of one prepared event, in channel order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyAdvertisingEvent {
    windows: [(u32, u32); ITEMS],
    items: u8,
}

impl LegacyAdvertisingEvent {
    pub(crate) const fn empty(items: u8) -> Self {
        Self {
            windows: [(0, 0); ITEMS],
            items,
        }
    }

    pub(crate) const fn set_window(&mut self, item: usize, start: u32, end: u32) {
        self.windows[item] = (start, end);
    }

    /// Item index and raw window of every item of the event.
    pub fn items(&self) -> impl Iterator<Item = (usize, u32, u32)> + '_ {
        self.windows[..usize::from(self.items)]
            .iter()
            .enumerate()
            .map(|(item, (start, end))| (item, *start, *end))
    }

    pub const fn item_count(&self) -> usize {
        self.items as usize
    }
}

impl<const N: usize> LegacyAdvertisingPool<N> {
    /// Install one complete legacy advertising PDU.
    pub fn prepare_packet(
        &mut self,
        instance: &SchedulerRoleInstance,
        pdu: &[u8],
    ) -> Result<(), LegacyAdvertisingError> {
        let cpu = self.cpu(instance).map_err(LegacyAdvertisingError::Pool)?;
        if !matches!(cpu.state, LegacyAdvertisingState::Empty) {
            return Err(LegacyAdvertisingError::State);
        }
        let length = cpu
            .graph
            .tx_packet
            .prepare_encoded_pdu(pdu)
            .map_err(LegacyAdvertisingError::Packet)?;
        *cpu.state = LegacyAdvertisingState::Packet(length);
        Ok(())
    }

    /// Apply the restricted reset for the prepared PDU.
    pub fn reset_link_state(
        &mut self,
        instance: &SchedulerRoleInstance,
        default_tx_power: crate::LeTxPower,
    ) -> Result<(), LegacyAdvertisingError> {
        let cpu = self.cpu(instance).map_err(LegacyAdvertisingError::Pool)?;
        let LegacyAdvertisingState::Packet(length) = *cpu.state else {
            return Err(LegacyAdvertisingError::State);
        };
        let own_address =
            LegacyAdvertisingOwnAddress::from_pdu(cpu.graph.tx_packet.prepared_pdu(length))
                .map_err(LegacyAdvertisingError::Pdu)?;
        cpu.graph.tx_packet.lower_advertiser_address(length);
        let words = cpu.graph.link_state.reviewed_words().reset(
            cpu.binding.tx_header,
            own_address,
            default_tx_power,
        );
        cpu.graph.link_state.write_reviewed_words(words);
        *cpu.state = LegacyAdvertisingState::Reset(length);
        Ok(())
    }

    /// Lower one event into one item per selected channel, each lasting
    /// `raw_item_duration` from `raw_start` on.
    pub fn prepare_event(
        &mut self,
        instance: &SchedulerRoleInstance,
        channels: LegacyAdvertisingPrimaryChannelPlan,
        raw_start: u32,
        raw_item_duration: u32,
        coexistence: AdvertisingCoexistencePriorities,
    ) -> Result<LegacyAdvertisingEvent, LegacyAdvertisingError> {
        let cpu = self.cpu(instance).map_err(LegacyAdvertisingError::Pool)?;
        let LegacyAdvertisingState::Reset(length) = *cpu.state else {
            return Err(LegacyAdvertisingError::State);
        };
        if cpu.graph.link_state.scheduler_head()
            != cpu.binding.items[0].controller_address().address()
        {
            return Err(LegacyAdvertisingError::SchedulerHeadMismatch);
        }
        let mut event = LegacyAdvertisingEvent {
            windows: [(0, 0); ITEMS],
            items: channels.channel_count() as u8,
        };
        let link_state = cpu.graph.link_state.reviewed_words();
        for index in 0..channels.channel_count() {
            let start = raw_start.wrapping_add(raw_item_duration.wrapping_mul(index as u32));
            let end = start.wrapping_add(raw_item_duration);
            let channel = channels
                .channel(index)
                .expect("a validated channel plan contains every active position");
            let item = &cpu.graph.items[index];
            item.write_reviewed_words(item.reviewed_words().prepare_event_item(
                link_state,
                channel,
                LegacyAdvertisingItemPosition::of(index),
                start,
                end,
            ));
            item.words[SCHEDULER_ITEM_COEX_PRIORITIES_OFFSET].set(
                (item.words[SCHEDULER_ITEM_COEX_PRIORITIES_OFFSET].get() & !LANES_MASK)
                    | lanes_image(&coexistence.lanes),
            );
            event.windows[index] = (start, end);
        }
        cpu.graph.link_state.set_scheduler_head(None);
        *cpu.state = LegacyAdvertisingState::Event {
            length,
            items: event.items,
        };
        Ok(event)
    }

    /// Return the items of a finished or abandoned event to their
    /// allocation-time image. The PDU and reset stay.
    pub fn finish_event(
        &mut self,
        instance: &SchedulerRoleInstance,
    ) -> Result<(), LegacyAdvertisingError> {
        let cpu = self.cpu(instance).map_err(LegacyAdvertisingError::Pool)?;
        let LegacyAdvertisingState::Event { length, .. } = *cpu.state else {
            return Err(LegacyAdvertisingError::State);
        };
        cpu.graph.initialize_items(cpu.binding);
        cpu.graph
            .link_state
            .set_scheduler_head(Some(cpu.binding.items[0]));
        *cpu.state = LegacyAdvertisingState::Reset(length);
        Ok(())
    }

    /// Return a quiescent instance to its allocation-time image.
    pub fn clear(
        &mut self,
        instance: &SchedulerRoleInstance,
    ) -> Result<(), LegacyAdvertisingError> {
        let cpu = self.cpu(instance).map_err(LegacyAdvertisingError::Pool)?;
        *cpu.state = cpu.graph.reinitialize(cpu.binding);
        Ok(())
    }

    /// The prepared PDU as supplied.
    pub fn pdu(&self, instance: &SchedulerRoleInstance) -> Option<&[u8]> {
        let (graph, _, state) = self.shared(instance).ok()?;
        match *state {
            LegacyAdvertisingState::Empty => None,
            LegacyAdvertisingState::Packet(length)
            | LegacyAdvertisingState::Reset(length)
            | LegacyAdvertisingState::Event { length, .. } => {
                Some(graph.tx_packet.prepared_pdu(length))
            }
        }
    }
}

#[cfg(test)]
mod tests;

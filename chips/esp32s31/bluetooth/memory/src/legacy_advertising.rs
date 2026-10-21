//! Non-connectable legacy advertising instances.
//!
//! One instance holds the advertising link state, the scheduler context, one
//! scheduler item per primary channel and the TX header and packet. A
//! prepared PDU and the restricted reset persist across events; each event
//! lowers one item per selected channel, and the scheduler executor inserts
//! the items one by one. After every item returns, the event is finished and
//! the items regain their allocation-time image.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::layouts::*;

use crate::{
    coexistence::{AdvertisingCoexistencePriorities, with_priorities},
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
        SchedulerPoolError, SchedulerRoleInstance, SchedulerRoleKind, SchedulerRolePool,
        SchedulerRoleStorage, sealed,
    },
    sram_link::ControllerSramLinkAddress,
};

/// Bytes reserved for the advertising link-state object.
pub const BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES: usize = BleLinkState::SIZE;
/// Bytes reserved for one advertising scheduler item.
pub const BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES: usize = BleSchedulerItem::SIZE;
/// Maximum primary-channel items in one legacy advertising event.
pub const BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY: usize = 3;
/// Maximum payload after the two-byte legacy advertising PDU header.
pub const BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES: usize = 37;
/// Complete controller TX allocation for the maximum legacy advertising PDU.
pub const BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES;

const ITEMS: usize = BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY;
/// SOURCE: chips/esp32s31/notes/bluetooth-legacy-advertising.md, allocation/reset profile (bt-lib pin 10c50778).
const LINK_STATE_ALLOCATION_CONFIG_IMAGE: BleLinkStateSequencePolicy =
    BleLinkStateSequencePolicy::ZERO
        .with_bits_9_12_opaque(0xf)
        .expect("reviewed field image fits");
/// SOURCE: chips/esp32s31/notes/bluetooth-legacy-advertising.md, scheduler allocation (bt-lib pin 10c50778).
const SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE: BleSchedulerItemHardwareNext =
    BleSchedulerItemHardwareNext::ZERO
        .with_bits_20_21_opaque(0x1)
        .expect("reviewed field image fits");

type AdvertisingTxPacketAddress = LeTxPacketAddress<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>;
type AdvertisingTxPacketLength =
    LeTxPacketPreparedLength<BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES>;

/// Opaque advertising link-state allocation.
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

    fn initialize(
        &self,
        first_item: ControllerSramLinkAddress,
        tx_header: ControllerSramLinkAddress,
    ) {
        crate::shared_layout::clear_link_state(&self.layout);
        self.layout
            .scheduler_head()
            .write(BleLinkStateSchedulerHead::from_bits(
                first_item.controller_address().address(),
            ));
        self.layout
            .rx_head()
            .write(BleLinkStateRxHead::from_bits(0));
        self.layout.tx_head().write(BleLinkStateTxHead::from_bits(
            tx_header.controller_address().address(),
        ));
        self.layout
            .rx_tail()
            .write(BleLinkStateRxTail::from_bits(0));
        self.layout.tx_tail().write(BleLinkStateTxTail::from_bits(
            tx_header.controller_address().address(),
        ));
        self.layout
            .rx_reserve()
            .write(BleLinkStateRxReserve::from_bits(0));
        self.layout
            .sequence_policy()
            .write(LINK_STATE_ALLOCATION_CONFIG_IMAGE);
    }

    fn reviewed_words(&self) -> LegacyAdvertisingLinkStateWords {
        LegacyAdvertisingLinkStateWords {
            word_00: self.layout.tx_path().read(),
            word_04: self.layout.scan_response().read(),
            word_08: self.layout.rx_path().read(),
            word_0c: self.layout.control_policy().read(),
            word_14: self.layout.packet_policy().read(),
            word_18: self.layout.receive_context().read(),
            word_24: self.layout.own_address_policy().read(),
            crc_init_word_2c: self.layout.crc_initialization().read(),
            word_30: self.layout.sequence_policy().read(),
            word_34: self.layout.event_span().read(),
            access_address_word_38: self.layout.access_address().read(),
            word_3c: self.layout.own_address_low().read(),
            word_40: self.layout.own_address_high().read(),
            word_50: self.layout.direction_finding().read(),
            word_60: self.layout.event().read(),
        }
    }

    fn write_reviewed_words(&self, words: LegacyAdvertisingLinkStateWords) {
        self.layout.tx_path().write(words.word_00);
        self.layout.scan_response().write(words.word_04);
        self.layout.rx_path().write(words.word_08);
        self.layout.control_policy().write(words.word_0c);
        self.layout.packet_policy().write(words.word_14);
        self.layout.receive_context().write(words.word_18);
        self.layout.own_address_policy().write(words.word_24);
        self.layout
            .crc_initialization()
            .write(words.crc_init_word_2c);
        self.layout.sequence_policy().write(words.word_30);
        self.layout.event_span().write(words.word_34);
        self.layout
            .access_address()
            .write(words.access_address_word_38);
        self.layout.own_address_low().write(words.word_3c);
        self.layout.own_address_high().write(words.word_40);
        self.layout.direction_finding().write(words.word_50);
        self.layout.event().write(words.word_60);
    }

    fn scheduler_head(&self) -> u32 {
        self.layout.scheduler_head().read().bits()
    }

    fn set_scheduler_head(&self, first_item: Option<ControllerSramLinkAddress>) {
        self.layout
            .scheduler_head()
            .write(BleLinkStateSchedulerHead::from_bits(
                first_item.map_or(0, |item| item.controller_address().address()),
            ));
    }
}

/// One advertising scheduler-item allocation.
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

    /// The vendor allocator's image: allocation prefix, context and
    /// link-state links and the advertising instance number.
    fn initialize(
        &self,
        scheduler_context: ControllerSramLinkAddress,
        link_state: ControllerSramLinkAddress,
        number: u16,
    ) {
        crate::shared_layout::clear_scheduler_item(&self.layout);
        self.layout
            .hardware_next()
            .write(SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE);
        self.layout
            .context()
            .write(BleSchedulerItemContext::from_bits(
                scheduler_context.compressed_image(),
            ));
        self.layout.link_state().write(
            BleSchedulerItemLinkState::from_bits(link_state.compressed_image())
                .with_kind(6)
                .expect("reviewed role prefix fits"),
        );
        self.layout
            .allocation_number()
            .write(BleSchedulerItemAllocationNumber::from_bits(u32::from(
                number,
            )));
    }

    fn reviewed_words(&self) -> LegacyAdvertisingSchedulerItemWords {
        LegacyAdvertisingSchedulerItemWords {
            word_00: self.layout.hardware_next().read(),
            word_04: self.layout.context().read(),
            word_14: self.layout.rate_and_power().read(),
            word_18: self.layout.channel_and_priority().read(),
            word_38: self.layout.status().read(),
            raw_start_word_44: self.layout.raw_start().read(),
            raw_end_word_48: self.layout.raw_end().read(),
            word_4c: self.layout.control().read(),
        }
    }

    fn write_reviewed_words(&self, words: LegacyAdvertisingSchedulerItemWords) {
        self.header().set_hardware_next_word(words.word_00.bits());
        self.layout.context().write(words.word_04);
        self.layout.rate_and_power().write(words.word_14);
        self.layout.channel_and_priority().write(words.word_18);
        self.header().set_status(words.word_38.bits());
        self.header().set_raw_start(words.raw_start_word_44.bits());
        self.header().set_raw_end(words.raw_end_word_48.bits());
        self.header().set_control(words.word_4c.bits());
    }
}

/// Controller-SRAM graph of one legacy advertising instance.
#[repr(C)]
#[derive(bytemuck::Zeroable)]
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
    const NUMBERS: usize = 1;
    const KIND: SchedulerRoleKind = SchedulerRoleKind::LegacyAdvertising;
    const ITEMS: usize = ITEMS;
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

    fn addresses<D: crate::ControllerDmaSpace>(
        &self,
        placement: &crate::Placement<D>,
        number: u16,
    ) -> LegacyAdvertisingBinding {
        LegacyAdvertisingBinding {
            link_state: ControllerSramLinkAddress::from_placed(placement.address(&self.link_state)),
            scheduler_context: ControllerSramLinkAddress::from_placed(
                placement.address(&self.scheduler_context),
            ),
            tx_header: ControllerSramLinkAddress::from_placed(placement.address(&self.tx_header)),
            items: core::array::from_fn(|index| {
                ControllerSramLinkAddress::from_placed(placement.address(&self.items[index]))
            }),
            tx_packet: AdvertisingTxPacketAddress::from_placed(placement, &self.tx_packet),
            number,
        }
    }

    fn item_layout(&self, item: usize) -> &BleSchedulerItem {
        &self.items[item].layout
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
            item.layout.coexistence_priority().write(with_priorities(
                item.layout.coexistence_priority().read(),
                &coexistence.lanes,
            ));
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

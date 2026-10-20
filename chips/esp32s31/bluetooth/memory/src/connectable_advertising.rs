//! Response-capable legacy advertising instances.
//!
//! One instance holds the advertising link state, the scheduler context, one
//! scheduler item per primary channel and two chained TX packets: `ADV_IND`
//! and its `SCAN_RSP`. Requests and connection indications arrive through the
//! global non-scanning receive chain under the instance's advertising number.
//! The PDUs and reset persist across events; each event lowers one item per
//! selected channel, the scheduler executor inserts them one by one, and
//! finishing the event restores every item.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::layouts::*;

use crate::{
    coexistence::{AdvertisingCoexistencePriorities, with_priorities},
    le_rx_chain::{LeRxChain, LeRxSource, LeRxTag},
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, LeTxBufferHeaderStorage, LeTxPacketAddress,
        LeTxPacketPreparedInput, LeTxPacketPreparedLength,
    },
    legacy_advertising::{
        BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY, LegacyAdvertisingEvent,
    },
    legacy_advertising_event_image::{
        LegacyAdvertisingItemPosition, LegacyAdvertisingLinkStateWords,
        LegacyAdvertisingOwnAddress, LegacyAdvertisingPrimaryChannelPlan,
        LegacyAdvertisingSchedulerItemWords,
    },
    legacy_advertising_tx_packet::LegacyAdvertisingTxPacketStorage,
    rx_memory_list::RxMemoryListClass,
    scheduler_context::SchedulerContextStorage,
    scheduler_item::SchedulerItemHeader,
    scheduler_pool::{
        SchedulerPoolBindError, SchedulerPoolError, SchedulerRoleInstance, SchedulerRoleKind,
        SchedulerRolePool, SchedulerRoleStorage, sealed,
    },
    sram_link::ControllerSramLinkAddress,
};

const LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES: usize = 37;
const LEGACY_ADVERTISING_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES;

type AdvertisingTxPacketAddress = LeTxPacketAddress<LEGACY_ADVERTISING_TX_PACKET_BYTES>;
type AdvertisingTxPacketLength = LeTxPacketPreparedLength<LEGACY_ADVERTISING_TX_PACKET_BYTES>;
type AdvertisingTxPacketInput<'a> = LeTxPacketPreparedInput<'a, LEGACY_ADVERTISING_TX_PACKET_BYTES>;

const ITEMS: usize = BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY;
/// SOURCE: pinned role allocation/reset body in the BLE controller-memory reference.
const LINK_STATE_ALLOCATION_CONFIG_IMAGE: BleLinkStateSequencePolicy =
    BleLinkStateSequencePolicy::ZERO
        .with_bits_9_12_opaque(0xf)
        .expect("reviewed field image fits");
// Common scheduler allocation installs both bits before the advertising role.
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0030_0000;
// Complete common allocator applied to the standalone module default.
/// SOURCE: pinned role allocation/reset body in the BLE controller-memory reference.
const SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE: BleSchedulerItemWord001cOpaque =
    BleSchedulerItemWord001cOpaque::ZERO.with_bits_0_31_opaque(0xffdf7fff);

/// Why an encoded advertising PDU cannot fit this S31 allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingPduFitError {
    /// Address insertion requires the complete six-byte advertiser address.
    AdvertiserAddressMissing { payload_bytes: usize },
    /// The complete encoded extent disagrees with the trusted payload length.
    EncodedExtentMismatch {
        expected_bytes: usize,
        actual_bytes: usize,
    },
    /// The payload exceeds the reviewed legacy-advertising allocation class.
    PayloadExceedsAllocation {
        payload_bytes: usize,
        capacity: usize,
    },
}

fn allocation_checked_packet(
    pdu: &[u8],
    payload_bytes: u8,
) -> Result<AdvertisingTxPacketInput<'_>, LegacyConnectableAdvertisingPduFitError> {
    let payload_bytes = usize::from(payload_bytes);
    if payload_bytes > LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES {
        return Err(
            LegacyConnectableAdvertisingPduFitError::PayloadExceedsAllocation {
                payload_bytes,
                capacity: LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES,
            },
        );
    }
    let expected_bytes = 2 + payload_bytes;
    if pdu.len() != expected_bytes {
        return Err(
            LegacyConnectableAdvertisingPduFitError::EncodedExtentMismatch {
                expected_bytes,
                actual_bytes: pdu.len(),
            },
        );
    }
    if payload_bytes < 6 {
        return Err(
            LegacyConnectableAdvertisingPduFitError::AdvertiserAddressMissing { payload_bytes },
        );
    }
    Ok(AdvertisingTxPacketInput::from_validated_encoded_pdu(
        pdu,
        payload_bytes as u8,
    ))
}

/// Allocation-fit projection of one protocol-validated `ADV_IND` PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvIndPacketInput<'a>(AdvertisingTxPacketInput<'a>);

impl<'a> LegacyConnectableAdvIndPacketInput<'a> {
    /// Check only S31 allocation fit; the caller owns protocol validity.
    pub fn try_from_encoded_extent(
        pdu: &'a [u8],
        payload_bytes: u8,
    ) -> Result<Self, LegacyConnectableAdvertisingPduFitError> {
        allocation_checked_packet(pdu, payload_bytes).map(Self)
    }
}

/// Allocation-fit projection of one protocol-validated `SCAN_RSP` PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableScanResponsePacketInput<'a>(AdvertisingTxPacketInput<'a>);

impl<'a> LegacyConnectableScanResponsePacketInput<'a> {
    /// Check only S31 allocation fit; the caller owns protocol validity.
    pub fn try_from_encoded_extent(
        pdu: &'a [u8],
        payload_bytes: u8,
    ) -> Result<Self, LegacyConnectableAdvertisingPduFitError> {
        allocation_checked_packet(pdu, payload_bytes).map(Self)
    }
}

/// Address behavior already selected by the chip protocol bridge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingOwnAddress {
    Public,
    Random([u8; 6]),
}

impl LegacyConnectableAdvertisingOwnAddress {
    const fn codec(self) -> LegacyAdvertisingOwnAddress {
        match self {
            Self::Public => LegacyAdvertisingOwnAddress::Public,
            Self::Random(address) => LegacyAdvertisingOwnAddress::Random(address),
        }
    }
}

/// Complete allocation-fit input of one response-capable advertisement.
///
/// The PDU wrappers check only controller-allocation fit and never parse
/// Bluetooth header semantics; the chip protocol bridge retains the
/// validated Link Layer owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyConnectableAdvertisingMemoryInput<'a> {
    adv_ind: LegacyConnectableAdvIndPacketInput<'a>,
    scan_response: Option<LegacyConnectableScanResponsePacketInput<'a>>,
    own_address: LegacyConnectableAdvertisingOwnAddress,
}

impl<'a> LegacyConnectableAdvertisingMemoryInput<'a> {
    /// `scan_response` is absent for a set that only receives, such as
    /// directed advertising: its primary TX header then has no successor.
    pub const fn new(
        adv_ind: LegacyConnectableAdvIndPacketInput<'a>,
        scan_response: Option<LegacyConnectableScanResponsePacketInput<'a>>,
        own_address: LegacyConnectableAdvertisingOwnAddress,
    ) -> Self {
        Self {
            adv_ind,
            scan_response,
            own_address,
        }
    }
}

#[repr(C, align(4))]
struct LinkStateStorage {
    layout: BleLinkState,
}

impl LinkStateStorage {
    const fn new() -> Self {
        Self {
            layout: BleLinkState::zeroed(),
        }
    }

    fn initialize(&self, binding: &LegacyConnectableAdvertisingBinding) {
        crate::shared_layout::clear_link_state(&self.layout);
        self.set_scheduler_head(Some(binding.items[0]));
        self.layout.tx_head().write(BleLinkStateTxHead::from_bits(
            binding.adv_ind_header.controller_address().address(),
        ));
        self.layout.tx_tail().write(BleLinkStateTxTail::from_bits(
            binding.scan_response_header.controller_address().address(),
        ));
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

    fn set_scheduler_head(&self, item: Option<ControllerSramLinkAddress>) {
        self.layout
            .scheduler_head()
            .write(BleLinkStateSchedulerHead::from_bits(
                item.map_or(0, |item| item.controller_address().address()),
            ));
    }

    fn prepare_profile(
        &self,
        binding: &LegacyConnectableAdvertisingBinding,
        (rx_head, rx_tail, rx_spare): (u32, u32, u32),
        own_address: LegacyAdvertisingOwnAddress,
        default_tx_power: crate::LeTxPower,
        scan_response: bool,
    ) {
        let mut words =
            self.reviewed_words()
                .reset(binding.adv_ind_header, own_address, default_tx_power);
        // The common no-response projection clears this consumer. Advertising
        // reset installs the primary TX header's successor for SCAN_RSP; a
        // set without one keeps it empty.
        words.word_04 = words.word_04.with_header(None).expect("empty link fits");
        if scan_response {
            words.word_04 = BleLinkStateScanResponse::from_bits(
                binding.scan_response_header.compressed_image(),
            )
            .with_bits_20_27_opaque(words.word_04.bits_20_27_opaque())
            .with_bits_28_31_opaque(words.word_04.bits_28_31_opaque())
            .expect("retained prefix fits");
        }
        // adv_alloc_rxbuf prepares an empty private RX link before reset.
        // The later memory-manager broker selects the global RX class and
        // updates software head/tail, without installing a private consumer.
        words.word_08 = words.word_08.with_header(None).expect("empty link fits");
        self.write_reviewed_words(words);
        let class = self.layout.rx_list();
        class.write(BleLinkStateRxList::from_bits(
            RxMemoryListClass::NonScanning.select_in_link_state(class.read().bits()),
        ));
        self.layout
            .rx_head()
            .write(BleLinkStateRxHead::from_bits(rx_head));
        self.layout
            .rx_tail()
            .write(BleLinkStateRxTail::from_bits(rx_tail));
        self.layout
            .rx_reserve()
            .write(BleLinkStateRxReserve::from_bits(rx_spare));
    }
}

#[repr(C, align(4))]
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

    fn initialize(&self, binding: &LegacyConnectableAdvertisingBinding) {
        crate::shared_layout::clear_scheduler_item(&self.layout);
        self.header()
            .set_hardware_next_word(SCHEDULER_ITEM_ALLOCATION_PREFIX);
        self.layout
            .context()
            .write(BleSchedulerItemContext::from_bits(
                binding.scheduler_context.compressed_image(),
            ));
        self.layout
            .word_001c_opaque()
            .write(SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE);
        self.layout
            .allocation_number()
            .write(BleSchedulerItemAllocationNumber::from_bits(u32::from(
                binding.number,
            )));
        self.layout.link_state().write(
            BleSchedulerItemLinkState::from_bits(binding.link_state.compressed_image())
                .with_bits_20_23_opaque(6)
                .expect("reviewed role prefix fits"),
        );
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

/// Controller-SRAM graph of one response-capable advertising instance.
#[repr(C)]
pub struct LegacyConnectableAdvertisingStorage {
    link_state: LinkStateStorage,
    scheduler_context: SchedulerContextStorage,
    items: [ItemStorage; ITEMS],
    adv_ind_header: LeTxBufferHeaderStorage,
    scan_response_header: LeTxBufferHeaderStorage,
    adv_ind_packet: LegacyAdvertisingTxPacketStorage<LEGACY_ADVERTISING_TX_PACKET_BYTES>,
    scan_response_packet: LegacyAdvertisingTxPacketStorage<LEGACY_ADVERTISING_TX_PACKET_BYTES>,
}

/// Addresses and advertising number of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct LegacyConnectableAdvertisingBinding {
    link_state: ControllerSramLinkAddress,
    scheduler_context: ControllerSramLinkAddress,
    items: [ControllerSramLinkAddress; ITEMS],
    adv_ind_header: ControllerSramLinkAddress,
    scan_response_header: ControllerSramLinkAddress,
    adv_ind_packet: AdvertisingTxPacketAddress,
    scan_response_packet: AdvertisingTxPacketAddress,
    number: u16,
}

/// The prepared advertisement of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct LegacyConnectableAdvertisingPrepared {
    adv_ind: AdvertisingTxPacketLength,
    scan_response: Option<AdvertisingTxPacketLength>,
}

/// Preparation state of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum LegacyConnectableAdvertisingState {
    Empty,
    Prepared(LegacyConnectableAdvertisingPrepared),
    Event {
        prepared: LegacyConnectableAdvertisingPrepared,
        items: u8,
    },
}

impl sealed::Sealed for LegacyConnectableAdvertisingStorage {}

impl SchedulerRoleStorage for LegacyConnectableAdvertisingStorage {
    const NUMBERS: usize = 1;
    const KIND: SchedulerRoleKind = SchedulerRoleKind::ConnectableAdvertising;
    const ITEMS: usize = ITEMS;
    const NEW: Self = Self {
        link_state: LinkStateStorage::new(),
        scheduler_context: SchedulerContextStorage::new(),
        items: [const { ItemStorage::new() }; ITEMS],
        adv_ind_header: LeTxBufferHeaderStorage::new(),
        scan_response_header: LeTxBufferHeaderStorage::new(),
        adv_ind_packet: LegacyAdvertisingTxPacketStorage::new(),
        scan_response_packet: LegacyAdvertisingTxPacketStorage::new(),
    };
    type Binding = LegacyConnectableAdvertisingBinding;
    type State = LegacyConnectableAdvertisingState;
    const INITIAL_STATE: LegacyConnectableAdvertisingState =
        LegacyConnectableAdvertisingState::Empty;

    fn bind(
        base: u32,
        number: u16,
    ) -> Result<LegacyConnectableAdvertisingBinding, SchedulerPoolBindError> {
        let link = |offset: usize| {
            ControllerSramLinkAddress::new(base + offset as u32)
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)
        };
        let packet = |offset: usize| {
            AdvertisingTxPacketAddress::new(base + offset as u32)
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)
        };
        let items = core::mem::offset_of!(Self, items);
        let item = core::mem::size_of::<ItemStorage>();
        Ok(LegacyConnectableAdvertisingBinding {
            link_state: link(core::mem::offset_of!(Self, link_state))?,
            scheduler_context: link(core::mem::offset_of!(Self, scheduler_context))?,
            items: [link(items)?, link(items + item)?, link(items + 2 * item)?],
            adv_ind_header: link(core::mem::offset_of!(Self, adv_ind_header))?,
            scan_response_header: link(core::mem::offset_of!(Self, scan_response_header))?,
            adv_ind_packet: packet(core::mem::offset_of!(Self, adv_ind_packet))?,
            scan_response_packet: packet(core::mem::offset_of!(Self, scan_response_packet))?,
            number,
        })
    }

    fn item_layout(&self, item: usize) -> &BleSchedulerItem {
        &self.items[item].layout
    }

    fn item_link(
        binding: &LegacyConnectableAdvertisingBinding,
        item: usize,
    ) -> ControllerSramLinkAddress {
        binding.items[item]
    }

    fn reinitialize(&mut self, binding: &LegacyConnectableAdvertisingBinding) -> Self::State {
        self.scheduler_context.clear();
        self.initialize_items(binding);
        self.adv_ind_header.initialize_bound_tx_with_successor(
            binding.adv_ind_packet,
            Some(binding.scan_response_header),
        );
        self.scan_response_header
            .initialize_bound_tx(binding.scan_response_packet);
        self.link_state.initialize(binding);
        self.adv_ind_packet.clear();
        self.scan_response_packet.clear();
        LegacyConnectableAdvertisingState::Empty
    }

    fn admits(state: &LegacyConnectableAdvertisingState, item: usize) -> bool {
        matches!(
            state,
            LegacyConnectableAdvertisingState::Event { items, .. } if item < usize::from(*items)
        )
    }
}

impl LegacyConnectableAdvertisingStorage {
    fn initialize_items(&self, binding: &LegacyConnectableAdvertisingBinding) {
        for item in &self.items {
            item.initialize(binding);
        }
    }
}

/// Pool of response-capable advertising instances.
pub type LegacyConnectableAdvertisingPool<const N: usize> =
    SchedulerRolePool<LegacyConnectableAdvertisingStorage, N>;

/// Why a response-capable advertising operation was refused. Nothing
/// changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyConnectableAdvertisingError {
    Pool(SchedulerPoolError),
    /// The operation does not follow the instance's preparation state.
    State,
    /// The chain receives another class.
    ForeignReceiveClass,
    /// The link state no longer names the item as its free head.
    SchedulerHeadMismatch,
}

impl<const N: usize> LegacyConnectableAdvertisingPool<N> {
    /// Install both PDUs and the restricted reset, and join the non-scanning
    /// receive chain.
    pub fn prepare<const PACKETS: usize>(
        &mut self,
        instance: &SchedulerRoleInstance,
        input: LegacyConnectableAdvertisingMemoryInput<'_>,
        chain: &LeRxChain<PACKETS>,
        default_tx_power: crate::LeTxPower,
    ) -> Result<(), LegacyConnectableAdvertisingError> {
        if chain.class() != RxMemoryListClass::NonScanning {
            return Err(LegacyConnectableAdvertisingError::ForeignReceiveClass);
        }
        let cpu = self
            .cpu(instance)
            .map_err(LegacyConnectableAdvertisingError::Pool)?;
        if !matches!(cpu.state, LegacyConnectableAdvertisingState::Empty) {
            return Err(LegacyConnectableAdvertisingError::State);
        }
        let graph = &mut *cpu.graph;
        let adv_ind = graph
            .adv_ind_packet
            .prepare_validated_encoded_pdu(input.adv_ind.0);
        graph.adv_ind_packet.lower_advertiser_address(adv_ind);
        let scan_response = input.scan_response.map(|input| {
            let length = graph
                .scan_response_packet
                .prepare_validated_encoded_pdu(input.0);
            graph.scan_response_packet.lower_advertiser_address(length);
            length
        });
        if scan_response.is_none() {
            // The primary header is the whole TX chain.
            graph
                .adv_ind_header
                .initialize_bound_tx_with_successor(cpu.binding.adv_ind_packet, None);
            graph
                .link_state
                .layout
                .tx_tail()
                .write(BleLinkStateTxTail::from_bits(
                    cpu.binding.adv_ind_header.controller_address().address(),
                ));
        }
        graph.link_state.prepare_profile(
            cpu.binding,
            chain.snapshot(),
            input.own_address.codec(),
            default_tx_power,
            scan_response.is_some(),
        );
        *cpu.state =
            LegacyConnectableAdvertisingState::Prepared(LegacyConnectableAdvertisingPrepared {
                adv_ind,
                scan_response,
            });
        Ok(())
    }

    /// Lower one event into one item per selected channel, each lasting
    /// `raw_item_duration` from `raw_start` on. `raw_sequence_lead` is the
    /// scheduler's accepted preparation lead.
    pub fn prepare_event(
        &mut self,
        instance: &SchedulerRoleInstance,
        channels: LegacyAdvertisingPrimaryChannelPlan,
        raw_start: u32,
        raw_item_duration: u32,
        raw_sequence_lead: u32,
        coexistence: AdvertisingCoexistencePriorities,
    ) -> Result<LegacyAdvertisingEvent, LegacyConnectableAdvertisingError> {
        let cpu = self
            .cpu(instance)
            .map_err(LegacyConnectableAdvertisingError::Pool)?;
        let LegacyConnectableAdvertisingState::Prepared(prepared) = *cpu.state else {
            return Err(LegacyConnectableAdvertisingError::State);
        };
        let graph = &cpu.graph;
        if graph.link_state.scheduler_head() != cpu.binding.items[0].controller_address().address()
        {
            return Err(LegacyConnectableAdvertisingError::SchedulerHeadMismatch);
        }
        let mut event = LegacyAdvertisingEvent::empty(channels.channel_count() as u8);
        let link_state = graph.link_state.reviewed_words();
        for index in 0..channels.channel_count() {
            let start = raw_start.wrapping_add(raw_item_duration.wrapping_mul(index as u32));
            let end = start.wrapping_add(raw_item_duration);
            let channel = channels
                .channel(index)
                .expect("a validated channel plan contains every active position");
            let item = &graph.items[index];
            item.write_reviewed_words(item.reviewed_words().prepare_event_item(
                link_state,
                channel,
                LegacyAdvertisingItemPosition::of(index),
                start,
                end,
            ));
            // Common r_btdm_sched_calc_seq_time projection.
            item.header().set_sequence(start, end, raw_sequence_lead);
            item.layout.coexistence_priority().write(with_priorities(
                item.layout.coexistence_priority().read(),
                &coexistence.lanes,
            ));
            event.set_window(index, start, end);
        }
        graph.link_state.set_scheduler_head(None);
        *cpu.state = LegacyConnectableAdvertisingState::Event {
            prepared,
            items: event.item_count() as u8,
        };
        Ok(event)
    }

    /// Restore the items after a finished or abandoned event.
    pub fn finish_event(
        &mut self,
        instance: &SchedulerRoleInstance,
    ) -> Result<(), LegacyConnectableAdvertisingError> {
        let cpu = self
            .cpu(instance)
            .map_err(LegacyConnectableAdvertisingError::Pool)?;
        let LegacyConnectableAdvertisingState::Event { prepared, .. } = *cpu.state else {
            return Err(LegacyConnectableAdvertisingError::State);
        };
        cpu.graph.initialize_items(cpu.binding);
        cpu.graph
            .link_state
            .set_scheduler_head(Some(cpu.binding.items[0]));
        *cpu.state = LegacyConnectableAdvertisingState::Prepared(prepared);
        Ok(())
    }

    /// Return a quiescent instance to its allocation-time image.
    pub fn clear(
        &mut self,
        instance: &SchedulerRoleInstance,
    ) -> Result<(), LegacyConnectableAdvertisingError> {
        let cpu = self
            .cpu(instance)
            .map_err(LegacyConnectableAdvertisingError::Pool)?;
        *cpu.state = cpu.graph.reinitialize(cpu.binding);
        Ok(())
    }

    fn prepared(
        &self,
        instance: &SchedulerRoleInstance,
    ) -> Option<(
        &LegacyConnectableAdvertisingStorage,
        LegacyConnectableAdvertisingPrepared,
    )> {
        let (graph, _, state) = self.shared(instance).ok()?;
        match *state {
            LegacyConnectableAdvertisingState::Empty => None,
            LegacyConnectableAdvertisingState::Prepared(prepared)
            | LegacyConnectableAdvertisingState::Event { prepared, .. } => Some((graph, prepared)),
        }
    }

    pub fn adv_ind_pdu(&self, instance: &SchedulerRoleInstance) -> Option<&[u8]> {
        self.prepared(instance)
            .map(|(graph, prepared)| graph.adv_ind_packet.prepared_pdu(prepared.adv_ind))
    }

    pub fn scan_response_pdu(&self, instance: &SchedulerRoleInstance) -> Option<&[u8]> {
        let (graph, prepared) = self.prepared(instance)?;
        prepared
            .scan_response
            .map(|length| graph.scan_response_packet.prepared_pdu(length))
    }

    /// Packets that the instance receives.
    pub fn receive_source(
        &self,
        instance: &SchedulerRoleInstance,
    ) -> Result<LeRxSource, LegacyConnectableAdvertisingError> {
        let (_, binding, _) = self
            .shared(instance)
            .map_err(LegacyConnectableAdvertisingError::Pool)?;
        let tag = LeRxTag::new(binding.number).expect("the allocation numbers fit twelve bits");
        Ok(LeRxSource::new(tag, RxMemoryListClass::NonScanning, false))
    }
}

#[cfg(test)]
mod tests;

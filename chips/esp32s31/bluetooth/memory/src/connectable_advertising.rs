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

use vcell::VolatileCell;

use crate::{
    coexistence::{AdvertisingCoexistencePriorities, LANES_MASK, lanes_image},
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
const LINK_STATE_WORDS: usize = 0x84 / 4;
const SCHEDULER_ITEM_WORDS: usize = 0x60 / 4;

const LINK_STATE_SCHEDULER_HEAD: usize = 0x64 / 4;
const LINK_STATE_RX_HEAD: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD: usize = 0x6c / 4;
const LINK_STATE_RX_TAIL: usize = 0x70 / 4;
const LINK_STATE_TX_TAIL: usize = 0x74 / 4;
const LINK_STATE_RX_SWAP_RESERVE: usize = 0x78 / 4;
const LINK_STATE_ALLOCATION_CONFIG: usize = 0x30 / 4;
const LINK_STATE_ALLOCATION_CONFIG_IMAGE: u32 = 0x0000_1e00;
const LINK_STATE_RX_LIST_CLASS: usize = 0x20 / 4;
const COMPRESSED_LINK_MASK: u32 = 0x000f_ffff;

const LINK_STATE_WORD_00: usize = 0;
const LINK_STATE_WORD_04: usize = 1;
const LINK_STATE_WORD_08: usize = 2;
const LINK_STATE_WORD_0C: usize = 3;
const LINK_STATE_WORD_14: usize = 0x14 / 4;
const LINK_STATE_WORD_18: usize = 0x18 / 4;
const LINK_STATE_WORD_24: usize = 0x24 / 4;
const LINK_STATE_WORD_2C: usize = 0x2c / 4;
const LINK_STATE_WORD_30: usize = 0x30 / 4;
const LINK_STATE_WORD_34: usize = 0x34 / 4;
const LINK_STATE_WORD_38: usize = 0x38 / 4;
const LINK_STATE_WORD_3C: usize = 0x3c / 4;
const LINK_STATE_WORD_40: usize = 0x40 / 4;
const LINK_STATE_WORD_50: usize = 0x50 / 4;
const LINK_STATE_WORD_60: usize = 0x60 / 4;

const SCHEDULER_ITEM_CONTEXT: usize = 1;
const SCHEDULER_ITEM_LINK_STATE: usize = 0x08 / 4;
const SCHEDULER_ITEM_WORD_14: usize = 0x14 / 4;
const SCHEDULER_ITEM_WORD_18: usize = 0x18 / 4;
const SCHEDULER_ITEM_ALLOCATION_FLAGS: usize = 0x1c / 4;
const SCHEDULER_ITEM_ALLOCATION_NUMBER: usize = 0x20 / 4;
const SCHEDULER_ITEM_COEX_PRIORITIES: usize = 0x24 / 4;
// Common scheduler allocation installs both bits before the advertising role.
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0030_0000;
const SCHEDULER_ITEM_LINK_STATE_PREFIX: u32 = 0x0060_0000;
// Complete common allocator applied to the standalone module default.
const SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE: u32 = 0xffdf_7fff;

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
    words: [VolatileCell<u32>; LINK_STATE_WORDS],
}

impl LinkStateStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; LINK_STATE_WORDS],
        }
    }

    fn initialize(&self, binding: &LegacyConnectableAdvertisingBinding) {
        for word in &self.words {
            word.set(0);
        }
        self.set_scheduler_head(Some(binding.items[0]));
        self.words[LINK_STATE_TX_HEAD].set(binding.adv_ind_header.controller_address().address());
        self.words[LINK_STATE_TX_TAIL]
            .set(binding.scan_response_header.controller_address().address());
        self.words[LINK_STATE_ALLOCATION_CONFIG].set(LINK_STATE_ALLOCATION_CONFIG_IMAGE);
    }

    fn reviewed_words(&self) -> LegacyAdvertisingLinkStateWords {
        LegacyAdvertisingLinkStateWords {
            word_00: self.words[LINK_STATE_WORD_00].get(),
            word_04: self.words[LINK_STATE_WORD_04].get(),
            word_08: self.words[LINK_STATE_WORD_08].get(),
            word_0c: self.words[LINK_STATE_WORD_0C].get(),
            word_14: self.words[LINK_STATE_WORD_14].get(),
            word_18: self.words[LINK_STATE_WORD_18].get(),
            word_24: self.words[LINK_STATE_WORD_24].get(),
            crc_init_word_2c: self.words[LINK_STATE_WORD_2C].get(),
            word_30: self.words[LINK_STATE_WORD_30].get(),
            word_34: self.words[LINK_STATE_WORD_34].get(),
            access_address_word_38: self.words[LINK_STATE_WORD_38].get(),
            word_3c: self.words[LINK_STATE_WORD_3C].get(),
            word_40: self.words[LINK_STATE_WORD_40].get(),
            word_50: self.words[LINK_STATE_WORD_50].get(),
            word_60: self.words[LINK_STATE_WORD_60].get(),
        }
    }

    fn write_reviewed_words(&self, words: LegacyAdvertisingLinkStateWords) {
        self.words[LINK_STATE_WORD_00].set(words.word_00);
        self.words[LINK_STATE_WORD_04].set(words.word_04);
        self.words[LINK_STATE_WORD_08].set(words.word_08);
        self.words[LINK_STATE_WORD_0C].set(words.word_0c);
        self.words[LINK_STATE_WORD_14].set(words.word_14);
        self.words[LINK_STATE_WORD_18].set(words.word_18);
        self.words[LINK_STATE_WORD_24].set(words.word_24);
        self.words[LINK_STATE_WORD_2C].set(words.crc_init_word_2c);
        self.words[LINK_STATE_WORD_30].set(words.word_30);
        self.words[LINK_STATE_WORD_34].set(words.word_34);
        self.words[LINK_STATE_WORD_38].set(words.access_address_word_38);
        self.words[LINK_STATE_WORD_3C].set(words.word_3c);
        self.words[LINK_STATE_WORD_40].set(words.word_40);
        self.words[LINK_STATE_WORD_50].set(words.word_50);
        self.words[LINK_STATE_WORD_60].set(words.word_60);
    }

    fn scheduler_head(&self) -> u32 {
        self.words[LINK_STATE_SCHEDULER_HEAD].get()
    }

    fn set_scheduler_head(&self, item: Option<ControllerSramLinkAddress>) {
        self.words[LINK_STATE_SCHEDULER_HEAD]
            .set(item.map_or(0, |item| item.controller_address().address()));
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
        words.word_04 &= !COMPRESSED_LINK_MASK;
        if scan_response {
            words.word_04 |= binding.scan_response_header.compressed_image();
        }
        // adv_alloc_rxbuf prepares an empty private RX link before reset.
        // The later memory-manager broker selects the global RX class and
        // updates software head/tail, without installing a private consumer.
        words.word_08 &= !COMPRESSED_LINK_MASK;
        self.write_reviewed_words(words);
        let class = &self.words[LINK_STATE_RX_LIST_CLASS];
        class.set(RxMemoryListClass::NonScanning.select_in_link_state(class.get()));
        self.words[LINK_STATE_RX_HEAD].set(rx_head);
        self.words[LINK_STATE_RX_TAIL].set(rx_tail);
        self.words[LINK_STATE_RX_SWAP_RESERVE].set(rx_spare);
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

    fn initialize(&self, binding: &LegacyConnectableAdvertisingBinding) {
        for word in &self.words {
            word.set(0);
        }
        self.header()
            .set_hardware_next_word(SCHEDULER_ITEM_ALLOCATION_PREFIX);
        self.words[SCHEDULER_ITEM_CONTEXT].set(binding.scheduler_context.compressed_image());
        self.words[SCHEDULER_ITEM_ALLOCATION_FLAGS].set(SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE);
        self.words[SCHEDULER_ITEM_ALLOCATION_NUMBER].set(u32::from(binding.number));
        self.words[SCHEDULER_ITEM_LINK_STATE]
            .set(SCHEDULER_ITEM_LINK_STATE_PREFIX | binding.link_state.compressed_image());
    }

    fn reviewed_words(&self) -> LegacyAdvertisingSchedulerItemWords {
        LegacyAdvertisingSchedulerItemWords {
            word_00: self.header().hardware_next_word(),
            word_04: self.words[SCHEDULER_ITEM_CONTEXT].get(),
            word_14: self.words[SCHEDULER_ITEM_WORD_14].get(),
            word_18: self.words[SCHEDULER_ITEM_WORD_18].get(),
            word_38: self.header().status(),
            raw_start_word_44: self.header().raw_start(),
            raw_end_word_48: self.header().raw_end(),
            word_4c: self.header().control(),
        }
    }

    fn write_reviewed_words(&self, words: LegacyAdvertisingSchedulerItemWords) {
        self.header().set_hardware_next_word(words.word_00);
        self.words[SCHEDULER_ITEM_CONTEXT].set(words.word_04);
        self.words[SCHEDULER_ITEM_WORD_14].set(words.word_14);
        self.words[SCHEDULER_ITEM_WORD_18].set(words.word_18);
        self.header().set_status(words.word_38);
        self.header().set_raw_start(words.raw_start_word_44);
        self.header().set_raw_end(words.raw_end_word_48);
        self.header().set_control(words.word_4c);
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
    const KIND: SchedulerRoleKind = SchedulerRoleKind::ConnectableAdvertising;
    const ITEMS: usize = ITEMS;
    const NUMBERS: usize = 1;
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

    fn item_words(&self, item: usize) -> &[VolatileCell<u32>] {
        &self.items[item].words
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
            graph.link_state.words[LINK_STATE_TX_TAIL]
                .set(cpu.binding.adv_ind_header.controller_address().address());
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
            item.words[SCHEDULER_ITEM_COEX_PRIORITIES].set(
                (item.words[SCHEDULER_ITEM_COEX_PRIORITIES].get() & !LANES_MASK)
                    | lanes_image(&coexistence.lanes),
            );
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

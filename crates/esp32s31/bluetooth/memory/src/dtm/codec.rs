//! Private SRAM layout and word codec for one DTM memory graph.

use crate::{
    dtm_event_image::{
        DtmLinkStateProfileWord, DtmLinkStateReviewedWords, DtmPositionalEventWords,
        DtmRxHeaderTailProjection, DtmSchedulerHardwareChainWord, DtmSchedulerItemReviewedWords,
        DtmTxHeaderHeadProjection,
    },
    dtm_rx_result::{DtmRxResultProjection, DtmRxResultProjectionError},
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_tx_packet::{
        BLUETOOTH_LE_BUFFER_HEADER_BYTES, BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES,
        LeTxBufferHeaderStorage, LeTxPacketAddress, LeTxPacketPreparedLength, LeTxPacketStorage,
    },
    scheduler_context::SchedulerContextStorage,
    scheduler_item::{SchedulerItemCompletionStatus, SchedulerItemHeader},
    sram_link::ControllerSramLinkAddress,
};

use oer_esp32s31_hal::types::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};

use super::{
    DtmExecutedWitness, DtmPositionalEventSeed, DtmPrepareError, DtmRxRotationError,
    DtmSchedulerItemCompletionStatus, DtmTxPacketPrepareError,
};
use crate::scheduler_pool::SchedulerPoolBindError;

use vcell::VolatileCell;

/// Bytes allocated for one DTM link-state object.
pub const BLUETOOTH_DTM_LINK_STATE_BYTES: usize = 0x84;
/// Bytes allocated for one DTM scheduler item.
pub const BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES: usize = 0x60;
/// Bytes preceding the maximum DTM receiver capacity.
pub const BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES: usize = 0x1e;
/// Maximum packet capacity supplied by the complete DTM allocator.
pub const BLUETOOTH_DTM_MAX_PACKET_CAPACITY: usize = u8::MAX as usize;
/// Logical bytes in the complete DTM TX packet allocation.
pub const BLUETOOTH_DTM_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_MAX_PACKET_CAPACITY;
/// Logical bytes in the complete DTM RX packet allocation.
pub const BLUETOOTH_DTM_RX_PACKET_BYTES: usize =
    BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_MAX_PACKET_CAPACITY;

type DtmTxPacketAddress = LeTxPacketAddress<BLUETOOTH_DTM_TX_PACKET_BYTES>;

const RX_PACKET_LAST_ALIGNED_OFFSET: u32 = 0x11c;
pub(super) const LINK_STATE_RX_TAIL_OFFSET: usize = 0x70 / 4;
const LINK_STATE_RX_HEAD_OFFSET: usize = 0x68 / 4;
const LINK_STATE_TX_HEAD_OFFSET: usize = 0x6c / 4;
const LINK_STATE_TX_TAIL_OFFSET: usize = 0x74 / 4;
const LINK_STATE_RX_SWAP_RESERVE_OFFSET: usize = 0x78 / 4;
const LINK_STATE_ALLOCATION_CONFIG_OFFSET: usize = 0x30 / 4;
const LINK_STATE_ALLOCATION_CONFIG_IMAGE: u32 = 0x0000_1e00;
const SCHEDULER_ITEM_ALLOCATION_PREFIX_OFFSET: usize = 0;
const SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE: u32 = 0x0030_0000;
const SCHEDULER_ITEM_CONTEXT_OFFSET: usize = 1;
const SCHEDULER_ITEM_LINK_STATE_OFFSET: usize = 0x08 / 4;
const SCHEDULER_ITEM_ALLOCATION_FLAGS_OFFSET: usize = 0x1c / 4;
const SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE: u32 = 0xffdf_ffff;
const SCHEDULER_ITEM_ALLOCATION_CONFIG_OFFSET: usize = 0x20 / 4;
const SCHEDULER_ITEM_POSITIONAL_24_OFFSET: usize = 0x24 / 4;
/// SOURCE: pinned `libble_app.a[dtm_4.o]::sym_dtm_qzyGhbH1fEoUEmtalnva`
/// (`r_ble_lll_dtm_alloc_memory`) stores `0x8421` in the low twenty bits,
/// four five-bit coexistence priority lanes of one each.
const SCHEDULER_ITEM_POSITIONAL_24_IMAGE: u32 = 0x0000_8421;
const SCHEDULER_ITEM_WORDS: usize = BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES / 4;
const RX_PACKET_WORDS: usize = BLUETOOTH_DTM_RX_PACKET_BYTES.div_ceil(4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DtmRxPacketAddressError {
    InvalidBase(BluetoothControllerSramAddressError),
    ZeroCompressedBase,
    ExtentOutsideControllerSram,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DtmRxPacketAddress {
    base: BluetoothControllerSramAddress,
}

impl DtmRxPacketAddress {
    const fn new(address: u32) -> Result<Self, DtmRxPacketAddressError> {
        let base = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => return Err(DtmRxPacketAddressError::InvalidBase(error)),
        };
        if base.compressed_image() == 0 {
            return Err(DtmRxPacketAddressError::ZeroCompressedBase);
        }
        let last_aligned_address = match address.checked_add(RX_PACKET_LAST_ALIGNED_OFFSET) {
            Some(address) => address,
            None => return Err(DtmRxPacketAddressError::ExtentOutsideControllerSram),
        };
        if BluetoothControllerSramAddress::new(last_aligned_address).is_err() {
            return Err(DtmRxPacketAddressError::ExtentOutsideControllerSram);
        }
        Ok(Self { base })
    }

    const fn compressed_image(self) -> u32 {
        self.base.compressed_image()
    }
}

/// Opaque CPU-owned link-state allocation.
#[repr(C, align(4))]
pub(super) struct DtmLinkStateStorage {
    words: [VolatileCell<u32>; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
}

impl DtmLinkStateStorage {
    const CRC_INIT_WORD: usize = 0x2c / 4;
    const ACCESS_ADDRESS_WORD: usize = 0x38 / 4;

    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BLUETOOTH_DTM_LINK_STATE_BYTES / 4],
        }
    }

    pub(super) fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    pub(super) fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    fn access_address(&self) -> LeAccessAddress {
        LeAccessAddress::from_controller_image(self.read_word(Self::ACCESS_ADDRESS_WORD))
    }

    fn write_access_address(&self, access_address: LeAccessAddress) {
        self.write_word(Self::ACCESS_ADDRESS_WORD, access_address.controller_image());
    }

    fn crc_init(&self) -> LeCrcInit {
        LeCrcInit::from_controller_word(self.read_word(Self::CRC_INIT_WORD))
    }

    fn write_crc_init(&self, crc_init: LeCrcInit) {
        let word = self.read_word(Self::CRC_INIT_WORD);
        self.write_word(Self::CRC_INIT_WORD, crc_init.apply_to_controller_word(word));
    }

    pub(super) fn reviewed_words(&self) -> DtmLinkStateReviewedWords {
        DtmLinkStateReviewedWords {
            word_00: self.read_word(0),
            word_08: self.read_word(2),
            profile_word_14: DtmLinkStateProfileWord::from_storage(self.read_word(5)),
            crc_init: self.crc_init(),
            word_30: self.read_word(12),
            word_34: self.read_word(13),
            access_address: self.access_address(),
            word_50: self.read_word(20),
            word_60: self.read_word(24),
        }
    }

    pub(super) fn write_reviewed_words(&self, words: DtmLinkStateReviewedWords) {
        self.write_word(0, words.word_00);
        self.write_word(2, words.word_08);
        self.write_word(5, words.profile_word_14.into_storage());
        self.write_crc_init(words.crc_init());
        self.write_word(12, words.word_30);
        self.write_word(13, words.word_34);
        self.write_access_address(words.access_address());
        self.write_word(20, words.word_50);
        self.write_word(24, words.word_60);
    }
}

/// Opaque hardware-shared scheduler-item allocation.
#[repr(C, align(4))]
pub(super) struct DtmSchedulerItemStorage {
    words: [VolatileCell<u32>; SCHEDULER_ITEM_WORDS],
}

impl DtmSchedulerItemStorage {
    pub(super) fn words(&self) -> &[VolatileCell<u32>] {
        &self.words
    }

    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; SCHEDULER_ITEM_WORDS],
        }
    }

    pub(super) fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    pub(super) fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    pub(super) fn header(
        &self,
    ) -> SchedulerItemHeader<'_, [VolatileCell<u32>; SCHEDULER_ITEM_WORDS]> {
        SchedulerItemHeader::new(&self.words)
    }

    fn hardware_chain_word(&self) -> DtmSchedulerHardwareChainWord {
        DtmSchedulerHardwareChainWord::from_storage(self.header().hardware_next_word())
    }

    fn write_hardware_chain_word(&self, word: DtmSchedulerHardwareChainWord) {
        self.header().set_hardware_next_word(word.into_storage());
    }

    fn terminate_hardware_chain(&self) -> DtmSchedulerHardwareChainWord {
        let previous = self.hardware_chain_word();
        self.write_hardware_chain_word(previous.terminate());
        previous
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    fn reviewed_words(&self) -> DtmSchedulerItemReviewedWords {
        DtmSchedulerItemReviewedWords {
            word_00: self.read_word(0),
            word_04: self.read_word(1),
            word_08: self.read_word(2),
            word_0c: self.read_word(3),
            word_14: self.read_word(5),
            word_18: self.read_word(6),
            word_2c: self.read_word(11),
            word_44: self.read_word(17),
            word_48: self.read_word(18),
            word_4c: self.read_word(19),
        }
    }

    fn write_reviewed_words(&self, words: DtmSchedulerItemReviewedWords) {
        self.write_word(0, words.word_00);
        self.write_word(1, words.word_04);
        self.write_word(2, words.word_08);
        self.write_word(3, words.word_0c);
        self.write_word(5, words.word_14);
        self.write_word(6, words.word_18);
        self.write_word(11, words.word_2c);
        self.write_word(17, words.word_44);
        self.write_word(18, words.word_48);
        self.write_word(19, words.word_4c);
    }
}

#[repr(C, align(4))]
pub(super) struct DtmRxBufferHeaderStorage {
    words: [VolatileCell<u32>; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
}

impl DtmRxBufferHeaderStorage {
    const COMPRESSED_LINK_MASK: u32 = 0x000f_ffff;
    const RX_COMPLETION_OBSERVED_MASK: u32 = 0x8000_0000;
    const RX_SOFTWARE_TERMINAL_MASK: u32 = 1;

    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
        }
    }

    fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    fn install(&self, words: [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4]) {
        for (cell, word) in self.words.iter().zip(words) {
            cell.set(word);
        }
    }

    fn initialize_bound_rx(&self, packet: DtmRxPacketAddress) {
        self.install([0, packet.compressed_image(), 0x8080_0000, 0, 0, 0]);
    }

    fn initialize_rx_swap_reserve(&self) {
        self.install([0, 0, 0x8080_0000, 0, 0, 0]);
    }

    fn rx_successor_link(&self) -> Option<DtmRxHeaderSuccessorLink> {
        DtmRxHeaderSuccessorLink::from_image(self.read_word(0) & Self::COMPRESSED_LINK_MASK)
    }

    fn set_rx_successor_link(&self, link: Option<DtmRxHeaderSuccessorLink>) {
        let current = self.read_word(0);
        let image = link.map_or(0, DtmRxHeaderSuccessorLink::image);
        self.write_word(0, (current & !Self::COMPRESSED_LINK_MASK) | image);
    }

    fn rx_packet_link(&self) -> Option<DtmRxPacketLink> {
        DtmRxPacketLink::from_image(self.read_word(1) & Self::COMPRESSED_LINK_MASK)
    }

    fn clear_rx_packet_link(&self) {
        let current = self.read_word(1);
        self.write_word(1, current & !Self::COMPRESSED_LINK_MASK);
    }

    fn observe_rx_completion_after_fence(&self, _fenced: &DtmExecutedWitness) -> bool {
        self.read_word(3) & Self::RX_COMPLETION_OBSERVED_MASK != 0
    }

    fn clear_rx_completion_observation(&self) {
        let current = self.read_word(3);
        self.write_word(3, current & !Self::RX_COMPLETION_OBSERVED_MASK);
    }

    #[cfg(test)]
    pub(super) fn model_controller_completion_observed(&self) {
        let current = self.read_word(3);
        self.write_word(3, current | Self::RX_COMPLETION_OBSERVED_MASK);
    }

    fn set_rx_software_terminal(&self, terminal: bool) {
        let current = self.read_word(4);
        let value = if terminal {
            current | Self::RX_SOFTWARE_TERMINAL_MASK
        } else {
            current & !Self::RX_SOFTWARE_TERMINAL_MASK
        };
        self.write_word(4, value);
    }

    fn rx_backlink(&self) -> Option<DtmRxHeaderBacklink> {
        DtmRxHeaderBacklink::from_address(self.read_word(5))
    }

    fn set_rx_backlink(&self, backlink: Option<DtmRxHeaderBacklink>) {
        self.write_word(5, backlink.map_or(0, DtmRxHeaderBacklink::address));
    }

    fn copy_complete_rx_image_from(&self, source: &Self) {
        self.install(core::array::from_fn(|index| source.read_word(index)));
    }

    #[cfg(test)]
    pub(super) fn model_retarget_rx_packet(&self, packet: DtmRxPacketAddress) {
        let current = self.read_word(1);
        self.write_word(
            1,
            (current & !Self::COMPRESSED_LINK_MASK) | packet.compressed_image(),
        );
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DtmRxHeaderSuccessorLink(u32);

impl DtmRxHeaderSuccessorLink {
    fn from_image(image: u32) -> Option<Self> {
        (image != 0).then_some(Self(image))
    }

    const fn from_bound(address: ControllerSramLinkAddress) -> Self {
        Self(address.compressed_image())
    }

    const fn image(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DtmRxPacketLink(u32);

impl DtmRxPacketLink {
    fn from_image(image: u32) -> Option<Self> {
        (image != 0).then_some(Self(image))
    }

    const fn from_bound(packet: DtmRxPacketAddress) -> Self {
        Self(packet.compressed_image())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DtmRxHeaderBacklink(u32);

impl DtmRxHeaderBacklink {
    fn from_address(address: u32) -> Option<Self> {
        (address != 0).then_some(Self(address))
    }

    const fn bound(address: BluetoothControllerSramAddress) -> Self {
        Self(address.address())
    }

    const fn address(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LeTestPduHeader(u8);

impl LeTestPduHeader {
    pub(super) const fn without_cte(payload_type: u8) -> Result<Self, DtmTxPacketPrepareError> {
        if payload_type <= 7 {
            Ok(Self(payload_type))
        } else {
            Err(DtmTxPacketPrepareError::UnsupportedPayloadType)
        }
    }

    const fn controller_image(self) -> u8 {
        self.0
    }
}

#[repr(C, align(4))]
pub(super) struct DtmRxPacketStorage {
    words: [VolatileCell<u32>; RX_PACKET_WORDS],
}

impl DtmRxPacketStorage {
    const CAPACITY_WORD: usize = 1;
    const RESULT_WORD: usize = 3;
    const AUXILIARY_REARM_WORD: usize = 6;
    const RESULT_REARM_SENTINEL: u32 = 0x00ff_ffff;
    const AUXILIARY_REARM_SENTINEL: u32 = 0x0000_ffff;

    const fn new() -> Self {
        let mut storage = Self {
            words: [const { VolatileCell::new(0) }; RX_PACKET_WORDS],
        };
        storage.words[Self::CAPACITY_WORD] = VolatileCell::new(0x0001_0100);
        storage.words[Self::RESULT_WORD] = VolatileCell::new(Self::RESULT_REARM_SENTINEL);
        storage.words[Self::AUXILIARY_REARM_WORD] =
            VolatileCell::new(Self::AUXILIARY_REARM_SENTINEL);
        storage
    }

    fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    fn write_word(&self, index: usize, value: u32) {
        self.words[index].set(value);
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    fn rearm_reviewed_packet_fields(&self) {
        self.write_word(
            Self::RESULT_WORD,
            self.read_word(Self::RESULT_WORD) | Self::RESULT_REARM_SENTINEL,
        );
        self.write_word(
            Self::AUXILIARY_REARM_WORD,
            self.read_word(Self::AUXILIARY_REARM_WORD) | Self::AUXILIARY_REARM_SENTINEL,
        );
    }

    fn initialize_reviewed_allocation_fields(&self) {
        self.write_word(Self::CAPACITY_WORD, 0x0001_0100);
        self.rearm_reviewed_packet_fields();
    }

    fn observe_after_fenced_completion(
        &self,
        _fenced: &DtmExecutedWitness,
    ) -> Result<
        Result<DtmRxResultProjection, DtmRxResultProjectionError>,
        DtmRxPacketCompletionObservationError,
    > {
        let result_word = self.read_word(Self::RESULT_WORD);
        if result_word & Self::RESULT_REARM_SENTINEL == Self::RESULT_REARM_SENTINEL {
            return Err(DtmRxPacketCompletionObservationError::ResultNotProduced);
        }
        if self.read_word(Self::AUXILIARY_REARM_WORD) & Self::AUXILIARY_REARM_SENTINEL
            == Self::AUXILIARY_REARM_SENTINEL
        {
            return Err(DtmRxPacketCompletionObservationError::AuxiliaryNotProduced);
        }
        Ok(DtmRxResultProjection::from_word(result_word))
    }

    #[cfg(test)]
    pub(super) fn model_controller_completion(&self, result_word: u32, auxiliary: u16) {
        self.write_word(Self::RESULT_WORD, result_word);
        self.write_word(Self::AUXILIARY_REARM_WORD, u32::from(auxiliary));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DtmRxPacketCompletionObservationError {
    ResultNotProduced,
    AuxiliaryNotProduced,
}

/// Controller-SRAM graph of one DTM instance.
#[repr(C)]
pub struct DtmStorage {
    #[cfg_attr(test, allow(dead_code))]
    pub(super) link_state: DtmLinkStateStorage,
    pub(super) scheduler_context: SchedulerContextStorage,
    pub(super) scheduler_item: DtmSchedulerItemStorage,
    pub(super) rx_header: DtmRxBufferHeaderStorage,
    pub(super) rx_swap_reserve: DtmRxBufferHeaderStorage,
    pub(super) tx_header: LeTxBufferHeaderStorage,
    pub(super) tx_packet: LeTxPacketStorage<BLUETOOTH_DTM_TX_PACKET_BYTES>,
    pub(super) rx_packet: DtmRxPacketStorage,
}

const LINK_STATE_STORAGE_OFFSET: u32 = core::mem::offset_of!(DtmStorage, link_state) as u32;
const SCHEDULER_CONTEXT_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(DtmStorage, scheduler_context) as u32;
const SCHEDULER_ITEM_STORAGE_OFFSET: u32 = core::mem::offset_of!(DtmStorage, scheduler_item) as u32;
const RX_HEADER_STORAGE_OFFSET: u32 = core::mem::offset_of!(DtmStorage, rx_header) as u32;
const RX_SWAP_RESERVE_STORAGE_OFFSET: u32 =
    core::mem::offset_of!(DtmStorage, rx_swap_reserve) as u32;
const TX_HEADER_STORAGE_OFFSET: u32 = core::mem::offset_of!(DtmStorage, tx_header) as u32;
const TX_PACKET_STORAGE_OFFSET: u32 = core::mem::offset_of!(DtmStorage, tx_packet) as u32;
const RX_PACKET_STORAGE_OFFSET: u32 = core::mem::offset_of!(DtmStorage, rx_packet) as u32;

/// Addresses and DTM number of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct DtmBinding {
    link_state: ControllerSramLinkAddress,
    scheduler_context: BluetoothControllerSramAddress,
    scheduler_item: ControllerSramLinkAddress,
    pub(super) rx_header: ControllerSramLinkAddress,
    pub(super) rx_swap_reserve: ControllerSramLinkAddress,
    tx_header: ControllerSramLinkAddress,
    pub(super) tx_packet: DtmTxPacketAddress,
    pub(super) rx_packet: DtmRxPacketAddress,
    number: u16,
}

impl DtmBinding {
    pub(super) fn new(base: u32, number: u16) -> Result<Self, SchedulerPoolBindError> {
        let address = |offset: u32| base + offset;
        let bound_link = |offset: u32| {
            ControllerSramLinkAddress::new(address(offset))
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)
        };
        Ok(Self {
            link_state: bound_link(LINK_STATE_STORAGE_OFFSET)?,
            scheduler_context: BluetoothControllerSramAddress::new(address(
                SCHEDULER_CONTEXT_STORAGE_OFFSET,
            ))
            .map_err(SchedulerPoolBindError::InvalidBase)?,
            scheduler_item: bound_link(SCHEDULER_ITEM_STORAGE_OFFSET)?,
            rx_header: bound_link(RX_HEADER_STORAGE_OFFSET)?,
            rx_swap_reserve: bound_link(RX_SWAP_RESERVE_STORAGE_OFFSET)?,
            tx_header: bound_link(TX_HEADER_STORAGE_OFFSET)?,
            tx_packet: DtmTxPacketAddress::new(address(TX_PACKET_STORAGE_OFFSET))
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)?,
            rx_packet: DtmRxPacketAddress::new(address(RX_PACKET_STORAGE_OFFSET))
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)?,
            number,
        })
    }

    pub(super) const fn scheduler_item_address(&self) -> ControllerSramLinkAddress {
        self.scheduler_item
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum DtmRxHeaderSlot {
    Primary,
    Swap,
}

impl DtmRxHeaderSlot {
    const fn other(self) -> Self {
        match self {
            Self::Primary => Self::Swap,
            Self::Swap => Self::Primary,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum DtmRxRotationPlan {
    NoReturnedPacket {
        predecessor: Option<DtmRxHeaderSlot>,
    },
    Rotate {
        returned: DtmRxHeaderSlot,
        copy_target: DtmRxHeaderSlot,
        steady: bool,
        projection: Result<DtmRxResultProjection, DtmRxResultProjectionError>,
    },
}

impl DtmRxRotationPlan {
    pub(super) const fn projection(
        self,
    ) -> Option<Result<DtmRxResultProjection, DtmRxResultProjectionError>> {
        match self {
            Self::NoReturnedPacket { .. } => None,
            Self::Rotate { projection, .. } => Some(projection),
        }
    }
}

impl DtmStorage {
    pub const fn new() -> Self {
        Self {
            link_state: DtmLinkStateStorage::new(),
            scheduler_context: SchedulerContextStorage::new(),
            scheduler_item: DtmSchedulerItemStorage::new(),
            rx_header: DtmRxBufferHeaderStorage::new(),
            rx_swap_reserve: DtmRxBufferHeaderStorage::new(),
            tx_header: LeTxBufferHeaderStorage::new(),
            tx_packet: LeTxPacketStorage::new(),
            rx_packet: DtmRxPacketStorage::new(),
        }
    }

    pub(super) fn initialize_reviewed_allocation(&mut self, binding: &DtmBinding) {
        let link_state = binding.link_state.compressed_image();
        let scheduler_context = binding.scheduler_context.compressed_image();
        let rx_header_address = binding.rx_header.controller_address().address();
        let rx_swap_address = binding.rx_swap_reserve.controller_address().address();
        let tx_header_address = binding.tx_header.controller_address().address();

        let storage = self;
        storage.link_state.clear();
        storage.scheduler_context.clear();
        storage.scheduler_item.clear();
        storage.rx_header.initialize_bound_rx(binding.rx_packet);
        storage.rx_swap_reserve.initialize_rx_swap_reserve();
        storage.tx_header.initialize_bound_tx(binding.tx_packet);
        storage.tx_packet.clear();
        storage.rx_packet.clear();
        storage.rx_packet.initialize_reviewed_allocation_fields();

        storage
            .link_state
            .write_word(LINK_STATE_RX_HEAD_OFFSET, rx_header_address);
        storage
            .link_state
            .write_word(LINK_STATE_TX_HEAD_OFFSET, tx_header_address);
        storage
            .link_state
            .write_word(LINK_STATE_RX_TAIL_OFFSET, rx_header_address);
        storage
            .link_state
            .write_word(LINK_STATE_TX_TAIL_OFFSET, tx_header_address);
        storage
            .link_state
            .write_word(LINK_STATE_RX_SWAP_RESERVE_OFFSET, rx_swap_address);
        storage.link_state.write_word(
            LINK_STATE_ALLOCATION_CONFIG_OFFSET,
            LINK_STATE_ALLOCATION_CONFIG_IMAGE,
        );
        storage.scheduler_item.write_word(
            SCHEDULER_ITEM_ALLOCATION_PREFIX_OFFSET,
            SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE,
        );
        storage
            .scheduler_item
            .write_word(SCHEDULER_ITEM_CONTEXT_OFFSET, scheduler_context);
        storage
            .scheduler_item
            .write_word(SCHEDULER_ITEM_LINK_STATE_OFFSET, link_state);
        storage.scheduler_item.write_word(
            SCHEDULER_ITEM_ALLOCATION_FLAGS_OFFSET,
            SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE,
        );
        storage.scheduler_item.write_word(
            SCHEDULER_ITEM_ALLOCATION_CONFIG_OFFSET,
            u32::from(binding.number),
        );
        storage.scheduler_item.write_word(
            SCHEDULER_ITEM_POSITIONAL_24_OFFSET,
            SCHEDULER_ITEM_POSITIONAL_24_IMAGE,
        );
    }

    pub(super) fn reviewed_event_words(&self) -> DtmPositionalEventWords {
        DtmPositionalEventWords::new(
            self.link_state.reviewed_words(),
            self.scheduler_item.reviewed_words(),
        )
    }

    pub(super) fn observe_completion_status(&self) -> Option<DtmSchedulerItemCompletionStatus> {
        self.scheduler_item
            .header()
            .completion_status()
            .map(|recorded| match recorded {
                SchedulerItemCompletionStatus::Zero => DtmSchedulerItemCompletionStatus::Zero,
                SchedulerItemCompletionStatus::NonZero(status) => {
                    DtmSchedulerItemCompletionStatus::NonZero(status)
                }
            })
    }

    pub(super) fn commit_scheduler_recycle(&mut self) {
        let scheduler_item = &self.scheduler_item;
        scheduler_item.header().set_completion_link(0);
        let _ = scheduler_item.terminate_hardware_chain();
    }

    pub(super) fn prepared_tx_packet_bytes(
        &self,
        packet_length: LeTxPacketPreparedLength<BLUETOOTH_DTM_TX_PACKET_BYTES>,
    ) -> &[u8] {
        self.tx_packet.prepared_allocation(packet_length)
    }

    pub(super) fn prepare_tx_packet(
        &mut self,
        pdu_header: LeTestPduHeader,
        payload: &[u8],
    ) -> LeTxPacketPreparedLength<BLUETOOTH_DTM_TX_PACKET_BYTES> {
        self.tx_packet
            .prepare_pdu(pdu_header.controller_image(), payload)
            .unwrap_or_else(|_| {
                unreachable!("the full DTM allocation accepts every eight-bit payload length")
            })
    }

    pub(super) fn positional_event_seed<BuildError>(
        &self,
        binding: &DtmBinding,
    ) -> Result<DtmPositionalEventSeed, DtmPrepareError<BuildError>> {
        let previous = self.reviewed_event_words();
        let tx_head_address = self.link_state.read_word(LINK_STATE_TX_HEAD_OFFSET);
        let rx_tail_address = self.link_state.read_word(LINK_STATE_RX_TAIL_OFFSET);
        let tx_head = ControllerSramLinkAddress::new(tx_head_address)
            .map_err(|_| DtmPrepareError::CurrentTxHeadUnbound)?;
        if tx_head != binding.tx_header {
            return Err(DtmPrepareError::CurrentTxHeadIdentityMismatch);
        }
        if self.tx_header.packet_base_link() != Some(binding.tx_packet.base_link()) {
            return Err(DtmPrepareError::CurrentTxHeaderPacketBaseMismatch);
        }
        if self.tx_header.pdu_target_link() != Some(binding.tx_packet.pdu_target_link()) {
            return Err(DtmPrepareError::CurrentTxHeaderPduTargetMismatch);
        }
        if !self.tx_header.retains_allocation_extent(binding.tx_packet) {
            return Err(DtmPrepareError::CurrentTxHeaderAllocationExtentMismatch);
        }
        let rx_tail = ControllerSramLinkAddress::new(rx_tail_address)
            .map_err(|_| DtmPrepareError::CurrentRxTailUnbound)?;
        if rx_tail != binding.rx_header && rx_tail != binding.rx_swap_reserve {
            return Err(DtmPrepareError::CurrentRxTailIdentityMismatch);
        }
        let selected_rx_tail = if rx_tail == binding.rx_header {
            &self.rx_header
        } else {
            &self.rx_swap_reserve
        };
        if selected_rx_tail.rx_packet_link() != Some(DtmRxPacketLink::from_bound(binding.rx_packet))
        {
            return Err(DtmPrepareError::CurrentRxTailPacketMismatch);
        }
        Ok(DtmPositionalEventSeed {
            words: previous,
            tx_header_head: DtmTxHeaderHeadProjection::from_bound(tx_head),
            rx_header_tail: DtmRxHeaderTailProjection::from_bound(rx_tail),
        })
    }

    pub(super) fn validate_and_commit_positional_event<BuildError>(
        &mut self,
        binding: &DtmBinding,
        seed: DtmPositionalEventSeed,
        candidate: DtmPositionalEventWords,
    ) -> Result<(), DtmPrepareError<BuildError>> {
        let expected_tx_head = seed.tx_header_head_projection();
        let observed_tx_head = candidate.tx_header_head_projection();
        if observed_tx_head != expected_tx_head {
            return Err(DtmPrepareError::LinkStateTxHeadMismatch {
                expected: expected_tx_head,
                observed: observed_tx_head,
            });
        }
        let expected_rx_tail = seed.rx_header_tail_projection();
        let observed_rx_tail = candidate.rx_header_tail_projection();
        if observed_rx_tail != expected_rx_tail {
            return Err(DtmPrepareError::LinkStateRxTailMismatch {
                expected: expected_rx_tail,
                observed: observed_rx_tail,
            });
        }
        if !candidate.scheduler_item_retains_link_state(binding.link_state) {
            return Err(DtmPrepareError::SchedulerItemLinkStateMismatch);
        }
        let storage = self;
        storage
            .link_state
            .write_reviewed_words(candidate.link_state());
        storage
            .scheduler_item
            .write_reviewed_words(candidate.scheduler_item());
        Ok(())
    }

    pub(super) fn validate_rx_rotation(
        &self,
        binding: &DtmBinding,
        recycle: &DtmExecutedWitness,
        status: DtmSchedulerItemCompletionStatus,
    ) -> Result<DtmRxRotationPlan, DtmRxRotationError> {
        if status != DtmSchedulerItemCompletionStatus::Zero {
            return Err(DtmRxRotationError::CompletionStatusMismatch);
        }
        let primary_address = binding.rx_header.controller_address().address();
        let swap_address = binding.rx_swap_reserve.controller_address().address();
        let slot = |address| {
            if address == primary_address {
                Some(DtmRxHeaderSlot::Primary)
            } else if address == swap_address {
                Some(DtmRxHeaderSlot::Swap)
            } else {
                None
            }
        };
        let head = slot(self.link_state.read_word(LINK_STATE_RX_HEAD_OFFSET))
            .ok_or(DtmRxRotationError::RxHeadIdentityMismatch)?;
        let tail = slot(self.link_state.read_word(LINK_STATE_RX_TAIL_OFFSET))
            .ok_or(DtmRxRotationError::RxTailIdentityMismatch)?;
        let reserve_address = self.link_state.read_word(LINK_STATE_RX_SWAP_RESERVE_OFFSET);
        let header = |slot| match slot {
            DtmRxHeaderSlot::Primary => &self.rx_header,
            DtmRxHeaderSlot::Swap => &self.rx_swap_reserve,
        };
        let successor = |slot| match slot {
            DtmRxHeaderSlot::Primary => DtmRxHeaderSuccessorLink::from_bound(binding.rx_header),
            DtmRxHeaderSlot::Swap => DtmRxHeaderSuccessorLink::from_bound(binding.rx_swap_reserve),
        };
        let backlink = |slot| match slot {
            DtmRxHeaderSlot::Primary => {
                DtmRxHeaderBacklink::bound(binding.rx_header.controller_address())
            }
            DtmRxHeaderSlot::Swap => {
                DtmRxHeaderBacklink::bound(binding.rx_swap_reserve.controller_address())
            }
        };

        let (returned, copy_target, steady) = if head == tail {
            let copy_target = head.other();
            let expected_reserve = match copy_target {
                DtmRxHeaderSlot::Primary => primary_address,
                DtmRxHeaderSlot::Swap => swap_address,
            };
            if reserve_address != expected_reserve {
                return Err(DtmRxRotationError::RxSwapIdentityMismatch);
            }
            let reserve = header(copy_target);
            if reserve.rx_successor_link().is_some() || reserve.rx_packet_link().is_some() {
                return Err(DtmRxRotationError::ReserveNotDetached);
            }
            if header(tail).rx_backlink().is_some() {
                return Err(DtmRxRotationError::InitialBacklinkUnexpected);
            }
            (tail, copy_target, false)
        } else {
            if reserve_address != 0 {
                return Err(DtmRxRotationError::SwapReserveUnexpected);
            }
            let predecessor = header(head);
            if predecessor.rx_packet_link().is_some() {
                return Err(DtmRxRotationError::PredecessorPacketStillBound);
            }
            if predecessor.rx_successor_link() != Some(successor(tail)) {
                return Err(DtmRxRotationError::PredecessorSuccessorMismatch);
            }
            if !predecessor.observe_rx_completion_after_fence(recycle) {
                return Err(DtmRxRotationError::PredecessorNotCompleted);
            }
            if header(tail).rx_backlink() != Some(backlink(head)) {
                return Err(DtmRxRotationError::SuccessorBacklinkMismatch);
            }
            (tail, head, true)
        };

        let returned_header = header(returned);
        if returned_header.rx_successor_link().is_some() {
            return Err(DtmRxRotationError::ReturnedHasSuccessor);
        }
        if returned_header.rx_packet_link() != Some(DtmRxPacketLink::from_bound(binding.rx_packet))
        {
            return Err(DtmRxRotationError::ReturnedPacketMismatch);
        }
        if !returned_header.observe_rx_completion_after_fence(recycle) {
            return Ok(DtmRxRotationPlan::NoReturnedPacket {
                predecessor: steady.then_some(copy_target),
            });
        }

        let projection = self
            .rx_packet
            .observe_after_fenced_completion(recycle)
            .map_err(|error| match error {
                DtmRxPacketCompletionObservationError::ResultNotProduced => {
                    DtmRxRotationError::ReturnedResultNotProduced
                }
                DtmRxPacketCompletionObservationError::AuxiliaryNotProduced => {
                    DtmRxRotationError::ReturnedAuxiliaryNotProduced
                }
            })?;
        Ok(DtmRxRotationPlan::Rotate {
            returned,
            copy_target,
            steady,
            projection,
        })
    }

    pub(super) fn commit_rx_rotation(&mut self, binding: &DtmBinding, plan: DtmRxRotationPlan) {
        let storage = self;
        match plan {
            DtmRxRotationPlan::NoReturnedPacket { predecessor } => {
                if let Some(predecessor) = predecessor {
                    match predecessor {
                        DtmRxHeaderSlot::Primary => &storage.rx_header,
                        DtmRxHeaderSlot::Swap => &storage.rx_swap_reserve,
                    }
                    .set_rx_software_terminal(true);
                }
            }
            DtmRxRotationPlan::Rotate {
                returned,
                copy_target,
                steady,
                projection: _,
            } => {
                let primary = &storage.rx_header;
                let swap = &storage.rx_swap_reserve;
                let returned_header = match returned {
                    DtmRxHeaderSlot::Primary => primary,
                    DtmRxHeaderSlot::Swap => swap,
                };
                let copy_header = match copy_target {
                    DtmRxHeaderSlot::Primary => primary,
                    DtmRxHeaderSlot::Swap => swap,
                };
                let address = |slot| match slot {
                    DtmRxHeaderSlot::Primary => binding.rx_header.controller_address().address(),
                    DtmRxHeaderSlot::Swap => binding.rx_swap_reserve.controller_address().address(),
                };
                let copy_link = match copy_target {
                    DtmRxHeaderSlot::Primary => {
                        DtmRxHeaderSuccessorLink::from_bound(binding.rx_header)
                    }
                    DtmRxHeaderSlot::Swap => {
                        DtmRxHeaderSuccessorLink::from_bound(binding.rx_swap_reserve)
                    }
                };
                let returned_backlink = match returned {
                    DtmRxHeaderSlot::Primary => {
                        DtmRxHeaderBacklink::bound(binding.rx_header.controller_address())
                    }
                    DtmRxHeaderSlot::Swap => {
                        DtmRxHeaderBacklink::bound(binding.rx_swap_reserve.controller_address())
                    }
                };

                if steady {
                    storage
                        .link_state
                        .write_word(LINK_STATE_RX_SWAP_RESERVE_OFFSET, address(copy_target));
                    storage
                        .link_state
                        .write_word(LINK_STATE_RX_HEAD_OFFSET, address(returned));
                    returned_header.set_rx_backlink(None);
                }
                returned_header.set_rx_software_terminal(false);
                copy_header.copy_complete_rx_image_from(returned_header);
                returned_header.clear_rx_packet_link();
                storage
                    .link_state
                    .write_word(LINK_STATE_RX_SWAP_RESERVE_OFFSET, 0);
                copy_header.set_rx_backlink(None);
                storage.rx_packet.rearm_reviewed_packet_fields();
                copy_header.clear_rx_completion_observation();
                copy_header.set_rx_successor_link(None);
                copy_header.set_rx_backlink(None);
                returned_header.set_rx_successor_link(Some(copy_link));
                storage
                    .link_state
                    .write_word(LINK_STATE_RX_TAIL_OFFSET, address(copy_target));
                returned_header.set_rx_software_terminal(true);
                copy_header.set_rx_backlink(Some(returned_backlink));
            }
        }
    }
}

impl Default for DtmStorage {
    fn default() -> Self {
        Self::new()
    }
}

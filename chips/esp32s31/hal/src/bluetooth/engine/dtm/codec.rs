//! Private SRAM layout and word codec for one DTM memory graph.

use crate::bluetooth::engine::{
    dtm_image::{
        DtmLinkStateProfileWord, DtmLinkStateReviewedWords, DtmPositionalEventWords,
        DtmRxHeaderTailProjection, DtmSchedulerHardwareChainWord, DtmSchedulerItemReviewedWords,
        DtmTxHeaderHeadProjection,
    },
    dtm_rx_result::{DtmRxResultProjection, DtmRxResultProjectionError},
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, LeTxBufferHeaderStorage, LeTxPacketAddress,
        LeTxPacketPreparedLength, LeTxPacketStorage,
    },
    scheduler_context::SchedulerContextStorage,
    scheduler_item::{SchedulerItemCompletionStatus, SchedulerItemHeader},
    sram_link::{ControllerSramLink, SchedulerItemLink},
};

use super::{
    DtmExecutedWitness, DtmPositionalEventSeed, DtmPrepareError, DtmRxRotationError,
    DtmSchedulerItemCompletionStatus, DtmTxPacketPrepareError,
};

use crate::bluetooth::engine::rx_packet_bytes::{RxPacketBytes, result_not_produced};
use crate::layouts::*;

/// Bytes allocated for one DTM link-state object.
pub const BLUETOOTH_DTM_LINK_STATE_BYTES: usize = BleLinkState::SIZE;
/// Bytes allocated for one DTM scheduler item.
pub const BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES: usize = BleSchedulerItem::SIZE;
/// Bytes preceding the maximum DTM receiver capacity.
pub const BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES: usize = BleRxPacketPrefix::SIZE;
/// Maximum packet capacity supplied by the complete DTM allocator.
pub const BLUETOOTH_DTM_MAX_PACKET_CAPACITY: usize = u8::MAX as usize;
/// Logical bytes in the complete DTM TX packet allocation.
pub const BLUETOOTH_DTM_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_MAX_PACKET_CAPACITY;
/// Logical bytes in the complete DTM RX packet allocation.
pub const BLUETOOTH_DTM_RX_PACKET_BYTES: usize =
    BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES + BLUETOOTH_DTM_MAX_PACKET_CAPACITY;

type DtmTxPacketAddress = LeTxPacketAddress<BLUETOOTH_DTM_TX_PACKET_BYTES>;

/// SOURCE: chips/esp32s31/notes/bluetooth-direct-test-mode.md, allocation/reset profile (bt-lib pin 10c50778).
const LINK_STATE_ALLOCATION_CONFIG_IMAGE: BleLinkStateSequencePolicy =
    BleLinkStateSequencePolicy::ZERO.with_bits_9_12_opaque_constant::<0xf>();
/// SOURCE: chips/esp32s31/notes/bluetooth-direct-test-mode.md, allocation/reset profile (bt-lib pin 10c50778).
const SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE: BleSchedulerItemHardwareNext =
    BleSchedulerItemHardwareNext::ZERO.with_bits_20_21_opaque_constant::<0x3>();
/// SOURCE: chips/esp32s31/notes/bluetooth-direct-test-mode.md, allocation/reset profile (bt-lib pin 10c50778).
const SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE: BleSchedulerItemWord001cOpaque =
    BleSchedulerItemWord001cOpaque::from_bits(0xffdfffff);
/// SOURCE: pinned `libble_app.a[dtm_4.o]::sym_dtm_qzyGhbH1fEoUEmtalnva`
/// (`r_ble_lll_dtm_alloc_memory`) stores `0x8421` in the low twenty bits,
/// four five-bit coexistence priority lanes of one each.
/// SOURCE: chips/esp32s31/notes/bluetooth-direct-test-mode.md, allocation/reset profile (bt-lib pin 10c50778).
const SCHEDULER_ITEM_POSITIONAL_24_IMAGE: BleSchedulerItemCoexistencePriority =
    BleSchedulerItemCoexistencePriority::ZERO
        .with_lane_0_constant::<0x1>()
        .with_lane_1_constant::<0x1>()
        .with_lane_2_constant::<0x1>()
        .with_lane_3_constant::<0x1>();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DtmRxPacketAddress {
    base: DmaWindowAddress,
}

impl DtmRxPacketAddress {
    const fn from_placed(base: DmaWindowAddress) -> Self {
        Self { base }
    }

    const fn address(self) -> u32 {
        self.base.get()
    }
}

/// Opaque CPU-owned link-state allocation.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub(super) struct DtmLinkStateStorage {
    pub(super) layout: BleLinkState,
}

impl DtmLinkStateStorage {
    const fn new() -> Self {
        Self {
            layout: BleLinkState::zeroed(),
        }
    }

    fn clear(&self) {
        crate::bluetooth::engine::shared_layout::clear_link_state(&self.layout);
    }

    fn access_address(&self) -> LeAccessAddress {
        LeAccessAddress::from_controller_image(self.layout.access_address().read().bits())
    }

    fn write_access_address(&self, access_address: LeAccessAddress) {
        self.layout
            .access_address()
            .write(BleLinkStateAccessAddress::from_bits(
                access_address.controller_image(),
            ));
    }

    fn crc_init(&self) -> LeCrcInit {
        LeCrcInit::from_controller_word(self.layout.crc_initialization().read().bits())
    }

    fn write_crc_init(&self, crc_init: LeCrcInit) {
        let word = self.layout.crc_initialization().read().bits();
        self.layout
            .crc_initialization()
            .write(BleLinkStateCrcInitialization::from_bits(
                crc_init.apply_to_controller_word(word),
            ));
    }

    pub(super) fn reviewed_words(&self) -> DtmLinkStateReviewedWords {
        DtmLinkStateReviewedWords {
            word_00: self.layout.tx_path().read(),
            word_08: self.layout.rx_path().read(),
            profile_word_14: DtmLinkStateProfileWord::from_storage(
                self.layout.packet_policy().read().bits(),
            ),
            crc_init: self.crc_init(),
            word_30: self.layout.sequence_policy().read(),
            word_34: self.layout.event_span().read(),
            access_address: self.access_address(),
            word_50: self.layout.direction_finding().read(),
            word_60: self.layout.event().read(),
        }
    }

    pub(super) fn write_reviewed_words(&self, words: DtmLinkStateReviewedWords) {
        self.layout.tx_path().write(words.word_00);
        self.layout.rx_path().write(words.word_08);
        self.layout
            .packet_policy()
            .write(BleLinkStatePacketPolicy::from_bits(
                words.profile_word_14.into_storage(),
            ));
        self.write_crc_init(words.crc_init());
        self.layout.sequence_policy().write(words.word_30);
        self.layout.event_span().write(words.word_34);
        self.write_access_address(words.access_address());
        self.layout.direction_finding().write(words.word_50);
        self.layout.event().write(words.word_60);
    }
}

/// Opaque hardware-shared scheduler-item allocation.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub(super) struct DtmSchedulerItemStorage {
    pub(super) layout: BleSchedulerItem,
}

impl DtmSchedulerItemStorage {
    const fn new() -> Self {
        Self {
            layout: BleSchedulerItem::zeroed(),
        }
    }

    pub(super) fn header(&self) -> SchedulerItemHeader<'_> {
        SchedulerItemHeader::new(&self.layout)
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
        crate::bluetooth::engine::shared_layout::clear_scheduler_item(&self.layout);
    }

    fn reviewed_words(&self) -> DtmSchedulerItemReviewedWords {
        DtmSchedulerItemReviewedWords {
            word_00: self.layout.hardware_next().read(),
            word_04: self.layout.context().read(),
            word_08: self.layout.link_state().read(),
            word_0c: self.layout.sequence_start().read(),
            word_14: self.layout.rate_and_power().read(),
            word_18: self.layout.channel_and_priority().read(),
            word_2c: self.layout.receive_wait().read(),
            word_44: self.layout.raw_start().read(),
            word_48: self.layout.raw_end().read(),
            word_4c: self.layout.control().read(),
        }
    }

    fn write_reviewed_words(&self, words: DtmSchedulerItemReviewedWords) {
        self.layout.hardware_next().write(words.word_00);
        self.layout.context().write(words.word_04);
        self.layout.link_state().write(words.word_08);
        self.layout.sequence_start().write(words.word_0c);
        self.layout.rate_and_power().write(words.word_14);
        self.layout.channel_and_priority().write(words.word_18);
        self.layout.receive_wait().write(words.word_2c);
        self.layout.raw_start().write(words.word_44);
        self.layout.raw_end().write(words.word_48);
        self.layout.control().write(words.word_4c);
    }
}

#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub(super) struct DtmRxBufferHeaderStorage {
    pub(super) layout: BleBufferHeader,
}

/// SOURCE: pinned `memMgmt_3.o`/`dtm_4.o` common RX-header allocation.
const DTM_RX_HEADER: crate::bluetooth::engine::shared_layout::BufferHeaderImage =
    crate::bluetooth::engine::shared_layout::BufferHeaderImage {
        pdu_target: BleBufferHeaderPduTarget::ZERO
            .with_bit_23_opaque(true)
            .with_bit_31_opaque(true),
        ..crate::bluetooth::engine::shared_layout::BufferHeaderImage::ZERO
    };

impl DtmRxBufferHeaderStorage {
    const fn new() -> Self {
        Self {
            layout: BleBufferHeader::zeroed(),
        }
    }
    fn initialize_bound_rx(&self, packet: DtmRxPacketAddress) {
        self.initialize_rx_swap_reserve();
        self.layout.packet_base().write(
            BleBufferHeaderPacketBase::ZERO
                .with_address(packet.base)
                .expect("the placed DTM RX buffer address is encodable"),
        );
    }
    fn initialize_rx_swap_reserve(&self) {
        DTM_RX_HEADER.write(&self.layout);
    }
    fn rx_successor_link(&self) -> Option<DtmRxHeaderSuccessorLink> {
        self.layout
            .next()
            .read()
            .address_address()
            .map(DtmRxHeaderSuccessorLink)
    }
    fn set_rx_successor_link(&self, link: Option<ControllerSramLink<BleBufferHeader>>) {
        self.layout.next().write(
            self.layout
                .next()
                .read()
                .with_address(link.map(ControllerSramLink::reference))
                .expect("a bound link is encodable"),
        );
    }
    fn rx_packet_link(&self) -> Option<DtmRxPacketLink> {
        self.layout
            .packet_base()
            .read()
            .address_address()
            .map(DtmRxPacketLink)
    }
    fn clear_rx_packet_link(&self) {
        self.layout
            .packet_base()
            .write(self.layout.packet_base().read().without_address());
    }
    fn observe_rx_completion_after_fence(&self, _fenced: &DtmExecutedWitness) -> bool {
        self.layout.completion().read().done()
    }
    fn clear_rx_completion_observation(&self) {
        self.layout
            .completion()
            .write(self.layout.completion().read().with_done(false));
    }
    #[cfg(test)]
    pub(super) fn model_controller_completion_observed(&self) {
        self.layout
            .completion()
            .write(self.layout.completion().read().with_done(true));
    }
    fn set_rx_software_terminal(&self, terminal: bool) {
        self.layout
            .allocation()
            .write(self.layout.allocation().read().with_terminal(terminal));
    }
    fn rx_backlink(&self) -> Option<DtmRxHeaderBacklink> {
        DtmRxHeaderBacklink::from_address(self.layout.previous().read().address())
    }
    fn set_rx_backlink(&self, backlink: Option<ControllerSramLink<BleBufferHeader>>) {
        self.layout.previous().write(
            BleBufferHeaderPrevious::ZERO.with_address(backlink.map(ControllerSramLink::reference)),
        );
    }
    fn copy_complete_rx_image_from(&self, source: &Self) {
        crate::bluetooth::engine::shared_layout::copy_buffer_header(&self.layout, &source.layout);
    }
    #[cfg(test)]
    pub(super) fn model_retarget_rx_packet(&self, packet: DtmRxPacketAddress) {
        self.layout.packet_base().write(
            self.layout
                .packet_base()
                .read()
                .with_address(packet.base)
                .expect("the placed DTM RX buffer address is encodable"),
        );
    }
}

/// The address a header's successor link decodes to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DtmRxHeaderSuccessorLink(u32);

impl DtmRxHeaderSuccessorLink {
    const fn from_bound(link: ControllerSramLink<BleBufferHeader>) -> Self {
        Self(link.controller_address())
    }
}

/// The address a header's packet link decodes to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DtmRxPacketLink(u32);

impl DtmRxPacketLink {
    const fn from_bound(packet: DtmRxPacketAddress) -> Self {
        Self(packet.address())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DtmRxHeaderBacklink(u32);

impl DtmRxHeaderBacklink {
    fn from_address(address: u32) -> Option<Self> {
        (address != 0).then_some(Self(address))
    }

    const fn bound(link: ControllerSramLink<BleBufferHeader>) -> Self {
        Self(link.controller_address())
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
#[derive(bytemuck::Zeroable)]
pub(super) struct DtmRxPacketStorage {
    pub(super) bytes: RxPacketBytes,
}

impl DtmRxPacketStorage {
    const fn new() -> Self {
        Self {
            bytes: RxPacketBytes::armed(),
        }
    }
    fn clear(&self) {
        self.bytes.clear();
    }
    fn rearm_reviewed_packet_fields(&self) {
        self.bytes.rearm();
    }
    fn initialize_reviewed_allocation_fields(&self) {
        self.bytes.initialize_fields();
    }
    fn observe_after_fenced_completion(
        &self,
        _fenced: &DtmExecutedWitness,
    ) -> Result<
        Result<DtmRxResultProjection, DtmRxResultProjectionError>,
        DtmRxPacketCompletionObservationError,
    > {
        let bytes = self.bytes.prefix();
        let prefix = BleRxPacketPrefix::new(&bytes);
        if result_not_produced(prefix.result()) {
            return Err(DtmRxPacketCompletionObservationError::ResultNotProduced);
        }
        if prefix.auxiliary().bits_0_15_opaque() == u16::MAX {
            return Err(DtmRxPacketCompletionObservationError::AuxiliaryNotProduced);
        }
        Ok(DtmRxResultProjection::from_word(prefix.result().bits()))
    }
    #[cfg(test)]
    pub(super) fn model_controller_completion(&self, result_word: u32, auxiliary: u16) {
        self.bytes.update_prefix(|prefix| {
            prefix.set_result(BleRxPacketPrefixResult::from_bits(result_word));
            prefix.set_auxiliary(prefix.auxiliary().with_bits_0_15_opaque(auxiliary));
        });
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DtmRxPacketCompletionObservationError {
    ResultNotProduced,
    AuxiliaryNotProduced,
}

/// Controller-SRAM graph of one DTM instance.
#[repr(C)]
#[derive(bytemuck::Zeroable)]
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

/// Addresses and DTM number of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct DtmBinding {
    link_state: ControllerSramLink<BleLinkState>,
    scheduler_context: ControllerSramLink<BleSchedulerContext>,
    scheduler_item: SchedulerItemLink,
    pub(super) rx_header: ControllerSramLink<BleBufferHeader>,
    pub(super) rx_swap_reserve: ControllerSramLink<BleBufferHeader>,
    tx_header: ControllerSramLink<BleBufferHeader>,
    pub(super) tx_packet: DtmTxPacketAddress,
    pub(super) rx_packet: DtmRxPacketAddress,
    number: u16,
}

impl DtmBinding {
    pub(super) fn new<S: crate::bluetooth::engine::ControllerDmaSpace>(
        storage: &DtmStorage,
        placement: &crate::bluetooth::engine::Placement<S>,
        number: u16,
    ) -> Self {
        Self {
            link_state: ControllerSramLink::new(placement.reference(&storage.link_state.layout)),
            scheduler_context: ControllerSramLink::new(
                placement.reference(storage.scheduler_context.layout()),
            ),
            scheduler_item: ControllerSramLink::new(
                placement.reference(&storage.scheduler_item.layout),
            ),
            rx_header: ControllerSramLink::new(placement.reference(&storage.rx_header.layout)),
            rx_swap_reserve: ControllerSramLink::new(
                placement.reference(&storage.rx_swap_reserve.layout),
            ),
            tx_header: ControllerSramLink::new(placement.reference(&storage.tx_header.layout)),
            tx_packet: DtmTxPacketAddress::from_placed(placement, &storage.tx_packet),
            rx_packet: DtmRxPacketAddress::from_placed(placement.address(&storage.rx_packet)),
            number,
        }
    }

    pub(super) const fn scheduler_item_address(&self) -> SchedulerItemLink {
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
        let rx_header = Some(binding.rx_header.reference());
        let rx_swap = Some(binding.rx_swap_reserve.reference());
        let tx_header = Some(binding.tx_header.reference());

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
            .layout
            .rx_head()
            .write(BleLinkStateRxHead::ZERO.with_address(rx_header));
        storage
            .link_state
            .layout
            .tx_head()
            .write(BleLinkStateTxHead::ZERO.with_address(tx_header));
        storage
            .link_state
            .layout
            .rx_tail()
            .write(BleLinkStateRxTail::ZERO.with_address(rx_header));
        storage
            .link_state
            .layout
            .tx_tail()
            .write(BleLinkStateTxTail::ZERO.with_address(tx_header));
        storage
            .link_state
            .layout
            .rx_reserve()
            .write(BleLinkStateRxReserve::ZERO.with_address(rx_swap));
        storage
            .link_state
            .layout
            .sequence_policy()
            .write(LINK_STATE_ALLOCATION_CONFIG_IMAGE);
        storage
            .scheduler_item
            .layout
            .hardware_next()
            .write(SCHEDULER_ITEM_ALLOCATION_PREFIX_IMAGE);
        storage.scheduler_item.layout.context().write(
            BleSchedulerItemContext::ZERO
                .with_address(Some(binding.scheduler_context.reference()))
                .expect("a bound link is encodable"),
        );
        storage.scheduler_item.layout.link_state().write(
            BleSchedulerItemLinkState::ZERO
                .with_address(Some(binding.link_state.reference()))
                .expect("a bound link is encodable"),
        );
        storage
            .scheduler_item
            .layout
            .word_001c_opaque()
            .write(SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE);
        storage.scheduler_item.layout.allocation_number().write(
            BleSchedulerItemAllocationNumber::from_bits(u32::from(binding.number)),
        );
        storage
            .scheduler_item
            .layout
            .coexistence_priority()
            .write(SCHEDULER_ITEM_POSITIONAL_24_IMAGE);
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
        scheduler_item.header().link_completion(None);
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
        let tx_head_address = self.link_state.layout.tx_head().read().bits();
        let rx_tail_address = self.link_state.layout.rx_tail().read().bits();
        if ble_word_index(tx_head_address).is_none() {
            return Err(DtmPrepareError::CurrentTxHeadUnbound);
        }
        if !binding.tx_header.is(Some(tx_head_address)) {
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
        if ble_word_index(rx_tail_address).is_none() {
            return Err(DtmPrepareError::CurrentRxTailUnbound);
        }
        let rx_tail = Some(rx_tail_address);
        if !binding.rx_header.is(rx_tail) && !binding.rx_swap_reserve.is(rx_tail) {
            return Err(DtmPrepareError::CurrentRxTailIdentityMismatch);
        }
        let (selected_rx_tail, rx_tail) = if binding.rx_header.is(rx_tail) {
            (&self.rx_header, binding.rx_header)
        } else {
            (&self.rx_swap_reserve, binding.rx_swap_reserve)
        };
        if selected_rx_tail.rx_packet_link() != Some(DtmRxPacketLink::from_bound(binding.rx_packet))
        {
            return Err(DtmPrepareError::CurrentRxTailPacketMismatch);
        }
        Ok(DtmPositionalEventSeed {
            words: previous,
            tx_header_head: DtmTxHeaderHeadProjection::from_bound(binding.tx_header),
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
        let primary_address = binding.rx_header.controller_address();
        let swap_address = binding.rx_swap_reserve.controller_address();
        let slot = |address| {
            if address == primary_address {
                Some(DtmRxHeaderSlot::Primary)
            } else if address == swap_address {
                Some(DtmRxHeaderSlot::Swap)
            } else {
                None
            }
        };
        let head = slot(self.link_state.layout.rx_head().read().bits())
            .ok_or(DtmRxRotationError::RxHeadIdentityMismatch)?;
        let tail = slot(self.link_state.layout.rx_tail().read().bits())
            .ok_or(DtmRxRotationError::RxTailIdentityMismatch)?;
        let reserve_address = self.link_state.layout.rx_reserve().read().bits();
        let header = |slot| match slot {
            DtmRxHeaderSlot::Primary => &self.rx_header,
            DtmRxHeaderSlot::Swap => &self.rx_swap_reserve,
        };
        let successor = |slot| match slot {
            DtmRxHeaderSlot::Primary => DtmRxHeaderSuccessorLink::from_bound(binding.rx_header),
            DtmRxHeaderSlot::Swap => DtmRxHeaderSuccessorLink::from_bound(binding.rx_swap_reserve),
        };
        let backlink = |slot| match slot {
            DtmRxHeaderSlot::Primary => DtmRxHeaderBacklink::bound(binding.rx_header),
            DtmRxHeaderSlot::Swap => DtmRxHeaderBacklink::bound(binding.rx_swap_reserve),
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
                let link = |slot| match slot {
                    DtmRxHeaderSlot::Primary => binding.rx_header,
                    DtmRxHeaderSlot::Swap => binding.rx_swap_reserve,
                };
                let address = |slot| Some(link(slot).reference());
                let copy_link = link(copy_target);
                let returned_backlink = link(returned);

                if steady {
                    storage
                        .link_state
                        .layout
                        .rx_reserve()
                        .write(BleLinkStateRxReserve::ZERO.with_address(address(copy_target)));
                    storage
                        .link_state
                        .layout
                        .rx_head()
                        .write(BleLinkStateRxHead::ZERO.with_address(address(returned)));
                    returned_header.set_rx_backlink(None);
                }
                returned_header.set_rx_software_terminal(false);
                copy_header.copy_complete_rx_image_from(returned_header);
                returned_header.clear_rx_packet_link();
                storage
                    .link_state
                    .layout
                    .rx_reserve()
                    .write(BleLinkStateRxReserve::ZERO.with_address(None));
                copy_header.set_rx_backlink(None);
                storage.rx_packet.rearm_reviewed_packet_fields();
                copy_header.clear_rx_completion_observation();
                copy_header.set_rx_successor_link(None);
                copy_header.set_rx_backlink(None);
                returned_header.set_rx_successor_link(Some(copy_link));
                storage
                    .link_state
                    .layout
                    .rx_tail()
                    .write(BleLinkStateRxTail::ZERO.with_address(address(copy_target)));
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

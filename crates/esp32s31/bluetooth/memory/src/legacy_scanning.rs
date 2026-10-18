//! Legacy LE 1M scanner instances.
//!
//! One instance holds the scanner link state, the scheduler context, three
//! scheduler items and the `SCAN_REQ` transmit node of an active scanner. Hardware receives into the global scanning chain and tags
//! each packet with the number of the receiving item; the instance only names
//! that source. The items form the vendor's private free chain: link-state
//! `+0x64` holds the free head, and each item's hardware next link names the
//! next free item. An event takes the free head, as `r_ble_lll_scan_restart`
//! does, and finishing the event returns it.

#![forbid(unsafe_code)]

use vcell::VolatileCell;

use crate::{
    coexistence::{LANES_MASK, LegacyScanCoexistencePriorities, lanes_image},
    le_rx_chain::{LeRxChain, LeRxSource, LeRxTag},
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, LeTxBufferHeaderStorage, LeTxPacketAddress,
        LeTxPacketStorage,
    },
    legacy_scanning_event_image::{
        BLUETOOTH_LEGACY_SCAN_LINK_STATE_WORDS, LegacyScanLinkStateImage, LegacyScanPrimaryChannel,
        LegacyScanResetConfig, LegacyScanRxHeadProjection, LegacyScanSchedulerItemWords,
        LegacyScanSchedulerWindow, LegacyScanStartSelection, LegacyScanType, LegacyScanWindowTicks,
    },
    rx_memory_list::RxMemoryListClass,
    scheduler_context::SchedulerContextStorage,
    scheduler_item::SchedulerItemHeader,
    scheduler_pool::{
        SchedulerPoolBindError, SchedulerPoolError, SchedulerRoleInstance, SchedulerRoleKind,
        SchedulerRolePool, SchedulerRoleStorage, sealed,
    },
    sram_link::ControllerSramLinkAddress,
};

/// Scheduler items of one scanner instance.
pub const BLUETOOTH_LEGACY_SCAN_SCHEDULER_ITEM_COUNT: usize = 3;

const ITEMS: usize = BLUETOOTH_LEGACY_SCAN_SCHEDULER_ITEM_COUNT;
/// The hardware TX link at link-state `+0x00`.
const LINK_STATE_TX_WORD: usize = 0;
const LINK_STATE_TX_HEAD_WORD: usize = 0x6c / 4;
const LINK_STATE_TX_TAIL_WORD: usize = 0x74 / 4;
const LINK_STATE_RX_CLASS_WORD: usize = 0x20 / 4;
/// SOURCE: pinned `libble_app.a[ble_6.o]::r_sym_ble_aTsLUeVnygnblNndYYVB`
/// (`r_ble_lll_scan_alloc_txbuf`) gives an active scanner one TX buffer
/// whose PDU header halfword is `0x0c03`: `SCAN_REQ` with a 12-octet
/// payload. It writes no payload; the Controller inserts ScanA and AdvA.
const SCAN_REQUEST_HEADER: u8 = 0x03;
const SCAN_REQUEST_PAYLOAD_BYTES: usize = 12;
const SCAN_REQUEST_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + SCAN_REQUEST_PAYLOAD_BYTES;
const COMPRESSED_LINK_MASK: u32 = 0x000f_ffff;
const LINK_STATE_SCHEDULER_HEAD_WORD: usize = 0x64 / 4;
const LINK_STATE_RX_HEAD_WORD: usize = 0x68 / 4;
const LINK_STATE_RX_TAIL_WORD: usize = 0x70 / 4;
const LINK_STATE_RX_SWAP_RESERVE_WORD: usize = 0x78 / 4;
const SCHEDULER_ITEM_BYTES: usize = 0x60;
const SCHEDULER_ITEM_WORDS: usize = SCHEDULER_ITEM_BYTES / 4;
const SCHEDULER_ITEM_CONTEXT_WORD: usize = 1;
const SCHEDULER_ITEM_LINK_STATE_WORD: usize = 0x08 / 4;
const SCHEDULER_ITEM_WORD_14: usize = 0x14 / 4;
const SCHEDULER_ITEM_WORD_18: usize = 0x18 / 4;
const SCHEDULER_ITEM_ALLOCATION_FLAGS_WORD: usize = 0x1c / 4;
const SCHEDULER_ITEM_ALLOCATION_NUMBER_WORD: usize = 0x20 / 4;
const SCHEDULER_ITEM_COEX_PRIORITIES_WORD: usize = 0x24 / 4;
const SCHEDULER_ITEM_EVENT_CLASS_WORD: usize = 0x2c / 4;
const SCHEDULER_ITEM_ALLOCATION_PREFIX: u32 = 0x0030_0000;
/// Item `+0x08` bits 23:20: the kind of scanner item.
const SCHEDULER_ITEM_KIND_MASK: u32 = 0x00f0_0000;
const SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE: u32 = 0x0fdf_ffff;
const SCHEDULER_ITEM_EVENT_CLASS_IMAGE: u32 = 1;

/// Scanner link state.
#[repr(C, align(4))]
struct LinkStateStorage {
    words: [VolatileCell<u32>; BLUETOOTH_LEGACY_SCAN_LINK_STATE_WORDS],
}

impl LinkStateStorage {
    const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BLUETOOTH_LEGACY_SCAN_LINK_STATE_WORDS],
        }
    }

    fn clear(&self) {
        for word in &self.words {
            word.set(0);
        }
    }

    fn install(&self, image: LegacyScanLinkStateImage) {
        for (cell, word) in self.words.iter().zip(image.words()) {
            cell.set(word);
        }
    }

    fn image(&self) -> LegacyScanLinkStateImage {
        LegacyScanLinkStateImage::from_words(core::array::from_fn(|index| self.words[index].get()))
    }

    fn free_head(&self) -> u32 {
        self.words[LINK_STATE_SCHEDULER_HEAD_WORD].get()
    }

    fn set_free_head(&self, head: Option<ControllerSramLinkAddress>) {
        self.words[LINK_STATE_SCHEDULER_HEAD_WORD]
            .set(head.map_or(0, |head| head.controller_address().address()));
    }

    /// Select the scanning class and snapshot the global chain's software
    /// endpoints, as the vendor global RX-link update does.
    fn join_receive_chain(&self, (head, tail, spare): (u32, u32, u32)) {
        let class = &self.words[LINK_STATE_RX_CLASS_WORD];
        class.set(RxMemoryListClass::Scanning.select_in_link_state(class.get()));
        self.words[LINK_STATE_RX_HEAD_WORD].set(head);
        self.words[LINK_STATE_RX_TAIL_WORD].set(tail);
        self.words[LINK_STATE_RX_SWAP_RESERVE_WORD].set(spare);
    }
}

/// One scanner scheduler item.
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

    /// The common and scanner allocators' image, with the item linked to its
    /// free-chain successor.
    fn initialize(
        &self,
        number: u16,
        next_free: Option<ControllerSramLinkAddress>,
        scheduler_context: ControllerSramLinkAddress,
        link_state: ControllerSramLinkAddress,
    ) {
        for word in &self.words {
            word.set(0);
        }
        let header = self.header();
        header.set_hardware_next_word(SCHEDULER_ITEM_ALLOCATION_PREFIX);
        header.link_hardware_next(next_free);
        self.words[SCHEDULER_ITEM_CONTEXT_WORD].set(scheduler_context.compressed_image());
        self.words[SCHEDULER_ITEM_LINK_STATE_WORD]
            .set(LegacyScanType::Passive.item_kind() | link_state.compressed_image());
        self.words[SCHEDULER_ITEM_ALLOCATION_FLAGS_WORD].set(SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE);
        self.words[SCHEDULER_ITEM_ALLOCATION_NUMBER_WORD].set(u32::from(number));
        self.words[SCHEDULER_ITEM_EVENT_CLASS_WORD].set(SCHEDULER_ITEM_EVENT_CLASS_IMAGE);
    }

    /// Mark the item as the kind of scanner item `scan_type` runs.
    fn set_kind(&self, scan_type: LegacyScanType) {
        let word = &self.words[SCHEDULER_ITEM_LINK_STATE_WORD];
        word.set((word.get() & !SCHEDULER_ITEM_KIND_MASK) | scan_type.item_kind());
    }

    fn reviewed_words(&self) -> LegacyScanSchedulerItemWords {
        let header = self.header();
        LegacyScanSchedulerItemWords {
            word_00: header.hardware_next_word(),
            word_04: self.words[SCHEDULER_ITEM_CONTEXT_WORD].get(),
            word_14: self.words[SCHEDULER_ITEM_WORD_14].get(),
            word_18: self.words[SCHEDULER_ITEM_WORD_18].get(),
            word_38: header.status(),
            raw_start_word_44: header.raw_start(),
            raw_end_word_48: header.raw_end(),
        }
    }

    fn write_reviewed_words(&self, words: LegacyScanSchedulerItemWords) {
        let header = self.header();
        header.set_hardware_next_word(words.word_00);
        self.words[SCHEDULER_ITEM_CONTEXT_WORD].set(words.word_04);
        self.words[SCHEDULER_ITEM_WORD_14].set(words.word_14);
        self.words[SCHEDULER_ITEM_WORD_18].set(words.word_18);
        header.set_status(words.word_38);
        header.set_raw_start(words.raw_start_word_44);
        header.set_raw_end(words.raw_end_word_48);
    }
}

/// Controller-SRAM graph of one scanner instance.
#[repr(C)]
pub struct LegacyScanStorage {
    link_state: LinkStateStorage,
    scheduler_context: SchedulerContextStorage,
    items: [ItemStorage; ITEMS],
    scan_request_header: LeTxBufferHeaderStorage,
    scan_request_packet: LeTxPacketStorage<SCAN_REQUEST_TX_PACKET_BYTES>,
}

/// Addresses and item numbers of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct LegacyScanBinding {
    link_state: ControllerSramLinkAddress,
    scheduler_context: ControllerSramLinkAddress,
    items: [ControllerSramLinkAddress; ITEMS],
    scan_request_header: ControllerSramLinkAddress,
    scan_request_packet: LeTxPacketAddress<SCAN_REQUEST_TX_PACKET_BYTES>,
    first_number: u16,
}

impl LegacyScanBinding {
    fn item_at(&self, address: u32) -> Option<usize> {
        self.items
            .iter()
            .position(|item| item.controller_address().address() == address)
    }
}

/// Preparation state of one instance.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum LegacyScanState {
    Empty,
    Reset { event: Option<u8> },
}

impl sealed::Sealed for LegacyScanStorage {}

impl SchedulerRoleStorage for LegacyScanStorage {
    const KIND: SchedulerRoleKind = SchedulerRoleKind::LegacyScanning;
    const ITEMS: usize = ITEMS;
    const NUMBERS: usize = ITEMS;
    const NEW: Self = Self {
        link_state: LinkStateStorage::new(),
        scheduler_context: SchedulerContextStorage::new(),
        items: [const { ItemStorage::new() }; ITEMS],
        scan_request_header: LeTxBufferHeaderStorage::new(),
        scan_request_packet: LeTxPacketStorage::new(),
    };
    type Binding = LegacyScanBinding;
    type State = LegacyScanState;
    const INITIAL_STATE: LegacyScanState = LegacyScanState::Empty;

    fn bind(base: u32, first_number: u16) -> Result<LegacyScanBinding, SchedulerPoolBindError> {
        let link = |offset: usize| {
            ControllerSramLinkAddress::new(base + offset as u32)
                .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)
        };
        let items = core::mem::offset_of!(Self, items);
        let item = core::mem::size_of::<ItemStorage>();
        Ok(LegacyScanBinding {
            link_state: link(core::mem::offset_of!(Self, link_state))?,
            scheduler_context: link(core::mem::offset_of!(Self, scheduler_context))?,
            items: [link(items)?, link(items + item)?, link(items + 2 * item)?],
            scan_request_header: link(core::mem::offset_of!(Self, scan_request_header))?,
            scan_request_packet: LeTxPacketAddress::new(
                base + core::mem::offset_of!(Self, scan_request_packet) as u32,
            )
            .map_err(|_| SchedulerPoolBindError::ZeroCompressedLink)?,
            first_number,
        })
    }

    fn item_words(&self, item: usize) -> &[VolatileCell<u32>] {
        &self.items[item].words
    }

    fn item_link(binding: &LegacyScanBinding, item: usize) -> ControllerSramLinkAddress {
        binding.items[item]
    }

    fn reinitialize(&mut self, binding: &LegacyScanBinding) -> Self::State {
        self.link_state.clear();
        self.scheduler_context.clear();
        for (index, item) in self.items.iter().enumerate() {
            item.initialize(
                binding.first_number + index as u16,
                index.checked_sub(1).map(|previous| binding.items[previous]),
                binding.scheduler_context,
                binding.link_state,
            );
        }
        self.link_state
            .set_free_head(Some(binding.items[ITEMS - 1]));
        self.scan_request_header
            .initialize_bound_tx(binding.scan_request_packet);
        self.scan_request_packet = LeTxPacketStorage::new();
        LegacyScanState::Empty
    }

    fn admits(state: &LegacyScanState, item: usize) -> bool {
        matches!(state, LegacyScanState::Reset { event: Some(event) } if usize::from(*event) == item)
    }
}

/// Pool of scanner instances.
pub type LegacyScanPool<const N: usize> = SchedulerRolePool<LegacyScanStorage, N>;

/// Why a scanner operation was refused. Nothing changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyScanError {
    Pool(SchedulerPoolError),
    /// The operation does not follow the instance's preparation state.
    State,
    /// The chain receives another class.
    ForeignReceiveClass,
    /// The free chain holds no item.
    NoFreeItem,
    /// The free head names no item of this instance.
    ForeignFreeHead,
}

/// Raw timing of one scan window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanEventTiming {
    /// Raw scheduler window of the item.
    pub window: LegacyScanSchedulerWindow,
    /// Receive window length the link state records.
    pub window_ticks: LegacyScanWindowTicks,
    /// Raw sequence lead of the common scheduler projection.
    pub raw_sequence_lead: u32,
}

/// Item and raw window of one prepared scan window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegacyScanEvent {
    item: usize,
    window: LegacyScanSchedulerWindow,
}

impl LegacyScanEvent {
    /// Item that carries the window.
    pub const fn item(&self) -> usize {
        self.item
    }

    /// Raw controller window of the item.
    pub const fn raw_window(&self) -> (u32, u32) {
        (self.window.start(), self.window.end())
    }
}

impl<const N: usize> LegacyScanPool<N> {
    /// Apply the restricted LE 1M reset, join the scanning chain and, for an
    /// active scanner, queue the `SCAN_REQ` transmit node.
    pub fn reset<const PACKETS: usize>(
        &mut self,
        instance: &SchedulerRoleInstance,
        chain: &LeRxChain<PACKETS>,
        config: LegacyScanResetConfig,
    ) -> Result<(), LegacyScanError> {
        if chain.class() != RxMemoryListClass::Scanning {
            return Err(LegacyScanError::ForeignReceiveClass);
        }
        let cpu = self.cpu(instance).map_err(LegacyScanError::Pool)?;
        if !matches!(cpu.state, LegacyScanState::Empty) {
            return Err(LegacyScanError::State);
        }
        let free_head = cpu.graph.link_state.free_head();
        cpu.graph.link_state.install(
            LegacyScanLinkStateImage::restricted_le_1m(
                LegacyScanRxHeadProjection::from_bound(chain.head_link()),
                config,
            )
            .started(),
        );
        cpu.graph.link_state.words[LINK_STATE_SCHEDULER_HEAD_WORD].set(free_head);
        cpu.graph.link_state.join_receive_chain(chain.snapshot());
        // The vendor scanner allocator marks every item with the scan type's
        // kind; hardware sends SCAN_REQ only from an active item.
        for item in &cpu.graph.items {
            item.set_kind(config.scan_type());
        }
        if config.scan_type() == LegacyScanType::Active {
            // The vendor reset copies the software TX head into the
            // hardware TX link; SCAN_REQ is then the one queued packet.
            cpu.graph
                .scan_request_packet
                .prepare_pdu(SCAN_REQUEST_HEADER, &[0; SCAN_REQUEST_PAYLOAD_BYTES])
                .expect("the allocation holds the SCAN_REQ payload");
            let header = cpu.binding.scan_request_header;
            let words = &cpu.graph.link_state.words;
            words[LINK_STATE_TX_WORD].set(
                (words[LINK_STATE_TX_WORD].get() & !COMPRESSED_LINK_MASK)
                    | header.compressed_image(),
            );
            words[LINK_STATE_TX_HEAD_WORD].set(header.controller_address().address());
            words[LINK_STATE_TX_TAIL_WORD].set(header.controller_address().address());
        }
        *cpu.state = LegacyScanState::Reset { event: None };
        Ok(())
    }

    /// Lower one passive window into the free head item with its
    /// coexistence lanes.
    pub fn prepare_event(
        &mut self,
        instance: &SchedulerRoleInstance,
        channel: LegacyScanPrimaryChannel,
        timing: LegacyScanEventTiming,
        start_selection: LegacyScanStartSelection,
        coexistence: LegacyScanCoexistencePriorities,
    ) -> Result<LegacyScanEvent, LegacyScanError> {
        let LegacyScanEventTiming {
            window,
            window_ticks,
            raw_sequence_lead,
        } = timing;
        let cpu = self.cpu(instance).map_err(LegacyScanError::Pool)?;
        if !matches!(cpu.state, LegacyScanState::Reset { event: None }) {
            return Err(LegacyScanError::State);
        }
        let head = cpu.graph.link_state.free_head();
        if head == 0 {
            return Err(LegacyScanError::NoFreeItem);
        }
        let index = cpu
            .binding
            .item_at(head)
            .ok_or(LegacyScanError::ForeignFreeHead)?;
        let item = &cpu.graph.items[index];
        let next_free = item.header().hardware_next_image();
        let next_free = cpu
            .binding
            .items
            .iter()
            .copied()
            .find(|link| next_free != 0 && link.compressed_image() == next_free);

        let link_state = &cpu.graph.link_state;
        link_state.install(link_state.image().with_window(window_ticks));
        item.write_reviewed_words(item.reviewed_words().prepare_first_event(
            link_state.image(),
            channel,
            window,
            start_selection,
        ));
        // Common r_btdm_sched_calc_seq_time projection, as for every other
        // role's item: hardware ends an item without a sequence at once.
        item.header()
            .set_sequence(window.start(), window.end(), raw_sequence_lead);
        let lanes = &item.words[SCHEDULER_ITEM_COEX_PRIORITIES_WORD];
        lanes.set((lanes.get() & !LANES_MASK) | lanes_image(&coexistence.lanes));
        // Detach the item from the free chain before the executor links it.
        item.header().link_hardware_next(None);
        link_state.set_free_head(next_free);
        *cpu.state = LegacyScanState::Reset {
            event: Some(index as u8),
        };
        Ok(LegacyScanEvent {
            item: index,
            window,
        })
    }

    /// Return the item of a finished or abandoned window to the free chain.
    pub fn finish_event(
        &mut self,
        instance: &SchedulerRoleInstance,
    ) -> Result<(), LegacyScanError> {
        let cpu = self.cpu(instance).map_err(LegacyScanError::Pool)?;
        let LegacyScanState::Reset { event: Some(index) } = *cpu.state else {
            return Err(LegacyScanError::State);
        };
        let index = usize::from(index);
        let head = cpu.graph.link_state.free_head();
        let next_free = cpu
            .binding
            .item_at(head)
            .map(|head| cpu.binding.items[head]);
        let item = &cpu.graph.items[index];
        item.header().link_hardware_next(next_free);
        item.header().set_status(0);
        cpu.graph
            .link_state
            .set_free_head(Some(cpu.binding.items[index]));
        *cpu.state = LegacyScanState::Reset { event: None };
        Ok(())
    }

    /// Packets that `item` of the instance receives.
    pub fn receive_source(
        &self,
        instance: &SchedulerRoleInstance,
        item: usize,
    ) -> Result<LeRxSource, LegacyScanError> {
        let (_, binding, _) = self.shared(instance).map_err(LegacyScanError::Pool)?;
        if item >= ITEMS {
            return Err(LegacyScanError::Pool(SchedulerPoolError::NoSuchItem));
        }
        let tag = LeRxTag::new(binding.first_number + item as u16)
            .expect("the allocation numbers fit twelve bits");
        Ok(LeRxSource::new(tag, RxMemoryListClass::Scanning, false))
    }
}

#[cfg(test)]
mod tests;

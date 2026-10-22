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

use crate::layouts::*;

use crate::bluetooth::engine::{
    coexistence::{LegacyScanCoexistencePriorities, with_priorities},
    le_rx_chain::{LeRxChain, LeRxSource, LeRxTag},
    le_tx_packet::{
        BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, LeTxBufferHeaderStorage, LeTxPacketAddress,
        LeTxPacketStorage,
    },
    rx_memory_list::RxMemoryListClass,
    scanning_image::{
        LegacyScanLinkStateImage, LegacyScanRxHeadProjection, LegacyScanSchedulerItemWords,
        LegacyScanSchedulerWindow, LegacyScanWindowTicks, ScanItemImage, ScanItemKind,
        ScanResetImage,
    },
    scheduler_context::SchedulerContextStorage,
    scheduler_item::SchedulerItemHeader,
    scheduler_pool::{
        SchedulerPoolError, SchedulerRoleInstance, SchedulerRoleKind, SchedulerRolePool,
        SchedulerRoleStorage, sealed,
    },
    sram_link::ControllerSramLinkAddress,
};

/// Scheduler items of one scanner instance.
pub const BLUETOOTH_LEGACY_SCAN_SCHEDULER_ITEM_COUNT: usize = 3;

const ITEMS: usize = BLUETOOTH_LEGACY_SCAN_SCHEDULER_ITEM_COUNT;
/// The hardware TX link at link-state `+0x00`.
/// SOURCE: pinned `libble_app.a[ble_6.o]::r_sym_ble_aTsLUeVnygnblNndYYVB`
/// (`r_ble_lll_scan_alloc_txbuf`) gives an active scanner one TX buffer
/// whose PDU header halfword is `0x0c03`: `SCAN_REQ` with a 12-octet
/// payload. It writes no payload; the Controller inserts ScanA and AdvA.
const SCAN_REQUEST_HEADER: u8 = 0x03;
const SCAN_REQUEST_PAYLOAD_BYTES: usize = 12;
const SCAN_REQUEST_TX_PACKET_BYTES: usize =
    BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + SCAN_REQUEST_PAYLOAD_BYTES;
/// SOURCE: chips/esp32s31/notes/bluetooth-passive-scanning.md, scheduler allocation (bt-lib pin 10c50778).
const SCHEDULER_ITEM_ALLOCATION_PREFIX: BleSchedulerItemHardwareNext =
    BleSchedulerItemHardwareNext::ZERO
        .with_bits_20_21_opaque(0x3)
        .expect("reviewed field image fits");
/// SOURCE: chips/esp32s31/notes/bluetooth-passive-scanning.md, allocation/reset profile (bt-lib pin 10c50778).
const SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE: BleSchedulerItemWord001cOpaque =
    BleSchedulerItemWord001cOpaque::from_bits(0xfdfffff);
/// SOURCE: chips/esp32s31/notes/bluetooth-passive-scanning.md, allocation/reset profile (bt-lib pin 10c50778).
const SCHEDULER_ITEM_EVENT_CLASS_IMAGE: BleSchedulerItemReceiveWait =
    BleSchedulerItemReceiveWait::ZERO.with_value(0x1);

/// Scanner link state.
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

    fn clear(&self) {
        crate::bluetooth::engine::shared_layout::clear_link_state(&self.layout);
    }

    fn install(&self, image: LegacyScanLinkStateImage) {
        image.0.write(&self.layout);
    }

    fn image(&self) -> LegacyScanLinkStateImage {
        LegacyScanLinkStateImage(
            crate::bluetooth::engine::shared_layout::LinkStateImage::read(&self.layout),
        )
    }

    fn free_head(&self) -> u32 {
        self.layout.scheduler_head().read().bits()
    }

    fn set_free_head(&self, head: Option<ControllerSramLinkAddress>) {
        self.layout
            .scheduler_head()
            .write(BleLinkStateSchedulerHead::from_bits(
                head.map_or(0, |head| head.controller_address().address()),
            ));
    }

    /// Select the scanning class and snapshot the global chain's software
    /// endpoints, as the vendor global RX-link update does.
    fn join_receive_chain(&self, (head, tail, spare): (u32, u32, u32)) {
        let class = self.layout.rx_list();
        class.write(BleLinkStateRxList::from_bits(
            RxMemoryListClass::Scanning.select_in_link_state(class.read().bits()),
        ));
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
}

/// One scanner scheduler item.
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

    /// The common and scanner allocators' image, with the item linked to its
    /// free-chain successor.
    fn initialize(
        &self,
        number: u16,
        next_free: Option<ControllerSramLinkAddress>,
        scheduler_context: ControllerSramLinkAddress,
        link_state: ControllerSramLinkAddress,
    ) {
        crate::bluetooth::engine::shared_layout::clear_scheduler_item(&self.layout);
        let header = self.header();
        self.layout
            .hardware_next()
            .write(SCHEDULER_ITEM_ALLOCATION_PREFIX);
        header.link_hardware_next(next_free);
        self.layout
            .context()
            .write(BleSchedulerItemContext::from_bits(
                scheduler_context.compressed_image(),
            ));
        self.layout.link_state().write(
            BleSchedulerItemLinkState::from_bits(link_state.compressed_image())
                .with_kind(ScanItemKind::Listen.item_kind())
                .expect("reviewed role kind fits"),
        );
        self.layout
            .word_001c_opaque()
            .write(SCHEDULER_ITEM_ALLOCATION_FLAGS_IMAGE);
        self.layout
            .allocation_number()
            .write(BleSchedulerItemAllocationNumber::from_bits(u32::from(
                number,
            )));
        self.layout
            .receive_wait()
            .write(SCHEDULER_ITEM_EVENT_CLASS_IMAGE);
    }

    /// Mark the item as the kind of scanner item `scan_type` runs.
    fn set_kind(&self, scan_type: ScanItemKind) {
        let word = self.layout.link_state();
        word.write(
            word.read()
                .with_kind(scan_type.item_kind())
                .expect("reviewed role kind fits"),
        );
    }

    fn reviewed_words(&self) -> LegacyScanSchedulerItemWords {
        LegacyScanSchedulerItemWords {
            word_00: self.layout.hardware_next().read(),
            word_04: self.layout.context().read(),
            word_14: self.layout.rate_and_power().read(),
            word_18: self.layout.channel_and_priority().read(),
            word_38: self.layout.status().read(),
            raw_start_word_44: self.layout.raw_start().read(),
            raw_end_word_48: self.layout.raw_end().read(),
        }
    }

    fn write_reviewed_words(&self, words: LegacyScanSchedulerItemWords) {
        let header = self.header();
        header.set_hardware_next_word(words.word_00.bits());
        self.layout.context().write(words.word_04);
        self.layout.rate_and_power().write(words.word_14);
        self.layout.channel_and_priority().write(words.word_18);
        header.set_status(words.word_38.bits());
        header.set_raw_start(words.raw_start_word_44.bits());
        header.set_raw_end(words.raw_end_word_48.bits());
    }
}

/// Controller-SRAM graph of one scanner instance.
#[repr(C)]
#[derive(bytemuck::Zeroable)]
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
    const NUMBERS: usize = ITEMS;
    const KIND: SchedulerRoleKind = SchedulerRoleKind::LegacyScanning;
    const ITEMS: usize = ITEMS;
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

    fn addresses<D: crate::bluetooth::engine::ControllerDmaSpace>(
        &self,
        placement: &crate::bluetooth::engine::Placement<D>,
        first_number: u16,
    ) -> LegacyScanBinding {
        LegacyScanBinding {
            link_state: ControllerSramLinkAddress::from_placed(placement.address(&self.link_state)),
            scheduler_context: ControllerSramLinkAddress::from_placed(
                placement.address(&self.scheduler_context),
            ),
            scan_request_header: ControllerSramLinkAddress::from_placed(
                placement.address(&self.scan_request_header),
            ),
            items: core::array::from_fn(|index| {
                ControllerSramLinkAddress::from_placed(placement.address(&self.items[index]))
            }),
            scan_request_packet: LeTxPacketAddress::from_placed(
                placement,
                &self.scan_request_packet,
            ),
            first_number,
        }
    }

    fn item_layout(&self, item: usize) -> &BleSchedulerItem {
        &self.items[item].layout
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
pub type LegacyScanPool<'pool, 'epoch, const N: usize> =
    SchedulerRolePool<'pool, 'epoch, LegacyScanStorage, N>;

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

impl<'pool, 'epoch, const N: usize> LegacyScanPool<'pool, 'epoch, N> {
    /// Apply the restricted LE 1M reset, join the scanning chain and, for an
    /// active scanner, queue the `SCAN_REQ` transmit node.
    pub(crate) fn reset<const PACKETS: usize>(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        chain: &LeRxChain<PACKETS>,
        config: ScanResetImage,
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
        cpu.graph
            .link_state
            .layout
            .scheduler_head()
            .write(BleLinkStateSchedulerHead::from_bits(free_head));
        cpu.graph.link_state.join_receive_chain(chain.snapshot());
        // The vendor scanner allocator marks every item with the scan type's
        // kind; hardware sends SCAN_REQ only from an active item.
        for item in &cpu.graph.items {
            item.set_kind(config.kind);
        }
        if config.kind == ScanItemKind::RequestResponse {
            // The vendor reset copies the software TX head into the
            // hardware TX link; SCAN_REQ is then the one queued packet.
            cpu.graph
                .scan_request_packet
                .prepare_pdu(SCAN_REQUEST_HEADER, &[0; SCAN_REQUEST_PAYLOAD_BYTES])
                .expect("the allocation holds the SCAN_REQ payload");
            let header = cpu.binding.scan_request_header;
            let layout = &cpu.graph.link_state.layout;
            layout.tx_path().write(
                layout
                    .tx_path()
                    .read()
                    .with_header_from(BleLinkStateTxPath::from_bits(header.compressed_image())),
            );
            layout.tx_head().write(BleLinkStateTxHead::from_bits(
                header.controller_address().address(),
            ));
            layout.tx_tail().write(BleLinkStateTxTail::from_bits(
                header.controller_address().address(),
            ));
        }
        *cpu.state = LegacyScanState::Reset { event: None };
        Ok(())
    }

    /// Lower one selected window into the free head item with its
    /// coexistence lanes.
    pub fn prepare_event(
        &mut self,
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
        image: ScanItemImage,
        timing: LegacyScanEventTiming,
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
            image,
            window,
        ));
        // Common r_btdm_sched_calc_seq_time projection, as for every other
        // role's item: hardware ends an item without a sequence at once.
        item.header()
            .set_sequence(window.start(), window.end(), raw_sequence_lead);
        let lanes = item.layout.coexistence_priority();
        lanes.write(with_priorities(lanes.read(), &coexistence.lanes));
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
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
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
        instance: &SchedulerRoleInstance<'pool, 'epoch>,
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

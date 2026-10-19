//! Global receive chain of one ESP32-S31 RX memory list class.
//!
//! Hardware receives every packet of a class into one chain of buffer
//! headers, whatever event is running. It records the allocation number of
//! the receiving scheduler item in the packet, and each role takes only the
//! packets that carry its number. This module ports the vendor walk
//! (`r_ble_lll_get_rxed_buffer`) and return (`r_ble_lll_append_rx_buffer`)
//! over a fixed set of nodes; the vendor allocator and its reference counts
//! are not part of it.
//!
//! The chain starts as one completed packetless cursor followed by every
//! packet node, and the cursor is published once as the class's current RX
//! header. Hardware may still hold the last completed node, so a returned
//! node that is still marked current keeps its header in the chain: the
//! packet moves to a spare header, taken from a packetless header that the
//! walk removed earlier.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use oer_esp32s31_hal::bluetooth::BluetoothControllerReset;

use oer_esp32s31_hal::types::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use pin_project::pin_project;

use crate::{
    le_rx_packet::{
        LeRxBufferHeaderStorage, LeRxError, LeRxNodeStorage, LeRxOutcome, LeRxPacketAddress,
        LeRxPacketStorage,
    },
    rx_memory_list::RxMemoryListClass,
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        ControllerSramLinkAddress,
    },
};

/// Allocation number of the scheduler item that received a packet: the low
/// twelve bits of item word `+0x20`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeRxTag(u16);

impl LeRxTag {
    pub(crate) const MAX: u16 = 0x0fff;

    pub(crate) const fn new(number: u16) -> Option<Self> {
        if number <= Self::MAX {
            Some(Self(number))
        } else {
            None
        }
    }

    pub(crate) const fn number(self) -> u16 {
        self.0
    }
}

/// The packets that one role item may take from a chain: its allocation
/// number, its class and the acceptance gate of its role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeRxSource {
    tag: LeRxTag,
    class: RxMemoryListClass,
    connection: bool,
}

impl LeRxSource {
    pub(crate) const fn new(tag: LeRxTag, class: RxMemoryListClass, connection: bool) -> Self {
        Self {
            tag,
            class,
            connection,
        }
    }

    pub const fn class(self) -> RxMemoryListClass {
        self.class
    }
}

/// Receive headers and packets of one chain: the initial packetless
/// cursor and one header per packet.
#[repr(C)]
pub struct LeRxNodes<const PACKETS: usize> {
    cursor: LeRxBufferHeaderStorage,
    nodes: [LeRxNodeStorage; PACKETS],
}

impl<const PACKETS: usize> LeRxNodes<PACKETS> {
    pub const fn new() -> Self {
        Self {
            cursor: LeRxBufferHeaderStorage::new(),
            nodes: [const { LeRxNodeStorage::new() }; PACKETS],
        }
    }
}

impl<const PACKETS: usize> Default for LeRxNodes<PACKETS> {
    fn default() -> Self {
        Self::new()
    }
}

/// Pinned storage of one global receive chain.
#[pin_project]
#[repr(C)]
pub struct LeRxChainStorage<const PACKETS: usize> {
    nodes: LeRxNodes<PACKETS>,
    #[pin]
    _pin: PhantomPinned,
}

impl<const PACKETS: usize> LeRxChainStorage<PACKETS> {
    pub const fn new() -> Self {
        Self {
            nodes: LeRxNodes::new(),
            _pin: PhantomPinned,
        }
    }
}

impl<const PACKETS: usize> Default for LeRxChainStorage<PACKETS> {
    fn default() -> Self {
        Self::new()
    }
}

/// Why a receive chain cannot bind its storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeRxChainBindError {
    AddressWidth,
    InvalidAddress(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    ZeroCompressedLink,
    /// A chain needs at least two packet nodes so that one stays writable
    /// while hardware holds the other.
    TooFewPackets,
}

/// Why the chain refused to continue. The vendor asserts on each case.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeRxChainError {
    /// A completed packet still carries a rearm sentinel.
    Packet(LeRxError),
    /// A header links to storage outside this chain.
    ForeignLink,
    /// A second packetless header would enter the spare slot.
    SpareOccupied,
    /// A current node was returned without a spare header.
    NoSpare,
    /// The source receives through the other class.
    ForeignClass,
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeRxChainModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl LeRxChainModelAddress {
    pub const fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        match BluetoothControllerSramAddress::new(address) {
            Ok(address) => Ok(Self(address)),
            Err(error) => Err(error),
        }
    }
}

/// Header positions: zero is the initial cursor, `n + 1` the header of node
/// `n`.
type Header = usize;

/// Addresses and software endpoints of one receive chain.
///
/// The ring holds what the vendor keeps in link-state words `+0x68`,
/// `+0x70` and `+0x78`: the software head and tail and the spare header.
#[derive(Clone, Copy)]
pub(crate) struct LeRxRing<const PACKETS: usize> {
    cursor: ControllerSramLinkAddress,
    headers: [ControllerSramLinkAddress; PACKETS],
    packets: [LeRxPacketAddress; PACKETS],
    head: Header,
    tail: Header,
    spare: Option<Header>,
}

impl<const PACKETS: usize> LeRxRing<PACKETS> {
    /// Addresses of nodes stored at `base`.
    pub(crate) fn bind(base: u32) -> Result<Self, LeRxChainBindError> {
        if PACKETS < 2 {
            return Err(LeRxChainBindError::TooFewPackets);
        }
        let link = |offset: usize| {
            ControllerSramLinkAddress::new(base + offset as u32).map_err(|error| match error {
                crate::ControllerSramLinkAddressError::InvalidAddress(error) => {
                    LeRxChainBindError::InvalidAddress(error)
                }
                crate::ControllerSramLinkAddressError::ZeroCompressedImage => {
                    LeRxChainBindError::ZeroCompressedLink
                }
            })
        };
        let nodes = core::mem::offset_of!(LeRxNodes<PACKETS>, nodes);
        let node = core::mem::size_of::<LeRxNodeStorage>();
        let packet = core::mem::offset_of!(LeRxNodeStorage, packet);
        let mut headers = [None; PACKETS];
        let mut packets = [None; PACKETS];
        for index in 0..PACKETS {
            headers[index] = Some(link(nodes + node * index)?);
            packets[index] = Some(
                LeRxPacketAddress::new(base + (nodes + node * index + packet) as u32)
                    .map_err(LeRxChainBindError::InvalidAddress)?,
            );
        }
        Ok(Self {
            cursor: link(core::mem::offset_of!(LeRxNodes<PACKETS>, cursor))?,
            headers: headers.map(|header| header.expect("every header is bound")),
            packets: packets.map(|packet| packet.expect("every packet is bound")),
            head: 0,
            tail: PACKETS,
            spare: None,
        })
    }

    pub(crate) fn view<'ring>(
        &'ring mut self,
        nodes: &'ring LeRxNodes<PACKETS>,
    ) -> LeRxRingView<'ring, PACKETS> {
        LeRxRingView { nodes, ring: self }
    }

    fn address(&self, header: Header) -> u32 {
        self.link(header).controller_address().address()
    }

    fn link(&self, header: Header) -> ControllerSramLinkAddress {
        match header {
            0 => self.cursor,
            node => self.headers[node - 1],
        }
    }

    /// Software head, tail and spare header, as the vendor snapshots them
    /// into a link state.
    pub(crate) fn snapshot(&self) -> (u32, u32, u32) {
        (
            self.address(self.head),
            self.address(self.tail),
            self.spare.map_or(0, |spare| self.address(spare)),
        )
    }

    /// Link of the software head.
    pub(crate) fn head_link(&self) -> ControllerSramLinkAddress {
        self.link(self.head)
    }

    /// The initial packetless cursor.
    pub(crate) const fn cursor(&self) -> ControllerSramLinkAddress {
        self.cursor
    }
}

/// One ring over its nodes.
pub(crate) struct LeRxRingView<'ring, const PACKETS: usize> {
    nodes: &'ring LeRxNodes<PACKETS>,
    ring: &'ring mut LeRxRing<PACKETS>,
}

impl<const PACKETS: usize> LeRxRingView<'_, PACKETS> {
    /// Take the oldest completed packet recorded for `tag` and return its
    /// node to the chain. A private chain holds only its owner's packets and
    /// takes every packet.
    pub(crate) fn take(
        &mut self,
        tag: Option<LeRxTag>,
        connection: bool,
    ) -> Result<Option<LeRxOutcome>, LeRxChainError> {
        let Some(header) = self.find(tag)? else {
            return Ok(None);
        };
        let packet = self.packet_of(header)?;
        let outcome = self.packet(packet).received(connection);
        let outcome = outcome.map_err(LeRxChainError::Packet)?;
        self.append(header)?;
        Ok(Some(outcome))
    }

    /// The vendor walk: find the first completed node recorded for `tag`.
    fn find(&mut self, tag: Option<LeRxTag>) -> Result<Option<Header>, LeRxChainError> {
        let mut current = self.ring.head;
        self.header(current).set_predecessor(None);
        loop {
            let header = self.header(current);
            if !header.completion_observed() {
                return Ok(None);
            }
            let next = header
                .successor()
                .map(|image| self.header_at(image))
                .transpose()?;
            let next_completed = match next {
                Some(next) => {
                    self.header(next)
                        .set_predecessor(Some(self.address(current)));
                    self.header(next).completion_observed()
                }
                None => false,
            };
            self.header(current).set_rotation_marker(!next_completed);
            if header.packet_image().is_some() {
                let packet = self.packet(self.packet_of(current)?);
                if tag.is_some_and(|tag| packet.receive_tag() != tag.number()) {
                    self.header(current).set_rotation_marker(false);
                    match next {
                        Some(next) => {
                            current = next;
                            continue;
                        }
                        None => return Ok(None),
                    }
                }
                return Ok(Some(current));
            }
            // A packetless header that hardware has left behind becomes the
            // spare header.
            let Some(next) = next.filter(|_| next_completed) else {
                return Ok(None);
            };
            if self.ring.spare.is_some() {
                return Err(LeRxChainError::SpareOccupied);
            }
            self.ring.spare = Some(current);
            match self.predecessor_of(current)? {
                Some(previous) => {
                    self.header(previous).link_successor(Some(self.link(next)));
                    self.header(next)
                        .set_predecessor(Some(self.address(previous)));
                }
                None => {
                    self.header(next).set_predecessor(None);
                    self.ring.head = next;
                }
            }
            current = next;
        }
    }

    /// The vendor return: rearm the node and append it after the tail.
    fn append(&mut self, header: Header) -> Result<(), LeRxChainError> {
        let node = if self.header(header).rotates_into_successor() {
            // Hardware may still hold this header: keep it in the chain
            // without its packet and move the packet to the spare header.
            self.header(header).set_rotation_marker(false);
            let spare = self.ring.spare.take().ok_or(LeRxChainError::NoSpare)?;
            self.header(spare).copy_from(self.header(header));
            self.header(header).clear_packet();
            self.header(spare).set_predecessor(None);
            spare
        } else {
            if self.ring.head == header {
                let image = self
                    .header(header)
                    .successor()
                    .ok_or(LeRxChainError::ForeignLink)?;
                let next = self.header_at(image)?;
                self.ring.head = next;
                self.header(next).set_predecessor(None);
            }
            header
        };
        let packet = self.packet_of(node)?;
        self.packet(packet).rearm();
        self.header(node).clear_completion();
        if let Some(previous) = self.predecessor_of(node)? {
            let successor = self
                .header(node)
                .successor()
                .map(|image| self.header_at(image));
            let successor = successor.transpose()?.map(|next| self.link(next));
            self.header(previous).link_successor(successor);
        }
        self.header(node).set_predecessor(None);
        self.header(node).link_successor(None);
        self.header(self.ring.tail)
            .link_successor(Some(self.link(node)));
        self.ring.tail = node;
        Ok(())
    }

    pub(crate) fn initialize(&mut self) {
        let storage = self.nodes;
        storage
            .cursor
            .install_completed_predecessor(self.ring.headers[0]);
        for (index, node) in storage.nodes.iter().enumerate() {
            node.packet.initialize();
            let previous = if index == 0 {
                self.ring.cursor
            } else {
                self.ring.headers[index - 1]
            };
            node.header.install(
                self.ring.packets[index],
                self.ring.headers.get(index + 1).copied(),
                Some(previous.controller_address()),
                false,
            );
        }
        self.ring.head = 0;
        self.ring.tail = PACKETS;
        self.ring.spare = None;
    }

    fn header(&self, header: Header) -> &LeRxBufferHeaderStorage {
        let storage = self.nodes;
        match header {
            0 => &storage.cursor,
            node => &storage.nodes[node - 1].header,
        }
    }

    fn packet(&self, packet: usize) -> &LeRxPacketStorage {
        &self.nodes.nodes[packet].packet
    }

    fn link(&self, header: Header) -> ControllerSramLinkAddress {
        self.ring.link(header)
    }

    fn address(&self, header: Header) -> u32 {
        self.ring.address(header)
    }

    fn header_at(&self, image: u32) -> Result<Header, LeRxChainError> {
        if image == self.ring.cursor.compressed_image() {
            return Ok(0);
        }
        self.ring
            .headers
            .iter()
            .position(|header| header.compressed_image() == image)
            .map(|index| index + 1)
            .ok_or(LeRxChainError::ForeignLink)
    }

    fn predecessor_of(&self, header: Header) -> Result<Option<Header>, LeRxChainError> {
        let Some(address) = self.header(header).predecessor() else {
            return Ok(None);
        };
        (0..=PACKETS)
            .find(|candidate| self.address(*candidate) == address)
            .map(Some)
            .ok_or(LeRxChainError::ForeignLink)
    }

    fn packet_of(&self, header: Header) -> Result<usize, LeRxChainError> {
        let image = self
            .header(header)
            .packet_image()
            .ok_or(LeRxChainError::ForeignLink)?;
        self.ring
            .packets
            .iter()
            .position(|packet| packet.compressed_image() == image)
            .ok_or(LeRxChainError::ForeignLink)
    }

    #[cfg(test)]
    pub(crate) fn emulate_receive(&self, pdu: &[u8], tag: u16, captured_at: u32) -> bool {
        // Hardware writes the successor of the last completed header.
        let mut current = self.ring.head;
        loop {
            let header = self.header(current);
            let Some(next) = header
                .successor()
                .map(|image| self.header_at(image).unwrap())
            else {
                return false;
            };
            if header.completion_observed() && !self.header(next).completion_observed() {
                let packet = self.packet(self.packet_of(next).unwrap());
                packet.emulate_hardware_receive(pdu, -40, captured_at);
                packet.emulate_receive_tag(tag);
                self.header(next).emulate_hardware_completion();
                return true;
            }
            current = next;
        }
    }

    #[cfg(test)]
    fn chain(&self) -> std::vec::Vec<(Header, bool, bool)> {
        let mut order = std::vec::Vec::new();
        let mut current = Some(self.ring.head);
        while let Some(header) = current {
            let storage = self.header(header);
            order.push((
                header,
                storage.packet_image().is_some(),
                storage.completion_observed(),
            ));
            current = storage
                .successor()
                .map(|image| self.header_at(image).unwrap());
        }
        order
    }
}

/// The global receive chain of one class.
pub struct LeRxChain<const PACKETS: usize> {
    storage: Pin<&'static mut LeRxChainStorage<PACKETS>>,
    class: RxMemoryListClass,
    nodes_base: u32,
    ring: LeRxRing<PACKETS>,
}

impl<const PACKETS: usize> LeRxChain<PACKETS> {
    /// Bind static storage at its linked address.
    #[cfg(target_arch = "riscv32")]
    pub fn bind(
        storage: &'static mut LeRxChainStorage<PACKETS>,
        class: RxMemoryListClass,
    ) -> Result<Self, LeRxChainBindError> {
        let base = u32::try_from(core::ptr::addr_of!(*storage).addr())
            .map_err(|_| LeRxChainBindError::AddressWidth)?;
        Self::bind_at(storage, class, base)
    }

    /// Bind static storage at a synthetic controller address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn bind_model(
        storage: &'static mut LeRxChainStorage<PACKETS>,
        class: RxMemoryListClass,
        base: LeRxChainModelAddress,
    ) -> Result<Self, LeRxChainBindError> {
        Self::bind_at(storage, class, base.0.address())
    }

    fn bind_at(
        storage: &'static mut LeRxChainStorage<PACKETS>,
        class: RxMemoryListClass,
        base: u32,
    ) -> Result<Self, LeRxChainBindError> {
        let bytes = core::mem::size_of::<LeRxChainStorage<PACKETS>>() as u32;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || bytes > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(LeRxChainBindError::ExtentOutsidePhysicalSram);
        }
        let nodes_base = base + core::mem::offset_of!(LeRxChainStorage<PACKETS>, nodes) as u32;
        let mut chain = Self {
            ring: LeRxRing::bind(nodes_base)?,
            storage: Pin::static_mut(storage),
            class,
            nodes_base,
        };
        let storage = chain.storage.as_ref().get_ref();
        chain.ring.view(&storage.nodes).initialize();
        Ok(chain)
    }

    /// Return every node to the chain in its allocation-time order after the
    /// Controller that received into it was reset. Packets not yet taken
    /// are discarded.
    pub fn reset_after_controller_reset(&mut self, _reset: &BluetoothControllerReset) {
        self.ring = LeRxRing::bind(self.nodes_base).expect("the chain bound at this base before");
        let storage = self.storage.as_ref().get_ref();
        self.ring.view(&storage.nodes).initialize();
    }

    /// The class that hardware selects for these packets.
    pub const fn class(&self) -> RxMemoryListClass {
        self.class
    }

    pub(crate) fn snapshot(&self) -> (u32, u32, u32) {
        self.ring.snapshot()
    }

    pub(crate) fn head_link(&self) -> ControllerSramLinkAddress {
        self.ring.head_link()
    }

    /// Header to publish as the class's current RX header, once, before any
    /// event of the class runs.
    pub const fn initial_cursor(&self) -> BluetoothControllerSramAddress {
        self.ring.cursor().controller_address()
    }

    /// Take the oldest completed packet recorded for `source` and return its
    /// node to the chain.
    ///
    /// `Ok(None)` means that no completed packet for `source` precedes the
    /// first incomplete node.
    pub fn take(&mut self, source: LeRxSource) -> Result<Option<LeRxOutcome>, LeRxChainError> {
        if source.class != self.class {
            return Err(LeRxChainError::ForeignClass);
        }
        let storage = self.storage.as_ref().get_ref();
        self.ring
            .view(&storage.nodes)
            .take(Some(source.tag), source.connection)
    }

    #[cfg(test)]
    pub(crate) fn emulate_receive(&mut self, pdu: &[u8], tag: u16, captured_at: u32) -> bool {
        self.with_view(|view| view.emulate_receive(pdu, tag, captured_at))
    }

    #[cfg(test)]
    fn with_view<R>(&mut self, f: impl FnOnce(&mut LeRxRingView<'_, PACKETS>) -> R) -> R {
        let storage = self.storage.as_ref().get_ref();
        f(&mut self.ring.view(&storage.nodes))
    }
}

#[cfg(test)]
mod tests;

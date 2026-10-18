//! Shared ESP32-S31 LE receive-packet SRAM codec.
//!
//! Role graphs own allocation topology and publication. This module owns only
//! the common controller buffer header, packet prefix and bounded semantic
//! extraction used by scanning and response-capable advertising.

#![forbid(unsafe_code)]

use crate::sram_link::ControllerSramLinkAddress;

use oer_esp32s31_hal::types::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};

use vcell::VolatileCell;

/// Bytes preceding a received Link Layer PDU in one controller allocation.
pub const BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES: usize = 0x1e;
/// Maximum Link Layer payload admitted by the shared receive allocation.
pub const BLUETOOTH_LE_RX_PAYLOAD_CAPACITY: usize = u8::MAX as usize;
/// Complete logical receive-packet allocation size.
pub const BLUETOOTH_LE_RX_PACKET_BYTES: usize =
    BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES + BLUETOOTH_LE_RX_PAYLOAD_CAPACITY;

const BUFFER_HEADER_BYTES: usize = 0x18;
const BUFFER_HEADER_WORDS: usize = BUFFER_HEADER_BYTES / 4;
const RX_PACKET_WORDS: usize = BLUETOOTH_LE_RX_PACKET_BYTES.div_ceil(4);
const RX_PACKET_LAST_ALIGNED_OFFSET: u32 = ((BLUETOOTH_LE_RX_PACKET_BYTES as u32 - 1) / 4) * 4;

/// Opaque raw-controller-time observation attached to one received LE packet.
///
/// The value is not scheduler time and is not yet the on-air packet start. It
/// can enter only the chip-private epoch and PHY-calibration operation before
/// protocol timing consumes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePacketCapturedTime(u32);

impl LePacketCapturedTime {
    fn from_controller_sram_word(word: u32) -> Self {
        Self(word)
    }

    /// Borrow the wrapping tick position for the chip-private epoch projector.
    #[doc(hidden)]
    pub const fn wrapping_controller_ticks(self) -> u32 {
        self.0
    }
}

/// One bounded Link Layer PDU copied from a completed controller packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeReceivedPdu {
    bytes: [u8; BLUETOOTH_LE_RX_PAYLOAD_CAPACITY + 2],
    length: u16,
    rssi_dbm: i8,
    captured_time: LePacketCapturedTime,
}

impl LeReceivedPdu {
    /// Construct one semantic host-model packet without emulating SRAM fields.
    #[cfg(not(target_arch = "riscv32"))]
    #[doc(hidden)]
    pub fn from_parts_for_validation(pdu: &[u8], rssi_dbm: i8, captured_time: u32) -> Option<Self> {
        if pdu.len() < 2
            || pdu.len() > BLUETOOTH_LE_RX_PAYLOAD_CAPACITY + 2
            || usize::from(pdu[1]) + 2 != pdu.len()
        {
            return None;
        }
        let mut bytes = [0; BLUETOOTH_LE_RX_PAYLOAD_CAPACITY + 2];
        bytes[..pdu.len()].copy_from_slice(pdu);
        Some(Self {
            bytes,
            length: pdu.len() as u16,
            rssi_dbm,
            captured_time: LePacketCapturedTime::from_controller_sram_word(captured_time),
        })
    }

    /// Complete two-byte Link Layer header and declared payload.
    pub const fn as_bytes(&self) -> &[u8] {
        self.bytes.split_at(self.length as usize).0
    }

    /// Number of copied Link Layer PDU octets.
    pub const fn len(&self) -> usize {
        self.length as usize
    }

    /// Whether the copied PDU is empty. A valid hardware result is never empty.
    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Signed receive-strength byte supplied by the controller packet prefix.
    pub const fn rssi_dbm(&self) -> i8 {
        self.rssi_dbm
    }

    /// Controller-time observation captured by hardware for this exact PDU.
    pub const fn captured_time(&self) -> LePacketCapturedTime {
        self.captured_time
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LeRxPacketError {
    ProducerSentinelRetained,
    EpochSentinelRetained,
}

/// Semantic outcome of the private controller receive-result gate.
///
/// Only `Dispatchable` carries a copied PDU. The raw controller status and its
/// positional descriptor encoding remain owned by this private codec.
enum LeRxPacketDisposition {
    Dispatchable,
    Discarded,
}

/// Result of one completed receive packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "the copied PDU is returned by value; no allocator exists below the controller"
)]
pub enum LeRxOutcome {
    /// A dispatchable PDU.
    Received(LeReceivedPdu),
    /// A completed observation rejected before PDU dispatch.
    Discarded,
}

/// Malformed completed LE receive storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeRxError {
    /// A completed header still points at an untouched producer sentinel.
    ProducerSentinelRetained,
    /// A completed header still points at an untouched receive-epoch sentinel.
    EpochSentinelRetained,
}

impl From<LeRxPacketError> for LeRxError {
    fn from(error: LeRxPacketError) -> Self {
        match error {
            LeRxPacketError::ProducerSentinelRetained => Self::ProducerSentinelRetained,
            LeRxPacketError::EpochSentinelRetained => Self::EpochSentinelRetained,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LeRxPacketAddress(BluetoothControllerSramAddress);

impl LeRxPacketAddress {
    pub(crate) fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        let address = BluetoothControllerSramAddress::new(address)?;
        let tail = address
            .address()
            .checked_add(RX_PACKET_LAST_ALIGNED_OFFSET)
            .ok_or(BluetoothControllerSramAddressError::OutsideEncodableWindow)?;
        BluetoothControllerSramAddress::new(tail)?;
        Ok(Self(address))
    }

    pub(crate) const fn compressed_image(self) -> u32 {
        self.0.compressed_image()
    }
}

/// Private controller-shared receive-buffer header.
#[repr(C, align(4))]
pub(crate) struct LeRxBufferHeaderStorage {
    words: [VolatileCell<u32>; BUFFER_HEADER_WORDS],
}

impl LeRxBufferHeaderStorage {
    const LINK_MASK: u32 = 0x000f_ffff;
    const ROTATION_MARKER: u32 = 1;
    const COMPLETION_GATE: u32 = 1 << 31;

    pub(crate) const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BUFFER_HEADER_WORDS],
        }
    }

    pub(crate) fn install(
        &self,
        packet: LeRxPacketAddress,
        successor: Option<ControllerSramLinkAddress>,
        predecessor: Option<BluetoothControllerSramAddress>,
        rotates_into_successor: bool,
    ) {
        let successor = successor.map_or(0, ControllerSramLinkAddress::compressed_image);
        let predecessor = predecessor.map_or(0, BluetoothControllerSramAddress::address);
        let rotation = if rotates_into_successor {
            Self::ROTATION_MARKER
        } else {
            0
        };
        let image = [
            successor,
            packet.compressed_image(),
            0x8080_0000,
            0,
            rotation,
            predecessor,
        ];
        for (cell, word) in self.words.iter().zip(image) {
            cell.set(word);
        }
    }

    pub(crate) fn completion_observed(&self) -> bool {
        self.words[3].get() & Self::COMPLETION_GATE != 0
    }

    /// Initial global RX cursor: an already completed header without a packet.
    /// The controller obtains writable storage by following its successor.
    pub(crate) fn install_completed_predecessor(&self, successor: ControllerSramLinkAddress) {
        let image = [
            successor.compressed_image(),
            0,
            0x8080_0000,
            Self::COMPLETION_GATE,
            0,
            0,
        ];
        for (cell, word) in self.words.iter().zip(image) {
            cell.set(word);
        }
    }

    #[cfg(test)]
    pub(crate) fn emulate_hardware_completion(&self) {
        self.words[3].set(self.words[3].get() | Self::COMPLETION_GATE);
    }

    pub(crate) fn successor(&self) -> Option<u32> {
        let image = self.words[0].get() & Self::LINK_MASK;
        (image != 0).then_some(image)
    }

    pub(crate) fn predecessor(&self) -> Option<u32> {
        let address = self.words[5].get();
        (address != 0).then_some(address)
    }

    pub(crate) fn rotates_into_successor(&self) -> bool {
        self.words[4].get() & Self::ROTATION_MARKER != 0
    }

    /// Set or clear the rotation marker in the `+0x10` halfword.
    pub(crate) fn set_rotation_marker(&self, marked: bool) {
        let word = self.words[4].get() & !Self::ROTATION_MARKER;
        self.words[4].set(if marked {
            word | Self::ROTATION_MARKER
        } else {
            word
        });
    }

    /// Replace the successor link while preserving bits 31:20.
    pub(crate) fn link_successor(&self, successor: Option<ControllerSramLinkAddress>) {
        let image = successor.map_or(0, ControllerSramLinkAddress::compressed_image);
        self.words[0].set((self.words[0].get() & !Self::LINK_MASK) | image);
    }

    /// Compressed packet link, or `None` for a packetless header.
    pub(crate) fn packet_image(&self) -> Option<u32> {
        let image = self.words[1].get() & Self::LINK_MASK;
        (image != 0).then_some(image)
    }

    /// Detach the packet while preserving bits 31:20.
    pub(crate) fn clear_packet(&self) {
        self.words[1].set(self.words[1].get() & !Self::LINK_MASK);
    }

    pub(crate) fn clear_completion(&self) {
        self.words[3].set(self.words[3].get() & !Self::COMPLETION_GATE);
    }

    /// Store the full address of the predecessor, or zero.
    pub(crate) fn set_predecessor(&self, predecessor: Option<u32>) {
        self.words[5].set(predecessor.unwrap_or(0));
    }

    /// Copy the complete header image from `other`.
    pub(crate) fn copy_from(&self, other: &Self) {
        for (cell, word) in self.words.iter().zip(&other.words) {
            cell.set(word.get());
        }
    }
}

/// Private controller-shared receive packet allocation.
#[repr(C, align(4))]
pub(crate) struct LeRxPacketStorage {
    words: [VolatileCell<u32>; RX_PACKET_WORDS],
}

impl LeRxPacketStorage {
    const CAPACITY_WORD: usize = 1;
    const RESULT_WORD: usize = 3;
    const CAPTURED_TIME_WORD: usize = 4;
    const EPOCH_WORD: usize = 6;
    const RESULT_REARM_SENTINEL: u32 = 0x00ff_ffff;
    const EPOCH_REARM_SENTINEL: u32 = 0x0000_ffff;
    const CAPACITY_IMAGE: u32 = 0x0001_0100;
    const UPPER_DISPATCH_BLOCKING_FLAGS: u32 = 0x0000_0601;

    pub(crate) const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; RX_PACKET_WORDS],
        }
    }

    pub(crate) fn initialize(&self) {
        for word in &self.words {
            word.set(0);
        }
        self.words[Self::CAPACITY_WORD].set(Self::CAPACITY_IMAGE);
        self.rearm();
    }

    pub(crate) fn rearm(&self) {
        self.words[Self::RESULT_WORD]
            .set(self.words[Self::RESULT_WORD].get() | Self::RESULT_REARM_SENTINEL);
        self.words[Self::EPOCH_WORD]
            .set(self.words[Self::EPOCH_WORD].get() | Self::EPOCH_REARM_SENTINEL);
    }

    fn disposition(&self, connection: bool) -> Result<LeRxPacketDisposition, LeRxPacketError> {
        let result = self.words[Self::RESULT_WORD].get();
        if result & Self::RESULT_REARM_SENTINEL == Self::RESULT_REARM_SENTINEL {
            return Err(LeRxPacketError::ProducerSentinelRetained);
        }
        // Connection RX has a different acceptance gate from advertising.
        // Current vendor conn_rx_process rejects result flags 0x17.
        let blocking = if connection {
            0x17
        } else {
            Self::UPPER_DISPATCH_BLOCKING_FLAGS
        };
        if result & blocking != 0 {
            return Ok(LeRxPacketDisposition::Discarded);
        }
        Ok(LeRxPacketDisposition::Dispatchable)
    }

    fn copy_dispatchable_pdu(&self) -> Result<LeReceivedPdu, LeRxPacketError> {
        let epoch = self.words[Self::EPOCH_WORD].get();
        if epoch & Self::EPOCH_REARM_SENTINEL == Self::EPOCH_REARM_SENTINEL {
            return Err(LeRxPacketError::EpochSentinelRetained);
        }

        let payload_length = self.read_byte(BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES - 1);
        let length = usize::from(payload_length) + 2;
        let mut bytes = [0; BLUETOOTH_LE_RX_PAYLOAD_CAPACITY + 2];
        let mut index = 0;
        while index < length {
            bytes[index] = self.read_byte(BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES - 2 + index);
            index += 1;
        }
        Ok(LeReceivedPdu {
            bytes,
            length: length as u16,
            rssi_dbm: self.read_byte(BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES - 15) as i8,
            captured_time: LePacketCapturedTime::from_controller_sram_word(
                self.words[Self::CAPTURED_TIME_WORD].get(),
            ),
        })
    }

    fn read_byte(&self, offset: usize) -> u8 {
        let word = self.words[offset / 4].get();
        ((word >> ((offset % 4) * 8)) & 0xff) as u8
    }

    #[cfg(test)]
    pub(crate) fn emulate_hardware_receive(&self, pdu: &[u8], rssi_dbm: i8, captured_time: u32) {
        assert!((2..=BLUETOOTH_LE_RX_PAYLOAD_CAPACITY + 2).contains(&pdu.len()));
        assert_eq!(usize::from(pdu[1]) + 2, pdu.len());
        self.words[Self::RESULT_WORD].set(0);
        self.words[Self::CAPTURED_TIME_WORD].set(captured_time);
        self.words[Self::EPOCH_WORD].set(0);
        for (offset, byte) in pdu.iter().copied().enumerate() {
            self.write_byte(BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES - 2 + offset, byte);
        }
        self.write_byte(BLUETOOTH_LE_RX_PACKET_PREFIX_BYTES - 15, rssi_dbm as u8);
    }

    #[cfg(test)]
    pub(crate) fn emulate_hardware_discard(&self) {
        self.words[Self::RESULT_WORD].set(Self::UPPER_DISPATCH_BLOCKING_FLAGS);
    }

    #[cfg(test)]
    fn write_byte(&self, offset: usize, value: u8) {
        let shift = (offset % 4) * 8;
        let word = self.words[offset / 4].get();
        self.words[offset / 4].set((word & !(0xff << shift)) | (u32::from(value) << shift));
    }

    /// Allocation number that hardware recorded in the `+0x18` halfword.
    pub(crate) fn receive_tag(&self) -> u16 {
        (self.words[Self::EPOCH_WORD].get() & Self::EPOCH_REARM_SENTINEL) as u16
    }

    /// Copy the result of a completed packet.
    pub(crate) fn received(&self, connection: bool) -> Result<LeRxOutcome, LeRxError> {
        Ok(match self.disposition(connection)? {
            LeRxPacketDisposition::Dispatchable => {
                LeRxOutcome::Received(self.copy_dispatchable_pdu()?)
            }
            LeRxPacketDisposition::Discarded => LeRxOutcome::Discarded,
        })
    }

    #[cfg(test)]
    pub(crate) fn emulate_receive_tag(&self, tag: u16) {
        let word = self.words[Self::EPOCH_WORD].get() & !Self::EPOCH_REARM_SENTINEL;
        self.words[Self::EPOCH_WORD].set(word | u32::from(tag));
    }
}

#[repr(C)]
pub(crate) struct LeRxNodeStorage {
    pub(crate) header: LeRxBufferHeaderStorage,
    pub(crate) packet: LeRxPacketStorage,
}

impl LeRxNodeStorage {
    pub(crate) const fn new() -> Self {
        Self {
            header: LeRxBufferHeaderStorage::new(),
            packet: LeRxPacketStorage::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_rejections_are_discarded_and_accepted_pdus_are_copied() {
        let packet = LeRxPacketStorage::new();
        let feature_req = [3, 9, 8, 0, 0, 0, 0, 0, 0, 0, 0];
        packet.initialize();
        packet.emulate_hardware_receive(&feature_req, -40, 123);
        let Ok(LeRxOutcome::Received(pdu)) = packet.received(true) else {
            panic!("an accepted connection PDU is dispatchable")
        };
        assert_eq!(pdu.as_bytes(), feature_req);
        // Each independently rejected controller outcome. These are
        // packet-result stimuli, not MMIO or generated-layout assertions.
        for rejected_result in [1, 2, 4, 16] {
            packet.words[LeRxPacketStorage::RESULT_WORD].set(rejected_result);
            assert_eq!(packet.received(true), Ok(LeRxOutcome::Discarded));
        }
        packet.rearm();
        assert_eq!(
            packet.received(true),
            Err(LeRxError::ProducerSentinelRetained)
        );
    }
}

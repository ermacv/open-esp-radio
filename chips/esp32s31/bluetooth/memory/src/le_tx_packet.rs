//! Common ESP32-S31 LE controller TX-packet allocation.
//!
//! Named same-chip `r_ble_lll_mmgmt_alloc_tx_buffer_and_hdr` supplies this
//! allocation to both DTM and advertising. The role-specific producer owns
//! the Link Layer header and payload; this module alone owns their placement
//! inside controller SRAM. Preparing bytes here does not publish an address or
//! grant scheduler ownership.

#![forbid(unsafe_code)]

use crate::sram_link::ControllerSramLinkAddress;

use oer_esp32s31_hal::types::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};

use vcell::VolatileCell;

/// Bytes in one common controller RX/TX buffer header.
pub const BLUETOOTH_LE_BUFFER_HEADER_BYTES: usize = 0x18;
/// Bytes preceding the maximum Link Layer payload in a controller TX allocation.
pub const BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES: usize = 0x12;

const CONTROLLER_METADATA_BYTES: usize = 0x10;
const PDU_HEADER_BYTE: usize = CONTROLLER_METADATA_BYTES;
const PDU_LENGTH_BYTE: usize = CONTROLLER_METADATA_BYTES + 1;
const ALLOCATION_CLASS_BYTE: usize = 0x05;
const ALLOCATION_STATE_BYTE: usize = 0x06;
const HEADER_PACKET_TARGET_OFFSET: u32 = CONTROLLER_METADATA_BYTES as u32;

/// Why a complete LE TX packet extent cannot inhabit controller SRAM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LeTxPacketAddressError {
    InvalidBase(BluetoothControllerSramAddressError),
    ZeroCompressedBase,
    UnsupportedAllocationSize,
    ExtentOutsideControllerSram,
}

/// Validated controller-SRAM geometry for one complete LE TX packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LeTxPacketAddress<const ALLOCATION_BYTES: usize> {
    base: BluetoothControllerSramAddress,
    header_packet_target: BluetoothControllerSramAddress,
}

impl<const ALLOCATION_BYTES: usize> LeTxPacketAddress<ALLOCATION_BYTES> {
    pub(super) const fn new(address: u32) -> Result<Self, LeTxPacketAddressError> {
        if ALLOCATION_BYTES < BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES
            || ALLOCATION_BYTES - BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES > u8::MAX as usize
            || ALLOCATION_BYTES > u32::MAX as usize
        {
            return Err(LeTxPacketAddressError::UnsupportedAllocationSize);
        }
        let base = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => return Err(LeTxPacketAddressError::InvalidBase(error)),
        };
        if base.compressed_image() == 0 {
            return Err(LeTxPacketAddressError::ZeroCompressedBase);
        }
        let header_packet_target_address = match address.checked_add(HEADER_PACKET_TARGET_OFFSET) {
            Some(address) => address,
            None => return Err(LeTxPacketAddressError::ExtentOutsideControllerSram),
        };
        let last_aligned_offset = ((ALLOCATION_BYTES as u32 - 1) / 4) * 4;
        let last_aligned_address = match address.checked_add(last_aligned_offset) {
            Some(address) => address,
            None => return Err(LeTxPacketAddressError::ExtentOutsideControllerSram),
        };
        let header_packet_target =
            match BluetoothControllerSramAddress::new(header_packet_target_address) {
                Ok(address) => address,
                Err(_) => {
                    return Err(LeTxPacketAddressError::ExtentOutsideControllerSram);
                }
            };
        if BluetoothControllerSramAddress::new(last_aligned_address).is_err() {
            return Err(LeTxPacketAddressError::ExtentOutsideControllerSram);
        }
        Ok(Self {
            base,
            header_packet_target,
        })
    }

    pub(super) const fn base_link(self) -> LeTxPacketBaseLink {
        LeTxPacketBaseLink(self.base.compressed_image())
    }

    pub(super) const fn pdu_target_link(self) -> LeTxPduTargetLink {
        LeTxPduTargetLink(self.header_packet_target.compressed_image())
    }

    const fn allocation_extent_image(self) -> u32 {
        ((ALLOCATION_BYTES - BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES) as u32) << 3
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LeTxPacketBaseLink(u32);

impl LeTxPacketBaseLink {
    fn from_image(image: u32) -> Option<Self> {
        (image != 0).then_some(Self(image))
    }

    const fn image(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LeTxPduTargetLink(u32);

impl LeTxPduTargetLink {
    fn from_image(image: u32) -> Option<Self> {
        (image != 0).then_some(Self(image))
    }

    const fn image(self) -> u32 {
        self.0
    }
}

/// CPU-owned common TX buffer header before graph publication.
#[repr(C, align(4))]
pub(super) struct LeTxBufferHeaderStorage {
    words: [VolatileCell<u32>; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
}

impl LeTxBufferHeaderStorage {
    const COMPRESSED_LINK_MASK: u32 = 0x000f_ffff;
    const ALLOCATION_EXTENT_MASK: u32 = 0x0000_07f8;

    pub(super) const fn new() -> Self {
        Self {
            words: [const { VolatileCell::new(0) }; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4],
        }
    }

    fn read_word(&self, index: usize) -> u32 {
        self.words[index].get()
    }

    fn install(&self, words: [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4]) {
        for (cell, word) in self.words.iter().zip(words) {
            cell.set(word);
        }
    }

    pub(super) fn initialize_bound_tx<const ALLOCATION_BYTES: usize>(
        &self,
        packet: LeTxPacketAddress<ALLOCATION_BYTES>,
    ) {
        self.initialize_bound_tx_with_successor(packet, None);
    }

    pub(super) fn initialize_bound_tx_with_successor<const ALLOCATION_BYTES: usize>(
        &self,
        packet: LeTxPacketAddress<ALLOCATION_BYTES>,
        successor: Option<ControllerSramLinkAddress>,
    ) {
        self.install([
            successor.map_or(0, ControllerSramLinkAddress::compressed_image),
            packet.base_link().image(),
            0x80a0_0000 | packet.pdu_target_link().image(),
            0,
            packet.allocation_extent_image(),
            0,
        ]);
    }

    /// Completed packetless cursor retained by a connection TX list.
    pub(super) fn initialize_empty_cursor(&self) {
        self.install([0, 0, 0, 0x8000_0000, 2, 0]);
    }

    #[cfg(test)]
    pub(super) fn successor_image(&self) -> u32 {
        self.read_word(0) & Self::COMPRESSED_LINK_MASK
    }

    pub(super) fn link_successor(&self, successor: ControllerSramLinkAddress) {
        self.words[0]
            .set((self.read_word(0) & !Self::COMPRESSED_LINK_MASK) | successor.compressed_image());
    }

    pub(super) fn transmission_completed(&self) -> bool {
        self.read_word(3) & 0x8000_0000 != 0
    }

    pub(super) fn mark_complete_connection_packet(&self, llid: u8) {
        assert!(
            (1..=3).contains(&llid),
            "connection TX requires a valid LLID"
        );
        // Software completion metadata: end of packet plus the two-bit LLID.
        self.words[4].set(self.read_word(4) | u32::from(llid << 1));
    }

    /// Retain the hardware cursor after acknowledging the packet to software.
    pub(super) fn release_completed_packet(&self) {
        self.words[1].set(self.read_word(1) & !Self::COMPRESSED_LINK_MASK);
    }

    #[cfg(test)]
    pub(super) fn model_complete_transmission(&self) {
        self.words[3].set(self.read_word(3) | 0x8000_0000);
    }

    pub(super) fn packet_base_link(&self) -> Option<LeTxPacketBaseLink> {
        LeTxPacketBaseLink::from_image(self.read_word(1) & Self::COMPRESSED_LINK_MASK)
    }

    pub(super) fn pdu_target_link(&self) -> Option<LeTxPduTargetLink> {
        LeTxPduTargetLink::from_image(self.read_word(2) & Self::COMPRESSED_LINK_MASK)
    }

    pub(super) fn retains_allocation_extent<const ALLOCATION_BYTES: usize>(
        &self,
        packet: LeTxPacketAddress<ALLOCATION_BYTES>,
    ) -> bool {
        self.read_word(4) & Self::ALLOCATION_EXTENT_MASK == packet.allocation_extent_image()
    }

    #[cfg(test)]
    pub(super) fn snapshot(&self) -> [u32; BLUETOOTH_LE_BUFFER_HEADER_BYTES / 4] {
        core::array::from_fn(|index| self.read_word(index))
    }
}

/// Why a Link Layer PDU cannot be installed in one controller TX allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeTxPacketPrepareError {
    /// The storage type cannot contain even the reviewed controller prefix.
    AllocationTooSmall { available: usize },
    /// The supplied payload exceeds this allocation or the LE length field.
    PayloadTooLong { length: usize, capacity: usize },
    /// An encoded Link Layer PDU omitted its two-byte header.
    EncodedPduTooShort { available: usize },
    /// The encoded length field does not describe all supplied payload bytes.
    EncodedPduLengthMismatch { declared: u8, actual: usize },
}

/// Complete encoded PDU proved to fit one controller allocation class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LeTxPacketPreparedInput<'a, const ALLOCATION_BYTES: usize> {
    pdu: &'a [u8],
    payload_length: u8,
}

impl<'a, const ALLOCATION_BYTES: usize> LeTxPacketPreparedInput<'a, ALLOCATION_BYTES> {
    pub(super) fn new(pdu: &'a [u8]) -> Result<Self, LeTxPacketPrepareError> {
        if ALLOCATION_BYTES < BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES {
            return Err(LeTxPacketPrepareError::AllocationTooSmall {
                available: ALLOCATION_BYTES,
            });
        }
        if pdu.len() < 2 {
            return Err(LeTxPacketPrepareError::EncodedPduTooShort {
                available: pdu.len(),
            });
        }
        let actual = pdu.len() - 2;
        let declared = pdu[1];
        if usize::from(declared) != actual {
            return Err(LeTxPacketPrepareError::EncodedPduLengthMismatch { declared, actual });
        }
        let capacity = ALLOCATION_BYTES
            .saturating_sub(BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES)
            .min(u8::MAX as usize);
        if actual > capacity {
            return Err(LeTxPacketPrepareError::PayloadTooLong {
                length: actual,
                capacity,
            });
        }
        Ok(Self {
            pdu,
            payload_length: declared,
        })
    }

    pub(super) const fn from_validated_encoded_pdu(pdu: &'a [u8], payload_length: u8) -> Self {
        Self {
            pdu,
            payload_length,
        }
    }

    pub(super) const fn as_bytes(self) -> &'a [u8] {
        self.pdu
    }

    pub(super) const fn payload_bytes(self) -> u8 {
        self.payload_length
    }
}

/// Opaque length accepted by the controller TX-packet codec.
///
/// This value proves only that the length fits its allocation-size class;
/// hardware visibility and allocation identity remain owned by the enclosing
/// graph typestate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeTxPacketPreparedLength<const ALLOCATION_BYTES: usize>(u8);

impl<const ALLOCATION_BYTES: usize> LeTxPacketPreparedLength<ALLOCATION_BYTES> {
    /// Number of Link Layer payload bytes following the two-byte PDU header.
    pub const fn payload_bytes(self) -> u8 {
        self.0
    }

    /// Complete Link Layer PDU size, including its two-byte header.
    pub const fn pdu_bytes(self) -> usize {
        2 + self.0 as usize
    }
}

/// CPU-owned storage for one common ESP32-S31 LE controller TX allocation.
///
/// `ALLOCATION_BYTES` includes the complete controller prefix. A role chooses
/// the smallest capacity it needs; DTM uses all 255 LE payload octets while
/// legacy advertising needs at most 37.
#[repr(C, align(4))]
#[derive(Debug, Eq, PartialEq)]
pub struct LeTxPacketStorage<const ALLOCATION_BYTES: usize> {
    bytes: [u8; ALLOCATION_BYTES],
}

impl<const ALLOCATION_BYTES: usize> LeTxPacketStorage<ALLOCATION_BYTES> {
    /// Reserve a zero-based, CPU-owned allocation.
    pub const fn new() -> Self {
        Self {
            bytes: [0; ALLOCATION_BYTES],
        }
    }

    /// Maximum Link Layer payload accepted by this allocation.
    pub const fn payload_capacity() -> usize {
        ALLOCATION_BYTES.saturating_sub(BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES)
    }

    /// Install a role-produced PDU header and its complete payload.
    pub fn prepare_pdu(
        &mut self,
        header: u8,
        payload: &[u8],
    ) -> Result<LeTxPacketPreparedLength<ALLOCATION_BYTES>, LeTxPacketPrepareError> {
        if ALLOCATION_BYTES < BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES {
            return Err(LeTxPacketPrepareError::AllocationTooSmall {
                available: ALLOCATION_BYTES,
            });
        }

        let capacity = Self::payload_capacity().min(u8::MAX as usize);
        if payload.len() > capacity {
            return Err(LeTxPacketPrepareError::PayloadTooLong {
                length: payload.len(),
                capacity,
            });
        }

        let length = payload.len() as u8;
        self.bytes[ALLOCATION_CLASS_BYTE] = 2;
        self.bytes[ALLOCATION_STATE_BYTE] = 0;
        self.bytes[PDU_HEADER_BYTE] = header;
        self.bytes[PDU_LENGTH_BYTE] = length;
        self.bytes[BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES..][..payload.len()].copy_from_slice(payload);
        Ok(LeTxPacketPreparedLength(length))
    }

    /// Install one complete, already encoded two-byte-header Link Layer PDU.
    pub fn prepare_encoded_pdu(
        &mut self,
        pdu: &[u8],
    ) -> Result<LeTxPacketPreparedLength<ALLOCATION_BYTES>, LeTxPacketPrepareError> {
        let packet = LeTxPacketPreparedInput::new(pdu)?;
        Ok(self.prepare_validated_encoded_pdu(packet))
    }

    pub(super) fn prepare_validated_encoded_pdu(
        &mut self,
        packet: LeTxPacketPreparedInput<'_, ALLOCATION_BYTES>,
    ) -> LeTxPacketPreparedLength<ALLOCATION_BYTES> {
        let pdu = packet.as_bytes();
        let length = packet.payload_bytes();
        self.bytes[ALLOCATION_CLASS_BYTE] = 2;
        self.bytes[ALLOCATION_STATE_BYTE] = 0;
        self.bytes[PDU_HEADER_BYTE] = pdu[0];
        self.bytes[PDU_LENGTH_BYTE] = length;
        self.bytes[BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES..][..usize::from(length)]
            .copy_from_slice(&pdu[2..]);
        LeTxPacketPreparedLength(length)
    }

    /// Legacy advertising hardware inserts AdvA before consuming these bytes.
    /// Keep its on-air length, while moving only advertising data after the header.
    pub(super) fn omit_legacy_advertiser_address(
        &mut self,
        length: LeTxPacketPreparedLength<ALLOCATION_BYTES>,
    ) {
        let payload_length = usize::from(length.payload_bytes());
        assert!(payload_length >= 6, "the advertising role supplies AdvA");
        let start = BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES;
        self.bytes
            .copy_within(start + 6..start + payload_length, start);
        self.bytes[start + payload_length - 6..start + payload_length].fill(0);
    }

    #[cfg(test)]
    pub(super) fn pdu_header(&self) -> [u8; 2] {
        [self.bytes[PDU_HEADER_BYTE], self.bytes[PDU_LENGTH_BYTE]]
    }

    /// Borrow the semantic Link Layer PDU installed by the enclosing owner.
    pub fn prepared_pdu(&self, length: LeTxPacketPreparedLength<ALLOCATION_BYTES>) -> &[u8] {
        &self.bytes[CONTROLLER_METADATA_BYTES..][..length.pdu_bytes()]
    }

    pub(super) fn prepared_allocation(
        &self,
        length: LeTxPacketPreparedLength<ALLOCATION_BYTES>,
    ) -> &[u8] {
        &self.bytes[..BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES + usize::from(length.payload_bytes())]
    }

    #[cfg(test)]
    pub(super) fn model_pdu(&self) -> &[u8] {
        &self.bytes[CONTROLLER_METADATA_BYTES..][..2 + usize::from(self.bytes[PDU_LENGTH_BYTE])]
    }

    pub(super) fn clear(&mut self) {
        self.bytes.fill(0);
    }

    #[cfg(test)]
    pub(super) const fn snapshot(&self) -> [u8; ALLOCATION_BYTES] {
        self.bytes
    }
}

impl<const ALLOCATION_BYTES: usize> Default for LeTxPacketStorage<ALLOCATION_BYTES> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

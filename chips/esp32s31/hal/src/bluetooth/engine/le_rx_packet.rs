//! Shared ESP32-S31 LE receive-packet SRAM codec.
//!
//! Role graphs own allocation topology and publication. This module owns only
//! the common controller buffer header, packet prefix and bounded semantic
//! extraction used by scanning and response-capable advertising.

#![forbid(unsafe_code)]

use crate::bluetooth::engine::sram_link::ControllerSramLinkAddress;

use crate::types::BluetoothControllerSramAddress;

use crate::bluetooth::engine::rx_packet_bytes::{RxPacketBytes, result_not_produced};
use crate::bluetooth::engine::shared_layout::BufferHeaderImage;
use crate::layouts::*;

/// Maximum Link Layer payload admitted by the shared receive allocation.
pub const BLUETOOTH_LE_RX_PAYLOAD_CAPACITY: usize = u8::MAX as usize;

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
pub(crate) struct LeRxPacketAddress(DmaWindowAddress);

impl LeRxPacketAddress {
    pub(crate) const fn from_placed(address: DmaWindowAddress) -> Self {
        Self(address)
    }

    pub(crate) const fn compressed_image(self) -> u32 {
        self.0
            .ble_compressed()
            .expect("a placed RX packet has a nonzero encodable address")
    }
}

/// Private controller-shared receive-buffer header.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub(crate) struct LeRxBufferHeaderStorage {
    pub(crate) layout: BleBufferHeader,
}

/// SOURCE: pinned `memMgmt_3.o` RX-header allocator, including its opaque PDU-target prefix.
const RX_HEADER: BufferHeaderImage = BufferHeaderImage {
    pdu_target: BleBufferHeaderPduTarget::ZERO
        .with_bit_23_opaque(true)
        .with_bit_31_opaque(true),
    ..BufferHeaderImage::ZERO
};

impl LeRxBufferHeaderStorage {
    pub(crate) const fn new() -> Self {
        Self {
            layout: BleBufferHeader::zeroed(),
        }
    }

    pub(crate) fn install(
        &self,
        packet: LeRxPacketAddress,
        successor: Option<ControllerSramLinkAddress>,
        predecessor: Option<BluetoothControllerSramAddress>,
        rotates_into_successor: bool,
    ) {
        BufferHeaderImage {
            next: BleBufferHeaderNext::from_bits(
                successor.map_or(0, ControllerSramLinkAddress::compressed_image),
            ),
            packet_base: BleBufferHeaderPacketBase::ZERO
                .with_address(packet.0)
                .expect("the placed RX buffer address is encodable"),
            allocation: BleBufferHeaderAllocation::ZERO.with_terminal(rotates_into_successor),
            previous: BleBufferHeaderPrevious::from_bits(
                predecessor.map_or(0, BluetoothControllerSramAddress::address),
            ),
            ..RX_HEADER
        }
        .write(&self.layout);
    }
    pub(crate) fn completion_observed(&self) -> bool {
        self.layout.completion().read().done()
    }
    pub(crate) fn install_completed_predecessor(&self, successor: ControllerSramLinkAddress) {
        BufferHeaderImage {
            next: BleBufferHeaderNext::from_bits(successor.compressed_image()),
            completion: BleBufferHeaderCompletion::ZERO.with_done(true),
            ..RX_HEADER
        }
        .write(&self.layout);
    }
    #[cfg(test)]
    pub(crate) fn emulate_hardware_completion(&self) {
        self.layout
            .completion()
            .write(self.layout.completion().read().with_done(true));
    }
    pub(crate) fn successor(&self) -> Option<u32> {
        let image = self.layout.next().read().address();
        (image != 0).then_some(image)
    }
    pub(crate) fn predecessor(&self) -> Option<u32> {
        let address = self.layout.previous().read().address();
        (address != 0).then_some(address)
    }
    pub(crate) fn rotates_into_successor(&self) -> bool {
        self.layout.allocation().read().terminal()
    }
    pub(crate) fn set_rotation_marker(&self, marked: bool) {
        self.layout
            .allocation()
            .write(self.layout.allocation().read().with_terminal(marked));
    }
    pub(crate) fn link_successor(&self, successor: Option<ControllerSramLinkAddress>) {
        self.layout
            .next()
            .write(
                self.layout
                    .next()
                    .read()
                    .with_address_from(BleBufferHeaderNext::from_bits(
                        successor.map_or(0, ControllerSramLinkAddress::compressed_image),
                    )),
            );
    }
    pub(crate) fn packet_image(&self) -> Option<u32> {
        let image = self.layout.packet_base().read().address();
        (image != 0).then_some(image)
    }
    pub(crate) fn clear_packet(&self) {
        self.layout
            .packet_base()
            .write(self.layout.packet_base().read().without_address());
    }
    pub(crate) fn clear_completion(&self) {
        self.layout
            .completion()
            .write(self.layout.completion().read().with_done(false));
    }
    pub(crate) fn set_predecessor(&self, predecessor: Option<u32>) {
        self.layout
            .previous()
            .write(BleBufferHeaderPrevious::from_bits(predecessor.unwrap_or(0)));
    }
    pub(crate) fn copy_from(&self, other: &Self) {
        crate::bluetooth::engine::shared_layout::copy_buffer_header(&self.layout, &other.layout);
    }
}

/// Private controller-shared receive packet allocation.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub(crate) struct LeRxPacketStorage {
    pub(crate) bytes: RxPacketBytes,
}

impl LeRxPacketStorage {
    pub(crate) const fn new() -> Self {
        Self {
            bytes: RxPacketBytes::zeroed(),
        }
    }
    pub(crate) fn initialize(&self) {
        self.bytes.initialize();
    }
    pub(crate) fn rearm(&self) {
        self.bytes.rearm();
    }

    fn disposition(&self, connection: bool) -> Result<LeRxPacketDisposition, LeRxPacketError> {
        let bytes = self.bytes.prefix();
        let result = BleRxPacketPrefix::new(&bytes).result();
        if result_not_produced(result) {
            return Err(LeRxPacketError::ProducerSentinelRetained);
        }
        // SOURCE: pinned connection RX rejects flags 0,1,2,4; ordinary RX rejects 0,9,10.
        let rejected = if connection {
            result.bit_0_opaque()
                || result.bit_1_opaque()
                || result.bit_2_opaque()
                || result.bit_4_opaque()
        } else {
            result.bit_0_opaque() || result.bit_9_opaque() || result.bit_10_opaque()
        };
        Ok(if rejected {
            LeRxPacketDisposition::Discarded
        } else {
            LeRxPacketDisposition::Dispatchable
        })
    }
    fn copy_dispatchable_pdu(&self) -> Result<LeReceivedPdu, LeRxPacketError> {
        let prefix_bytes = self.bytes.prefix();
        let prefix = BleRxPacketPrefix::new(&prefix_bytes);
        if prefix.auxiliary().bits_0_15_opaque() == u16::MAX {
            return Err(LeRxPacketError::EpochSentinelRetained);
        }
        let payload_length = usize::from(prefix.payload_length().value());
        let length = payload_length + 2;
        let mut bytes = [0; BLUETOOTH_LE_RX_PAYLOAD_CAPACITY + 2];
        bytes[0] = prefix.pdu_header().value();
        bytes[1] = payload_length as u8;
        for (target, source) in bytes[2..length].iter_mut().zip(&self.bytes.payload) {
            *target = source.get();
        }
        Ok(LeReceivedPdu {
            bytes,
            length: length as u16,
            rssi_dbm: prefix.result().rssi() as i8,
            captured_time: LePacketCapturedTime::from_controller_sram_word(
                prefix.byte_0010_opaque().bits_0_31_opaque(),
            ),
        })
    }
    #[cfg(test)]
    pub(crate) fn emulate_hardware_receive(&self, pdu: &[u8], rssi_dbm: i8, captured_time: u32) {
        assert!((2..=BLUETOOTH_LE_RX_PAYLOAD_CAPACITY + 2).contains(&pdu.len()));
        assert_eq!(usize::from(pdu[1]) + 2, pdu.len());
        let bytes = self.bytes.prefix();
        let original = BleRxPacketPrefix::new(&bytes);
        let produced = [
            original.byte_0000_opaque().bits().to_le_bytes(),
            original.byte_0004_opaque().bits().to_le_bytes(),
            original.byte_0008_opaque().bits().to_le_bytes(),
            BleRxPacketPrefixResult::from_bits(0)
                .with_rssi(rssi_dbm as u8)
                .bits()
                .to_le_bytes(),
            captured_time.to_le_bytes(),
            original.byte_0014_opaque().bits().to_le_bytes(),
            original
                .auxiliary()
                .with_bits_0_15_opaque(0)
                .bits()
                .to_le_bytes(),
        ];
        self.bytes
            .model_prefix(produced.into_iter().flatten().chain([pdu[0], pdu[1]]));
        for (cell, byte) in self.bytes.payload.iter().zip(&pdu[2..]) {
            cell.set(*byte);
        }
    }
    #[cfg(test)]
    pub(crate) fn emulate_hardware_discard(&self) {
        self.bytes.update_prefix(|prefix| {
            prefix.set_result(
                prefix
                    .result()
                    .with_bit_0_opaque(true)
                    .with_bit_9_opaque(true)
                    .with_bit_10_opaque(true),
            )
        });
    }
    pub(crate) fn receive_tag(&self) -> u16 {
        BleRxPacketPrefix::new(&self.bytes.prefix())
            .auxiliary()
            .bits_0_15_opaque()
    }
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
        self.bytes.update_prefix(|prefix| {
            prefix.set_auxiliary(prefix.auxiliary().with_bits_0_15_opaque(tag))
        });
    }
}

#[repr(C)]
#[derive(bytemuck::Zeroable)]
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
    fn relinking_preserves_the_unassigned_header_image() {
        let header = LeRxBufferHeaderStorage::new();
        header
            .layout
            .next()
            .write(BleBufferHeaderNext::from_bits(0xa5a5_4321));
        header.link_successor(Some(ControllerSramLinkAddress::new(0x2f00_4000).unwrap()));
        assert_eq!(header.layout.next().read().bits(), 0xa5a0_1000);
        header.link_successor(None);
        assert_eq!(header.layout.next().read().bits(), 0xa5a0_0000);
    }

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
            packet.bytes.update_prefix(|prefix| {
                prefix.set_result(BleRxPacketPrefixResult::from_bits(rejected_result))
            });
            assert_eq!(packet.received(true), Ok(LeRxOutcome::Discarded));
        }
        packet.rearm();
        assert_eq!(
            packet.received(true),
            Err(LeRxError::ProducerSentinelRetained)
        );
    }
}

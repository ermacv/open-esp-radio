//! Descriptor images, burst sizing, chain construction, and validation.

#[cfg(feature = "axi-gdma-mem2mem")]
use super::{
    registers::{INTERNAL_SRAM_END, INTERNAL_SRAM_START},
    transfer::{AxiGdmaMem2MemError, AxiGdmaMem2MemSegment, validate_range},
};
#[cfg(feature = "axi-gdma-mem2mem")]
use core::ptr;

const DESCRIPTOR_MAX_BYTES: usize = 4095;

const DESCRIPTOR_OWNER_DMA: u32 = 1 << 31;

const DESCRIPTOR_SUCCESS_EOF: u32 = 1 << 30;

/// AXI-GDMA data burst size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BurstSize {
    Bytes16 = 16,
    Bytes32 = 32,
    Bytes64 = 64,
}

impl BurstSize {
    const fn bytes(self) -> usize {
        self as usize
    }

    #[cfg(feature = "axi-gdma-mem2mem")]
    pub(super) const fn register_value(self) -> u8 {
        match self {
            Self::Bytes16 => 1,
            Self::Bytes32 => 2,
            Self::Bytes64 => 3,
        }
    }
}

const fn descriptor_payload_bytes(burst: BurstSize) -> usize {
    DESCRIPTOR_MAX_BYTES & !(burst.bytes() - 1)
}

pub(super) const fn required_descriptors(bytes: usize, burst: BurstSize) -> usize {
    bytes.div_ceil(descriptor_payload_bytes(burst))
}

const fn descriptor_flags(bytes: usize, end_of_frame: bool) -> u32 {
    bytes as u32
        | (bytes as u32) << 12
        | if end_of_frame {
            DESCRIPTOR_SUCCESS_EOF
        } else {
            0
        }
        | DESCRIPTOR_OWNER_DMA
}

#[cfg(feature = "axi-gdma-mem2mem")]
/// One hardware AXI-GDMA linked-list item.
///
/// S31 requires eight-byte item alignment. Padding the twelve-byte wire
/// image to sixteen bytes also makes every item in a Rust slice aligned.
#[repr(C, align(8))]
#[derive(Clone, Copy, Debug, Eq, PartialEq, bytemuck::Zeroable)]
pub struct AxiGdmaDescriptor {
    flags: u32,
    buffer: u32,
    next: u32,
    reserved: u32,
}

#[cfg(feature = "axi-gdma-mem2mem")]
impl AxiGdmaDescriptor {
    pub const EMPTY: Self = Self {
        flags: 0,
        buffer: 0,
        next: 0,
        reserved: 0,
    };

    pub(super) fn owner_is_dma(self) -> bool {
        self.flags & DESCRIPTOR_OWNER_DMA != 0
    }

    pub(super) fn received_bytes(self) -> usize {
        ((self.flags >> 12) & 0x0fff) as usize
    }
}

#[cfg(feature = "axi-gdma-mem2mem")]
pub(super) fn validate_descriptors(
    descriptors: &[AxiGdmaDescriptor],
    required: usize,
) -> Result<(), AxiGdmaMem2MemError> {
    if descriptors.len() < required {
        return Err(AxiGdmaMem2MemError::InsufficientDescriptors);
    }
    let address = descriptors.as_ptr() as usize;
    let size = required
        .checked_mul(core::mem::size_of::<AxiGdmaDescriptor>())
        .ok_or(AxiGdmaMem2MemError::AddressOverflow)?;
    if !address.is_multiple_of(8) {
        return Err(AxiGdmaMem2MemError::DescriptorAlignment);
    }
    validate_range(address, size, INTERNAL_SRAM_START, INTERNAL_SRAM_END).map_err(|error| {
        match error {
            AxiGdmaMem2MemError::AddressOverflow => error,
            _ => AxiGdmaMem2MemError::DescriptorOutsideInternalSram,
        }
    })
}

#[cfg(feature = "axi-gdma-mem2mem")]
pub(super) fn build_chain(
    descriptors: &mut [AxiGdmaDescriptor],
    buffer: *mut u8,
    bytes: usize,
    count: usize,
    burst: BurstSize,
    transmit: bool,
) {
    build_chain_range(descriptors, 0, buffer, bytes, count, count, burst, transmit);
}

#[cfg(feature = "axi-gdma-mem2mem")]
pub(super) fn build_segment_chains(
    tx_descriptors: &mut [AxiGdmaDescriptor],
    rx_descriptors: &mut [AxiGdmaDescriptor],
    segments: &mut [AxiGdmaMem2MemSegment<'_>],
    descriptor_count: usize,
    burst: BurstSize,
) {
    let mut cursor = 0usize;
    for segment in segments {
        let count = required_descriptors(segment.len(), burst);
        build_chain_range(
            tx_descriptors,
            cursor,
            segment.source.as_mut_ptr(),
            segment.len(),
            count,
            descriptor_count,
            burst,
            true,
        );
        build_chain_range(
            rx_descriptors,
            cursor,
            segment.destination.as_mut_ptr(),
            segment.len(),
            count,
            descriptor_count,
            burst,
            false,
        );
        cursor += count;
    }
    debug_assert_eq!(cursor, descriptor_count);
}

#[cfg(feature = "axi-gdma-mem2mem")]
#[allow(
    clippy::too_many_arguments,
    reason = "descriptor chain construction keeps all hardware dimensions explicit"
)]
fn build_chain_range(
    descriptors: &mut [AxiGdmaDescriptor],
    start: usize,
    buffer: *mut u8,
    bytes: usize,
    count: usize,
    total_count: usize,
    burst: BurstSize,
    transmit: bool,
) {
    let chunk_capacity = descriptor_payload_bytes(burst);
    let mut remaining = bytes;
    let mut offset = 0usize;
    for index in start..start + count {
        let chunk = remaining.min(chunk_capacity);
        let terminal = index + 1 == total_count;
        let next = if terminal {
            0
        } else {
            ptr::addr_of_mut!(descriptors[index + 1]) as u32
        };
        descriptors[index] = AxiGdmaDescriptor {
            // ESP32-S31's AXI M2M link contract publishes the transfer
            // length as well as the buffer capacity on both sides. This
            // differs from the conventional peripheral-RX descriptor
            // convention where software initially leaves length at zero.
            flags: descriptor_flags(chunk, transmit && terminal),
            buffer: buffer.wrapping_add(offset) as u32,
            next,
            reserved: 0,
        };
        offset += chunk;
        remaining -= chunk;
    }
    debug_assert_eq!(remaining, 0);
}

#[cfg(test)]
mod tests;

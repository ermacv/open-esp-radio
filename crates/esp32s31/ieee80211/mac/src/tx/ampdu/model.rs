//! Host-only value models used to qualify recovered A-MPDU transitions.
//!
//! These functions intentionally do not form part of the 32-bit production
//! driver. They preserve finite descriptor transformations for native oracle
//! tests without exposing pointer-shaped vendor structures to the live path.

const BASIC_HT_RATE_MIN: u8 = 16;
const BASIC_HT_RATE_MAX: u8 = 35;
pub(crate) const TX_DESCRIPTOR_HE_BIT: u32 = 0x8000_0000;
const TX_DESCRIPTOR_BAR_BIT: u32 = 0x0020_0000;
const TX_DESCRIPTOR_AMPDU_BIT: u32 = 0x0040_0000;
const TX_DESCRIPTOR_AMPDU_FIRST_BITS: u32 = 0x0048_0000;
pub(crate) const TX_BUFFER_END_BIT: u32 = 0x4000_0000;
const FIRST_MPDU_RETRY_HEADER_BIT: u32 = 0x0100_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BasicHtAmpduAssemblyError {
    AggregateShorterThanHeader,
    UnsupportedRate(u8),
    UnsupportedDescriptor(u32),
    TailAlreadyTerminated(u32),
}

/// Inputs consumed by the finite HT `ppAssembleAMPDU` leaf.
///
/// This value form keeps the recovered bit transition host-testable without
/// exposing the vendor ESF pointer layout to the owned TX path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BasicHtAmpduAssemblyInput {
    pub aggregate_length: u16,
    pub first_header_length: u16,
    pub first_payload_word: u32,
    pub first_descriptor_flags: u32,
    pub first_descriptor_word1: u32,
    pub first_rate: u8,
    pub tail_buffer_flags: u32,
    pub tail_timestamp: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BasicHtAmpduAssemblyOutput {
    pub first_remaining_length: u16,
    pub first_payload_word: u32,
    pub first_descriptor_flags: u32,
    pub first_descriptor_word1: u32,
    pub tail_buffer_flags: u32,
    pub first_timestamp: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BasicHtAmpduCompletionInput {
    pub descriptor_flags: u32,
    pub descriptor_queue_word: u32,
    pub frame_control: u16,
    pub acknowledged: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BasicHtAmpduCompletionOutput {
    pub descriptor_flags: u32,
    pub descriptor_queue_word: u32,
    pub frame_control: u16,
}

/// Reproduce the per-MPDU markers observed around `ppResortTxAMPDU` after the
/// aggregate topology has been detached.
///
/// Acknowledged MPDUs retain the aggregate marker so the later TX-done stage
/// skips duplicate per-frame rate control and gain bit 24 in the descriptor
/// queue word. A missing MPDU remains a normal detached descriptor and gains
/// only the IEEE 802.11 Retry bit; its CCMP-ready payload is reused unchanged.
#[inline(always)]
pub const fn basic_ht_ampdu_completion(
    input: BasicHtAmpduCompletionInput,
) -> BasicHtAmpduCompletionOutput {
    if input.acknowledged {
        BasicHtAmpduCompletionOutput {
            descriptor_flags: input.descriptor_flags | TX_DESCRIPTOR_AMPDU_BIT,
            descriptor_queue_word: input.descriptor_queue_word | 0x0100_0000,
            frame_control: input.frame_control,
        }
    } else {
        BasicHtAmpduCompletionOutput {
            descriptor_flags: input.descriptor_flags,
            descriptor_queue_word: input.descriptor_queue_word,
            frame_control: input.frame_control | 0x0800,
        }
    }
}

/// Reproduce the mutation made by the pinned non-HE `ppAssembleAMPDU` body.
///
/// There is no allocation, callback, lock, timer, retry or pointer access in
/// this value-level operation.
#[inline(always)]
pub const fn basic_ht_ampdu_assembly(
    input: BasicHtAmpduAssemblyInput,
) -> Result<BasicHtAmpduAssemblyOutput, BasicHtAmpduAssemblyError> {
    if input.aggregate_length < input.first_header_length {
        return Err(BasicHtAmpduAssemblyError::AggregateShorterThanHeader);
    }
    if input.first_rate < BASIC_HT_RATE_MIN || input.first_rate > BASIC_HT_RATE_MAX {
        return Err(BasicHtAmpduAssemblyError::UnsupportedRate(input.first_rate));
    }
    if input.first_descriptor_flags
        & (TX_DESCRIPTOR_HE_BIT | TX_DESCRIPTOR_BAR_BIT | TX_DESCRIPTOR_AMPDU_BIT)
        != 0
    {
        return Err(BasicHtAmpduAssemblyError::UnsupportedDescriptor(
            input.first_descriptor_flags,
        ));
    }
    if input.tail_buffer_flags & TX_BUFFER_END_BIT != 0 {
        return Err(BasicHtAmpduAssemblyError::TailAlreadyTerminated(
            input.tail_buffer_flags,
        ));
    }
    Ok(BasicHtAmpduAssemblyOutput {
        first_remaining_length: input
            .aggregate_length
            .wrapping_sub(input.first_header_length),
        first_payload_word: input.first_payload_word & !FIRST_MPDU_RETRY_HEADER_BIT,
        first_descriptor_flags: (input.first_descriptor_flags & !TX_BUFFER_END_BIT)
            | TX_DESCRIPTOR_AMPDU_FIRST_BITS,
        // The ROM leaf performs one byte load followed by a word store.
        first_descriptor_word1: input.first_descriptor_word1 & 0xff,
        tail_buffer_flags: input.tail_buffer_flags | TX_BUFFER_END_BIT,
        first_timestamp: input.tail_timestamp,
    })
}

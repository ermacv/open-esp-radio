//! LE Test packet payloads.
//!
//! The eight patterns and their test payload types follow Core 6.0 Vol 6
//! Part F section 4.1.5. The PRBS generators reproduce the complete 255-byte
//! tables of the current and initial ESP32-S31 `dtm_tx_create_ctx` bodies.

/// Payload of an LE Test packet.
///
/// Multi-bit names describe the transmitted least-significant-bit-first bit
/// sequence; each variant documents its stored byte.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmPayloadPattern {
    /// PRBS9, payload type zero.
    Prbs9,
    /// Repeated `11110000`, stored as `0x0f`.
    Repeated11110000,
    /// Repeated `10101010`, stored as `0x55`.
    Repeated10101010,
    /// PRBS15.
    Prbs15,
    /// Repeated `11111111`, stored as `0xff`.
    RepeatedAllOnes,
    /// Repeated `00000000`, stored as `0x00`.
    RepeatedAllZeros,
    /// Repeated `00001111`, stored as `0xf0`.
    Repeated00001111,
    /// Repeated `01010101`, stored as `0xaa`.
    Repeated01010101,
}

impl DtmPayloadPattern {
    /// The pattern of an HCI payload selector.
    pub const fn from_hci_selector(selector: u8) -> Option<Self> {
        match selector {
            0 => Some(Self::Prbs9),
            1 => Some(Self::Repeated11110000),
            2 => Some(Self::Repeated10101010),
            3 => Some(Self::Prbs15),
            4 => Some(Self::RepeatedAllOnes),
            5 => Some(Self::RepeatedAllZeros),
            6 => Some(Self::Repeated00001111),
            7 => Some(Self::Repeated01010101),
            _ => None,
        }
    }

    /// The LE Test PDU payload type field, which equals the HCI selector.
    pub const fn payload_type(self) -> u8 {
        self.radio_payload_type().value()
    }

    /// The validated LE Test PDU payload type.
    pub const fn radio_payload_type(self) -> crate::TestPayloadType {
        match self {
            Self::Prbs9 => constant_payload_type::<0>(),
            Self::Repeated11110000 => constant_payload_type::<1>(),
            Self::Repeated10101010 => constant_payload_type::<2>(),
            Self::Prbs15 => constant_payload_type::<3>(),
            Self::RepeatedAllOnes => constant_payload_type::<4>(),
            Self::RepeatedAllZeros => constant_payload_type::<5>(),
            Self::Repeated00001111 => constant_payload_type::<6>(),
            Self::Repeated01010101 => constant_payload_type::<7>(),
        }
    }

    /// Fill `payload` with the pattern from its first byte.
    pub fn fill(self, payload: &mut [u8]) {
        match self {
            Self::Prbs9 => fill_prbs::<9, 0, 4>(payload),
            Self::Repeated11110000 => payload.fill(0x0f),
            Self::Repeated10101010 => payload.fill(0x55),
            Self::Prbs15 => fill_prbs::<15, 6, 10>(payload),
            Self::RepeatedAllOnes => payload.fill(0xff),
            Self::RepeatedAllZeros => payload.fill(0x00),
            Self::Repeated00001111 => payload.fill(0xf0),
            Self::Repeated01010101 => payload.fill(0xaa),
        }
    }
}

const fn constant_payload_type<const VALUE: u8>() -> crate::TestPayloadType {
    const {
        let value = match crate::TestPayloadType::new(VALUE) {
            Ok(value) => Some(value),
            Err(_) => None,
        };
        if !value.is_some() {
            panic!("invalid constant payload type");
        }
        value.unwrap()
    }
}

fn fill_prbs<const WIDTH: u32, const FIRST_TAP: u32, const SECOND_TAP: u32>(payload: &mut [u8]) {
    let mut state = const {
        if !(WIDTH > 0 && WIDTH < 16 && FIRST_TAP < WIDTH && SECOND_TAP < WIDTH) {
            panic!("invalid PRBS geometry");
        }
        (1_u16 << WIDTH) - 1
    };
    for output in payload {
        let mut image = 0_u8;
        for output_bit in 0..8 {
            image |= ((state & 1) as u8) << output_bit;
            let feedback = ((state >> FIRST_TAP) ^ (state >> SECOND_TAP)) & 1;
            state = (state >> 1) | (feedback << const { WIDTH - 1 });
        }
        *output = image;
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;

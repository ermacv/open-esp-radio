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
        match self {
            Self::Prbs9 => 0,
            Self::Repeated11110000 => 1,
            Self::Repeated10101010 => 2,
            Self::Prbs15 => 3,
            Self::RepeatedAllOnes => 4,
            Self::RepeatedAllZeros => 5,
            Self::Repeated00001111 => 6,
            Self::Repeated01010101 => 7,
        }
    }

    /// Fill `payload` with the pattern from its first byte.
    pub fn fill(self, payload: &mut [u8]) {
        match self {
            Self::Prbs9 => fill_prbs(payload, 9, 0, 4),
            Self::Repeated11110000 => payload.fill(0x0f),
            Self::Repeated10101010 => payload.fill(0x55),
            Self::Prbs15 => fill_prbs(payload, 15, 6, 10),
            Self::RepeatedAllOnes => payload.fill(0xff),
            Self::RepeatedAllZeros => payload.fill(0x00),
            Self::Repeated00001111 => payload.fill(0xf0),
            Self::Repeated01010101 => payload.fill(0xaa),
        }
    }
}

fn fill_prbs(payload: &mut [u8], width: u32, first_tap: u32, second_tap: u32) {
    let mut state = (1_u16 << width) - 1;
    for output in payload {
        let mut image = 0_u8;
        for output_bit in 0..8 {
            image |= ((state & 1) as u8) << output_bit;
            let feedback = ((state >> first_tap) ^ (state >> second_tap)) & 1;
            state = (state >> 1) | (feedback << (width - 1));
        }
        *output = image;
    }
}

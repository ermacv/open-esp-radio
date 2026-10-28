//! Data Length Update procedure of one connection (Core Vol 6 Part B 5.1.9).
//!
//! Each side announces the longest data PDU payload it will send and receive,
//! in octets and in air time. The effective length in each direction is the
//! smaller of what one side sends and the other receives, never below the
//! 27-octet, 328 us minimum. Octets exclude the MIC; air time includes it.
//! This profile uses LE 1M only, where a payload of `n` octets with a MIC of
//! `m` octets lasts `(n + m + 10) * 8` us.

/// Octets of the shortest data PDU payload every connection supports.
pub const LE_DATA_LENGTH_MINIMUM_OCTETS: u16 = 27;
/// Air time of the shortest data PDU on LE 1M, MIC included.
pub const LE_DATA_LENGTH_MINIMUM_TIME_MICROS: u16 = 328;
/// Octets of the longest data PDU payload.
pub const LE_DATA_LENGTH_MAXIMUM_OCTETS: u16 = 251;
/// Air time of the longest data PDU the Host may request (Coded PHY).
pub const LE_DATA_LENGTH_MAXIMUM_TIME_MICROS: u16 = 17_040;
/// Air time of the longest data PDU on LE 1M, MIC included.
pub const LE_DATA_LENGTH_1M_MAXIMUM_TIME_MICROS: u16 = 2_120;

/// One direction's data PDU length: payload octets and air time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDataLength {
    octets: u16,
    time_micros: u16,
}

impl LeDataLength {
    /// The length every connection starts with.
    pub const MINIMUM: Self = Self {
        octets: LE_DATA_LENGTH_MINIMUM_OCTETS,
        time_micros: LE_DATA_LENGTH_MINIMUM_TIME_MICROS,
    };

    /// The longest length this LE 1M Controller sends or receives.
    pub const SUPPORTED_MAXIMUM: Self = Self {
        octets: LE_DATA_LENGTH_MAXIMUM_OCTETS,
        time_micros: LE_DATA_LENGTH_1M_MAXIMUM_TIME_MICROS,
    };

    /// A length within the ranges the specification allows.
    pub const fn new(octets: u16, time_micros: u16) -> Option<Self> {
        if octets < LE_DATA_LENGTH_MINIMUM_OCTETS
            || octets > LE_DATA_LENGTH_MAXIMUM_OCTETS
            || time_micros < LE_DATA_LENGTH_MINIMUM_TIME_MICROS
            || time_micros > LE_DATA_LENGTH_MAXIMUM_TIME_MICROS
        {
            return None;
        }
        Some(Self {
            octets,
            time_micros,
        })
    }

    pub const fn octets(self) -> u16 {
        self.octets
    }

    pub const fn time_micros(self) -> u16 {
        self.time_micros
    }

    /// The smaller octets and the smaller time of both.
    pub const fn min(self, other: Self) -> Self {
        Self {
            octets: if self.octets < other.octets {
                self.octets
            } else {
                other.octets
            },
            time_micros: if self.time_micros < other.time_micros {
                self.time_micros
            } else {
                other.time_micros
            },
        }
    }

    /// This length limited to what this Controller supports.
    pub const fn supported(self) -> Self {
        self.min(Self::SUPPORTED_MAXIMUM)
    }

    /// The longest LE 1M payload this length allows, with or without a MIC.
    pub const fn payload_octets(self, mic_octets: u16) -> Result<u16, LeDataLengthError> {
        let Some(overhead) = mic_octets.checked_add(10) else {
            return Err(LeDataLengthError::MicLengthOverflow { mic_octets });
        };
        let by_time = (self.time_micros / 8).saturating_sub(overhead);
        Ok(if by_time < self.octets {
            by_time
        } else {
            self.octets
        })
    }
}

/// Invalid arithmetic input to a data length calculation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDataLengthError {
    /// The MIC and fixed packet overhead do not fit the input's octet domain.
    MicLengthOverflow { mic_octets: u16 },
}

/// The lengths of both directions of one connection end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDataLengths {
    pub transmit: LeDataLength,
    pub receive: LeDataLength,
}

impl LeDataLengths {
    /// Both directions at the minimum.
    pub const MINIMUM: Self = Self {
        transmit: LeDataLength::MINIMUM,
        receive: LeDataLength::MINIMUM,
    };

    /// The `LL_LENGTH_REQ` or `LL_LENGTH_RSP` body after its opcode.
    pub const fn wire(self) -> [u8; 8] {
        let rx_octets = self.receive.octets.to_le_bytes();
        let rx_time = self.receive.time_micros.to_le_bytes();
        let tx_octets = self.transmit.octets.to_le_bytes();
        let tx_time = self.transmit.time_micros.to_le_bytes();
        [
            rx_octets[0],
            rx_octets[1],
            rx_time[0],
            rx_time[1],
            tx_octets[0],
            tx_octets[1],
            tx_time[0],
            tx_time[1],
        ]
    }

    /// Decode a peer's announcement. Values below the minimum are invalid;
    /// values above the maximum the peer may announce are limited to it.
    pub fn from_wire(body: &[u8]) -> Option<Self> {
        let [a, b, c, d, e, f, g, h] = *body else {
            return None;
        };
        let direction = |octets: u16, time: u16| {
            if octets < LE_DATA_LENGTH_MINIMUM_OCTETS || time < LE_DATA_LENGTH_MINIMUM_TIME_MICROS {
                return None;
            }
            Some(LeDataLength {
                octets: octets.min(LE_DATA_LENGTH_MAXIMUM_OCTETS),
                time_micros: time.min(LE_DATA_LENGTH_MAXIMUM_TIME_MICROS),
            })
        };
        Some(Self {
            receive: direction(u16::from_le_bytes([a, b]), u16::from_le_bytes([c, d]))?,
            transmit: direction(u16::from_le_bytes([e, f]), u16::from_le_bytes([g, h]))?,
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;

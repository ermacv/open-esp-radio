//! Espressif Long Range descriptor identities.
//! Independent of the MAC protocol using them; no LR publication authority.

/// One recovered ESP32-S31 LoRa schedule rate selected without assigning an
/// unevidenced on-air bitrate or PLCP interpretation.
///
/// The reviewed source-owned LoRa callback and schedule reconstruction maps
/// code `0x2a` to record zero and code `0x29` to record one. These codes remain
/// chip descriptor values; neither variant authorizes queue-vector
/// publication.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MacLongRangeRateCode {
    #[default]
    RateCode2a,
    RateCode29,
}

impl MacLongRangeRateCode {
    pub const fn from_descriptor_rate_code(code: u8) -> Option<Self> {
        match code {
            0x2a => Some(Self::RateCode2a),
            0x29 => Some(Self::RateCode29),
            _ => None,
        }
    }

    pub const fn descriptor_rate_code(self) -> u8 {
        match self {
            Self::RateCode2a => 0x2a,
            Self::RateCode29 => 0x29,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_range_descriptor_identities_round_trip() {
        for rate in [
            MacLongRangeRateCode::RateCode2a,
            MacLongRangeRateCode::RateCode29,
        ] {
            assert_eq!(
                MacLongRangeRateCode::from_descriptor_rate_code(rate.descriptor_rate_code()),
                Some(rate)
            );
        }
        assert_eq!(MacLongRangeRateCode::from_descriptor_rate_code(0), None);
    }
}

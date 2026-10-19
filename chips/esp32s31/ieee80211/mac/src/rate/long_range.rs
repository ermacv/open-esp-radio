//! Espressif Long Range descriptor identities and retry policy.
//! Independent of the MAC protocol using them; no LR publication authority.

use super::schedule::{
    RateScheduleKind, RateScheduleRef, schedule_publication_limit, schedule_rate_after_failures,
};

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

    pub const fn retry_schedule(self) -> RateScheduleRef {
        const RECORD_2A: RateScheduleRef = {
            #[expect(clippy::panic, reason = "evaluated at build")]
            const fn checked() -> RateScheduleRef {
                match RateScheduleRef::new(RateScheduleKind::Lora, 0) {
                    Some(schedule) => schedule,
                    None => panic!("long-range record zero exists"),
                }
            }
            checked()
        };
        const RECORD_29: RateScheduleRef = {
            #[expect(clippy::panic, reason = "evaluated at build")]
            const fn checked() -> RateScheduleRef {
                match RateScheduleRef::new(RateScheduleKind::Lora, 1) {
                    Some(schedule) => schedule,
                    None => panic!("long-range record one exists"),
                }
            }
            checked()
        };
        match self {
            Self::RateCode2a => RECORD_2A,
            Self::RateCode29 => RECORD_29,
        }
    }

    /// Decode one exact attempt from the reviewed LoRa retry record.
    ///
    /// The return value remains a descriptor-code identity. It does not
    /// authorize LR PLCP or queue-vector publication.
    pub fn retry_rate_after_failures(self, failed_attempts: u8) -> Option<Self> {
        Self::from_descriptor_rate_code(schedule_rate_after_failures(
            self.retry_schedule(),
            failed_attempts,
        )?)
    }

    /// Complete ordinary-MPDU publication budget stored in this LoRa record.
    pub fn retry_publication_limit(self) -> u8 {
        schedule_publication_limit(self.retry_schedule())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_range_records_preserve_both_descriptor_identities_and_limits() {
        let fast = MacLongRangeRateCode::RateCode2a;
        assert_eq!(fast.descriptor_rate_code(), 0x2a);
        assert_eq!(fast.retry_publication_limit(), 32);
        assert_eq!(fast.retry_rate_after_failures(0), Some(fast));
        assert_eq!(fast.retry_rate_after_failures(17), Some(fast));
        assert_eq!(
            fast.retry_rate_after_failures(18),
            Some(MacLongRangeRateCode::RateCode29)
        );
        assert_eq!(
            fast.retry_rate_after_failures(31),
            Some(MacLongRangeRateCode::RateCode29)
        );
        assert_eq!(fast.retry_rate_after_failures(32), None);

        let robust = MacLongRangeRateCode::RateCode29;
        assert_eq!(robust.descriptor_rate_code(), 0x29);
        assert_eq!(robust.retry_publication_limit(), 32);
        assert_eq!(robust.retry_rate_after_failures(0), Some(robust));
        assert_eq!(robust.retry_rate_after_failures(31), Some(robust));
        assert_eq!(robust.retry_rate_after_failures(32), None);
        assert_eq!(MacLongRangeRateCode::from_descriptor_rate_code(0x28), None);
    }
}

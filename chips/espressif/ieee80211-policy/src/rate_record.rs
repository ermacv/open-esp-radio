//! Opaque rate-schedule records and the hardware inputs the family's
//! association rate control selects.

use oer_ieee80211_port::backend::RateScheduleKind;

/// Octets in an opaque rate record supplied by the family policy.
pub const RATE_SCHEDULE_RECORD_SIZE: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Immutable record identity supplied by the family rate policy; contains no retry state.
pub struct RateScheduleRef {
    kind: RateScheduleKind,
    record: &'static [u8; RATE_SCHEDULE_RECORD_SIZE],
}

impl RateScheduleRef {
    /// Bind an immutable record from the family rate policy to its format family.
    pub const fn from_record(
        kind: RateScheduleKind,
        record: &'static [u8; RATE_SCHEDULE_RECORD_SIZE],
    ) -> Self {
        Self { kind, record }
    }
    /// The exact immutable record supplied by the family policy.
    pub const fn record(self) -> &'static [u8; RATE_SCHEDULE_RECORD_SIZE] {
        self.record
    }
    /// The format family of the supplied record.
    pub const fn kind(self) -> RateScheduleKind {
        self.kind
    }
    /// The record index carried in its source format.
    pub const fn index(self) -> u8 {
        let [_, _, _, _, _, _, _, _, _, _, index, _] = self.record;
        *index
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Decoded record values supplied by the family rate policy.
pub struct RateScheduleRecordState {
    /// Initial rate code in the family's domain.
    pub rate: u8,
    /// Publication budget carried by the record.
    pub retry_limit: u8,
    /// Record index used by adaptive rate control.
    pub index: u8,
    /// Selector used by the family's adaptive rate control.
    pub adaptive: u8,
}

/// Hardware inputs selected by the family's association rate-control policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeamformingReportRate {
    mode: u8,
    rate: u16,
    dcm: bool,
    ersu: bool,
    ersu_ack: bool,
}

impl BeamformingReportRate {
    /// Construct the complete report-rate and matching ACK request.
    pub const fn new(mode: u8, rate: u16, dcm: bool, ersu: bool, ersu_ack: bool) -> Self {
        Self {
            mode,
            rate,
            dcm,
            ersu,
            ersu_ack,
        }
    }

    /// PHY signal mode.
    pub const fn signal_mode(self) -> u8 {
        self.mode
    }

    /// Hardware rate code.
    pub const fn rate_code(self) -> u16 {
        self.rate
    }

    /// DCM selector.
    pub const fn dcm(self) -> bool {
        self.dcm
    }

    /// ER SU selector.
    pub const fn extended_range_single_user(self) -> bool {
        self.ersu
    }

    /// Matching ER SU ACK selector.
    pub const fn extended_range_ack(self) -> bool {
        self.ersu_ack
    }
}

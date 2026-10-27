//! Association rate-record decoding and backend observation values.

/// Octets in a profile-owned opaque rate record.
pub const RATE_SCHEDULE_RECORD_SIZE: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
/// Format family of one profile-owned rate-schedule record.
pub enum RateScheduleKind {
    // Deliberately sparse internal tags keep LLVM from materializing a
    // nine-pointer jump table in internal SRAM.  These values never cross the
    // source-owned schedule identity.
    /// OFDM control exchange record.
    BarOfdm = 0x03,
    /// Basic OFDM record.
    BasicOfdm = 0x19,
    /// HE record.
    Dot11Ax = 0x2f,
    /// DSSS/CCK record.
    Dot11B = 0x44,
    /// Non-HT OFDM record.
    Dot11G = 0x62,
    /// HT record.
    Dot11N = 0x83,
    /// Espressif LR record.
    Lora = 0xa1,
    /// Dedicated P2P OFDM record.
    P2pDot11G = 0xc8,
    /// Dedicated P2P HT record.
    P2pDot11N = 0xf1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Immutable record identity supplied by a transmit profile; contains no retry state.
pub struct RateScheduleRef {
    kind: RateScheduleKind,
    record: &'static [u8; RATE_SCHEDULE_RECORD_SIZE],
}

impl RateScheduleRef {
    /// Bind one profile-owned immutable schedule record to its format family.
    pub const fn from_record(
        kind: RateScheduleKind,
        record: &'static [u8; RATE_SCHEDULE_RECORD_SIZE],
    ) -> Self {
        Self { kind, record }
    }
    /// The exact immutable record supplied by the profile.
    pub const fn record(self) -> &'static [u8; RATE_SCHEDULE_RECORD_SIZE] {
        self.record
    }
    /// The profile record format family.
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
/// Decoded record values supplied by the profile.
pub struct RateScheduleRecordState {
    /// Initial rate code in the profile domain.
    pub rate: u8,
    /// Publication budget carried by the record.
    pub retry_limit: u8,
    /// Record index used by adaptive rate control.
    pub index: u8,
    /// Profile-specific adaptive rate-control selector.
    pub adaptive: u8,
}

/// Which retry counter a collision of the frame advances: IEEE 802.11
/// classifies a frame as long when it is longer than dot11RTSThreshold.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FrameClass {
    #[default]
    /// Frame within dot11RTSThreshold.
    Short,
    /// Frame exceeding dot11RTSThreshold.
    Long,
}

/// The counters of one frame's failed attempts.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetryCounters {
    /// Unacknowledged transmissions of the frame itself.
    pub mpdu: u8,
    /// Failures charged to the short retry counter.
    pub short: u8,
    /// Failures charged to the long retry counter.
    pub long: u8,
}

impl RetryCounters {
    /// Failures that walk the rate ladder: every ACK timeout and every
    /// failure the short counter records. A long-frame collision leaves the
    /// rate where it is.
    pub const fn ladder_step(self) -> u8 {
        if self.mpdu > self.short {
            self.mpdu
        } else {
            self.short
        }
    }
}

/// Initial AIFS and contention exponents supplied by the family policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DefaultContention {
    /// AIFS in slot units after SIFS.
    pub aifsn: u8,
    /// Minimum contention exponent.
    pub ecw_min: u8,
    /// Maximum contention exponent.
    pub ecw_max: u8,
}
/// Hardware inputs selected by the association rate-control profile.
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
